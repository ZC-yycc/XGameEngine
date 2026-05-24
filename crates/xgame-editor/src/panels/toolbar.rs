//! Unity-style toolbar — Play / Pause / Stop controls and transform tool selection.

use crate::panels::state::{EditorState, PlayMode, ToolMode};

/// Height of the toolbar in logical pixels.
pub const TOOLBAR_HEIGHT: f32 = 32.0;

/// Render the top toolbar.
pub fn show(state: &mut EditorState, ctx: &egui::Context) {
    egui::TopBottomPanel::top("toolbar")
        .min_height(TOOLBAR_HEIGHT)
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                // ── Playback controls ──
                let play_btn = egui::Button::new(if state.play_mode == PlayMode::Playing {
                    "⏸  Playing"
                } else {
                    "▶  Play"
                });
                if ui.add(play_btn).clicked() {
                    state.play_mode = match state.play_mode {
                        PlayMode::Edit => {
                            state.log_info("Entered Play mode");
                            PlayMode::Playing
                        }
                        PlayMode::Playing => PlayMode::Edit,
                        PlayMode::Paused => PlayMode::Playing,
                    };
                }

                let pause_btn = egui::Button::new("⏸  Pause");
                if ui.add(pause_btn).clicked() {
                    state.play_mode = match state.play_mode {
                        PlayMode::Playing => {
                            state.log_info("Play mode paused");
                            PlayMode::Paused
                        }
                        _ => PlayMode::Paused,
                    };
                }

                let stop_btn = egui::Button::new("⏹  Stop");
                if ui.add(stop_btn).clicked() && state.play_mode != PlayMode::Edit {
                    state.play_mode = PlayMode::Edit;
                    state.log_info("Returned to Edit mode");
                }

                ui.separator();

                // ── Transform tools ──
                let tools = [
                    (ToolMode::Translate, "↔"),
                    (ToolMode::Rotate, "↻"),
                    (ToolMode::Scale, "⇔"),
                    (ToolMode::Rect, "▭"),
                ];
                for (mode, icon) in &tools {
                    let is_active = state.tool_mode == *mode;
                    let btn = egui::SelectableLabel::new(is_active, *icon);
                    let size = egui::Vec2::new(28.0, 22.0);
                    if ui.add_sized(size, btn).clicked() {
                        state.tool_mode = *mode;
                    }
                }

                ui.separator();

                // ── View toggles ──
                let mut grid = state.show_grid;
                if ui
                    .add(egui::Checkbox::new(&mut grid, "Grid"))
                    .changed()
                {
                    state.show_grid = grid;
                }

                let mut physics = state.show_physics_debug;
                if ui
                    .add(egui::Checkbox::new(&mut physics, "Physics"))
                    .changed()
                {
                    state.show_physics_debug = physics;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!(
                        "{} | {} entities",
                        state.scene.name,
                        state.scene.entities.len()
                    ));
                });
            });
        });
}
