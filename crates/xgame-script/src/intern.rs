//! 脚本路径驻留表：路径 → 稳定 `u32`。
//!
//! # 为什么需要它（先量后改的"量"）
//!
//! 派发队列和每 (实体, 脚本) 状态表原本都拿 `String` 当载体：建队列时每实体
//! 克隆一次、构造状态表 key 时每钩子再克隆一次 —— 两者都在**每帧热路径**上。
//! 本机实测（1000 实体 × 300 帧，探针数据）：队列克隆比 u32 驻留贵
//! ≈32.6 µs/帧，状态表 String key 比 `(EntityId, u32)` 贵 ≈77.8 µs/帧，
//! 合计 ≈110 µs/帧的纯克隆开销。驻留表把每条路径的 `String` 只分配一次，
//! 热路径上换成 `u32` 拷贝（纳秒级，且不碰分配器）。
//!
//! # 不能推翻的约定：id 永不回收，表永不清空
//!
//! 状态表的键是 `(EntityId, 路径 id)`。热重载（宿主上的 `invalidate` /
//! `invalidate_all`）只清**编译缓存**，驻留表必须原样保留 —— 否则同一个脚本
//! 重编译后拿到新 id，每个实体的状态会静默错位（A 脚本读到 B 脚本的计时器，
//! 两边都是 `f32` 时无从发现）。宿主测试
//! `state_survives_hot_reload_even_when_intern_order_changes` 钉死了这条
//! （它的"红对照"是"热重载时清空驻留表"这个看似合理的错误实现）。
//!
//! 代价只有"改名后的旧路径各占一条记录"（永不清），按项目规模（几十条脚本）
//! 可以忽略；换来的是热路径上不需要任何"这个 id 还有效吗"的判断。
//!
//! 两个宿主（Rhai / Lua）各持一份，互不共享 —— 两轨按扩展名分流，路径集合
//! 天然不相交，共享一份反而要多解释一层"为什么这张表里有别的轨的路径"。

use std::collections::HashMap;

/// 脚本路径驻留表（见模块文档；两宿主各持一份）。
#[derive(Debug, Default)]
pub(crate) struct PathIntern {
    /// 路径 → id。热路径上只有"命中"分支（一次哈希查找，零分配）。
    by_path: HashMap<String, u32>,
    /// id → 路径，下标即 id（`resolve` 走这里，O(1) 不哈希）。
    paths: Vec<String>,
}

impl PathIntern {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 取（必要时创建）这条路径的稳定 id。
    ///
    /// 首见才分配 `String`；之后每次只是一次哈希查找 —— 派发热路径上走的
    /// 都是"已驻留"分支（preload 时就已经驻留过一遍）。
    pub(crate) fn intern(&mut self, path: &str) -> u32 {
        if let Some(&id) = self.by_path.get(path) {
            return id;
        }
        let id = self.paths.len() as u32;
        self.paths.push(path.to_owned());
        self.by_path.insert(path.to_owned(), id);
        id
    }

    /// 用 id 换回路径（日志 / `GameCtx::source` 用）。零分配：借的是常驻 `String`。
    ///
    /// id 只能由 [`PathIntern::intern`] 产生，必然在表内；`get` 只是把"理论上
    /// 的越界"降级成空串，避免热路径上放一个不可能触发的 `unwrap` panic。
    pub(crate) fn resolve(&self, id: u32) -> &str {
        self.paths.get(id as usize).map_or("", String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_roundtrips_and_is_stable() {
        let mut intern = PathIntern::new();
        let a = intern.intern("Scripts/a.rhai");
        let b = intern.intern("Scripts/b.rhai");
        assert_ne!(a, b, "不同路径必须拿到不同 id");
        assert_eq!(
            intern.intern("Scripts/a.rhai"),
            a,
            "重复驻留必须拿回同一个 id（状态表键的稳定性依赖这条）"
        );
        assert_eq!(intern.resolve(a), "Scripts/a.rhai");
        assert_eq!(intern.resolve(b), "Scripts/b.rhai");
    }

    #[test]
    fn resolve_of_unknown_id_is_empty_not_a_panic() {
        let intern = PathIntern::new();
        // 没驻留过的路径 resolve 也拿不到东西（防空 id 被误当成有效路径）。
        assert_eq!(intern.resolve(0), "");
    }
}
