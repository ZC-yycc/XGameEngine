//! 示例 cdylib 插件 —— 双轨架构 P0 的 Rust 轨道演示。
//!
//! 与 `projects/XGame/Scripts/Spinner.lua` 同款行为（绕 Y 轴 75°/s），
//! 另加一个销毁者组件证明四钩子全通：
//!
//! - `Spinner`：四个钩子各给导出计数器 +1，`update` 旋转让实体动起来；
//! - `Reaper`：第一帧 update 里销毁 `target` 实体，触发对方的 `on_destroy`。
//!
//! 导出计数器用 `#[no_mangle] pub static` —— 宿主测试经 libloading 按符号名
//! 读取（跨边界的最朴素观测通道；真实游戏里对应的是插件自己维护的状态）。

use std::sync::atomic::{AtomicU32, Ordering};

use xgame::prelude::*;

/// 各钩子被调用的次数（宿主按符号名读取，见集成测试）。
#[no_mangle]
pub static SPINNER_STARTS: AtomicU32 = AtomicU32::new(0);
#[no_mangle]
pub static SPINNER_UPDATES: AtomicU32 = AtomicU32::new(0);
#[no_mangle]
pub static SPINNER_FIXED_UPDATES: AtomicU32 = AtomicU32::new(0);
#[no_mangle]
pub static SPINNER_DESTROYS: AtomicU32 = AtomicU32::new(0);

/// 与 Spinner.lua 同值：绕 Y 轴每秒转 75 度。
const SPIN_SPEED: f32 = 75.0;

/// 与 Spinner.lua 同款：绕 Y 轴匀速旋转。
///
/// 没有可调字段（`SPIN_SPEED` 是常量）—— `#[derive(Inspect)]` 生成一张空表，
/// 编辑器里就会显示成"没有参数"。可调字段的完整示范见
/// `projects/XGame`。
#[derive(Default, Inspect)]
pub struct Spinner;

impl Lifecycle for Spinner {
    fn start(&mut self, _ctx: &mut Ctx) {
        SPINNER_STARTS.fetch_add(1, Ordering::Relaxed);
    }
    fn update(&mut self, ctx: &mut Ctx, dt: f32) {
        SPINNER_UPDATES.fetch_add(1, Ordering::Relaxed);
        let r = ctx.rot();
        ctx.set_rot(Vec3::new(r.x, r.y + SPIN_SPEED * dt, r.z));
    }
    fn fixed_update(&mut self, _ctx: &mut Ctx, _dt: f32) {
        SPINNER_FIXED_UPDATES.fetch_add(1, Ordering::Relaxed);
    }
    fn on_destroy(&mut self, _ctx: &mut Ctx) {
        SPINNER_DESTROYS.fetch_add(1, Ordering::Relaxed);
    }
}

/// 第一帧 update 里销毁 `target` 实体（用来触发受害者的 `on_destroy`）。
///
/// `victim` 是 `start` 期解析出来的内部状态，不进编辑器 —— `#[inspect(skip)]`
/// 就是为这种字段准备的（不 skip 的话它类型不支持，derive 会**编译期报错**）。
#[derive(Default, Inspect)]
pub struct Reaper {
    #[inspect(skip)]
    victim: Option<EntityId>,
}

impl Lifecycle for Reaper {
    fn start(&mut self, ctx: &mut Ctx) {
        self.victim = ctx.find_entity_id("target");
    }
    fn update(&mut self, ctx: &mut Ctx, _dt: f32) {
        if let Some(id) = self.victim.take() {
            ctx.destroy(id);
        }
    }
}

xgame::export_plugin!(|registry| {
    registry.register::<Spinner>("Spinner");
    registry.register::<Reaper>("Reaper");
});
