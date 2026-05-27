//! 类 Unity 检视面板——显示选中实体的属性。

use crate::panels::state::EditorState;

/// 在 `Ui` 内渲染检视面板内容。
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {
    ui.separator();

    if let Some(entity) = state.entity_mut() {
        // ── 实体名称 ──
        ui.horizontal(|ui| {
            ui.label("Name:");
            ui.add(
                egui::TextEdit::singleline(&mut entity.name)
                    .desired_width(f32::INFINITY),
            );
        });

        ui.separator();

        // ── 变换组件 ──
        ui.heading("Transform");
        ui.indent("transform", |ui| {
            // 位置
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

            // 旋转（欧拉角）
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

            // 缩放
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

        // ── 模型路径 ──
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
                    // 将来：重新加载网格
                }
            });
        });

        ui.separator();

        // ── 添加组件按钮（占位符）──
        if ui.button("+  Add Component").clicked() {
            // 占位符——将来：显示组件选择器
        }
    } else {
        ui.weak("Select an entity from the Hierarchy to inspect its properties.");
    }
}
