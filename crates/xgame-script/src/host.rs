//! 脚本宿主的公共类型：钩子枚举 + 预载报告 + 命令报告。
//!
//! Lua 宿主实现在 [`crate::LuaHost`]（`lua_host.rs`）。Rhai 宿主已于双轨
//! P3 删除 —— P2.5 闸门判定 Lua 三负载全占优（规则见计划文档 §六，数据见
//! PLAN.md 二十），本模块只留下两轨共用过的类型，API 面不变。

use xgame_core::EntityId;

use crate::context::RemovedEntity;

/// 脚本可以实现的生命周期钩子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    /// 实体进入运行时后调用一次。
    Start,
    /// 每个渲染帧调用一次。
    Update,
    /// 每个固定步长调用一次（物理步进同频）。
    FixedUpdate,
    /// 与另一个碰撞体**开始**接触时调用一次（参数：对方实体 id）。
    CollisionEnter,
    /// 与另一个碰撞体**结束**接触时调用一次（参数：对方实体 id）。
    CollisionExit,
    /// 实体被销毁后调用一次（自身已不在场景，状态表仍可读）。
    Destroy,
}

impl Hook {
    /// 脚本里对应的函数名。
    pub fn fn_name(self) -> &'static str {
        match self {
            Hook::Start => "start",
            Hook::Update => "update",
            Hook::FixedUpdate => "fixed_update",
            Hook::CollisionEnter => "on_collision_enter",
            Hook::CollisionExit => "on_collision_exit",
            Hook::Destroy => "on_destroy",
        }
    }
}

/// [`crate::LuaHost::preload`] 的结果。
#[derive(Debug, Clone, Default)]
pub struct PreloadReport {
    pub loaded: Vec<String>,
    pub failed: Vec<String>,
}

/// 这一轮脚本产生的、需要**运行时**接手的事件。
///
/// 场景本身已经被脚本改完了（见 [`crate::ScriptCommand`]）；这份报告只是告诉运行时
/// 哪些实体的物理体需要补建 / 清理 / 重新定位。
#[derive(Debug, Clone, Default)]
pub struct CommandReport {
    /// 新生成的实体（运行时按需建物理体）。
    pub spawned: Vec<EntityId>,
    /// 被移除的实体及其脚本清单，含整棵子树。
    pub removed: Vec<RemovedEntity>,
    /// 换了父节点的实体（运行时应把刚体移到新的世界位置）。
    pub reparented: Vec<EntityId>,
    /// 换了层的实体（运行时按项目矩阵改碰撞组）。同一帧同一实体多次更改时
    /// **后写覆盖前写** —— 与场景字段的最后一次写入一致，不积累中间态。
    pub relayered: Vec<(EntityId, u8)>,
    /// 脚本请求加载的场景（项目相对路径）。同一帧多次请求时**最后一个生效** ——
    /// 一帧只换一次场景，不允许"切三次最后停在第二次"这种中间态。
    pub scene_load: Option<String>,
}

impl CommandReport {
    /// 这份报告里有没有事情需要运行时处理。
    pub fn is_empty(&self) -> bool {
        self.spawned.is_empty()
            && self.removed.is_empty()
            && self.reparented.is_empty()
            && self.relayered.is_empty()
            && self.scene_load.is_none()
    }
}
