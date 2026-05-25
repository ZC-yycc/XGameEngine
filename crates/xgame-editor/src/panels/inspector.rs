//! Unity-style Inspector panel — shows properties of the selected entity.

use crate::panels::state::EditorState;

/// Render the inspector panel content inside a `Ui`.
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {
    ui.separator();

    if let Some(entity) = state.selected_entity_mut() {
        // ── Entity name ──
        ui.horizontal(|ui| {
            ui.label("Name:");
            ui.add(
                egui::TextEdit::singleline(&mut entity.name)
                    .desired_width(f32::INFINITY),
            );
        });

        ui.separator();

        // ── Transform section ──
        ui.heading("Transform");
        ui.indent("transform", |ui| {
            // Translation
            ui.label("Position");
            ui.horizontal(|ui| {
                ui.label("X");
                ui.add(
                    egui::DragValue::new(&mut entity.transform.translation.x)
                        .speed(0.1)
                        .prefix("  "),
                );
                ui.label("Y");
                ui.add(
                    egui::DragValue::new(&mut entity.transform.translation.y)
                        .speed(0.1)
                        .prefix("  "),
                );
                ui.label("Z");
                ui.add(
                    egui::DragValue::new(&mut entity.transform.translation.z)
                        .speed(0.1)
                        .prefix("  "),
                );
            });

            // Rotation (Euler angles)
            ui.label("Rotation");
            let euler = entity.transform.rotation.to_euler(
                xgame_math::glam::EulerRot::XYZ,
            );
            let mut rot = [euler.0.to_degrees(), euler.1.to_degrees(), euler.2.to_degrees()];
            let changed = ui
                .horizontal(|ui| {
                    ui.label("X");
                    let cx = ui.add(
                        egui::DragValue::new(&mut rot[0])
                            .speed(0.5)
                            .suffix("°"),
                    );
                    ui.label("Y");
                    let cy = ui.add(
                        egui::DragValue::new(&mut rot[1])
                            .speed(0.5)
                            .suffix("°"),
                    );
                    ui.label("Z");
                    let cz = ui.add(
                        egui::DragValue::new(&mut rot[2])
                            .speed(0.5)
                            .suffix("°"),
                    );
                    (cx.changed(), cy.changed(), cz.changed())
                })
                .inner;
            if changed.0 || changed.1 || changed.2 {
                entity.transform.rotation = xgame_math::glam::Quat::from_euler(
                    xgame_math::glam::EulerRot::XYZ,
                    rot[0].to_radians(),
                    rot[1].to_radians(),
                    rot[2].to_radians(),
                );
            }

            // Scale
            ui.label("Scale");
            ui.horizontal(|ui| {
                ui.label("X");
                ui.add(
                    egui::DragValue::new(&mut entity.transform.scale.x)
                        .speed(0.05)
                        .range(0.01..=f32::MAX),
                );
                ui.label("Y");
                ui.add(
                    egui::DragValue::new(&mut entity.transform.scale.y)
                        .speed(0.05)
                        .range(0.01..=f32::MAX),
                );
                ui.label("Z");
                ui.add(
                    egui::DragValue::new(&mut entity.transform.scale.z)
                        .speed(0.05)
                        .range(0.01..=f32::MAX),
                );
            });
        });

        ui.separator();

        // ── Model path ──
        ui.heading("Mesh");
        ui.indent("mesh", |ui| {
            ui.horizontal(|ui| {
                ui.label("Model:");
                let mut path = entity.model.clone();
                ui.add(
                    egui::TextEdit::singleline(&mut path)
                        .desired_width(f32::INFINITY),
                );
                if path != entity.model {
                    entity.model = path;
                    // In the future: reload the mesh
                }
            });
        });

        ui.separator();

        // ── Add Component button (placeholder) ──
        if ui.button("+  Add Component").clicked() {
            // Placeholder — future: show component picker
        }
    } else {
        ui.weak("Select an entity from the Hierarchy to inspect its properties.");
    }
}
