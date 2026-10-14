//! XGameEngine 运行时。
//!
//! 这一层只做一件事：**定义游戏每帧按什么顺序发生**。
//!
//! ```text
//!             ┌──────────────┐
//!   xgame-core│  Scene/Entity │  数据
//!             └──────┬───────┘
//!      ┌─────────────┼─────────────┬──────────────┐
//!      ▼             ▼             ▼              ▼
//!  xgame-physics xgame-input  xgame-script   xgame-render
//!      └─────────────┴─────────────┴──────────────┘
//!                    │
//!                    ▼
//!            xgame-runtime::World          ← 编排（本 crate）
//!                    │
//!          ┌─────────┴─────────┐
//!          ▼                   ▼
//!   xgame-editor(Play)   xgame-game(独立运行)
//! ```
//!
//! 编辑器按下「▶ Play」和双击打包出来的游戏跑的是**同一段** [`World::update`]，
//! 所以"编辑器里跑得好好的、打出去就不对"这类差异从架构上就被消掉了。
//!
//! # 用法
//!
//! ```ignore
//! use xgame_runtime::{World, WorldConfig};
//!
//! let mut world = World::new(scene, project_root, WorldConfig::default());
//!
//! // 每个渲染帧：
//! let report = world.update(frame_dt);
//! for log in world.take_logs() {
//!     println!("[{}] {}", log.script, log.message);
//! }
//! // world.scene() 现在就是这一帧的状态，拿去渲染。
//! ```

mod lifecycle;
mod registry;
mod world;

pub use lifecycle::{Behavior, Ctx, Lifecycle};
pub use registry::BehaviorRegistry;
pub use world::{FrameReport, World, WorldConfig};

// 常用类型顺手重导出，免得宿主到处写 `xgame_script::...`。
pub use xgame_input::InputMap;
pub use xgame_script::{LogLevel, ScriptLog};
