//! Unity-style Hierarchy panel — lists all entities in the scene as a tree.

use crate::panels::state::EditorState;

/// Render the hierarchy panel content inside a `Ui`.
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {
    // ── Header with scene name + add button ──
    ui.horizontal(|ui| {
        ui.heading("Hierarchy");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("+").clicked() {
                state.create_entity();
            }
        });
    });
    ui.separator();

    // ── Search filter ──
    let mut filter = String::new();
    ui.add(
        egui::TextEdit::singleline(&mut filter)
            .hint_text("Search...")
            .desired_width(f32::INFINITY),
    );

    // ── Entity list ──
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

        for &i in &filtered_indices {
            let entity = &state.scene.entities[i];
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
            // Context menu on right-click
            resp.context_menu(|ui| {
                let selected = state.selected_entity == Some(i);
                if ui.button("Create Empty").clicked() {
                    state.create_entity();
                    ui.close_menu();
                }
                if selected {
                    if ui.button("Duplicate").clicked() {
                        state.duplicate_entity();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Delete").clicked() {
                        state.delete_selected();
                        ui.close_menu();
                    }
                }
            });
        }
    });

    // ── Create Empty button at bottom ──
    ui.separator();
    if ui.button("Create Empty").clicked() {
        state.create_entity();
    }

    // ── Keyboard shortcuts ──
    if ui.input(|i| i.key_pressed(egui::Key::Delete)) {
        state.delete_selected();
    }
    if ui.input(|i| i.key_pressed(egui::Key::D) && i.modifiers.ctrl) {
        state.duplicate_entity();
    }
}
