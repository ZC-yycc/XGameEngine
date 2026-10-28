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

/// 可分配 ID 的上限。
///
/// `u64::MAX` 是**坏数据哨兵**：它会把水位顶到天花板，于是下一次 `EntityId::new()`
/// 立刻返回与文件里那个实体**相同**的 ID，再下一次还会回绕到 0 —— "删一个连坐删
/// 别的"就是这么来的。读盘边界直接拒绝，比事后修补便宜得多。
pub const MAX_ASSIGNABLE_ID: u64 = u64::MAX - 1;

impl EntityId {
    /// 分配一个新的唯一 ID（进程内原子递增）。
    ///
    /// `fetch_add` 在 `u64::MAX` 处会回绕到 0，而 0 从来不是分配器的产物
    /// （手写文件里可能有）—— 回绕后就会撞车。这里用 CAS 循环跳过 0。
    /// 真正走到"空间耗尽"需要 2⁶⁴ 次分配，属于纯理论；语义上保证的是
    /// **不回绕**，而不是无限供应。
    pub fn new() -> Self {
        let mut current = COUNTER.load(Ordering::Relaxed);
        loop {
            let next = current.wrapping_add(1);
            let next = if next == 0 { 1 } else { next };
            match COUNTER.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return Self(current),
                Err(actual) => current = actual,
            }
        }
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
    ///
    /// 超过 [`MAX_ASSIGNABLE_ID`] 的值直接拒绝：接受它等于把计数器顶死（见该常量）。
    fn reserve(raw: u64) -> Result<Self, String> {
        if raw > MAX_ASSIGNABLE_ID {
            return Err(format!(
                "实体 ID {raw} 超过可分配上限 {MAX_ASSIGNABLE_ID}：文件里的 ID 被手改过？"
            ));
        }
        // `fetch_max`（而不是 `store`）：多个场景并发加载时不能把计数器往回拽。
        COUNTER.fetch_max(raw.saturating_add(1), Ordering::Relaxed);
        Ok(Self(raw))
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

        Raw::deserialize(deserializer)
            .and_then(|raw| Self::reserve(raw.0).map_err(serde::de::Error::custom))
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

/// 统计重复的实体 ID：`(id, 出现次数)`，按**首次出现**排序。
///
/// 场景与预制体共用这一份实现 —— 两边的"唯一性作用域"不同（见
/// `Scene::duplicate_ids` 与 `Prefab::duplicate_ids` 的说明），但**怎么看**应当
/// 一致，否则同一份坏数据在两个入口给出不同顺序的报告。
///
/// 顺序不能用 `HashMap` 的迭代顺序：`--check` 的输出要能直接 diff，随机顺序会
/// 让每次运行都不一样。这里用"首次出现过的 ID 序列"当排序依据，一遍扫描出结果。
pub fn duplicate_ids<I>(ids: I) -> Vec<(EntityId, usize)>
where
    I: IntoIterator<Item = EntityId>,
{
    let mut counts: EntityMap<usize> = EntityMap::new();
    let mut first_seen: Vec<EntityId> = Vec::new();
    for id in ids {
        let slot = counts.entry(id).or_insert(0);
        if *slot == 0 {
            first_seen.push(id);
        }
        *slot += 1;
    }
    first_seen
        .into_iter()
        .filter_map(|id| {
            let count = counts[&id];
            (count > 1).then_some((id, count))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;

    use super::*;

    /// 极值 ID 必须在读盘边界就被拒绝。
    ///
    /// 接受它的后果不是"这个实体有点怪"，而是**污染全局计数器**：水位被顶到
    /// `u64::MAX` 之后，下一次 `new()` 直接返回同一个值 —— 两个实体共用一个 ID。
    #[test]
    fn max_id_is_rejected_at_the_load_boundary() {
        let err =
            ron::from_str::<EntityId>("(18446744073709551615)").expect_err("u64::MAX 必须被拒绝");
        let text = err.to_string();
        assert!(
            text.contains("18446744073709551615"),
            "错误信息要带原值才好定位，实得：{text}"
        );
        assert!(
            text.contains("18446744073709551614"),
            "错误信息要给出可分配上限，实得：{text}"
        );
    }

    /// 上限本身（`u64::MAX - 1`）仍然合法 —— 拒绝面只覆盖那一个哨兵值。
    /// 这里只断言"判定"，不真的把它读进来：那会把全局计数器顶死，污染同进程的
    /// 其他测试。
    #[test]
    fn only_the_sentinel_is_out_of_range() {
        // `black_box` 挡掉常量折叠，否则 clippy 会把这几条断言判成恒真。
        let accept = |raw: u64| black_box(raw) <= MAX_ASSIGNABLE_ID;
        assert!(accept(0), "手写文件里的 0 照旧接受");
        assert!(accept(1));
        assert!(accept(4096));
        assert!(accept(MAX_ASSIGNABLE_ID), "上限本身合法");
        assert!(!accept(u64::MAX), "只有哨兵值被拒");
    }

    /// 分配器只前进、不回退，返回的 ID 也永远不是 0。
    #[test]
    fn new_ids_are_increasing_and_never_zero() {
        let a = EntityId::new();
        let b = EntityId::new();
        assert!(a.raw() > 0 && b.raw() > 0, "0 不参与分配");
        assert!(b.raw() > a.raw(), "{b:?} 必须排在 {a:?} 之后");
    }
}
