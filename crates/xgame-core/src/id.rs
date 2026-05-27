//! 稳定实体标识符。
//!
//! `EntityId` 是一个轻量的 u64 包装类型，在进程内由原子计数器保证唯一。
//! 用于取代脆弱的 `usize` 索引，实现实体跨帧、跨系统的稳定引用。

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// 实体的稳定唯一标识符。
///
/// 在调用 `EntityId::new()` 时自动分配，不会在实体删除后重复使用。
///
/// # Example
///
/// ```ignore
/// let id = EntityId::new();
/// assert_ne!(id, EntityId::new());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityId(u64);

impl EntityId {
    /// 分配一个新的唯一 ID（进程内原子递增）。
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// 从原始 u64 值构造（用于反序列化或测试）。
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// 返回底层 u64 值。
    pub fn raw(self) -> u64 {
        self.0
    }
}

impl Default for EntityId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EntityId({})", self.0)
    }
}

// ---------------------------------------------------------------------------
// 辅助容器类型
// ---------------------------------------------------------------------------

/// 以 `EntityId` 为键的 map 的泛型别名。
pub type EntityMap<V> = std::collections::HashMap<EntityId, V>;

/// 以 `EntityId` 为元素的集合。
pub type EntitySet = std::collections::HashSet<EntityId>;
