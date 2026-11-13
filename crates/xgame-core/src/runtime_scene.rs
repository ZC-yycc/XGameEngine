//! 运行时场景载荷：组件值只有一份，v2 稳定身份单独随场景所有权移动。
//!
//! [`Scene`] 仍是物理、渲染和脚本共同修改的值模型。若直接把完整
//! [`crate::scene_v2::SceneV2`] 再塞进运行时，就会出现两份组件值，脚本改了哪份、
//! 查询读哪份会变成新的同步问题。这里因此只从 v2 提取不可变的寻址信息：本地 ID、
//! 来源 ID、持久高水位和组件种类；装载与切关时构造一次，每帧不重建。

use crate::component_id::{ComponentId, ComponentIdSequence, ComponentIdentity};
use crate::scene_v2::{ComponentData, SceneV2};
use crate::{BuiltinComponent, Entity, EntityId, Scene};

/// 组件在 v1 值模型中的稳定种类键。
///
/// 内建组件按枚举定位；Rust / Lua 目前仍分别以注册名和脚本路径区分实例。
/// 将来允许同名多实例时，这里必须与运行时参数 / 脚本状态一同改为按组件 ID 隔离。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeComponentKind {
    Builtin(BuiltinComponent),
    Rust(String),
    Lua(String),
}

impl RuntimeComponentKind {
    fn from_data(data: &ComponentData) -> Self {
        match data {
            ComponentData::Render(_) => Self::Builtin(BuiltinComponent::Render),
            ComponentData::Ui(_) => Self::Builtin(BuiltinComponent::Ui),
            ComponentData::Animator(_) => Self::Builtin(BuiltinComponent::Animator),
            ComponentData::Physics(_) => Self::Builtin(BuiltinComponent::Physics),
            ComponentData::Character(_) => Self::Builtin(BuiltinComponent::Character),
            ComponentData::Audio(_) => Self::Builtin(BuiltinComponent::Audio),
            ComponentData::Camera(_) => Self::Builtin(BuiltinComponent::Camera),
            ComponentData::Particle(_) => Self::Builtin(BuiltinComponent::Particle),
            ComponentData::MaterialOverride(_) => Self::Builtin(BuiltinComponent::MaterialOverride),
            ComponentData::Rust {
                registered_name, ..
            } => Self::Rust(registered_name.clone()),
            ComponentData::Lua { path } => Self::Lua(path.clone()),
        }
    }

    /// 身份侧车只认仍挂在当前值实体上的组件，避免宿主经 `scene_mut()` 删除字段后
    /// 继续查询到一个已经不存在的组件。
    pub fn is_present_on(&self, entity: &Entity) -> bool {
        match self {
            Self::Builtin(kind) => kind.present(entity),
            Self::Rust(name) => entity.behaviors.iter().any(|item| item == name),
            Self::Lua(path) => entity.scripts.iter().any(|item| item == path),
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
            .map(|entity| RuntimeEntityIdentity {
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
            })
            .collect();
        Self { entities }
    }

    pub fn entities(&self) -> &[RuntimeEntityIdentity] {
        &self.entities
    }

    pub fn entity(&self, id: EntityId) -> Option<&RuntimeEntityIdentity> {
        self.entities.iter().find(|entity| entity.entity_id == id)
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
