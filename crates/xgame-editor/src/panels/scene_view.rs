//! Unity-style Scene View — the main 3D viewport.
//!
//! Currently renders as a simulated viewport with grid overlay and entity labels.
//! Future: will render the actual 3D scene to an egui texture via wgpu.

use crate::panels::state::EditorState;

/// Render the scene view in the central panel.
pub fn show(state: &mut EditorState, ctx: &egui::Context) {
    egui::CentralPanel::default().show(ctx, |ui| {
        // ── Top button strip ──
        ui.horizontal(|ui| {
            ui.label("Scene");
            ui.separator();
            ui.label(state.scene.name.clone());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let cam = &state.editor_camera;
                ui.monospace(format!(
                    "{:.1}, {:.1}, {:.1}",
                    cam.eye().x,
                    cam.eye().y,
                    cam.eye().z,
                ));
            });
        });

        // ── Viewport area ──
        let available = ui.available_size();
        let (rect, response) = ui.allocate_exact_size(available, egui::Sense::click_and_drag());

        // Paint a dark background for the viewport
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(30, 30, 36));

        // ── Grid (if enabled) ──
        if state.show_grid {
            let grid_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 60, 70));
            let center = rect.center();
            let step = 40.0; // grid spacing in logical px
            let half_w = rect.width() / 2.0;
            let half_h = rect.height() / 2.0;

            // Vertical lines
            let mut x = 0.0;
            while x <= half_w {
                let x0 = center.x + x;
                let x1 = center.x - x;
                painter.vline(x0, rect.y_range(), grid_stroke);
                painter.vline(x1, rect.y_range(), grid_stroke);
                x += step;
            }
            // Horizontal lines
            let mut y = 0.0;
            while y <= half_h {
                let y0 = center.y + y;
                let y1 = center.y - y;
                painter.hline(rect.x_range(), y0, grid_stroke);
                painter.hline(rect.x_range(), y1, grid_stroke);
                y += step;
            }

            // Center cross (Y=up blue, X=right red)
            let cross_len = 20.0;
            let cx = center.x;
            let cy = center.y;
            painter.line_segment(
                [egui::pos2(cx, cy), egui::pos2(cx + cross_len, cy)],
                egui::Stroke::new(2.0, egui::Color32::RED),
            );
            painter.line_segment(
                [egui::pos2(cx, cy), egui::pos2(cx, cy - cross_len)],
                egui::Stroke::new(2.0, egui::Color32::BLUE),
            );
        }

        // ── Entity labels in viewport ──
        for (i, entity) in state.scene.entities.iter().enumerate() {
            let is_sel = state.selected_entity == Some(i);
            let col = if is_sel {
                egui::Color32::YELLOW
            } else {
                egui::Color32::GRAY
            };
            // Map world position to screen (simplified: just scatter around)
            let t = entity.transform.translation;
            let center = rect.center();
            let screen_x = center.x + t.x * 30.0;
            let screen_y = center.y - t.y * 30.0 - t.z * 20.0;

            if rect.contains(egui::pos2(screen_x, screen_y)) {
                painter.text(
                    egui::pos2(screen_x, screen_y),
                    egui::Align2::CENTER_CENTER,
                    &entity.name,
                    egui::FontId::proportional(12.0),
                    col,
                );
            }
        }

        // ── Orbit camera interaction ──
        if response.hovered() {
            // Rotate on middle mouse drag
            if ctx.input(|i| i.pointer.button_down(egui::PointerButton::Middle)) {
                let delta = ctx.input(|i| i.pointer.delta());
                state.editor_camera.rotate(delta.x, delta.y);
            }
            // Zoom on scroll
            let scroll = ctx.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                state.editor_camera.zoom(scroll);
            }
        }

        // ── Bottom status overlay ──
        painter.text(
            egui::pos2(rect.left() + 8.0, rect.bottom() - 8.0),
            egui::Align2::LEFT_BOTTOM,
            &format!(
                "{:.0} FPS | Tool: {}",
                ctx.input(|i| i.predicted_dt).recip().round(),
                state.tool_mode.label(),
            ),
            egui::FontId::monospace(11.0),
            egui::Color32::from_gray(140),
        );
    });
}
