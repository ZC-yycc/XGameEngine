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
use crate::state::{EditorState, GizmoPart, ToolMode};
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

// ============================================================================
// 顶部栏相关
// ============================================================================

/// 绘制场景视图的顶部栏（场景名称和相机位置）
fn draw_top_bar(state: &EditorState, ui: &mut egui::Ui) {
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
}

// ============================================================================
// 视口与渲染相关
// ============================================================================

/// 视口尺寸信息
struct ViewportInfo {
    pixel_size: (u32, u32),
    rect: egui::Rect,
    response: egui::Response,
}

/// 初始化视口并分配响应区域
fn setup_viewport(ui: &mut egui::Ui, state: &mut EditorState) -> ViewportInfo {
    let available = ui.available_size();
    let ppf = ui.ctx().pixels_per_point();
    let vp_w = (available.x * ppf).max(1.0) as u32;
    let vp_h = (available.y * ppf).max(1.0) as u32;

    state.viewport_logical_size = available;
    state.viewport_pixel_size = (vp_w, vp_h);

    let (rect, response) = ui.allocate_exact_size(available, egui::Sense::click_and_drag());
    
    ViewportInfo {
        pixel_size: (vp_w, vp_h),
        rect,
        response,
    }
}

/// 构建视图投影矩阵
fn build_view_projection(state: &EditorState, vp_w: u32, vp_h: u32) -> Mat4 {
    let aspect = if vp_h > 0 { vp_w as f32 / vp_h as f32 } else { 1.0 };
    let proj = xgame_math::projection::perspective(60f32.to_radians(), aspect, 0.1, 100.0);
    let cam = &state.editor_camera;
    let view = xgame_math::projection::look_at(
        cam.position,
        cam.position + cam.forward(),
        glam::Vec3::Y,
    );
    proj * view
}

/// 渲染 3D 场景并返回纹理 ID
fn render_scene(state: &mut EditorState, vp_w: u32, vp_h: u32, view_proj: &Mat4) -> Option<egui::TextureId> {
    if vp_w == 0 || vp_h == 0 {
        return None;
    }
    
    state.scene_renderer.as_mut().and_then(|renderer| {
        renderer.set_viewport_size(vp_w, vp_h);
        renderer.render(&state.scene.entities, view_proj, &state.editor_camera.eye());
        renderer.texture_id()
    })
}

/// 绘制渲染纹理或空白背景
fn draw_render_texture(painter: &egui::Painter, rect: egui::Rect, tex_id: Option<egui::TextureId>) {
    if let Some(tex_id) = tex_id {
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(tex_id, rect, uv, egui::Color32::WHITE);
    } else {
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(30, 30, 36));
    }
}

// ============================================================================
// 物理调试线框相关
// ============================================================================

/// 绘制物理调试线框
fn draw_physics_debug_lines(
    state: &EditorState,
    painter: &egui::Painter,
    view_proj: &Mat4,
    rect: egui::Rect,
) {
    if !state.show_physics_debug || state.game_runtime.is_none() {
        return;
    }
    
    let debug_lines = state.debug_lines();
    if debug_lines.is_empty() {
        return;
    }
    
    let screen_size = state.viewport_logical_size;
    let offset = rect.min.to_vec2();
    
    for line in &debug_lines {
        let start_screen = project_to_screen(line.start, view_proj, screen_size, offset);
        let end_screen = project_to_screen(line.end, view_proj, screen_size, offset);

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

// ============================================================================
// 实体拾取相关
// ============================================================================

/// 通过射线检测拾取实体
fn pick_entity(
    state: &EditorState,
    mouse_pos: egui::Pos2,
    view_proj: &Mat4,
    screen_offset: egui::Vec2,
) -> Option<usize> {
    let (origin, dir) = gizmo::screen_to_ray(mouse_pos, view_proj, state.viewport_logical_size, screen_offset);
    
    let mut closest_dist = f32::MAX;
    let mut closest_idx = None;
    
    for (idx, entity) in state.scene.entities.iter().enumerate() {
        if !entity.has_model() {
            continue;
        }
        // 使用 scale 长度的一半作为包围球半径
        let radius = entity.transform.scale.length().max(0.1) * 0.5;
        if let Some(t) = ray_intersect_sphere(origin, dir, entity.transform.position, radius) {
            if t < closest_dist {
                closest_dist = t;
                closest_idx = Some(idx);
            }
        }
    }
    
    closest_idx
}

// ============================================================================
// Gizmo 交互相关
// ============================================================================

/// 构建 Gizmo 上下文（用于交互）
fn build_gizmo_ctx(
    state: &EditorState,
    view_proj: Mat4,
    screen_offset: egui::Vec2,
    entity_pos: Vec3,
) -> gizmo::GizmoCtx {
    let gizmo_size = gizmo::gizmo_world_size(state.editor_camera.eye(), entity_pos);
    gizmo::GizmoCtx {
        view_proj,
        screen_size: state.viewport_logical_size,
        screen_offset,
        entity_pos,
        gizmo_size,
        hovered: state.gizmo_hovered,
        active: state.gizmo_active.clone(),
    }
}

/// 处理 Gizmo 交互（悬停、拖拽、释放、绘制）
fn handle_gizmo_interaction(
    state: &mut EditorState,
    painter: &egui::Painter,
    view_proj: Mat4,
    viewport: &ViewportInfo,
    mouse_pos: Option<egui::Pos2>,
    primary_clicked: bool,
    primary_released: bool,
) {
    if !state.is_editing() {
        return;
    }
    
    let Some(selected_idx) = state.selected_entity else {
        // 没有选中实体时清除 gizmo 状态
        state.gizmo_hovered = None;
        state.gizmo_active = None;
        return;
    };
    
    let entity_pos = state.scene.entities[selected_idx].transform.position;
    let screen_offset = viewport.rect.min.to_vec2();
    
    // 构建当前帧的 Gizmo 上下文
    let mut ctx = build_gizmo_ctx(state, view_proj, screen_offset, entity_pos);
    
    // ── 更新悬停状态（仅在非拖拽时）──
    if state.gizmo_active.is_none() {
        state.gizmo_hovered = match mouse_pos {
            Some(mp) => gizmo::pick_gizmo(mp, &ctx),
            None => None,
        };
        ctx.hovered = state.gizmo_hovered; // 同步更新 ctx
    }
    
    // ── 开始拖拽（左键按下且悬停在 gizmo 上）──
    if primary_clicked && state.gizmo_active.is_none() {
        if let Some(hovered_part) = state.gizmo_hovered {
            if let Some(mp) = mouse_pos {
                let entity = &state.scene.entities[selected_idx];
                let start_value = match state.tool_mode {
                    ToolMode::Translate => entity.transform.position,
                    ToolMode::Scale => entity.transform.scale,
                    ToolMode::Rotate | ToolMode::Rect => Vec3::ZERO,
                };
                
                let start_hit = match hovered_part {
                    GizmoPart::AxisX | GizmoPart::AxisY | GizmoPart::AxisZ => {
                        let axis = match hovered_part {
                            GizmoPart::AxisX => Vec3::X,
                            GizmoPart::AxisY => Vec3::Y,
                            _ => Vec3::Z,
                        };
                        gizmo::hit_position_on_axis(mp, &ctx, axis).unwrap_or(entity_pos)
                    }
                    GizmoPart::PlaneXY | GizmoPart::PlaneXZ | GizmoPart::PlaneYZ => {
                        let normal = match hovered_part {
                            GizmoPart::PlaneXY => Vec3::Z,
                            GizmoPart::PlaneXZ => Vec3::Y,
                            _ => Vec3::X,
                        };
                        gizmo::hit_position_on_plane(mp, &ctx, normal).unwrap_or(entity_pos)
                    }
                    _ => entity_pos,
                };
                
                state.gizmo_active = Some(crate::state::GizmoDrag {
                    part: hovered_part,
                    start_value,
                    start_hit,
                    start_screen_pos: mp,
                });
                
                // 更新 ctx 中的 active 状态
                ctx.active = state.gizmo_active.clone();
            }
        }
    }
    
    // ── 更新拖拽（每帧）──
    if let Some(drag) = state.gizmo_active.clone() {
        if let Some(mp) = mouse_pos {
            if let Some(delta) = gizmo::compute_transform(&drag, mp, &ctx, state.tool_mode) {
                let entity = &mut state.scene.entities[selected_idx];
                match delta {
                    gizmo::TransformDelta::Position(new_pos) => entity.transform.position = new_pos,
                    gizmo::TransformDelta::Rotation(rel_rot, _) => entity.transform.rotation = rel_rot * entity.transform.rotation,
                    gizmo::TransformDelta::Scale(new_scale) => entity.transform.scale = new_scale,
                }
            }
        }
    }
    
    // ── 释放拖拽 ──
    if primary_released {
        state.gizmo_active = None;
    }
    
    // ── 绘制 Gizmo（使用最新的状态）──
    let gizmo_size = gizmo::gizmo_world_size(state.editor_camera.eye(), entity_pos);
    gizmo::draw_gizmo(
        painter, &view_proj, entity_pos, state.viewport_logical_size,
        state.tool_mode, state.gizmo_hovered, state.gizmo_active.as_ref(),
        gizmo_size, screen_offset,
    );
}

// ============================================================================
// 相机控制相关
// ============================================================================

/// 处理飞行模式相机控制
fn handle_fly_camera(state: &mut EditorState, ui: &egui::Ui, response: &egui::Response) {
    let right_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Secondary));
    
    if response.hovered() && right_down {
        state.fly_state = true;
    }
    if !right_down {
        state.fly_state = false;
    }
    
    if !state.fly_state {
        return;
    }
    
    ui.ctx().request_repaint();
    let dt = ui.input(|i| i.predicted_dt);
    
    if response.hovered() {
        let delta = ui.input(|i| i.pointer.delta());
        state.editor_camera.rotate(delta.x, delta.y);
    }
    
    // 键盘移动
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
    
    // 滚轮缩放
    if response.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            state.editor_camera.zoom(scroll);
            state.zoom_indicator = 1.5;
        }
    }
    
    state.zoom_indicator = (state.zoom_indicator - dt).max(0.0);
}

// ============================================================================
// UI 叠加层相关
// ============================================================================

/// 绘制缩放速度指示器
fn draw_zoom_indicator(state: &EditorState, painter: &egui::Painter, rect: egui::Rect) {
    if state.zoom_indicator <= 0.0 {
        return;
    }
    
    painter.text(
        egui::pos2(rect.right() - 8.0, rect.bottom() - 28.0),
        egui::Align2::RIGHT_BOTTOM,
        &format!("Speed: {:.1}", state.editor_camera.pan_speed),
        egui::FontId::monospace(12.0),
        egui::Color32::from_rgb(180, 220, 255),
    );
}

/// 绘制底部状态栏（FPS 和当前工具）
fn draw_bottom_status_bar(state: &EditorState, painter: &egui::Painter, rect: egui::Rect, ui: &egui::Ui) {
    let tool_label = match state.tool_mode {
        ToolMode::Translate => "Translate",
        ToolMode::Rotate => "Rotate",
        ToolMode::Scale => "Scale",
        ToolMode::Rect => "Rect",
    };
    
    // 提前计算 FPS，避免在 painter.text 中借用 ui
    let fps = ui.input(|i| i.predicted_dt).recip().round();
    
    painter.text(
        egui::pos2(rect.left() + 8.0, rect.bottom() - 8.0),
        egui::Align2::LEFT_BOTTOM,
        &format!("{:.0} FPS | Tool: {}", fps, tool_label),
        egui::FontId::monospace(11.0),
        egui::Color32::from_gray(140),
    );
}

// ============================================================================
// 主入口函数
// ============================================================================

/// 在 `Ui` 内渲染场景视图内容。
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {
    // 顶部栏
    draw_top_bar(state, ui);
    
    // 视口区域
    let viewport = setup_viewport(ui, state);
    let painter = ui.painter();
    
    // 构建视图投影矩阵
    let (vp_w, vp_h) = viewport.pixel_size;
    let view_proj = build_view_projection(state, vp_w, vp_h);
    
    // 渲染 3D 场景
    let tex_id = render_scene(state, vp_w, vp_h, &view_proj);
    draw_render_texture(&painter, viewport.rect, tex_id);
    
    // 物理调试线框
    draw_physics_debug_lines(state, &painter, &view_proj, viewport.rect);
    
    // Gizmo 交互
    let mouse_pos = viewport.response.hover_pos();
    let primary_clicked = viewport.response.drag_started_by(egui::PointerButton::Primary) && state.is_editing();
    let primary_released = viewport.response.drag_stopped_by(egui::PointerButton::Primary) && state.is_editing();
    
    handle_gizmo_interaction(
        state, &painter, view_proj, &viewport,
        mouse_pos, primary_clicked, primary_released,
    );
    
    // 实体拾取（左键点击，非 gizmo 拖拽时）
    if primary_clicked
        && state.gizmo_active.is_none()
        && state.gizmo_hovered.is_none()
        && state.is_editing()
    {
        if let Some(mp) = mouse_pos {
            let screen_offset = viewport.rect.min.to_vec2();
            state.selected_entity = pick_entity(state, mp, &view_proj, screen_offset);
        }
    }
    
    // ── 工具切换快捷键（有选中实体时全局生效）──
    if state.is_editing() && state.selected_entity.is_some() {
        if ui.input(|i| i.key_pressed(egui::Key::W)) {
            state.tool_mode = ToolMode::Translate;
        } else if ui.input(|i| i.key_pressed(egui::Key::E)) {
            state.tool_mode = ToolMode::Rotate;
        } else if ui.input(|i| i.key_pressed(egui::Key::R)) {
            state.tool_mode = ToolMode::Scale;
        }
    }

    // 相机控制
    handle_fly_camera(state, ui, &viewport.response);
    
    // UI 叠加层
    draw_zoom_indicator(state, &painter, viewport.rect);
    draw_bottom_status_bar(state, &painter, viewport.rect, ui);
}