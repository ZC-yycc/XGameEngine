//! Unity-style editor panels.
//!
//! Each panel is a function that takes `(&mut EditorState, &egui::Context)`
//! and renders its content. The main layout is assembled in `main.rs`.

pub mod console;
pub mod hierarchy;
pub mod inspector;
pub mod scene_view;
pub mod state;
pub mod theme;
pub mod toolbar;
