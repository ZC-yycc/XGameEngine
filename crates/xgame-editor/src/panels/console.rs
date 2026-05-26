//! 类 Unity 控制台面板——按严重等级显示日志消息。

use crate::panels::state::EditorState;

/// Render the console panel content inside a `Ui`.
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {
    // ── Header with filter toggles ──
    ui.horizontal(|ui| {
        
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
    let start = total.saturating_sub(500);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for msg in &state.console_messages[start..] {
                let (color, icon) = (msg.level.log_level_color(), msg.level.log_level_icon());
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
}
