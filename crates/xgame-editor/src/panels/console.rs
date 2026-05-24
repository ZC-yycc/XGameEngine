//! Unity-style Console panel — displays log messages with severity levels.

use crate::panels::state::{EditorState, LogLevel};

/// Default height (or width when docked) for the console panel.
pub const DEFAULT_HEIGHT: f32 = 150.0;

/// Render the console panel at the bottom.
pub fn show(state: &mut EditorState, ctx: &egui::Context) {
    egui::TopBottomPanel::bottom("console")
        .default_height(DEFAULT_HEIGHT)
        .resizable(true)
        .show(ctx, |ui| {
            // ── Header with filter toggles ──
            ui.horizontal(|ui| {
                ui.heading("Console");
                ui.separator();

                // Quick filter: show last error-only, etc.
                if ui.button("Clear").clicked() {
                    state.console_messages.clear();
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{} messages", state.console_messages.len()));
                });
            });
            ui.separator();

            // ── Message list ──
            let total = state.console_messages.len();
            let start = total.saturating_sub(500); // cap visible

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for msg in &state.console_messages[start..] {
                        let (color, icon) = match msg.level {
                            LogLevel::Info => (egui::Color32::WHITE, "ℹ"),
                            LogLevel::Warning => (egui::Color32::YELLOW, "⚠"),
                            LogLevel::Error => (egui::Color32::RED, "✖"),
                            LogLevel::Success => (egui::Color32::GREEN, "✔"),
                        };

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(icon).color(color).monospace(),
                            );
                            ui.label(
                                egui::RichText::new(&msg.text).color(color),
                            );
                            if msg.count > 1 {
                                ui.label(
                                    egui::RichText::new(format!(" (×{})", msg.count))
                                        .weak(),
                                );
                            }
                        });
                    }
                });
        });
}
