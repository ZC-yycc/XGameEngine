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
        // 只有视口宽度和高度都大于 0 时才执行渲染
        state.scene_renderer.as_mut().map(|renderer| {
            // 1. 设置渲染器的视口大小
            renderer.set_viewport_size(vp_w, vp_h);

            // 2. 获取编辑器摄像机
            let cam = &state.editor_camera;
            let aspect = vp_w as f32 / vp_h as f32;

            // 3. 创建透视投影矩阵（60度视野，最近0.1，最远100.0）
            let proj = xgame_math::projection::perspective(
                60f32.to_radians(),
                aspect,
                0.1,
                100.0,
            );

            // 4. 创建视图矩阵（摄像机位置、朝向、上方向）
            let view = xgame_math::projection::look_at(
                cam.position,
                cam.position + cam.forward(),
                glam::Vec3::Y,
            );

            // 5. 合并为视图-投影矩阵
            let view_proj = proj * view;

            // 6. 执行渲染，传入场景实体、矩阵和摄像机位置
            renderer.render(&state.scene.entities, &view_proj, &cam.eye());

            // 7. 返回渲染结果的纹理 ID
            renderer.texture_id()
        })
    } else {
        None  // 视口无效时不渲染
    };

    // ── Display the rendered texture ──
    if let Some(tex_id) = tex_id.flatten() {
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(tex_id, rect, uv, egui::Color32::WHITE);
    } else {
        // No renderer or first frame: solid dark background
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(30, 30, 36));
    }

    // ── Fly mode state — updated every frame ──
    let right_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Secondary));
    // Enter fly mode: right-click held over viewport
    if response.hovered() && right_down {
        state.fly_state = true;
    }
    // Exit fly mode: right-click released (regardless of hover)
    if !right_down {
        state.fly_state = false;
    }

    // ── Camera control ──
    if state.fly_state {
        // Force continuous repaint — egui pauses when no events arrive
        // (mouse still, no keyboard repeats), which would stall key_down().
        ui.ctx().request_repaint();

        let dt = ui.input(|i| i.predicted_dt);

        // Rotate via drag
        if response.hovered() {
            let delta = ui.input(|i| i.pointer.delta());
            state.editor_camera.rotate(delta.x, delta.y);
        }

        // WASD pan + QE lift (frame‑rate independent via dt)
        if ui.input(|i| i.key_down(egui::Key::W)) {
            state.editor_camera.pan_forward(1.0, dt);
        }
        if ui.input(|i| i.key_down(egui::Key::S)) {
            state.editor_camera.pan_forward(-1.0, dt);
        }
        if ui.input(|i| i.key_down(egui::Key::A)) {
            state.editor_camera.pan_right(-1.0, dt);
        }
        if ui.input(|i| i.key_down(egui::Key::D)) {
            state.editor_camera.pan_right(1.0, dt);
        }
        if ui.input(|i| i.key_down(egui::Key::Q)) {
            state.editor_camera.move_up(-1.0, dt);
        }
        if ui.input(|i| i.key_down(egui::Key::E)) {
            state.editor_camera.move_up(1.0, dt);
        }

        // Scroll to zoom — active in fly mode (hover required).
        if response.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                state.editor_camera.zoom(scroll);
                state.zoom_indicator = 1.5;  // show speed overlay for 1.5 s
            }
        }

        // Decrement zoom indicator
        state.zoom_indicator = (state.zoom_indicator - dt).max(0.0);
    }

    // ── Zoom speed overlay (briefly shown after scroll) ──
    if state.zoom_indicator > 0.0 {
        painter.text(
            egui::pos2(rect.right() - 8.0, rect.bottom() - 28.0),
            egui::Align2::RIGHT_BOTTOM,
            &format!("Speed: {:.1}", state.editor_camera.pan_speed),
            egui::FontId::monospace(12.0),
            egui::Color32::from_rgb(180, 220, 255),
        );
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