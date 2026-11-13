//! 运行时场景载荷：组件值只有一份，v2 稳定身份单独随场景所有权移动。
//!
//! [`Scene`] 仍是物理、渲染和脚本共同修改的值模型。若直接把完整
//! [`crate::scene_v2::SceneV2`] 再塞进运行时，就会出现两份组件值，脚本改了哪份、
//! 查询读哪份会变成新的同步问题。这里因此只从 v2 提取不可变的寻址信息：本地 ID、
//! 来源 ID、持久高水位和组件种类；装载与切关时构造一次，每帧不重建。

use crate::component_id::{ComponentId, ComponentIdSequence, ComponentIdentity};
use crate::component_upgrade::preview_component_upgrade;
use crate::scene_v2::{ComponentData, EntityV2, PrefabV2, SceneV2};
use crate::{BuiltinComponent, ComponentAddress, Entity, EntityId, Prefab, Scene};

/// 组件在 v1 值模型中的稳定种类键。
///
/// 内建组件按枚举定位；Rust / Lua 目前仍分别以注册名和脚本路径区分实例。
/// 将来允许同名多实例时，这里必须与运行时参数 / 脚本状态一同改为按组件 ID 隔离。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeComponentKind {
    Builtin(BuiltinComponent),
    Rust(String),
    Lua(String),
    /// 拆分前 v2 文件的复合视觉身份；新文件不会生成。
    LegacyRender,
    /// 拆分前 v2 文件单列的材质覆盖身份；新文件不会生成。
    LegacyMaterialOverride,
}

impl RuntimeComponentKind {
    fn from_data(data: &ComponentData) -> Self {
        match data {
            ComponentData::Mesh(_) => Self::Builtin(BuiltinComponent::Mesh),
            ComponentData::Lod(_) => Self::Builtin(BuiltinComponent::Lod),
            ComponentData::Material(_) => Self::Builtin(BuiltinComponent::Material),
            ComponentData::Ui(_) => Self::Builtin(BuiltinComponent::Ui),
            ComponentData::Animator(_) => Self::Builtin(BuiltinComponent::Animator),
            ComponentData::Physics(_) => Self::Builtin(BuiltinComponent::Physics),
            ComponentData::Character(_) => Self::Builtin(BuiltinComponent::Character),
            ComponentData::Audio(_) => Self::Builtin(BuiltinComponent::Audio),
            ComponentData::Camera(_) => Self::Builtin(BuiltinComponent::Camera),
            ComponentData::Particle(_) => Self::Builtin(BuiltinComponent::Particle),
            ComponentData::Rust {
                registered_name, ..
            } => Self::Rust(registered_name.clone()),
            ComponentData::Lua { path } => Self::Lua(path.clone()),
            ComponentData::LegacyRender(_) => Self::LegacyRender,
            ComponentData::LegacyMaterialOverride(_) => Self::LegacyMaterialOverride,
        }
    }

    fn from_address(address: ComponentAddress, name: String) -> Self {
        match address {
            ComponentAddress::Builtin(kind) => Self::Builtin(kind),
            ComponentAddress::Rust(_) => Self::Rust(name),
            ComponentAddress::Lua(_) => Self::Lua(name),
        }
    }

    /// 身份侧车只认仍挂在当前值实体上的组件，避免宿主经 `scene_mut()` 删除字段后
    /// 继续查询到一个已经不存在的组件。
    pub fn is_present_on(&self, entity: &Entity) -> bool {
        match self {
            Self::Builtin(kind) => kind.present(entity),
            Self::Rust(name) => entity.behaviors.iter().any(|item| item == name),
            Self::Lua(path) => entity.scripts.iter().any(|item| item == path),
            Self::LegacyRender => {
                !entity.model.is_empty()
                    || !entity.lod.is_empty()
                    || entity.material_asset.is_some()
                    || !entity.morph_weights.is_empty()
            }
            Self::LegacyMaterialOverride => entity.material.is_some(),
        }
    }
}

/// 一项可按稳定 ID 查询的运行时组件身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeComponentIdentity {
    pub identity: ComponentIdentity,
    pub kind: RuntimeComponentKind,
}

/// 单个实体的组件身份表；Transform 单列，因为它不可移除。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeEntityIdentity {
    entity_id: EntityId,
    transform: RuntimeComponentIdentity,
    next_component_id: ComponentIdSequence,
    components: Vec<RuntimeComponentIdentity>,
}

impl RuntimeEntityIdentity {
    fn from_v2(entity: &EntityV2) -> Self {
        Self {
            entity_id: entity.id,
            transform: RuntimeComponentIdentity {
                identity: entity.transform_identity,
                kind: RuntimeComponentKind::Builtin(BuiltinComponent::Transform),
            },
            next_component_id: entity.components.next_component_id,
            components: entity
                .components
                .entries
                .iter()
                .map(|component| RuntimeComponentIdentity {
                    identity: component.identity,
                    kind: RuntimeComponentKind::from_data(&component.data),
                })
                .collect(),
        }
    }

    /// 给运行中临时生成的 v1 实体分配本次 World 生命周期内稳定的组件 ID。
    /// 这是生成事件的低频路径，不进入每帧扫描。
    pub fn from_legacy(entity: &Entity) -> Result<Self, String> {
        let plan = preview_component_upgrade(entity, None)?;
        let mut components = plan.components.into_iter();
        let transform = components
            .next()
            .filter(|component| {
                component.address == ComponentAddress::Builtin(BuiltinComponent::Transform)
            })
            .ok_or("运行时实体缺少 Transform 身份")?;
        Ok(Self {
            entity_id: entity.id,
            transform: RuntimeComponentIdentity {
                identity: transform.identity,
                kind: RuntimeComponentKind::Builtin(BuiltinComponent::Transform),
            },
            next_component_id: plan.next_component_id,
            components: components
                .map(|component| RuntimeComponentIdentity {
                    identity: component.identity,
                    kind: RuntimeComponentKind::from_address(component.address, component.name),
                })
                .collect(),
        })
    }

    /// 预制体资产身份 → 新实例的本地身份。初次实例化可让本地 ID 与来源 ID 同值，
    /// 但两者仍分别存放；实例后续新增组件只推进自己的高水位。
    fn instantiate_from_source(&self, entity_id: EntityId) -> Self {
        let sourced = |component: &RuntimeComponentIdentity| RuntimeComponentIdentity {
            identity: ComponentIdentity {
                id: component.identity.id,
                source_component_id: Some(component.identity.id),
            },
            kind: component.kind.clone(),
        };
        Self {
            entity_id,
            transform: sourced(&self.transform),
            next_component_id: self.next_component_id,
            components: self.components.iter().map(sourced).collect(),
        }
    }

    pub fn entity_id(&self) -> EntityId {
        self.entity_id
    }

    pub fn transform(&self) -> &RuntimeComponentIdentity {
        &self.transform
    }

    pub fn next_component_id(&self) -> ComponentIdSequence {
        self.next_component_id
    }

    pub fn components(&self) -> &[RuntimeComponentIdentity] {
        &self.components
    }

    pub fn component(&self, id: ComponentId) -> Option<&RuntimeComponentIdentity> {
        if self.transform.identity.id == id {
            Some(&self.transform)
        } else {
            self.components
                .iter()
                .find(|component| component.identity.id == id)
        }
    }
}

/// 一份场景的 v2 组件寻址侧车。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSceneIdentity {
    entities: Vec<RuntimeEntityIdentity>,
}

impl RuntimeSceneIdentity {
    fn from_v2(scene: &SceneV2) -> Self {
        let entities = scene
            .entities
            .iter()
            .map(RuntimeEntityIdentity::from_v2)
            .collect();
        Self { entities }
    }

    pub fn entities(&self) -> &[RuntimeEntityIdentity] {
        &self.entities
    }

    pub fn entity(&self, id: EntityId) -> Option<&RuntimeEntityIdentity> {
        self.entities.iter().find(|entity| entity.entity_id == id)
    }

    /// 运行时生成实体时登记；实体 ID 已存在就拒绝覆盖原身份。
    pub fn insert_entity(&mut self, entity: RuntimeEntityIdentity) -> bool {
        if self.entity(entity.entity_id).is_some() {
            return false;
        }
        self.entities.push(entity);
        true
    }

    pub fn remove_entity(&mut self, id: EntityId) -> Option<RuntimeEntityIdentity> {
        let index = self
            .entities
            .iter()
            .position(|entity| entity.entity_id == id)?;
        Some(self.entities.remove(index))
    }
}

/// 交给运行时的完整场景所有权：一份可变值 + 可选 v2 身份侧车。
#[derive(Debug, Clone)]
pub struct RuntimeScene {
    scene: Scene,
    identities: Option<RuntimeSceneIdentity>,
}

impl RuntimeScene {
    /// v1 / 测试兼容入口：没有持久组件 ID。
    pub fn legacy(scene: Scene) -> Self {
        Self {
            scene,
            identities: None,
        }
    }

    /// v2 装载边界：先提取身份，再把强类型值投影到现有运行系统唯一读取的 Scene。
    pub fn from_v2(scene: SceneV2) -> Result<Self, String> {
        let identities = RuntimeSceneIdentity::from_v2(&scene);
        let scene = scene.to_v1()?;
        Ok(Self {
            scene,
            identities: Some(identities),
        })
    }

    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    pub fn identities(&self) -> Option<&RuntimeSceneIdentity> {
        self.identities.as_ref()
    }

    pub fn into_scene(self) -> Scene {
        self.scene
    }

    pub fn into_parts(self) -> (Scene, Option<RuntimeSceneIdentity>) {
        (self.scene, self.identities)
    }
}

impl From<Scene> for RuntimeScene {
    fn from(scene: Scene) -> Self {
        Self::legacy(scene)
    }
}

/// 运行时预制体载荷；v2 资产身份只在实例化时转成场景本地身份。
#[derive(Debug, Clone)]
pub struct RuntimePrefab {
    prefab: Prefab,
    identities: Option<Vec<RuntimeEntityIdentity>>,
}

impl RuntimePrefab {
    pub fn legacy(prefab: Prefab) -> Self {
        Self {
            prefab,
            identities: None,
        }
    }

    pub fn from_v2(prefab: PrefabV2) -> Result<Self, String> {
        let identities = prefab
            .entities
            .iter()
            .map(RuntimeEntityIdentity::from_v2)
            .collect();
        let prefab = prefab.to_v1()?;
        Ok(Self {
            prefab,
            identities: Some(identities),
        })
    }

    pub fn prefab(&self) -> &Prefab {
        &self.prefab
    }

    pub fn into_prefab(self) -> Prefab {
        self.prefab
    }

    /// 分配场景实体 ID、重定向父级，并把资产组件 ID 写成每项的来源 ID。
    pub fn instantiate(&self, source: &str) -> Option<RuntimePrefabInstance> {
        let entities = self.prefab.instantiate(source);
        let root_offset = entities
            .iter()
            .position(|entity| entity.prefab.as_ref().is_some_and(|link| link.is_root))?;
        let identities = if let Some(sources) = &self.identities {
            debug_assert_eq!(sources.len(), entities.len());
            sources
                .iter()
                .zip(&entities)
                .map(|(identity, entity)| Some(identity.instantiate_from_source(entity.id)))
                .collect()
        } else {
            // v1 资产没有持久来源 ID；仍尽量给 v2 World 中的临时实例分配本地 ID。
            // 老数据有重复挂载物时预览会失败，只放弃该实体的身份，不阻断既有运行行为。
            entities
                .iter()
                .map(|entity| RuntimeEntityIdentity::from_legacy(entity).ok())
                .collect()
        };
        Some(RuntimePrefabInstance {
            entities,
            identities,
            root_offset,
        })
    }
}

impl From<Prefab> for RuntimePrefab {
    fn from(prefab: Prefab) -> Self {
        Self::legacy(prefab)
    }
}

/// 一次预制体实例化的对齐结果；三组数据按同一实体顺序排列。
#[derive(Debug)]
pub struct RuntimePrefabInstance {
    entities: Vec<Entity>,
    identities: Vec<Option<RuntimeEntityIdentity>>,
    root_offset: usize,
}

impl RuntimePrefabInstance {
    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    pub fn identities(&self) -> &[Option<RuntimeEntityIdentity>] {
        &self.identities
    }

    pub fn root_offset(&self) -> usize {
        self.root_offset
    }

    pub fn into_parts(self) -> (Vec<Entity>, Vec<Option<RuntimeEntityIdentity>>, usize) {
        (self.entities, self.identities, self.root_offset)
    }
}
