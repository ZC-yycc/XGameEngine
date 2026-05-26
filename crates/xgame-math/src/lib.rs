//! XGameEngine 数学库。
//!
//! 重新导出 `glam` 用于线性代数，并添加引擎专用类型
//! 如 `Transform` 用于层级空间变换。

pub use glam;

pub mod projection;
mod transform;

pub use transform::Transform;
