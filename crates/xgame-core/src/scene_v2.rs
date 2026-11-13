//! 当前场景文件模型：稳定组件身份、严格 RON、编辑器合并及预制体来源同步。
//!
//! 组件值在运行时仍投影到唯一的 [`Scene`] 值模型；这里持有的是跨保存身份与
//! 文件边界。所有 GUID 编解码和条件写盘继续由 [`crate::ProjectHandle`] 负责。

use std::collections::{BTreeMap, BTreeSet};

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Serialize};

/// 当前格式若把未知嵌套字段吞掉再保存，会不可逆地删数据。
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
            "场景文件含不认识的字段，拒绝丢弃: {}",
            ignored.join(", ")
        ))
    }
}

use crate::component_id::{ComponentId, ComponentIdSequence, ComponentIdentity};
use crate::component_identity_plan::plan_runtime_component_identities;
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
    Mesh(MeshData),
    Lod(LodGroup),
    Material(MaterialData),
    Ui(UiElement),
    Animator(StrictAnimator),
    Physics(PhysicsBody),
    Character(CharacterController),
    Audio(AudioSource),
    Camera(Camera),
    Particle(ParticleEmitter),
    Rust {
        registered_name: String,
        /// None 与 Some(空表) 语义不同，投影时保留原样，不能压成一个默认值。
        fields: Option<StrictFieldValues>,
    },
    Lua {
        path: String,
    },
}

/// 普通 BTreeMap 遇到同名字段会保留后者；文件边界拒绝这种不可逆覆盖。
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
pub struct MeshData {
    pub model: String,
    pub morph_weights: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialData {
    pub asset: Option<String>,
    pub overrides: Option<MaterialOverride>,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum ComponentKey {
    Builtin(BuiltinComponent),
    Rust(String),
    Lua(String),
}

impl ComponentKey {
    fn from_address(address: ComponentAddress, name: &str) -> Self {
        match address {
            ComponentAddress::Builtin(kind) => Self::Builtin(kind),
            ComponentAddress::Rust(_) => Self::Rust(name.to_owned()),
            ComponentAddress::Lua(_) => Self::Lua(name.to_owned()),
        }
    }
}

impl MeshData {
    fn is_absent_in_runtime(&self) -> bool {
        self.model.is_empty() && self.morph_weights.is_empty()
    }
}

impl MaterialData {
    fn is_absent_in_runtime(&self) -> bool {
        self.asset.is_none() && self.overrides.is_none()
    }
}

impl ComponentData {
    /// Agent 输入的单个组件也必须走与 v2 文件相同的严格字段解码。
    pub fn from_ron(text: &str) -> Result<Self, String> {
        parse_v2(text)
    }

    fn key(&self) -> ComponentKey {
        match self {
            Self::Mesh(_) => ComponentKey::Builtin(BuiltinComponent::Mesh),
            Self::Lod(_) => ComponentKey::Builtin(BuiltinComponent::Lod),
            Self::Material(_) => ComponentKey::Builtin(BuiltinComponent::Material),
            Self::Ui(_) => ComponentKey::Builtin(BuiltinComponent::Ui),
            Self::Animator(_) => ComponentKey::Builtin(BuiltinComponent::Animator),
            Self::Physics(_) => ComponentKey::Builtin(BuiltinComponent::Physics),
            Self::Character(_) => ComponentKey::Builtin(BuiltinComponent::Character),
            Self::Audio(_) => ComponentKey::Builtin(BuiltinComponent::Audio),
            Self::Camera(_) => ComponentKey::Builtin(BuiltinComponent::Camera),
            Self::Particle(_) => ComponentKey::Builtin(BuiltinComponent::Particle),
            Self::Rust {
                registered_name, ..
            } => ComponentKey::Rust(registered_name.clone()),
            Self::Lua { path } => ComponentKey::Lua(path.clone()),
        }
    }

    fn from_runtime(entity: &Entity, address: ComponentAddress) -> Self {
        match address {
            ComponentAddress::Builtin(kind) => match kind {
                BuiltinComponent::Transform => unreachable!("Transform 留在实体元数据中"),
                BuiltinComponent::Mesh => Self::Mesh(MeshData {
                    model: entity.model.clone(),
                    morph_weights: entity.morph_weights.clone(),
                }),
                BuiltinComponent::Lod => Self::Lod(entity.lod.clone()),
                BuiltinComponent::Material => Self::Material(MaterialData {
                    asset: entity.material_asset.clone(),
                    overrides: entity.material,
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

    fn apply_to_runtime(self, entity: &mut Entity) -> Result<(), String> {
        match self {
            Self::Mesh(data) => {
                if data.is_absent_in_runtime() {
                    return Err("运行时值模型无法表达空的 Mesh 组件".into());
                }
                entity.model = data.model;
                entity.morph_weights = data.morph_weights;
            }
            Self::Lod(data) => {
                if data.is_empty() {
                    return Err("运行时值模型无法表达空的 LOD 组件".into());
                }
                entity.lod = data;
            }
            Self::Material(data) => {
                if data.is_absent_in_runtime() {
                    return Err("运行时值模型无法表达空的 Material 组件".into());
                }
                entity.material_asset = data.asset;
                entity.material = data.overrides;
            }
            Self::Ui(value) => entity.ui = Some(value),
            Self::Animator(value) => entity.animator = Some(value.0),
            Self::Physics(value) => entity.physics = Some(value),
            Self::Character(value) => entity.controller = Some(value),
            Self::Audio(value) => entity.audio = Some(value),
            Self::Camera(value) => entity.camera = Some(value),
            Self::Particle(value) => entity.particle = Some(value),
            Self::Rust {
                registered_name,
                fields,
            } => {
                if entity.behaviors.contains(&registered_name) {
                    return Err(format!(
                        "运行时值模型无法表达同名 Rust 组件 {registered_name:?}"
                    ));
                }
                entity.behaviors.push(registered_name.clone());
                if let Some(fields) = fields {
                    entity.behavior_params.insert(registered_name, fields.0);
                }
            }
            Self::Lua { path } => {
                if entity.scripts.contains(&path) {
                    return Err(format!("运行时值模型无法表达重复 Lua 脚本 {path:?}"));
                }
                entity.scripts.push(path);
            }
        }
        Ok(())
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Mesh(_) => "Mesh",
            Self::Lod(_) => "Lod",
            Self::Material(_) => "Material",
            Self::Ui(_) => "Ui",
            Self::Animator(_) => "Animator",
            Self::Physics(_) => "Physics",
            Self::Character(_) => "Character",
            Self::Audio(_) => "Audio",
            Self::Camera(_) => "Camera",
            Self::Particle(_) => "Particle",
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
            .any(|entry| entry.data.key() == data.key())
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
        if old.identity.source_component_id.is_some() && old.data.key() != data.key() {
            return Err("带预制体来源的组件不能改变关联名称或路径".into());
        }
        if self
            .components
            .entries
            .iter()
            .enumerate()
            .any(|(i, entry)| i != index && entry.data.key() == data.key())
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
                ComponentData::Mesh(mesh) => visit(&mut mesh.model),
                ComponentData::Lod(lod) => {
                    for level in &mut lod.levels {
                        visit(&mut level.model);
                    }
                }
                ComponentData::Material(material) => {
                    if let Some(material) = &mut material.asset {
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
                | ComponentData::Rust { .. } => {}
            }
        }
    }

    fn from_runtime(entity: &Entity, source: Option<&EntityV2>) -> Result<Self, String> {
        if entity.prefab.is_some() != source.is_some() {
            return Err(format!("实体 {:?} 的预制体来源尚未提供", entity.name));
        }
        let plan = plan_runtime_component_identities(entity, None)?;
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
                        .get(&ComponentKey::from_address(
                            component.address,
                            &component.name,
                        ))
                        .copied(),
                    ..component.identity
                },
                data: ComponentData::from_runtime(entity, component.address),
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
            if !kinds.insert(entry.data.key()) {
                return Err(format!(
                    "实体 {:?} 的 {} 组件重复",
                    self.name,
                    entry.data.kind()
                ));
            }
        }
        Ok(())
    }

    fn into_runtime(self) -> Result<Entity, String> {
        self.validate()?;
        let mut entity = Entity::new(self.name);
        entity.id = self.id;
        entity.parent = self.parent;
        entity.prefab = self.prefab;
        entity.layer = self.layer;
        entity.transform = self.transform;
        for entry in self.components.entries {
            entry.data.apply_to_runtime(&mut entity)?;
        }
        Ok(entity)
    }

    /// 资产实体上的组件身份是来源 ID 的真相；资产组件重排或删除后高水位不回退。
    fn source_keys(&self) -> Result<BTreeMap<ComponentKey, ComponentId>, String> {
        self.validate()?;
        let mut keys = BTreeMap::new();
        for entry in &self.components.entries {
            if keys
                .insert(entry.data.key(), entry.identity.id)
                .is_some()
            {
                return Err(format!("预制体实体 {:?} 存在无法关联的重复组件", self.name));
            }
        }
        Ok(keys)
    }

    fn validate_source_identity(&self, source: &EntityV2) -> Result<(), String> {
        if self.transform_identity.source_component_id != Some(source.transform_identity.id) {
            return Err(format!("实体 {:?} 的 Transform 来源 ID 不匹配", self.name));
        }
        let keys = source.source_keys()?;
        for entry in &self.components.entries {
            if let Some(id) = entry.identity.source_component_id {
                if keys.get(&entry.data.key()) != Some(&id) {
                    return Err(format!(
                        "实体 {:?} 的组件来源 ID {} 不匹配",
                        self.name,
                        id.raw()
                    ));
                }
            }
        }
        Ok(())
    }
}

impl SceneV2 {
    /// 项目读取边界保留既有的场景自愈契约：后出现的重复实体换新 ID，父引用仍
    /// 指向第一个同 ID 实体。直接 [`Self::from_ron`] 继续严格拒绝重复，避免工具
    /// 或写入路径把坏文件误当成可原样保存的文件模型。
    pub(crate) fn from_ron_repairing_duplicate_entity_ids(
        text: &str,
    ) -> Result<(Self, usize), String> {
        let mut parsed: Self = parse_v2(text)?;
        parsed.validate_format_version()?;
        let mut ids = BTreeSet::new();
        let mut repaired = 0;
        for entity in &mut parsed.entities {
            if ids.insert(entity.id.raw()) {
                continue;
            }
            entity.id = EntityId::new();
            ids.insert(entity.id.raw());
            repaired += 1;
        }
        parsed.validate()?;
        Ok((parsed, repaired))
    }

    /// 撤销只恢复组件内容；已经发放过的 ID 不随历史快照回退。
    pub fn keep_allocated_ids(&mut self, later: &Self) {
        for entity in &mut self.entities {
            if let Some(other) = later.entities.iter().find(|e| e.id == entity.id) {
                let next = entity
                    .components
                    .next_component_id
                    .next_id()
                    .max(other.components.next_component_id.next_id());
                entity.components.next_component_id =
                    ComponentIdSequence::from_next(next).expect("两份已验证场景的高水位必须合法");
            }
        }
    }

    /// 将编辑器的可变 [`Scene`] 运行时视图合入组件身份。先完整构造候选再替换，
    /// 避免一份坏草稿让会话中的已保存 ID / 高水位发生半途改变。
    pub fn reconcile_from_runtime(
        &mut self,
        scene: &Scene,
        prefabs: &BTreeMap<String, PrefabV2>,
    ) -> Result<(), String> {
        self.reconcile_from_runtime_inner(scene, prefabs, true)
    }

    /// 撤销快照允许保留已被外部资产删除的旧来源；不能将结果当成可保存场景。
    pub fn reconcile_from_runtime_for_history(
        &mut self,
        scene: &Scene,
        prefabs: &BTreeMap<String, PrefabV2>,
    ) -> Result<(), String> {
        self.reconcile_from_runtime_inner(scene, prefabs, false)
    }

    fn reconcile_from_runtime_inner(
        &mut self,
        scene: &Scene,
        prefabs: &BTreeMap<String, PrefabV2>,
        verify_sources: bool,
    ) -> Result<(), String> {
        if !scene.duplicate_ids().is_empty() {
            return Err("编辑器场景的实体 ID 无法安全合入文件模型".into());
        }
        let previous: BTreeMap<_, _> = self.entities.iter().map(|e| (e.id.raw(), e)).collect();
        let mut entities = Vec::with_capacity(scene.entities.len());
        for entity in &scene.entities {
            let source = if let Some(link) = &entity.prefab {
                Some(
                    prefabs
                        .get(&link.source)
                        .ok_or_else(|| format!("找不到来源预制体 {}", link.source))?
                        .entities
                        .iter()
                        .find(|e| e.id == link.source_id)
                        .ok_or_else(|| {
                            format!(
                                "来源预制体 {} 缺少实体 ID {}",
                                link.source,
                                link.source_id.raw()
                            )
                        })?,
                )
            } else {
                None
            };
            let Some(old) = previous.get(&entity.id.raw()) else {
                entities.push(EntityV2::from_runtime(entity, source)?);
                continue;
            };
            let mut updated = EntityV2::from_runtime(entity, source)?;
            if old.prefab.is_some() && updated.prefab.is_some() && old.prefab != updated.prefab {
                return Err(format!(
                    "实体 {:?} 的预制体来源发生变化；请先完成预制体身份同步",
                    entity.name
                ));
            }
            let newly_linked = old.prefab.is_none() && updated.prefab.is_some();
            let detached = old.prefab.is_some() && updated.prefab.is_none();
            let source_keys = source.map(EntityV2::source_keys).transpose()?;
            updated.transform_identity = old.transform_identity;
            if newly_linked {
                updated.transform_identity.source_component_id =
                    source.map(|s| s.transform_identity.id);
            } else if detached {
                updated.transform_identity.source_component_id = None;
            }
            updated.components.next_component_id = old.components.next_component_id;
            let mut pending = updated.components.entries;
            let mut entries = Vec::with_capacity(pending.len());
            let mut unmatched = Vec::new();
            for old_entry in &old.components.entries {
                if let Some(index) = pending
                    .iter()
                    .position(|new| new.data.key() == old_entry.data.key())
                {
                    let mut matched = pending.remove(index);
                    matched.identity = old_entry.identity;
                    if newly_linked {
                        matched.identity.source_component_id = source_keys
                            .as_ref()
                            .and_then(|keys| keys.get(&matched.data.key()).copied());
                    } else if detached {
                        matched.identity.source_component_id = None;
                    }
                    entries.push(matched);
                } else {
                    unmatched.push(old_entry);
                }
            }
            let mut renamed = vec![None; pending.len()];
            for old_entry in &unmatched {
                // Lua 路径 / Rust 注册名在运行时视图里同时充当列表键；只有唯一的
                // 旧项与唯一的新项时才能判定为原组件改了字段。多项时宁可分配新 ID。
                let kind = old_entry.data.kind();
                if (verify_sources && old_entry.identity.source_component_id.is_some())
                    || !matches!(
                        old_entry.data,
                        ComponentData::Lua { .. } | ComponentData::Rust { .. }
                    )
                    || unmatched
                        .iter()
                        .filter(|item| item.data.kind() == kind)
                        .count()
                        != 1
                {
                    continue;
                }
                let mut candidates = pending
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| entry.data.kind() == kind);
                if let (Some((index, _)), None) = (candidates.next(), candidates.next()) {
                    renamed[index] = Some(old_entry.identity);
                }
            }
            for (index, mut entry) in pending.drain(..).enumerate() {
                if let Some(identity) = renamed[index] {
                    entry.identity = identity;
                } else {
                    entry.identity.id = updated.components.next_component_id.allocate()?;
                    // 新实例上的本地组件不因资产恰有同名种类就认领来源。
                    if !newly_linked {
                        entry.identity.source_component_id = None;
                    }
                }
                entries.push(entry);
            }
            updated.components.entries = entries;
            entities.push(updated);
        }
        let candidate = Self {
            format_version: 2,
            name: scene.name.clone(),
            entities,
            lights: scene.lights.clone(),
            ambient: scene.ambient,
        };
        if verify_sources {
            candidate.validate_prefab_sources(prefabs)?;
        } else {
            candidate.validate()?;
        }
        *self = candidate;
        Ok(())
    }

    /// Revert 后依来源组件 ID 认回实例局部 ID，而不是依当前值里的 Lua 路径等旧键。
    /// 被资产删去的组件一并消失；资产新增组件从原实例高水位分配。
    pub fn reconcile_reverted_from_runtime(
        &mut self,
        scene: &Scene,
        prefabs: &BTreeMap<String, PrefabV2>,
        restored_ids: &[EntityId],
    ) -> Result<(), String> {
        let before = self.clone();
        let mut candidate = before.clone();
        candidate
            .entities
            .retain(|entity| !restored_ids.contains(&entity.id));
        // 其他实例可能还指向被删掉的来源组件；逐个 Revert 完成前
        // 仅让历史保留它们，保存时仍须验证整个场景。
        candidate.reconcile_from_runtime_for_history(scene, prefabs)?;
        for entity in &mut candidate.entities {
            if !restored_ids.contains(&entity.id) {
                continue;
            }
            let Some(old) = before.entities.iter().find(|old| old.id == entity.id) else {
                continue;
            };
            let Some(link) = &entity.prefab else {
                return Err(format!("还原的实体 {:?} 缺少来源关联", entity.name));
            };
            let source = prefabs
                .get(&link.source)
                .and_then(|prefab| {
                    prefab
                        .entities
                        .iter()
                        .find(|item| item.id == link.source_id)
                })
                .ok_or_else(|| {
                    format!(
                        "预制体 {} 缺少实体 ID {}",
                        link.source,
                        link.source_id.raw()
                    )
                })?;
            entity.transform_identity.id = old.transform_identity.id;
            entity.transform_identity.source_component_id = Some(source.transform_identity.id);
            let mut next = old.components.next_component_id;
            for entry in &mut entity.components.entries {
                if let Some(previous) = old.components.entries.iter().find(|previous| {
                    previous.identity.source_component_id.is_some()
                        && previous.identity.source_component_id
                            == entry.identity.source_component_id
                        && previous.data.kind() == entry.data.kind()
                }) {
                    entry.identity.id = previous.identity.id;
                } else {
                    entry.identity.id = next.allocate()?;
                }
            }
            entity.components.next_component_id = next;
        }
        let mut restored_only = candidate.clone();
        restored_only
            .entities
            .retain(|entity| restored_ids.contains(&entity.id));
        restored_only.validate_prefab_sources(prefabs)?;
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    pub(crate) fn visit_asset_references(&mut self, mut visit: impl FnMut(&mut String)) {
        for entity in &mut self.entities {
            entity.visit_asset_references(&mut visit);
        }
    }

    pub fn from_runtime(scene: &Scene) -> Result<Self, String> {
        Self::from_runtime_with_prefabs(scene, &BTreeMap::new())
    }

    /// 来源资产按项目相对路径提供；只做内存投影，不写项目文件。
    pub fn from_runtime_with_prefabs(
        scene: &Scene,
        prefabs: &BTreeMap<String, PrefabV2>,
    ) -> Result<Self, String> {
        if !scene.duplicate_ids().is_empty() {
            return Err("场景实体 ID 无法安全投影到文件模型".into());
        }
        let entities = scene
            .entities
            .iter()
            .map(|entity| {
                let source = if let Some(reference) = &entity.prefab {
                    let prefab = prefabs.get(&reference.source).ok_or_else(|| {
                        format!(
                            "找不到实体 {:?} 的来源预制体 {}",
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
                EntityV2::from_runtime(entity, source)
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
            entity.validate_source_identity(source)?;
        }
        Ok(())
    }

    /// 项目检查只验证能够成功读取且仍含目标实体的来源；缺文件、坏文件和悬空
    /// `source_id` 由各自诊断分桶。这样组件来源损坏仍是场景内容错误，同时一个坏
    /// 预制体不会遮住整张场景里的其余问题。
    pub(crate) fn validate_available_prefab_sources(
        &self,
        prefabs: &BTreeMap<String, PrefabV2>,
    ) -> Result<(), String> {
        self.validate()?;
        for entity in &self.entities {
            let Some(reference) = &entity.prefab else {
                continue;
            };
            let Some(prefab) = prefabs.get(&reference.source) else {
                continue;
            };
            prefab.validate()?;
            let Some(source) = prefab
                .entities
                .iter()
                .find(|source| source.id == reference.source_id)
            else {
                continue;
            };
            entity.validate_source_identity(source)?;
        }
        Ok(())
    }

    /// 让一份场景适配预制体的新来源目录，只改来源元数据，不覆盖实例本地值。
    ///
    /// 结构性 Apply 不能用 Revert 同步其他实例：Revert 会把字段值也覆盖成资产值，
    /// 等于静默吃掉尚未有逐字段 override 记录的本地修改。这里因此只做三件事：
    /// 存活来源继续关联；已删除实体变成本地实体；已删除或改名的组件解除来源关联。
    pub fn synchronize_prefab_source_metadata(
        &mut self,
        source: &str,
        prefab: &PrefabV2,
    ) -> Result<(), String> {
        prefab.validate()?;
        for entity in &mut self.entities {
            let Some(reference) = entity
                .prefab
                .as_ref()
                .filter(|reference| reference.source == source)
            else {
                continue;
            };
            let source_id = reference.source_id;
            let Some(asset_entity) = prefab.entities.iter().find(|item| item.id == source_id)
            else {
                entity.prefab = None;
                entity.transform_identity.source_component_id = None;
                for entry in &mut entity.components.entries {
                    entry.identity.source_component_id = None;
                }
                continue;
            };
            entity.transform_identity.source_component_id =
                Some(asset_entity.transform_identity.id);
            for entry in &mut entity.components.entries {
                let Some(source_component_id) = entry.identity.source_component_id else {
                    continue;
                };
                let still_matches = asset_entity.components.entries.iter().any(|asset_entry| {
                    asset_entry.identity.id == source_component_id
                        && asset_entry.data.key() == entry.data.key()
                });
                if !still_matches {
                    entry.identity.source_component_id = None;
                }
            }
        }
        self.validate()
    }

    pub fn into_runtime(self) -> Result<Scene, String> {
        self.validate()?;
        let mut scene = Scene::new(self.name);
        scene.entities = self
            .entities
            .into_iter()
            .map(EntityV2::into_runtime)
            .collect::<Result<_, _>>()?;
        scene.lights = self.lights;
        scene.ambient = self.ambient;
        Ok(scene)
    }

    fn validate(&self) -> Result<(), String> {
        self.validate_format_version()?;
        let mut ids = BTreeSet::new();
        for entity in &self.entities {
            if !ids.insert(entity.id.raw()) {
                return Err(format!("重复的场景实体 ID {}", entity.id.raw()));
            }
            entity.validate()?;
        }
        Ok(())
    }

    pub(crate) fn validate_format_version(&self) -> Result<(), String> {
        if self.format_version != 2 {
            return Err(format!("不支持场景格式版本 {}", self.format_version));
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
    /// 将可变的运行时值合入已有文件身份；实体和组件删除不回退高水位。
    pub fn reconcile_from_runtime(&mut self, prefab: &Prefab) -> Result<(), String> {
        let mut values = Scene::new(prefab.name.clone());
        values.entities.clone_from(&prefab.entities);
        let mut identities = SceneV2 {
            format_version: 2,
            name: self.name.clone(),
            entities: self.entities.clone(),
            lights: values.lights.clone(),
            ambient: values.ambient,
        };
        identities.reconcile_from_runtime(&values, &BTreeMap::new())?;
        let candidate = Self {
            format_version: 2,
            name: identities.name,
            entities: identities.entities,
        };
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    /// 从一个编辑中的实例生成新的资产结构，并把该实例绑定到新来源身份。
    ///
    /// `identity` 必须已经由 [`SceneV2::reconcile_from_runtime_for_history`] 合入当前
    /// `scene` 的值。存活来源沿用资产 ID；新增实体 / 组件从资产侧独立分配，不能
    /// 把实例本地 ID 误当成资产 ID。全部候选验证成功后才替换两个输入。
    pub fn apply_structure_from_instance(
        &self,
        scene: &mut Scene,
        identity: &mut SceneV2,
        root_idx: usize,
        source: &str,
    ) -> Result<Self, String> {
        self.validate()?;
        identity.validate()?;
        if !scene.duplicate_ids().is_empty() {
            return Err("场景含重复实体 ID，不能安全应用预制体结构".into());
        }
        let root = scene.entities.get(root_idx).ok_or("预制体实例根索引越界")?;
        if !root
            .prefab
            .as_ref()
            .is_some_and(|reference| reference.source == source && reference.is_root)
        {
            return Err("所选实体不是目标预制体的实例根".into());
        }

        let mut scene_candidate = scene.clone();
        let mut identity_candidate = identity.clone();
        let indices = scene_candidate.subtree(root_idx);
        if indices.is_empty() {
            return Err("预制体实例为空".into());
        }

        // 先冻结实体映射，父级重写与后续组件处理都只读这一份。
        let mut asset_ids = BTreeMap::new();
        let mut used_asset_ids = BTreeSet::new();
        for &index in &indices {
            let entity = &scene_candidate.entities[index];
            let asset_id = match entity
                .prefab
                .as_ref()
                .filter(|reference| reference.source == source)
            {
                Some(reference) => {
                    if !self
                        .entities
                        .iter()
                        .any(|item| item.id == reference.source_id)
                    {
                        return Err(format!(
                            "实例实体 {:?} 指向资产中不存在的来源实体 ID {}",
                            entity.name,
                            reference.source_id.raw()
                        ));
                    }
                    reference.source_id
                }
                None => EntityId::new(),
            };
            if !used_asset_ids.insert(asset_id.raw()) {
                return Err(format!("实例里重复引用来源实体 ID {}", asset_id.raw()));
            }
            asset_ids.insert(entity.id.raw(), asset_id);
        }

        let mut asset_entities = Vec::with_capacity(indices.len());
        for &index in &indices {
            let local_id = scene_candidate.entities[index].id;
            let value_name = scene_candidate.entities[index].name.clone();
            let value_parent = scene_candidate.entities[index].parent;
            let old_source_id = scene_candidate.entities[index]
                .prefab
                .as_ref()
                .filter(|reference| reference.source == source)
                .map(|reference| reference.source_id);
            let asset_id = asset_ids[&local_id.raw()];
            let identity_index = identity_candidate
                .entities
                .iter()
                .position(|item| item.id == local_id)
                .ok_or_else(|| format!("实体 {value_name:?} 缺少 v2 组件身份"))?;
            let mut local = identity_candidate.entities[identity_index].clone();
            let old_asset = old_source_id
                .and_then(|source_id| self.entities.iter().find(|item| item.id == source_id));

            let mut asset_entries = Vec::with_capacity(local.components.entries.len());
            let (asset_transform_id, mut next_component_id) = if let Some(old) = old_asset {
                (old.transform_identity.id, old.components.next_component_id)
            } else {
                let mut next = ComponentIdSequence::default();
                (next.allocate()?, next)
            };
            let mut local_sources = Vec::with_capacity(local.components.entries.len());
            for entry in &local.components.entries {
                let asset_component_id =
                    if let Some(source_component_id) = entry.identity.source_component_id {
                        let previous = old_asset
                            .and_then(|old| {
                                old.components
                                    .entries
                                    .iter()
                                    .find(|item| item.identity.id == source_component_id)
                            })
                            .ok_or_else(|| {
                                format!(
                                    "实例实体 {:?} 的来源组件 ID {} 已不在旧资产中",
                                    value_name,
                                    source_component_id.raw()
                                )
                            })?;
                        if previous.data.kind() != entry.data.kind() {
                            return Err(format!(
                                "实例实体 {:?} 的来源组件 ID {} 种类发生变化",
                                value_name,
                                source_component_id.raw()
                            ));
                        }
                        source_component_id
                    } else {
                        next_component_id.allocate()?
                    };
                local_sources.push(asset_component_id);
                asset_entries.push(ComponentInstance {
                    identity: ComponentIdentity {
                        id: asset_component_id,
                        source_component_id: None,
                    },
                    data: entry.data.clone(),
                });
            }

            let link = PrefabInstanceRef {
                source: source.to_owned(),
                source_id: asset_id,
                is_root: index == root_idx,
            };
            scene_candidate.entities[index].prefab = Some(link.clone());
            local.id = local_id;
            local.parent = value_parent;
            local.prefab = Some(link);
            local.transform_identity.source_component_id = Some(asset_transform_id);
            for (entry, source_component_id) in local
                .components
                .entries
                .iter_mut()
                .zip(local_sources.iter().copied())
            {
                entry.identity.source_component_id = Some(source_component_id);
            }
            identity_candidate.entities[identity_index] = local.clone();

            local.id = asset_id;
            local.parent = value_parent.and_then(|parent| asset_ids.get(&parent.raw()).copied());
            local.prefab = None;
            local.transform_identity = ComponentIdentity {
                id: asset_transform_id,
                source_component_id: None,
            };
            local.components = ComponentSet {
                next_component_id,
                entries: asset_entries,
            };
            asset_entities.push(local);
        }

        let candidate = Self {
            format_version: 2,
            name: self.name.clone(),
            entities: asset_entities,
        };
        candidate.validate()?;
        identity_candidate.synchronize_prefab_source_metadata(source, &candidate)?;
        // 来源实体被删除时，身份侧车会把对应场景实体转成本地实体；值视图必须同步
        // 这一个元数据字段，否则下一次 reconcile 又会把旧来源写回来。
        for value in &mut scene_candidate.entities {
            if let Some(stable) = identity_candidate
                .entities
                .iter()
                .find(|item| item.id == value.id)
            {
                value.prefab.clone_from(&stable.prefab);
            }
        }
        identity_candidate.validate()?;
        *scene = scene_candidate;
        *identity = identity_candidate;
        Ok(candidate)
    }

    /// 仅当资产和实例结构一致时合入值：来源组件 ID 与高水位逐条沿用。
    /// 结构变更需同时写回场景来源关联，不能借此入口隐式发布。
    pub fn apply_values_from_runtime(&mut self, prefab: &Prefab) -> Result<(), String> {
        let incoming = Self::from_runtime(prefab)?;
        if incoming.entities.len() != self.entities.len() {
            return Err("预制体实体结构已变化，须同步场景来源关联".into());
        }
        let mut candidate = self.clone();
        candidate.name = incoming.name;
        for entity in &mut candidate.entities {
            let next = incoming
                .entities
                .iter()
                .find(|item| item.id == entity.id)
                .ok_or_else(|| format!("预制体实体 {:?} 的来源 ID 已变化", entity.name))?;
            if next.components.entries.len() != entity.components.entries.len() {
                return Err(format!("实体 {:?} 的组件结构已变化", entity.name));
            }
            entity.name.clone_from(&next.name);
            entity.parent = next.parent;
            entity.layer = next.layer;
            entity.transform = next.transform;
            for entry in &mut entity.components.entries {
                let value = next
                    .components
                    .entries
                    .iter()
                    .find(|item| item.data.key() == entry.data.key())
                    .ok_or_else(|| format!("实体 {:?} 的组件种类或关联键已变化", entity.name))?;
                entry.data = value.data.clone();
            }
        }
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    pub(crate) fn visit_asset_references(&mut self, mut visit: impl FnMut(&mut String)) {
        for entity in &mut self.entities {
            entity.visit_asset_references(&mut visit);
        }
    }

    pub fn from_runtime(prefab: &Prefab) -> Result<Self, String> {
        if !prefab.duplicate_ids().is_empty() {
            return Err("预制体实体 ID 无法安全投影到文件模型".into());
        }
        let entities = prefab
            .entities
            .iter()
            .map(|entity| EntityV2::from_runtime(entity, None))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            format_version: 2,
            name: prefab.name.clone(),
            entities,
        })
    }

    pub fn into_runtime(self) -> Result<Prefab, String> {
        self.validate()?;
        let mut prefab = Prefab::new(self.name);
        prefab.entities = self
            .entities
            .into_iter()
            .map(EntityV2::into_runtime)
            .collect::<Result<_, _>>()?;
        Ok(prefab)
    }

    fn validate(&self) -> Result<(), String> {
        self.validate_format_version()?;
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

    pub(crate) fn validate_format_version(&self) -> Result<(), String> {
        if self.format_version != 2 {
            return Err(format!("不支持预制体格式版本 {}", self.format_version));
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
        let scene = SceneV2::from_runtime(&Scene::new("Schema")).unwrap();
        let SceneV2 {
            format_version,
            name,
            entities,
            lights,
            ambient,
        } = scene;
        let prefab = PrefabV2::from_runtime(&Prefab::new("Schema")).unwrap();
        let PrefabV2 {
            format_version,
            name,
            entities,
        } = prefab;
        let entity = EntityV2::from_runtime(&Entity::new("Schema"), None).unwrap();
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
        let mesh = MeshData {
            model: String::new(),
            morph_weights: Vec::new(),
        };
        let MeshData {
            model,
            morph_weights,
        } = mesh;
        let material = MaterialData {
            asset: None,
            overrides: None,
        };
        let MaterialData { asset, overrides } = material;
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
            ("MeshData", vec!["model", "morph_weights"]),
            ("MaterialData", vec!["asset", "overrides"]),
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
