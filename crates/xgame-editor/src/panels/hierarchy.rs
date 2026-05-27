//! 类 Unity 层级面板——以树形列表显示场景中的所有实体。

use std::cell::Cell;
use crate::panels::state::{EditorState, PlayMode};

/// 在 `Ui` 内渲染层级面板内容。
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {

    // ── 搜索过滤器 ──
    let mut filter = String::new();
    let is_editing = state.is_editing();
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut filter)
                .hint_text("Search...")
                .desired_width(f32::INFINITY),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let resp = ui.add_enabled(is_editing, egui::Button::new("+"))
                .on_disabled_hover_text("Cannot modify scene while playing");
            if resp.clicked() {
                state.create_entity();
            }
        });
    });

    // 在 Play 模式下显示状态提示
    if !is_editing {
        let play_label = match state.play_mode {
            PlayMode::Playing => "▶ Playing",
            PlayMode::Paused => "⏸ Paused",
            PlayMode::Edit => "",
        };
        ui.label(
            egui::RichText::new(format!("{} — Scene locked", play_label))
                .color(egui::Color32::YELLOW)
                .size(11.0),
        );
    }

    ui.separator();


    // ── 实体列表 ──
    egui::ScrollArea::vertical().show(ui, |ui| {
        let filtered_indices: Vec<usize> = state
            .scene
            .entities
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                filter.is_empty()
                    || e.name.to_lowercase().contains(&filter.to_lowercase())
            })
            .map(|(i, _)| i)
            .collect();

        // 单元格标志：当右键菜单删除实体时设置，
        // 使后续迭代跳过（filtered_indices 可能已过时）。
        let deleted = Cell::new(false);

        for &i in &filtered_indices {
            if deleted.get() {
                break;
            }

            // 安全保护：实体可能已被先前的
            // 右键菜单操作（针对不同索引）删除。
            let Some(entity) = state.scene.entities.get(i) else {
                continue;
            };
            let is_selected = state.selected_entity == Some(i);
            let label = if entity.model.is_empty() {
                format!("  {}", entity.name)
            } else {
                format!("  📦 {}", entity.name)
            };
            let resp = ui.selectable_label(is_selected, label);
            if resp.clicked() {
                state.selected_entity = Some(i);
            }
            // 右键上下文菜单
            resp.context_menu(|ui| {
                let selected = state.selected_entity == Some(i);
                let ce_resp = ui.add_enabled(is_editing, egui::Button::new("Create Empty"))
                    .on_disabled_hover_text("Cannot modify scene while playing");
                if ce_resp.clicked() {
                    state.create_entity();
                    ui.close_menu();
                }
                if selected {
                    let dup_resp = ui.add_enabled(is_editing, egui::Button::new("Duplicate"))
                        .on_disabled_hover_text("Cannot modify scene while playing");
                    if dup_resp.clicked() {
                        state.duplicate_entity();
                        ui.close_menu();
                    }
                    ui.separator();
                    let del_resp = ui.add_enabled(is_editing, egui::Button::new("Delete"))
                        .on_disabled_hover_text("Cannot modify scene while playing");
                    if del_resp.clicked() {
                        state.delete_selected();
                        deleted.set(true);
                        ui.close_menu();
                    }
                }
            });
        }
    });

    // ── 底部创建空对象按钮 ──
    ui.separator();
    let create_resp = ui.add_enabled(is_editing, egui::Button::new("Create Empty"))
        .on_disabled_hover_text("Cannot modify scene while playing");
    if create_resp.clicked() {
        state.create_entity();
    }

    // ── 键盘快捷键 ──
    if is_editing {
        if ui.input(|i| i.key_pressed(egui::Key::Delete)) {
            state.delete_selected();
        }
        if ui.input(|i| i.key_pressed(egui::Key::D) && i.modifiers.ctrl) {
            state.duplicate_entity();
        }
    }
}
