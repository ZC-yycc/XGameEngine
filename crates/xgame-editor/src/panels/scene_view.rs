//! 类 Unity 场景视图——主 3D 视口。
//!
//! Triggers the 3D scene renderer (owned by `EditorState`) and paints
//! the resulting egui texture.  Rendering only happens when the panel
//! is visible and has a non-zero size.
//!
//! Orbit camera interaction (middle-mouse drag to orbit, scroll to zoom)
//! and status overlays (FPS, tool, camera position) are handled here.

use crate::panels::state::EditorState;

/// Render the scene view content inside a `Ui`.
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {
    // ── Self‑tracked key state ──
    // KeyPressed → pressed_keys insert; KeyReleased → remove.
    // This avoids the frame-local nature of egui's key_down() which is rebuilt
    // from scratch each frame and is empty when no keyboard events arrive.
    ui.input(|i| {
        for event in &i.events {
            if let egui::Event::Key { key, pressed: true, .. } = event {
                state.pressed_keys.insert(*key);
            }
            if let egui::Event::Key { key, pressed: false, .. } = event {
                state.pressed_keys.remove(key);
            }
        }
    });
    // ── Top bar: scene name + camera position ──
    ui.horizontal(|ui| {
        ui.label(&state.scene.name);
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
    let ppf = ui.ctx().pixels_per_point();
    let vp_w = (available.x * ppf).max(1.0) as u32;
    let vp_h = (available.y * ppf).max(1.0) as u32;

    state.viewport_logical_size = available;
    state.viewport_pixel_size = (vp_w, vp_h);

    let (rect, response) =
        ui.allocate_exact_size(available, egui::Sense::click_and_drag());
    let painter = ui.painter();

    // ── Render 3D scene (only when viewport has a valid size) ──
    let tex_id = if vp_w > 0 && vp_h > 0 {
        state.scene_renderer.as_mut().map(|renderer| {
            renderer.set_viewport_size(vp_w, vp_h);

            let cam = &state.editor_camera;
            let aspect = vp_w as f32 / vp_h as f32;
            let proj = xgame_math::projection::perspective(
                60f32.to_radians(),
                aspect,
                0.1,
                100.0,
            );
            let view = xgame_math::projection::look_at(
                cam.position,
                cam.position + cam.forward(),
                glam::Vec3::Y,
            );
            let view_proj = proj * view;

            renderer.render(&state.scene.entities, &view_proj, &cam.eye());
            renderer.texture_id()
        })
    } else {
        None
    };

    // ── Display the rendered texture ──
    if let Some(tex_id) = tex_id.flatten() {
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(tex_id, rect, uv, egui::Color32::WHITE);
    } else {
        // No renderer or first frame: solid dark background
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(30, 30, 36));
    }

    // ── Camera control (hover over viewport required) ──
    if response.hovered() {
        let dt = ui.input(|i| i.predicted_dt);

        // Right‑click drag to rotate
        if ui.input(|i| i.pointer.button_down(egui::PointerButton::Secondary)) {
            let delta = ui.input(|i| i.pointer.delta());
            state.editor_camera.rotate(delta.x, delta.y);
        }

        // Scroll to zoom
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            state.editor_camera.zoom(scroll);
        }

        // WASD pan + QE lift (frame‑rate independent via dt)
        // Uses self‑tracked key set, not OS key‑repeat, so hold is instant.
        if state.pressed_keys.contains(&egui::Key::W) {
            state.editor_camera.pan_forward(1.0, dt);
        }
        if state.pressed_keys.contains(&egui::Key::S) {
            state.editor_camera.pan_forward(-1.0, dt);
        }
        if state.pressed_keys.contains(&egui::Key::A) {
            state.editor_camera.pan_right(-1.0, dt);
        }
        if state.pressed_keys.contains(&egui::Key::D) {
            state.editor_camera.pan_right(1.0, dt);
        }
        if state.pressed_keys.contains(&egui::Key::Q) {
            state.editor_camera.move_up(-1.0, dt);
        }
        if state.pressed_keys.contains(&egui::Key::E) {
            state.editor_camera.move_up(1.0, dt);
        }
    }


    // ── Bottom status overlay ──
    painter.text(
        egui::pos2(rect.left() + 8.0, rect.bottom() - 8.0),
        egui::Align2::LEFT_BOTTOM,
        &format!(
            "{:.0} FPS | Tool: {}",
            ui.input(|i| i.predicted_dt).recip().round(),
            state.tool_mode,
        ),
        egui::FontId::monospace(11.0),
        egui::Color32::from_gray(140),
    );
}