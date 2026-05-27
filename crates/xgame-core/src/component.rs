//! 组件 trait 与组件存储。
//!
//! 提供轻量级运行时组件系统：任何 `Send + Sync + 'static` 类型均可作为组件，
//! 通过 `ComponentMap` 附着到实体上。每种组件类型在每个实体上最多出现一次
//!（单件模式），通过 `TypeId` 进行类型安全的存取。

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt::Debug;

// ---------------------------------------------------------------------------
// Component trait
// ---------------------------------------------------------------------------

/// 组件标记 trait。
///
/// 任何 `Send + Sync + 'static` 的类型自动实现 `Component`。
/// 这是引擎组件体系的基础——所有系统（物理、渲染、逻辑）通过组件类型查询实体。
///
/// # 实现示例
///
/// ```ignore
/// #[derive(Clone, Debug)]
/// pub struct Health(pub f32);
///
/// // Health 自动实现 Component，无需额外代码。
/// ```
pub trait Component: Send + Sync + 'static {}

/// Blanket implementation：所有满足约束的类型自动成为组件。
impl<T: Send + Sync + 'static> Component for T {}

// ---------------------------------------------------------------------------
// ComponentMap
// ---------------------------------------------------------------------------

/// 单个实体上附着的组件集合。
///
/// 每种组件类型最多出现一次。提供类型安全的 `insert` / `get` / `get_mut` /
/// `remove` / `has` 方法，无需向下转型样板代码。
///
/// # Example
///
/// ```ignore
/// let mut map = ComponentMap::new();
/// map.insert(Health(100.0));
/// map.insert(Name("Player".into()));
///
/// assert_eq!(map.get::<Name>().unwrap().0, "Player");
/// ```
#[derive(Default)]
pub struct ComponentMap {
    inner: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl ComponentMap {
    /// 创建一个空的组件集合。
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    /// 插入一个组件。如果该类型已存在，返回旧值。
    ///
    /// 时间复杂度：O(1) 摊销。
    pub fn insert<T: Component>(&mut self, component: T) -> Option<T> {
        self.inner
            .insert(TypeId::of::<T>(), Box::new(component))
            .and_then(|b| b.downcast::<T>().ok().map(|b| *b))
    }

    /// 获取组件的不可变引用。
    pub fn get<T: Component>(&self) -> Option<&T> {
        self.inner
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T>())
    }

    /// 获取组件的可变引用。
    pub fn get_mut<T: Component>(&mut self) -> Option<&mut T> {
        self.inner
            .get_mut(&TypeId::of::<T>())
            .and_then(|b| b.downcast_mut::<T>())
    }

    /// 检查是否包含某类型的组件。
    pub fn has<T: Component>(&self) -> bool {
        self.inner.contains_key(&TypeId::of::<T>())
    }

    /// 移除并返回指定类型的组件。
    pub fn remove<T: Component>(&mut self) -> Option<T> {
        self.inner
            .remove(&TypeId::of::<T>())
            .and_then(|b| b.downcast::<T>().ok().map(|b| *b))
    }

    /// 清空所有组件。
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// 组件数量。
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// 迭代所有组件的 TypeId。
    pub fn type_ids(&self) -> impl Iterator<Item = &TypeId> {
        self.inner.keys()
    }
}

// 手动实现 Debug，避免要求所有组件类型实现 Debug。
impl Debug for ComponentMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentMap")
            .field("count", &self.inner.len())
            .finish()
    }
}

// 手动实现 Clone：运行时组件不会被复制（根据各系统需要重新初始化）。
impl Clone for ComponentMap {
    fn clone(&self) -> Self {
        Self::new()
    }
}

// ComponentMap 本身是 Send + Sync（内部所有元素均为 Send + Sync）。
unsafe impl Send for ComponentMap {}
unsafe impl Sync for ComponentMap {}

// ---------------------------------------------------------------------------
// 内置组件类型
// ---------------------------------------------------------------------------

/// 实体名称组件（替换 Entity 的 `name` 字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name(pub String);

impl Name {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl From<String> for Name {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for Name {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// 模型资产引用——标识实体绑定的 glTF 模型路径。
///
/// 对应 Entity 的 `model` 字段，作为组件使用时优先于此字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPath(pub String);

impl ModelPath {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    pub fn path(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<String> for ModelPath {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ModelPath {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}
