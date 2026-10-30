//! CPU 动画共享层的兼容转出。
//!
//! 数据、采样与播放时钟住在 `xgame-core`，runtime 因而能在不依赖 GPU / render 的
//! 普通无头帧循环中推进动画并消费根运动。保留本模块路径，避免渲染器、基准与外部
//! 调用方因所有权下沉而集体改名。

pub use xgame_core::animation_runtime::*;
