//! v2 的强类型文件模型；此阶段仅做内存转换与 RON 往返。
//!
//! 正式项目加载 / 保存仍只接受 v1。待 GUID 边界、预制体关联和编辑器结构操作
//! 接好后再开放文件级升级，避免把不能编辑的 v2 文件提前交给用户。

use std::collections::BTreeSet;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Serialize};

/// v1 的 serde 宽容读取要保留；v2 若把未知嵌套字段吞掉再保存，会不可逆地删数据。
fn parse_v2<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T, String> {
    let mut deserializer = ron::de::Deserializer::from_str(text).map_err(|e| format!("{e:#}"))?;
    let mut ignored = Vec::new();
    let parsed = serde_ignored::deserialize(&mut deserializer, |path| {
        ignored.push(path.to_string());
    })
    .map_err(|e| format!("{e:#}"))?;
    deserializer.end().map_err(|e| format!("{e:#}"))?;
    if ignored.is_empty() {
        Ok(parsed)
    } else {
        Err(format!(
            "v2 文件含不认识的字段，拒绝丢弃: {}",
            ignored.join(", ")
        ))
    }
}

use crate::component_id::{ComponentIdSequence, ComponentIdentity};
use crate::component_upgrade::preview_component_upgrade;
pub use crate::v2_animator::StrictAnimator;
use crate::{
    AudioSource, BuiltinComponent, Camera, CharacterController, ComponentAddress, Entity, EntityId,
    Light, LodGroup, MaterialOverride, ParticleEmitter, PhysicsBody, Prefab, PrefabInstanceRef,
    Scene, Transform, UiElement,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneV2 {
    format_version: u32,
    pub name: String,
    pub entities: Vec<EntityV2>,
    pub lights: Vec<Light>,
    pub ambient: [f32; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefabV2 {
    format_version: u32,
    pub name: String,
    pub entities: Vec<EntityV2>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityV2 {
    pub id: EntityId,
    pub name: String,
    pub parent: Option<EntityId>,
    pub prefab: Option<PrefabInstanceRef>,
    pub layer: u8,
    pub transform: Transform,
    /// Transform 仍是实体的固有属性，但在组件列表中有自己的稳定身份。
    pub transform_identity: ComponentIdentity,
    pub components: ComponentSet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentSet {
    pub next_component_id: ComponentIdSequence,
    pub entries: Vec<ComponentInstance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentInstance {
    pub identity: ComponentIdentity,
    pub data: ComponentData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComponentData {
    Render(RenderData),
    Ui(UiElement),
    Animator(StrictAnimator),
    Physics(PhysicsBody),
    Character(CharacterController),
    Audio(AudioSource),
    Camera(Camera),
    Particle(ParticleEmitter),
    MaterialOverride(MaterialOverride),
    Rust {
        registered_name: String,
        /// None 与 Some(空表) 在旧文件中不同；转换期保留原样，不能压成一个默认值。
        fields: Option<StrictFieldValues>,
    },
    Lua {
        path: String,
    },
}

/// v1 的 BTreeMap 遇到同名字段会保留后者；v2 拒绝这种不可逆覆盖。
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct StrictFieldValues(pub crate::inspect::FieldValues);

impl<'de> Deserialize<'de> for StrictFieldValues {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueFields;

        impl<'de> Visitor<'de> for UniqueFields {
            type Value = StrictFieldValues;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("字段名不重复的 Rust 参数表")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields = crate::inspect::FieldValues::new();
                while let Some((name, value)) = map.next_entry()? {
                    if fields.insert(name, value).is_some() {
                        return Err(serde::de::Error::custom("v2 Rust 参数里有重复字段名"));
                    }
                }
                Ok(StrictFieldValues(fields))
            }
        }

        deserializer.deserialize_map(UniqueFields)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderData {
    pub model: String,
    pub lod: LodGroup,
    pub material_asset: Option<String>,
    pub morph_weights: Vec<f32>,
}

impl RenderData {
    fn is_legacy_absent(&self) -> bool {
        self.model.is_empty()
            && self.lod.is_empty()
            && self.material_asset.is_none()
            && self.morph_weights.is_empty()
    }
}

impl ComponentData {
    fn from_v1(entity: &Entity, address: ComponentAddress) -> Self {
        match address {
            ComponentAddress::Builtin(kind) => match kind {
                BuiltinComponent::Transform => unreachable!("Transform 留在实体元数据中"),
                BuiltinComponent::Render => Self::Render(RenderData {
                    model: entity.model.clone(),
                    lod: entity.lod.clone(),
                    material_asset: entity.material_asset.clone(),
                    morph_weights: entity.morph_weights.clone(),
                }),
                BuiltinComponent::Ui => Self::Ui(entity.ui.clone().expect("只映射已挂载组件")),
                BuiltinComponent::Animator => Self::Animator(StrictAnimator(
                    entity.animator.clone().expect("只映射已挂载组件"),
                )),
                BuiltinComponent::Physics => {
                    Self::Physics(entity.physics.expect("只映射已挂载组件"))
                }
                BuiltinComponent::Character => {
                    Self::Character(entity.controller.expect("只映射已挂载组件"))
                }
                BuiltinComponent::Audio => {
                    Self::Audio(entity.audio.clone().expect("只映射已挂载组件"))
                }
                BuiltinComponent::Camera => {
                    Self::Camera(entity.camera.clone().expect("只映射已挂载组件"))
                }
                BuiltinComponent::Particle => {
                    Self::Particle(entity.particle.clone().expect("只映射已挂载组件"))
                }
                BuiltinComponent::MaterialOverride => {
                    Self::MaterialOverride(entity.material.expect("只映射已挂载组件"))
                }
            },
            ComponentAddress::Rust(slot) => {
                let registered_name = entity.behaviors[slot].clone();
                let fields = entity
                    .behavior_params
                    .get(&registered_name)
                    .cloned()
                    .map(StrictFieldValues);
                Self::Rust {
                    registered_name,
                    fields,
                }
            }
            ComponentAddress::Lua(slot) => Self::Lua {
                path: entity.scripts[slot].clone(),
            },
        }
    }

    fn apply_to_v1(self, entity: &mut Entity) -> Result<(), String> {
        match self {
            Self::Render(data) => {
                if data.is_legacy_absent() {
                    return Err("旧格式无法表达空的 Render 组件".into());
                }
                entity.model = data.model;
                entity.lod = data.lod;
                entity.material_asset = data.material_asset;
                entity.morph_weights = data.morph_weights;
            }
            Self::Ui(value) => entity.ui = Some(value),
            Self::Animator(value) => entity.animator = Some(value.0),
            Self::Physics(value) => entity.physics = Some(value),
            Self::Character(value) => entity.controller = Some(value),
            Self::Audio(value) => entity.audio = Some(value),
            Self::Camera(value) => entity.camera = Some(value),
            Self::Particle(value) => entity.particle = Some(value),
            Self::MaterialOverride(value) => entity.material = Some(value),
            Self::Rust {
                registered_name,
                fields,
            } => {
                if entity.behaviors.contains(&registered_name) {
                    return Err(format!("旧格式无法表达同名 Rust 组件 {registered_name:?}"));
                }
                entity.behaviors.push(registered_name.clone());
                if let Some(fields) = fields {
                    entity.behavior_params.insert(registered_name, fields.0);
                }
            }
            Self::Lua { path } => {
                if entity.scripts.contains(&path) {
                    return Err(format!("旧格式无法表达重复 Lua 脚本 {path:?}"));
                }
                entity.scripts.push(path);
            }
        }
        Ok(())
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Render(_) => "Render",
            Self::Ui(_) => "Ui",
            Self::Animator(_) => "Animator",
            Self::Physics(_) => "Physics",
            Self::Character(_) => "Character",
            Self::Audio(_) => "Audio",
            Self::Camera(_) => "Camera",
            Self::Particle(_) => "Particle",
            Self::MaterialOverride(_) => "MaterialOverride",
            Self::Rust { .. } => "Rust",
            Self::Lua { .. } => "Lua",
        }
    }
}

impl EntityV2 {
    /// 所有磁盘资产路径仍由 ProjectHandle 在读写边界转换。
    pub(crate) fn visit_asset_references(&mut self, visit: &mut impl FnMut(&mut String)) {
        if let Some(prefab) = &mut self.prefab {
            visit(&mut prefab.source);
        }
        for entry in &mut self.components.entries {
            match &mut entry.data {
                ComponentData::Render(render) => {
                    visit(&mut render.model);
                    for level in &mut render.lod.levels {
                        visit(&mut level.model);
                    }
                    if let Some(material) = &mut render.material_asset {
                        visit(material);
                    }
                }
                ComponentData::Lua { path } => visit(path),
                ComponentData::Audio(audio) => visit(&mut audio.clip),
                ComponentData::Ui(ui) => {
                    if let Some(image) = &mut ui.image {
                        visit(&mut image.path);
                    }
                }
                ComponentData::Animator(_)
                | ComponentData::Physics(_)
                | ComponentData::Character(_)
                | ComponentData::Camera(_)
                | ComponentData::Particle(_)
                | ComponentData::MaterialOverride(_)
                | ComponentData::Rust { .. } => {}
            }
        }
    }

    fn from_v1(entity: &Entity) -> Result<Self, String> {
        if entity.prefab.is_some() {
            return Err(format!(
                "实体 {:?} 来自预制体；尚未建立来源组件 ID，不允许转换",
                entity.name
            ));
        }
        let plan = preview_component_upgrade(entity, None)?;
        let transform_identity = plan.components.first().expect("Transform 恒存在").identity;
        let entries = plan
            .components
            .into_iter()
            .skip(1)
            .map(|component| ComponentInstance {
                identity: component.identity,
                data: ComponentData::from_v1(entity, component.address),
            })
            .collect();
        Ok(Self {
            id: entity.id,
            name: entity.name.clone(),
            parent: entity.parent,
            prefab: None,
            layer: entity.layer,
            transform: entity.transform,
            transform_identity,
            components: ComponentSet {
                next_component_id: plan.next_component_id,
                entries,
            },
        })
    }

    fn validate(&self) -> Result<(), String> {
        if self.prefab.is_some() {
            return Err("预制体实例尚未建立 v2 来源关联".into());
        }
        if self.transform_identity.source_component_id.is_some()
            || self
                .components
                .entries
                .iter()
                .any(|entry| entry.identity.source_component_id.is_some())
        {
            return Err("普通实体不能携带预制体来源组件 ID".into());
        }
        self.components.next_component_id.validate(
            std::iter::once(self.transform_identity.id).chain(
                self.components
                    .entries
                    .iter()
                    .map(|entry| entry.identity.id),
            ),
        )?;
        let mut kinds = BTreeSet::new();
        for entry in &self.components.entries {
            match &entry.data {
                ComponentData::Rust { .. } | ComponentData::Lua { .. } => {}
                other => {
                    if !kinds.insert(other.kind()) {
                        return Err(format!(
                            "实体 {:?} 的内建组件 {} 重复",
                            self.name,
                            other.kind()
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn into_v1(self) -> Result<Entity, String> {
        self.validate()?;
        let mut entity = Entity::new(self.name);
        entity.id = self.id;
        entity.parent = self.parent;
        entity.layer = self.layer;
        entity.transform = self.transform;
        for entry in self.components.entries {
            entry.data.apply_to_v1(&mut entity)?;
        }
        Ok(entity)
    }
}

impl SceneV2 {
    pub(crate) fn visit_asset_references(&mut self, mut visit: impl FnMut(&mut String)) {
        for entity in &mut self.entities {
            entity.visit_asset_references(&mut visit);
        }
    }

    pub fn from_v1_plain(scene: &Scene) -> Result<Self, String> {
        if scene.format_version != 1 || !scene.duplicate_ids().is_empty() {
            return Err("场景版本或实体 ID 无法安全转换".into());
        }
        let entities = scene
            .entities
            .iter()
            .map(EntityV2::from_v1)
            .collect::<Result<_, _>>()?;
        Ok(Self {
            format_version: 2,
            name: scene.name.clone(),
            entities,
            lights: scene.lights.clone(),
            ambient: scene.ambient,
        })
    }

    pub fn to_v1(self) -> Result<Scene, String> {
        self.validate()?;
        let mut scene = Scene::new(self.name);
        scene.entities = self
            .entities
            .into_iter()
            .map(EntityV2::into_v1)
            .collect::<Result<_, _>>()?;
        scene.lights = self.lights;
        scene.ambient = self.ambient;
        Ok(scene)
    }

    fn validate(&self) -> Result<(), String> {
        if self.format_version != 2 {
            return Err(format!("不支持场景格式版本 {}", self.format_version));
        }
        let mut ids = BTreeSet::new();
        for entity in &self.entities {
            if !ids.insert(entity.id.raw()) {
                return Err(format!("重复的场景实体 ID {}", entity.id.raw()));
            }
            entity.validate()?;
        }
        Ok(())
    }

    pub fn from_ron(text: &str) -> Result<Self, String> {
        let parsed: Self = parse_v2(text)?;
        parsed.validate()?;
        Ok(parsed)
    }

    pub fn to_ron(&self) -> Result<String, String> {
        self.validate()?;
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| e.to_string())
    }
}

impl PrefabV2 {
    pub(crate) fn visit_asset_references(&mut self, mut visit: impl FnMut(&mut String)) {
        for entity in &mut self.entities {
            entity.visit_asset_references(&mut visit);
        }
    }

    pub fn from_v1(prefab: &Prefab) -> Result<Self, String> {
        if prefab.format_version != 1 || !prefab.duplicate_ids().is_empty() {
            return Err("预制体版本或实体 ID 无法安全转换".into());
        }
        let entities = prefab
            .entities
            .iter()
            .map(EntityV2::from_v1)
            .collect::<Result<_, _>>()?;
        Ok(Self {
            format_version: 2,
            name: prefab.name.clone(),
            entities,
        })
    }

    pub fn to_v1(self) -> Result<Prefab, String> {
        self.validate()?;
        let mut prefab = Prefab::new(self.name);
        prefab.entities = self
            .entities
            .into_iter()
            .map(EntityV2::into_v1)
            .collect::<Result<_, _>>()?;
        Ok(prefab)
    }

    fn validate(&self) -> Result<(), String> {
        if self.format_version != 2 {
            return Err(format!("不支持预制体格式版本 {}", self.format_version));
        }
        let mut ids = BTreeSet::new();
        for entity in &self.entities {
            if !ids.insert(entity.id.raw()) {
                return Err(format!("重复的预制体实体 ID {}", entity.id.raw()));
            }
            entity.validate()?;
        }
        Ok(())
    }

    pub fn from_ron(text: &str) -> Result<Self, String> {
        let parsed: Self = parse_v2(text)?;
        parsed.validate()?;
        Ok(parsed)
    }

    pub fn to_ron(&self) -> Result<String, String> {
        self.validate()?;
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| e.to_string())
    }
}
