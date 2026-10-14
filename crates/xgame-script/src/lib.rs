//! XGameEngine 脚本层（嵌入式 Rhai）。
//!
//! 一个游戏引擎和"场景查看器"的分界，几乎就是**有没有脚本**。这个 crate 让
//! 实体可以挂一个 `.rhai` 文件，并在固定步长 / 每帧 / 销毁时收到回调。
//!
//! # 脚本长什么样
//!
//! ```rhai
//! // Scripts/Spinner.rhai —— 让物体绕 Y 轴转，按空格跳一下
//! let speed = 90.0;
//!
//! fn update(ctx) {
//!     let r = rot(ctx);
//!     set_rot(ctx, r.x, r.y + speed * delta(ctx), r.z);
//!
//!     if action_pressed(ctx, "Jump") {
//!         let v = velocity(ctx, self_id(ctx));
//!         set_velocity(ctx, self_id(ctx), v.x, 5.0, v.z);
//!     }
//! }
//! ```
//!
//! 钩子一共四个，全部可选：
//!
//! | 钩子 | 时机 |
//! |---|---|
//! | `fn start(ctx)` | 实体进入运行时后一次 |
//! | `fn update(ctx)` | 每个渲染帧 |
//! | `fn fixed_update(ctx)` | 每个物理固定步长 |
//! | `fn on_destroy(ctx)` | 实体被销毁前 |
//!
//! 完整 API 见 [`GameCtx`] 的方法表（`ctx.rs` —— 真相源；`api.rs` 只是
//! 每个函数一行的 Rhai 转发壳，也可看仓库 `Docs/脚本 API.md`）。
//!
//! # 三条设计约定
//!
//! 1. **脚本按路径共享**。实体存的是脚本的**项目相对路径**，多个实体挂同一个
//!    文件只编译一次；想改逻辑就改那个文件，不用碰场景。
//! 2. **结构性改动立即生效，但要告知运行时**。`spawn_entity` / `destroy` /
//!    `set_parent` 当场就改场景（否则"生成之后马上设位置"会静默失效），
//!    同时攒一条通知，让运行时把对应的物理体补上 / 清掉。
//! 3. **脚本崩溃不拖垮引擎**。编译错误、运行时错误、超时（操作数上限）都会
//!    记成一条日志，脚本这帧不生效，游戏继续跑。

mod api;
mod context;
mod ctx;
mod host;
mod intern;
mod lua_api;
mod lua_host;

pub use context::{
    FrameCtx, LogLevel, ScriptCommand, ScriptCtx, ScriptLog, SpawnRequest,
};
pub use ctx::{GameCtx, RayHit};
pub use host::{CommandReport, Hook, PreloadReport, ScriptHost};
pub use lua_host::{LuaHost, LuaPreloadReport};
