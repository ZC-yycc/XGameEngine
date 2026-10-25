//! 稳定实体标识符。
//!
//! `EntityId` 是一个轻量的 u64 包装类型，在进程内由原子计数器保证唯一。
//! 用于取代脆弱的 `usize` 索引，实现实体跨帧、跨系统的稳定引用。

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// 全局 ID 分配器。
///
/// 放在模块级（而不是 `new()` 里的 `static`）是因为**反序列化也要推进它**：
/// 读回来的 ID 是上一次运行分配的，见 [`EntityId::reserve`]。
static COUNTER: AtomicU64 = AtomicU64::new(1);

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct EntityId(u64);

impl EntityId {
    /// 分配一个新的唯一 ID（进程内原子递增）。
    pub fn new() -> Self {
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// 收下一个**外部来源**的 ID（读盘 / 反序列化），并把计数器推到它之后。
    ///
    /// # 为什么必须推进计数器
    ///
    /// 场景与预制体文件里存的是裸 ID（`id: (12)`），它们是**上一次运行**分配的。
    /// 而计数器每次进程启动都从 1 重新开始 —— 不在这里对齐的话：
    ///
    /// 1. 读出实体 A（id = 1）；
    /// 2. 新建实体 B，`new()` 返回 1 —— 与 A **撞 ID**；
    /// 3. 删除 B，`Scene::remove_subtree` 按 ID 匹配，把 A 一起删掉。
    ///
    /// 这条链是"删一个物体却连带删了别的"的直接成因，所以在**读盘边界**就把计数器
    /// 推过去，而不是指望每个调用方自己记得补一刀。
    fn reserve(raw: u64) -> Self {
        // `fetch_max`（而不是 `store`）：多个场景并发加载时不能把计数器往回拽。
        // `saturating_add`：`u64::MAX` 是坏数据，别让它回绕成 0。
        COUNTER.fetch_max(raw.saturating_add(1), Ordering::Relaxed);
        Self(raw)
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

/// 反序列化时先 [`EntityId::reserve`] 再落值。
///
/// 这里套一层影子结构体（`Raw`）而不是手写 `deserialize_newtype_struct` +
/// `Visitor`：格式兼容性（RON 的 `(12)`、以及它允许的"有无括号"两种写法）
/// 交回给 serde 的派生实现，减少一次"自己实现 newtype 约定"的机会。
impl<'de> Deserialize<'de> for EntityId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw(u64);

        Raw::deserialize(deserializer).map(|raw| Self::reserve(raw.0))
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
