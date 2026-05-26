//! 类 Unity 编辑器面板。
//!
//! 每个面板都是一个函数，接收 `(&mut EditorState, &egui::Context)` 并渲染其内容。
//! 主布局在 `main.rs` 中组装。

pub mod console;
pub mod dock;
pub mod hierarchy;
pub mod inspector;
pub mod scene_view;
pub mod state;
pub mod theme;
pub mod toolbar;

