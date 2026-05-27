//! 场景与实体类型。
//!
//! 场景是游戏世界的顶层容器，包含实体列表。实体现在拥有稳定 ID
//! 和可选的运行时组件集合，向后兼容旧版 RON 场景文件。

use serde::{Deserialize, Serialize};

use crate::component::ComponentMap;
use crate::id::EntityId;
use crate::transform::Transform;

/// 场景图中的单个游戏对象。
///
/// 每个实体有一个稳定标识符（`id`）、名称、变换和关联模型路径。
/// `components` 字段存储运行时组件，不会被序列化到场景文件中。
///
/// 旧版 .ron 文件（不含 `id` 字段）仍可正确反序列化，因为
/// `id` 使用 `#[serde(default)]`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    /// 实体的显示名称。
    pub name: String,
    /// 局部空间变换。
    pub transform: Transform,
    /// glTF 模型路径（相对于工作区）。空字符串表示没有模型。
    pub model: String,
    /// 稳定唯一标识符。旧版文件反序列化时自动分配新 ID。
    #[serde(default)]
    pub id: EntityId,
    /// 运行时组件集合（反序列化时默认为空）。
    #[serde(skip)]
    pub components: ComponentMap,
}

impl Entity {
    /// 创建一个新实体，自动分配 ID。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            transform: Transform::IDENTITY,
            model: String::new(),
            id: EntityId::new(),
            components: ComponentMap::new(),
        }
    }

    /// 从组件创建实体（用于逐步迁移到纯组件方式）。
    pub fn from_name(name: impl Into<String>) -> Self {
        Self::new(name)
    }

    /// 给实体附加模型路径。
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// 设置变换。
    pub fn with_transform(mut self, transform: Transform) -> Self {
        self.transform = transform;
        self
    }

    /// 向实体添加一个运行时组件。
    pub fn add_component<T: crate::component::Component>(&mut self, component: T) -> Option<T> {
        self.components.insert(component)
    }

    /// 获取运行时组件的引用。
    pub fn component<T: crate::component::Component>(&self) -> Option<&T> {
        self.components.get()
    }

    /// 获取运行时组件的可变引用。
    pub fn component_mut<T: crate::component::Component>(&mut self) -> Option<&mut T> {
        self.components.get_mut()
    }

    /// 实体是否有模型关联。
    pub fn has_model(&self) -> bool {
        !self.model.is_empty()
    }
}

/// 包含实体列表的场景。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene {
    /// 场景名称。
    pub name: String,
    /// 场景中的实体列表。
    pub entities: Vec<Entity>,
}

impl Scene {
    /// 创建一个空白场景。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            entities: Vec::new(),
        }
    }

    /// 按 EntityId 查找实体索引（线性搜索）。
    pub fn find_entity(&self, id: EntityId) -> Option<usize> {
        self.entities.iter().position(|e| e.id == id)
    }

    /// 按名称查找实体。
    pub fn find_by_name(&self, name: &str) -> Option<&Entity> {
        self.entities.iter().find(|e| e.name == name)
    }

    /// 添加实体并返回其 ID。
    pub fn add_entity(&mut self, entity: Entity) -> EntityId {
        let id = entity.id;
        self.entities.push(entity);
        id
    }

    /// 按 ID 移除实体。
    pub fn remove_entity(&mut self, id: EntityId) -> Option<Entity> {
        if let Some(idx) = self.find_entity(id) {
            Some(self.entities.remove(idx))
        } else {
            None
        }
    }

    /// 场景是否为空（无实体）。
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// 实体数量。
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// 返回所有具有模型的实体（用于物理初始化等）。
    pub fn entities_with_model(&self) -> impl Iterator<Item = &Entity> {
        self.entities.iter().filter(|e| e.has_model())
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_new_has_id() {
        let e = Entity::new("Test");
        assert_eq!(e.name, "Test");
        // ID 自动分配且非零
        assert_ne!(e.id.raw(), 0);
    }

    #[test]
    fn scene_add_find() {
        let mut scene = Scene::new("Test");
        let e = Entity::new("Box").with_model("test.glb");
        let id = scene.add_entity(e);
        assert_eq!(scene.find_entity(id), Some(0));
        assert!(scene.find_by_name("Box").is_some());
    }

    #[test]
    fn scene_remove() {
        let mut scene = Scene::new("Test");
        let e = Entity::new("ToRemove");
        let id = scene.add_entity(e);
        assert!(scene.remove_entity(id).is_some());
        assert!(scene.is_empty());
    }

    #[test]
    fn entity_id_uniqueness() {
        let a = EntityId::new();
        let b = EntityId::new();
        assert_ne!(a, b);
    }
}
