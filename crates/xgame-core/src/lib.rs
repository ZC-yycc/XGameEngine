//! XGameEngine 核心类型与 trait。
//!
//! 这是整个引擎的基础层，提供所有上层 crate 依赖的核心数据结构。
//!
//! # 模块
//!
//! | 模块 | 说明 |
//! |---|---|
//! | `color` | RGBA 颜色类型（线性浮点存储） |
//! | `config` | 引擎/窗口/内容路径配置 |
//! | `transform` | 层级空间变换（位置/旋转/缩放）|
//! | `scene` | 场景图：`Scene`, `Entity` |
//! | `id` | 稳定实体标识符 `EntityId` |
//! | `component` | `Component` trait + `ComponentMap` + 内置组件 |
//! | `time` | `GameClock`, `Timer`, `Stopwatch` |
//! | `asset` | `AssetId` 轻量资产句柄 |
//!
//! # 依赖方向
//!
//! ```text
//! core (本 crate)
//!   ↑
//! math (glam 重导出)
//!   ↑
//! {render, physics}
//!   ↑
//! editor / game
//! ```

mod color;
mod config;
mod scene;
mod transform;

// 新增模块
mod asset;
mod component;
mod id;
mod time;

// -- 导出：旧有类型（保持向后兼容）
pub use color::Color;
pub use config::{ContentConfig, EngineConfig, WindowConfig};
pub use scene::{Entity, Scene};
pub use transform::Transform;

// -- 导出：新增基础设施
pub use asset::AssetId;
pub use component::{Component, ComponentMap, ModelPath, Name};
pub use id::{EntityId, EntityMap, EntitySet};
pub use time::{GameClock, Stopwatch, Timer};
