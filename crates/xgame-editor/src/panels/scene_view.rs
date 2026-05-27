//! 类 Unity 场景视图——主 3D 视口。
//!
//! Triggers the 3D scene renderer (owned by `EditorState`) and paints
//! the resulting egui texture.  Rendering only happens when the panel
//! is visible and has a non-zero size.
//!
//! 在 Edit 模式下，如果选中了实体，还会在场景纹理之上叠加绘制 gizmo
//! 操作柄（Translate / Rotate / Scale），并处理鼠标拖拽交互。
//!
//! 在 Play 模式下，如果 `show_physics_debug` 启用，会叠加绘制碰撞体线框。

use xgame_math::glam::{Mat4, Vec3};
use crate::panels::state::{EditorState, ToolMode};
use crate::gizmo;

/// 将 3D 世界坐标投影到 egui 屏幕坐标（面板相对坐标）。
///
/// `screen_offset` 是视口在面板中的偏移（`rect.min.to_vec2()`），
/// 用于将 gizmo::project_to_screen 返回的视口相对坐标转换为面板相对坐标。
fn project_to_screen(point: Vec3, view_proj: &Mat4, screen_size: egui::Vec2, screen_offset: egui::Vec2) -> egui::Pos2 {
    gizmo::project_to_screen(point, view_proj, screen_size, screen_offset)
}

/// 检测 3D 射线与包围球是否相交。
/// 返回沿射线的参数 t（最近交点）。
fn ray_intersect_sphere(
    ray_origin: Vec3,
    ray_dir: Vec3,
    center: Vec3,
    radius: f32,
) -> Option<f32> {
    let oc = ray_origin - center;
    let a = ray_dir.dot(ray_dir);
    let b = 2.0 * oc.dot(ray_dir);
    let c = oc.dot(oc) - radius * radius;
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return None;
    }
    let sqrt_disc = disc.sqrt();
    let t1 = (-b - sqrt_disc) / (2.0 * a);
    let t2 = (-b + sqrt_disc) / (2.0 * a);
    let t = if t1 > 0.0 { t1 } else if t2 > 0.0 { t2 } else { return None };
    Some(t)
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
            let offset = rect.min.to_vec2();
            for line in &debug_lines {
                let start_screen = project_to_screen(line.start, &view_proj, screen_size, offset);
                let end_screen = project_to_screen(line.end, &view_proj, screen_size, offset);

                if start_screen.x < -5000.0 || end_screen.x < -5000.0 {
                    continue;
                }

                let c = &line.color;
                let color32 = egui::Color32::from_rgba_premultiplied(
                    (c.r * 255.0) as u8,
                    (c.g * 255.0) as u8,
                    (c.b * 255.0) as u8,
                    (c.a * 180.0) as u8,
                );

                painter.add(egui::Shape::line_segment(
                    [start_screen, end_screen],
                    egui::Stroke::new(1.5, color32),
                ));
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Gizmo 交互（仅在 Edit 模式 + 选中实体时）
    // ─────────────────────────────────────────────────────────────────────────
    // 使用 response.hover_pos() 获取视口相对坐标——与 gizmo::project_to_screen
    // 返回的视口相对坐标系统一致，从而正确匹配拾取检测。
    let mouse_pos = response.hover_pos();
    let primary_clicked = response.clicked() && state.is_editing();
    let screen_size = state.viewport_logical_size;
    let screen_offset = rect.min.to_vec2();

    if state.is_editing() {
        if let Some(selected_idx) = state.selected_entity {
            let entity_pos = state.scene.entities[selected_idx].transform.translation;
            let gizmo_size = gizmo::gizmo_world_size(cam.eye(), entity_pos);

            // ── 绘制 Gizmo ──
            gizmo::draw_gizmo(
                &painter, &view_proj, entity_pos, screen_size,
                state.tool_mode, state.gizmo_hovered, state.gizmo_active.as_ref(), gizmo_size,
                screen_offset,
            );
        }
    }

    // ── 左键点击 Scene View → 拾取实体或取消选择 ──
    if primary_clicked
        && state.gizmo_active.is_none()
        && state.gizmo_hovered.is_none()
        && state.is_editing()
    {
        if let Some(mp) = mouse_pos {
            let (origin, dir) = gizmo::screen_to_ray(mp, &view_proj, screen_size);

            // 对场景中的每个实体进行射线-包围球检测
            let mut closest_dist = f32::MAX;
            let mut closest_idx = None;

            for (idx, entity) in state.scene.entities.iter().enumerate() {
                if !entity.has_model() {
                    continue;
                }
                // 使用 scale 长度的一半作为包围球半径
                let radius = entity.transform.scale.length().max(0.1) * 0.5;
                if let Some(t) = ray_intersect_sphere(origin, dir, entity.transform.translation, radius) {
                    if t < closest_dist {
                        closest_dist = t;
                        closest_idx = Some(idx);
                    }
                }
            }

            state.selected_entity = closest_idx;
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
    let tool_label = match state.tool_mode {
        ToolMode::Translate => "Translate",
        ToolMode::Rotate => "Rotate",
        ToolMode::Scale => "Scale",
        ToolMode::Rect => "Rect",
    };
    painter.text(
        egui::pos2(rect.left() + 8.0, rect.bottom() - 8.0),
        egui::Align2::LEFT_BOTTOM,
        &format!(
            "{:.0} FPS | Tool: {}",
            ui.input(|i| i.predicted_dt).recip().round(),
            tool_label,
        ),
        egui::FontId::monospace(11.0),
        egui::Color32::from_gray(140),
    );
}
