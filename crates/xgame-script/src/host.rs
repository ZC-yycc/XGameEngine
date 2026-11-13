//! 脚本宿主的公共类型：钩子枚举 + 预载报告 + 命令报告。
//!
//! Lua 宿主实现在 [`crate::LuaHost`]（`lua_host.rs`）。Rhai 宿主已于双轨
//! P3 删除 —— P2.5 闸门判定 Lua 三负载全占优（规则见计划文档 §六，数据见
//! PLAN.md 二十），本模块只留下两轨共用过的类型，API 面不变。

use std::path::PathBuf;

use xgame_core::{EntityId, SaveVars};

use crate::context::RemovedEntity;

/// 脚本侧的存档上下文：**变量表 + 项目根**。
///
/// 变量表住在这里而不是塞进 `Entity`：它是玩法层数据，引擎不解释语义；塞进
/// 实体会让场景文件长出一块作者没写过的字段，还会把"关卡长什么样"与
/// "这一局玩到哪"混进同一次保存（见 `xgame_core::savegame` 的模块文档）。
///
/// 项目根只给**查询**用（`has_save` / `save_slots`）：真正的存 / 读动作是
/// `ScriptCommand`，由运行时在帧末统一做 —— 那里才拿得到刚体速度、也才能
/// 处理"存档里的场景与当前场景不同"（要切场景）。
///
/// `root == None`（测试 / 宿主没接）时所有查询静默返回"没有"，
/// 与"没接音频时播放静默 no-op"同款容错哲学。
#[derive(Debug, Clone, Default)]
pub struct SaveContext {
    /// 项目根目录（`SaveContext` 由 `World` 持有，恒为 `Some`；测试可为 `None`）。
    pub root: Option<PathBuf>,
    /// 活变量表（存档时摊进快照，读档时替换回来）。
    pub vars: SaveVars,
}

impl SaveContext {
    /// 绑定项目根的上下文。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: Some(root.into()),
            vars: SaveVars::default(),
        }
    }
}

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
    /// 动画事件：基础通道剪辑时间轴上的命名时刻（参数：事件名）。
    ///
    /// 由运行时在 `World::update` 里派发（时钟走完本帧之后），见
    /// `xgame_core::AnimEvent`。分层 / 叠加层不触发它。
    AnimEvent,
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
            Hook::AnimEvent => "on_anim_event",
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
    /// 与新实体一同生成的稳定组件身份；没有文件身份侧车的运行时会安全忽略。
    pub spawned_identities: Vec<xgame_core::RuntimeEntityIdentity>,
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
    /// 脚本请求存档（槽名）。帧末结算：此刻场景正被 `&mut` 借给 `GameCtx`，
    /// 而且刚体速度只有在帧末才与实体的可见状态对齐。
    pub save_game: Option<String>,
    /// 脚本请求读档（槽名）。帧末结算；存档里的场景与当前不同时由运行时切过去。
    pub load_game: Option<String>,
    /// 脚本请求的 UI 焦点。`Set` / `Clear` 是两种**明确**的请求 ——
    /// 不能只靠"场景里有没有 `focused` 标志"推断，那样"清空焦点"与
    /// "没人碰过焦点"分不开。同一帧多次请求时**最后一个生效**。
    pub ui_focus: Option<FocusRequest>,
}

/// 脚本对 UI 焦点的请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusRequest {
    /// 把焦点交给这个元素。
    Set(xgame_core::EntityId),
    /// 清空焦点（下一次推方向键会重新选第一个）。
    Clear,
}

impl CommandReport {
    /// 这份报告里有没有事情需要运行时处理。
    pub fn is_empty(&self) -> bool {
        self.spawned.is_empty()
            && self.removed.is_empty()
            && self.reparented.is_empty()
            && self.relayered.is_empty()
            && self.scene_load.is_none()
            && self.save_game.is_none()
            && self.load_game.is_none()
            && self.ui_focus.is_none()
    }
}
