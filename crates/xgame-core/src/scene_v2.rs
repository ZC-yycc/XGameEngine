//! v2 的强类型文件模型；此阶段仅做内存转换与 RON 往返。
//!
//! 正式项目加载 / 保存仍只接受 v1。待 GUID 边界、预制体关联和编辑器结构操作
//! 接好后再开放文件级升级，避免把不能编辑的 v2 文件提前交给用户。

use std::collections::{BTreeMap, BTreeSet};

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

use crate::component_id::{ComponentId, ComponentIdSequence, ComponentIdentity};
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

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum LegacyComponentKey {
    Builtin(BuiltinComponent),
    Rust(String),
    Lua(String),
}

impl LegacyComponentKey {
    fn from_address(address: ComponentAddress, name: &str) -> Self {
        match address {
            ComponentAddress::Builtin(kind) => Self::Builtin(kind),
            ComponentAddress::Rust(_) => Self::Rust(name.to_owned()),
            ComponentAddress::Lua(_) => Self::Lua(name.to_owned()),
        }
    }
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
    /// Agent 输入的单个组件也必须走与 v2 文件相同的严格字段解码。
    pub fn from_ron(text: &str) -> Result<Self, String> {
        parse_v2(text)
    }

    fn legacy_key(&self) -> LegacyComponentKey {
        match self {
            Self::Render(_) => LegacyComponentKey::Builtin(BuiltinComponent::Render),
            Self::Ui(_) => LegacyComponentKey::Builtin(BuiltinComponent::Ui),
            Self::Animator(_) => LegacyComponentKey::Builtin(BuiltinComponent::Animator),
            Self::Physics(_) => LegacyComponentKey::Builtin(BuiltinComponent::Physics),
            Self::Character(_) => LegacyComponentKey::Builtin(BuiltinComponent::Character),
            Self::Audio(_) => LegacyComponentKey::Builtin(BuiltinComponent::Audio),
            Self::Camera(_) => LegacyComponentKey::Builtin(BuiltinComponent::Camera),
            Self::Particle(_) => LegacyComponentKey::Builtin(BuiltinComponent::Particle),
            Self::MaterialOverride(_) => {
                LegacyComponentKey::Builtin(BuiltinComponent::MaterialOverride)
            }
            Self::Rust {
                registered_name, ..
            } => LegacyComponentKey::Rust(registered_name.clone()),
            Self::Lua { path } => LegacyComponentKey::Lua(path.clone()),
        }
    }

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
    /// 组件 ID 在本实体内稳定；Transform 是固有属性，不在可移除列表中。
    pub fn component(&self, id: ComponentId) -> Option<&ComponentData> {
        self.components
            .entries
            .iter()
            .find(|entry| entry.identity.id == id)
            .map(|entry| &entry.data)
    }

    pub fn add_component(&mut self, data: ComponentData) -> Result<ComponentId, String> {
        self.validate()?;
        if self
            .components
            .entries
            .iter()
            .any(|entry| entry.data.legacy_key() == data.legacy_key())
        {
            return Err(format!("实体 {:?} 已有同名组件 {}", self.name, data.kind()));
        }
        let id = self.components.next_component_id.allocate()?;
        self.components.entries.push(ComponentInstance {
            identity: ComponentIdentity {
                id,
                source_component_id: None,
            },
            data,
        });
        Ok(id)
    }

    pub fn remove_component(&mut self, id: ComponentId) -> Result<ComponentInstance, String> {
        self.validate()?;
        if id == self.transform_identity.id {
            return Err("Transform 是固有组件，不能移除".into());
        }
        let index = self
            .components
            .entries
            .iter()
            .position(|entry| entry.identity.id == id)
            .ok_or_else(|| format!("实体 {:?} 找不到组件 ID {}", self.name, id.raw()))?;
        Ok(self.components.entries.remove(index))
    }

    /// 只替换同种类的值；本地和来源身份保持不变。
    pub fn replace_component(
        &mut self,
        id: ComponentId,
        data: ComponentData,
    ) -> Result<(), String> {
        self.validate()?;
        let index = self
            .components
            .entries
            .iter()
            .position(|entry| entry.identity.id == id)
            .ok_or_else(|| format!("实体 {:?} 找不到可替换的组件 ID {}", self.name, id.raw()))?;
        let old = &self.components.entries[index];
        if old.data.kind() != data.kind() {
            return Err("组件种类不能通过替换操作改变".into());
        }
        if old.identity.source_component_id.is_some() && old.data.legacy_key() != data.legacy_key()
        {
            return Err("带预制体来源的组件不能改变关联名称或路径".into());
        }
        if self
            .components
            .entries
            .iter()
            .enumerate()
            .any(|(i, entry)| i != index && entry.data.legacy_key() == data.legacy_key())
        {
            return Err("替换后的组件与实体已有的挂载物重复".into());
        }
        self.components.entries[index].data = data;
        Ok(())
    }

    pub fn move_component(&mut self, id: ComponentId, to: usize) -> Result<(), String> {
        self.validate()?;
        let count = self.components.entries.len();
        if to >= count {
            return Err(format!("组件目标位置 {to} 超出 0..{count}"));
        }
        let index = self
            .components
            .entries
            .iter()
            .position(|entry| entry.identity.id == id)
            .ok_or_else(|| format!("实体 {:?} 找不到可重排的组件 ID {}", self.name, id.raw()))?;
        let entry = self.components.entries.remove(index);
        self.components.entries.insert(to, entry);
        Ok(())
    }

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

    fn from_v1(entity: &Entity, source: Option<&EntityV2>) -> Result<Self, String> {
        if entity.prefab.is_some() != source.is_some() {
            return Err(format!("实体 {:?} 的预制体来源尚未提供", entity.name));
        }
        let plan = preview_component_upgrade(entity, None)?;
        let source_ids = source
            .map(EntityV2::source_keys)
            .transpose()?
            .unwrap_or_default();
        let transform_identity = plan.components.first().expect("Transform 恒存在").identity;
        let transform_identity = ComponentIdentity {
            source_component_id: source.map(|s| s.transform_identity.id),
            ..transform_identity
        };
        let entries = plan
            .components
            .into_iter()
            .skip(1)
            .map(|component| ComponentInstance {
                identity: ComponentIdentity {
                    source_component_id: source_ids
                        .get(&LegacyComponentKey::from_address(
                            component.address,
                            &component.name,
                        ))
                        .copied(),
                    ..component.identity
                },
                data: ComponentData::from_v1(entity, component.address),
            })
            .collect();
        Ok(Self {
            id: entity.id,
            name: entity.name.clone(),
            parent: entity.parent,
            prefab: entity.prefab.clone(),
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
        if self.prefab.is_none()
            && (self.transform_identity.source_component_id.is_some()
                || self
                    .components
                    .entries
                    .iter()
                    .any(|entry| entry.identity.source_component_id.is_some()))
        {
            return Err("普通实体不能携带预制体来源组件 ID".into());
        }
        if self.prefab.is_some() && self.transform_identity.source_component_id.is_none() {
            return Err("预制体实例缺少 Transform 来源组件 ID".into());
        }
        let mut sources = BTreeSet::new();
        for id in std::iter::once(self.transform_identity.source_component_id)
            .chain(
                self.components
                    .entries
                    .iter()
                    .map(|entry| entry.identity.source_component_id),
            )
            .flatten()
        {
            if !sources.insert(id) {
                return Err(format!(
                    "实体 {:?} 重复引用来源组件 ID {}",
                    self.name,
                    id.raw()
                ));
            }
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
            if !kinds.insert(entry.data.legacy_key()) {
                return Err(format!(
                    "实体 {:?} 的 {} 组件重复",
                    self.name,
                    entry.data.kind()
                ));
            }
        }
        Ok(())
    }

    fn into_v1(self) -> Result<Entity, String> {
        self.validate()?;
        let mut entity = Entity::new(self.name);
        entity.id = self.id;
        entity.parent = self.parent;
        entity.prefab = self.prefab;
        entity.layer = self.layer;
        entity.transform = self.transform;
        for entry in self.components.entries {
            entry.data.apply_to_v1(&mut entity)?;
        }
        Ok(entity)
    }

    /// 资产实体上的组件身份是来源 ID 的真相；资产组件重排或删除后高水位不回退。
    fn source_keys(&self) -> Result<BTreeMap<LegacyComponentKey, ComponentId>, String> {
        self.validate()?;
        let mut keys = BTreeMap::new();
        for entry in &self.components.entries {
            if keys
                .insert(entry.data.legacy_key(), entry.identity.id)
                .is_some()
            {
                return Err(format!("预制体实体 {:?} 存在无法关联的重复组件", self.name));
            }
        }
        Ok(keys)
    }
}

impl SceneV2 {
    pub(crate) fn visit_asset_references(&mut self, mut visit: impl FnMut(&mut String)) {
        for entity in &mut self.entities {
            entity.visit_asset_references(&mut visit);
        }
    }

    pub fn from_v1_plain(scene: &Scene) -> Result<Self, String> {
        Self::from_v1_with_prefabs(scene, &BTreeMap::new())
    }

    /// 来源资产必须先升级并按项目相对路径提供；只做内存转换，不写项目文件。
    pub fn from_v1_with_prefabs(
        scene: &Scene,
        prefabs: &BTreeMap<String, PrefabV2>,
    ) -> Result<Self, String> {
        if scene.format_version != 1 || !scene.duplicate_ids().is_empty() {
            return Err("场景版本或实体 ID 无法安全转换".into());
        }
        let entities = scene
            .entities
            .iter()
            .map(|entity| {
                let source = if let Some(reference) = &entity.prefab {
                    let prefab = prefabs.get(&reference.source).ok_or_else(|| {
                        format!(
                            "找不到实体 {:?} 的已升级预制体 {}",
                            entity.name, reference.source
                        )
                    })?;
                    prefab.validate()?;
                    Some(
                        prefab
                            .entities
                            .iter()
                            .find(|e| e.id == reference.source_id)
                            .ok_or_else(|| {
                                format!(
                                    "预制体 {} 缺少来源实体 ID {}",
                                    reference.source,
                                    reference.source_id.raw()
                                )
                            })?,
                    )
                } else {
                    None
                };
                EntityV2::from_v1(entity, source)
            })
            .collect::<Result<_, _>>()?;
        let result = Self {
            format_version: 2,
            name: scene.name.clone(),
            entities,
            lights: scene.lights.clone(),
            ambient: scene.ambient,
        };
        result.validate_prefab_sources(prefabs)?;
        Ok(result)
    }

    /// 项目读边界取得来源资产后再次检查，防止来源资产变化使 ID 指向其他组件。
    pub fn validate_prefab_sources(
        &self,
        prefabs: &BTreeMap<String, PrefabV2>,
    ) -> Result<(), String> {
        self.validate()?;
        for entity in &self.entities {
            let Some(reference) = &entity.prefab else {
                continue;
            };
            let prefab = prefabs
                .get(&reference.source)
                .ok_or_else(|| format!("缺少来源预制体 {}", reference.source))?;
            prefab.validate()?;
            let source = prefab
                .entities
                .iter()
                .find(|e| e.id == reference.source_id)
                .ok_or_else(|| {
                    format!(
                        "来源预制体 {} 缺少实体 ID {}",
                        reference.source,
                        reference.source_id.raw()
                    )
                })?;
            if entity.transform_identity.source_component_id != Some(source.transform_identity.id) {
                return Err(format!(
                    "实体 {:?} 的 Transform 来源 ID 不匹配",
                    entity.name
                ));
            }
            let keys = source.source_keys()?;
            for entry in &entity.components.entries {
                if let Some(id) = entry.identity.source_component_id {
                    if keys.get(&entry.data.legacy_key()) != Some(&id) {
                        return Err(format!(
                            "实体 {:?} 的组件来源 ID {} 不匹配",
                            entity.name,
                            id.raw()
                        ));
                    }
                }
            }
        }
        Ok(())
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
            .map(|entity| EntityV2::from_v1(entity, None))
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
            if entity.prefab.is_some() {
                return Err("预制体资产不能包含实例来源引用".into());
            }
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

#[cfg(test)]
mod schema_contract {
    use super::*;

    // 新增落盘字段时让目录与字段表同步变成编译/测试门禁。
    #[allow(unused_variables)]
    #[test]
    fn every_v2_file_field_is_listed_in_the_shared_schema() {
        let doc = crate::schema::scene_schema();
        let scene = SceneV2::from_v1_plain(&Scene::new("Schema")).unwrap();
        let SceneV2 {
            format_version,
            name,
            entities,
            lights,
            ambient,
        } = scene;
        let prefab = PrefabV2::from_v1(&Prefab::new("Schema")).unwrap();
        let PrefabV2 {
            format_version,
            name,
            entities,
        } = prefab;
        let entity = EntityV2::from_v1(&Entity::new("Schema"), None).unwrap();
        let EntityV2 {
            id,
            name,
            parent,
            prefab,
            layer,
            transform,
            transform_identity,
            components,
        } = entity;
        let ComponentSet {
            next_component_id,
            entries,
        } = components;
        let example = ComponentInstance {
            identity: transform_identity,
            data: ComponentData::Camera(Camera::default()),
        };
        let ComponentInstance { identity, data } = example;
        let ComponentIdentity {
            id,
            source_component_id,
        } = identity;
        let render = RenderData {
            model: String::new(),
            lod: LodGroup::default(),
            material_asset: None,
            morph_weights: Vec::new(),
        };
        let RenderData {
            model,
            lod,
            material_asset,
            morph_weights,
        } = render;
        for (name, fields) in [
            (
                "SceneV2",
                vec!["format_version", "name", "entities", "lights", "ambient"],
            ),
            ("PrefabV2", vec!["format_version", "name", "entities"]),
            (
                "EntityV2",
                vec![
                    "id",
                    "name",
                    "parent",
                    "prefab",
                    "layer",
                    "transform",
                    "transform_identity",
                    "components",
                ],
            ),
            ("ComponentSet", vec!["next_component_id", "entries"]),
            ("ComponentInstance", vec!["identity", "data"]),
            ("ComponentIdentity", vec!["id", "source_component_id"]),
            (
                "RenderData",
                vec!["model", "lod", "material_asset", "morph_weights"],
            ),
        ] {
            let catalogued: Vec<_> = doc
                .type_schema(name)
                .unwrap()
                .fields
                .iter()
                .map(|f| f.name.as_str())
                .collect();
            assert_eq!(catalogued, fields, "{name} 的落盘字段目录需要更新");
        }
    }
}
