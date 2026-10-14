//! Rust 组件的生命周期 trait —— 双轨架构里 Rust 轨道的用户接口。
//!
//! 与 Rhai / Lua 的四个钩子**同名同语义**（见 `xgame-script::Hook`）：
//! 脚本作者与 Rust 作者共享心智模型，也共享调度时机（start-before-update、
//! 每帧 update 一次、fixed_update 与物理同频）。
//!
//! ```ignore
//! use xgame::prelude::*;
//!
//! struct Spinner { speed: f32 }
//!
//! impl Lifecycle for Spinner {
//!     fn update(&mut self, ctx: &mut Ctx, dt: f32) {
//!         let r = ctx.rot();
//!         ctx.set_rot(r.x, r.y + self.speed * dt, r.z);
//!     }
//! }
//! ```
//!
//! 组件实例由 `Entity::behaviors`（注册表名字）在运行时装配
//! （见 [`crate::BehaviorRegistry`]）；名字未注册时记日志跳过，
//! 沿用"脚本崩溃不拖垮引擎"的容错哲学。

use std::any::Any;

use xgame_core::Inspect;

/// 组件钩子的上下文 —— [`GameCtx`] 的别名（见模块文档末尾的取舍说明）。
///
/// [`GameCtx`]: xgame_script::GameCtx
pub type Ctx<'a> = xgame_script::GameCtx<'a>;

/// 一个可挂到实体上的 Rust 行为组件。
///
/// 四个钩子全部可选（默认空实现）。约束解释：
/// - `Any`（含 `'static`）：注册表按 `TypeId` 反查类型（P1 的 Inspector
///   自动渲染也要用）；
/// - `Send + Sync`：与脚本组件同一纪律 —— 引擎不假定钩子只在主线程跑。
pub trait Lifecycle: Any + Send + Sync {
    /// 实体进入运行时后调用一次（先于它的第一个 update）。
    fn start(&mut self, ctx: &mut Ctx) {
        let _ = ctx;
    }
    /// 每个渲染帧调用一次。
    fn update(&mut self, ctx: &mut Ctx, dt: f32) {
        let _ = (ctx, dt);
    }
    /// 每个固定步长调用一次（与物理步进同频）。
    fn fixed_update(&mut self, ctx: &mut Ctx, dt: f32) {
        let _ = (ctx, dt);
    }
    /// 实体被销毁时调用一次。
    ///
    /// 注意调用时机：命令被消费时实体**已经离开场景**（脚本轨的
    /// `on_destroy` 接线沿用同一时机），钩子里读不到自身变换 ——
    /// 用来做"通知外部清理"，不要读 `ctx` 里的自身状态。
    fn on_destroy(&mut self, ctx: &mut Ctx) {
        let _ = ctx;
    }
}

// ---------------------------------------------------------------------------
// 组件契约：生命周期 + 可检视
// ---------------------------------------------------------------------------

/// **组件的完整契约** —— 四个钩子（[`Lifecycle`]）+ 字段检视（[`Inspect`]）。
///
/// # 为什么"可检视"是必选项而不是可选能力
///
/// 运行时把组件存成 `Box<dyn Behavior>`。要让编辑器在不链接用户代码的前提下
/// 拿到 `&mut dyn Inspect`，存储的 trait 对象本身就必须能转过去 ——
/// 而 `Box<dyn Lifecycle>` **不能**安全地降级成 `&mut dyn Inspect`
/// （跨 DLL 边界 `Any::downcast` 靠 `TypeId`，而 `TypeId` 在两个编译单元之间
/// 不保证相同；同名不同类型会静默错配）。
///
/// 所以两条能力合并成一个 trait：注册时同时拿到钩子函数与字段表，一次擦除、
/// 终身可用。代价是"不想在编辑器里调参"的组件也要给个空表 ——
/// `xgame_core::impl_no_inspect!` 或 `#[inspect(skip)]` 把成本压到一行。
///
/// 这个绑定也是 P2 的伏笔：Lua 宿主要把组件字段暴露成脚本参数，用的正是
/// 同一张表（`Docs/双轨语言架构计划.md`：一份元数据，两个消费者）。
pub trait Behavior: Lifecycle + Inspect {}
impl<T: Lifecycle + Inspect + ?Sized> Behavior for T {}
