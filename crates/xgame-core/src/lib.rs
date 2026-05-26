//! XGameEngine 核心类型。
//!
//! 基础层：窗口配置、颜色、引擎设置和基础 trait。

mod color;
mod config;

pub use color::Color;
pub use config::{EngineConfig, WindowConfig};
