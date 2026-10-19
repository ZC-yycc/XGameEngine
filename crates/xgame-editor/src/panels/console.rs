//! 类 UE Output Log 的控制台面板——按严重等级显示日志消息。
//!
//! 排版纪律：每行一条消息、18px 行高、等级图标固定 11px 列宽（用矢量图标而不是
//! ℹ/⚠/✖ 字形 —— 字形宽度不一，等级列会左右错位），消息文字单行截断（悬停看全文），
//! 重复次数右对齐。日志刷屏时这里是每帧几百行的热点，所以一律不做换行测量。

use crate::panels::{icons, theme};
use crate::state::{EditorState, LogLevel};

/// 等级 → (图标, 颜色)。颜色统一问 [`LogLevel::color`]，这里只决定图形。
fn level_style(level: LogLevel) -> (icons::Icon, egui::Color32) {
    let icon = match level {
        LogLevel::Info => icons::Icon::Dot,
        LogLevel::Warning => icons::Icon::Warning,
        LogLevel::Error => icons::Icon::Close,
        LogLevel::Success => icons::Icon::Check,
    };
    (icon, level.color())
}

/// 在 `Ui` 内渲染控制台面板内容。
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {
    // ── 标题栏 ──
    //
    // 左："Clear" 扁平按钮；右：条数（等宽数字，位数变化时不抖）。
    // 不用 `ui.separator()`：紧贴页签条下方，那条线会和页签的底边叠成双线。
    ui.horizontal(|ui| {
        if icons::text_button(ui, "Clear", "清空控制台消息（不影响场景）").clicked() {
            state.console_messages.clear();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let count = state.console_messages.len();
            ui.label(
                egui::RichText::new(format!("{count}"))
                    .monospace()
                    .size(11.0)
                    .color(theme::colors::TEXT_SECONDARY),
            );
            ui.label(egui::RichText::new("messages").size(11.0).color(theme::colors::TEXT_DIM));
        });
    });

    // ── 消息列表 ──
    let total = state.console_messages.len();
    let start = total.saturating_sub(500);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for msg in &state.console_messages[start..] {
                let (icon, color) = level_style(msg.level);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    icons::small_icon(ui, icon, 11.0, color);
                    // 单行截断：一条 2KB 的报错不应该把控制台撑成一面墙。
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&msg.text)
                                .size(11.5)
                                .color(color),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&msg.text);
                    if msg.count > 1 {
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(format!("×{}", msg.count))
                                        .monospace()
                                        .size(10.5)
                                        .color(theme::colors::TEXT_DIM),
                                );
                            },
                        );
                    }
                });
            }
        });
}
