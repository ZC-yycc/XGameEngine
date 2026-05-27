//! 类 Unity 场景视图——主 3D 视口。
//!
//! Triggers the 3D scene renderer (owned by `EditorState`) and paints
//! the resulting egui texture.  Rendering only happens when the panel
//! is visible and has a non-zero size.
//!
//! Orbit camera interaction (middle-mouse drag to orbit, scroll to zoom)
//! and status overlays (FPS, tool, camera position) are handled here.
//!
//! 在 Play 模式下，如果 `show_physics_debug` 启用，还会在场景纹理之上
//! 叠加绘制碰撞体线框。

use xgame_math::glam::{Mat4, Vec3, Vec4};
use crate::panels::state::EditorState;

/// 将 3D 世界坐标投影到 egui 屏幕坐标。
///
/// `view_proj` 是视图-投影矩阵，`screen_size` 是视口的逻辑像素尺寸。
fn project_to_screen(point: Vec3, view_proj: &Mat4, screen_size: egui::Vec2) -> egui::Pos2 {
    let clip = *view_proj * Vec4::new(point.x, point.y, point.z, 1.0);
    if clip.w.abs() < 1e-10 {
        return egui::pos2(-10000.0, -10000.0); // off-screen
    }
    let ndc = clip.truncate() / clip.w; // Normalized Device Coordinates: [-1, 1]
    let x = (ndc.x + 1.0) * 0.5 * screen_size.x;
    let y = (1.0 - ndc.y) * 0.5 * screen_size.y; // flip Y for egui
    egui::pos2(x, y)
}

/// 在 `Ui` 内渲染场景视图内容。
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {
    // ── 顶部栏：场景名称 + 相机位置 ──
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

    // ── 视口区域 ──
    let available = ui.available_size();
    let ppf = ui.ctx().pixels_per_point();
    let vp_w = (available.x * ppf).max(1.0) as u32;
    let vp_h = (available.y * ppf).max(1.0) as u32;

    state.viewport_logical_size = available;
    state.viewport_pixel_size = (vp_w, vp_h);

    let (rect, response) =
        ui.allocate_exact_size(available, egui::Sense::click_and_drag());
    let painter = ui.painter();

    // ── 构建 view_proj 矩阵（用于渲染和调试线投影） ──
    let aspect = if vp_h > 0 { vp_w as f32 / vp_h as f32 } else { 1.0 };
    let proj = xgame_math::projection::perspective(60f32.to_radians(), aspect, 0.1, 100.0);
    let cam = &state.editor_camera;
    let view = xgame_math::projection::look_at(
        cam.position,
        cam.position + cam.forward(),
        glam::Vec3::Y,
    );
    let view_proj = proj * view;

    // ── 渲染 3D 场景（仅当视口尺寸有效时）──
    let tex_id = if vp_w > 0 && vp_h > 0 {
        state.scene_renderer.as_mut().map(|renderer| {
            renderer.set_viewport_size(vp_w, vp_h);
            renderer.render(&state.scene.entities, &view_proj, &cam.eye());
            renderer.texture_id()
        })
    } else {
        None
    };

    // ── 显示渲染纹理 ──
    if let Some(tex_id) = tex_id.flatten() {
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(tex_id, rect, uv, egui::Color32::WHITE);
    } else {
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(30, 30, 36));
    }

    // ── Physics Debug 线框叠加绘制（仅在 Play 模式 + 启用时）──
    if state.show_physics_debug && state.game_runtime.is_some() {
        let debug_lines = state.debug_lines();
        if !debug_lines.is_empty() {
            let screen_size = state.viewport_logical_size;
            for line in &debug_lines {
                let start_screen = project_to_screen(line.start, &view_proj, screen_size);
                let end_screen = project_to_screen(line.end, &view_proj, screen_size);

                // 检查点是否在合理范围内
                if start_screen.x < -5000.0 || end_screen.x < -5000.0 {
                    continue;
                }

                let c = &line.color;
                let color32 = egui::Color32::from_rgba_premultiplied(
                    (c.r * 255.0) as u8,
                    (c.g * 255.0) as u8,
                    (c.b * 255.0) as u8,
                    (c.a * 180.0) as u8, // 半透明
                );

                painter.add(egui::Shape::line_segment(
                    [start_screen, end_screen],
                    egui::Stroke::new(1.5, color32),
                ));
            }
        }
    }

    // ── 飞行模式状态——每帧更新 ──
    let right_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Secondary));
    if response.hovered() && right_down {
        state.fly_state = true;
    }
    if !right_down {
        state.fly_state = false;
    }

    // ── 相机控制 ──
    if state.fly_state {
        ui.ctx().request_repaint();
        let dt = ui.input(|i| i.predicted_dt);

        if response.hovered() {
            let delta = ui.input(|i| i.pointer.delta());
            state.editor_camera.rotate(delta.x, delta.y);
        }

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

        if response.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                state.editor_camera.zoom(scroll);
                state.zoom_indicator = 1.5;
            }
        }

        state.zoom_indicator = (state.zoom_indicator - dt).max(0.0);
    }

    // ── 缩放速度叠加层 ──
    if state.zoom_indicator > 0.0 {
        painter.text(
            egui::pos2(rect.right() - 8.0, rect.bottom() - 28.0),
            egui::Align2::RIGHT_BOTTOM,
            &format!("Speed: {:.1}", state.editor_camera.pan_speed),
            egui::FontId::monospace(12.0),
            egui::Color32::from_rgb(180, 220, 255),
        );
    }

    // ── 底部状态叠加层 ──
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
