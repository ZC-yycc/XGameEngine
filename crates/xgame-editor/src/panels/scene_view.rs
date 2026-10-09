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

/// 把世界坐标点换算成某父节点下的局部坐标。
///
/// 父世界矩阵不可逆（含 0 缩放）时原样返回世界坐标——宁可位置略偏，也不要
/// 把一个 NaN 塞进实体的变换里，那会让整个物体从画面上消失。
fn world_point_to_local(parent_world: Mat4, world_point: Vec3) -> Vec3 {
    if parent_world.determinant().abs() > 1e-12 {
        (parent_world.inverse() * world_point.extend(1.0)).truncate()
    } else {
        world_point
    }
}

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

/// 一帧里与视口鼠标相关的信号，全部来自同一个 `viewport.response`。
///
/// 之所以把「点击」和「拖拽」分开记：egui 对 `click_and_drag` 的控件，**按下—原地
/// 松手**与**按住并移动**是两条互斥的路径 —— 前者只给 `clicked()`，后者只给
/// `drag_started_by()`。拾取得看 `clicked`，gizmo 拖拽得看 `drag_started`，
/// 拿错一个就是"点不中物体"或"拖不动操作柄"。
struct PointerInput {
    /// 指针在视口内的位置（不在视口里时为 `None`）。
    pos: Option<egui::Pos2>,
    /// 左键是否**按住**。gizmo 悬停靠它冻结，见 `handle_gizmo_interaction`。
    primary_down: bool,
    /// 本帧指针移动过足够距离，被判定为拖拽开始。
    drag_started: bool,
    /// 本帧拖拽结束（松手）。
    drag_stopped: bool,
    /// 本帧是一次纯点击（按下—几乎没移动—松手）。
    clicked: bool,
}

impl PointerInput {
    /// 从视口响应与本帧输入状态里读出全部信号。
    fn read(response: &egui::Response, ui: &egui::Ui, editing: bool) -> Self {
        let primary_down = ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary));
        let pressed = |f: fn(&egui::Response, egui::PointerButton) -> bool| {
            editing && f(response, egui::PointerButton::Primary)
        };
        Self {
            pos: response.hover_pos(),
            primary_down,
            drag_started: pressed(egui::Response::drag_started_by),
            drag_stopped: pressed(egui::Response::drag_stopped_by),
            clicked: editing && response.clicked(),
        }
    }
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
        renderer.render(
            &state.scene,
            view_proj,
            &state.editor_camera.eye(),
            xgame_render::scene_pipeline::SceneLighting::new(
                state.scene.ambient,
                &state.scene.lights,
            ),
        );
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
            egui::Stroke::new(1.5_f32, color32),
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

    // 拾取用的是**世界**位置与尺寸：有父节点的实体，局部坐标不是它在屏幕上的位置。
    let world_matrices = state.scene.world_matrices();
    for (idx, entity) in state.scene.entities.iter().enumerate() {
        if !entity.has_model() {
            continue;
        }
        let Some(world) = world_matrices.get(idx) else { continue };
        let (scale, _, translation) = world.to_scale_rotation_translation();
        // 使用 scale 长度的一半作为包围球半径
        let radius = scale.length().max(0.1) * 0.5;
        if let Some(t) = ray_intersect_sphere(origin, dir, translation, radius) {
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
///
/// `drag_started` / `drag_stopped` 是**真正判定成拖拽**的那一对信号，与"点击"无关：
/// egui 对 `click_and_drag` 的控件要等指针移动过一段距离才认定拖拽开始，所以
/// "按住不放、原地松手"不会走到这里 —— 那属于拾取（见 `show_content`）。
fn handle_gizmo_interaction(
    state: &mut EditorState,
    painter: &egui::Painter,
    view_proj: Mat4,
    viewport: &ViewportInfo,
    pointer: &PointerInput,
) {
    let PointerInput {
        pos: mouse_pos,
        primary_down,
        drag_started,
        drag_stopped,
        ..
    } = *pointer;
    if !state.is_editing() {
        // 中途被切出编辑态（例如点到了工具栏）：这次拖拽作废。快照一起丢掉，
        // 否则它会在下一次任意拖拽松手时被推入撤销栈，撤销就"跳"到别的状态上。
        state.gizmo_active = None;
        state.undo_pending = None;
        return;
    }
    
    let Some(selected_idx) = state.selected_entity else {
        // 没有选中实体时清除 gizmo 状态
        state.gizmo_hovered = None;
        if state.gizmo_active.take().is_some() {
            state.undo_pending = None;
        }
        return;
    };
    
    let entity_world = state.scene.world_transform(selected_idx);
    // Gizmo 画在**世界**位置：实体的局部变换在父节点之下，直接拿来当世界坐标会偏。
    let entity_pos = entity_world.position;
    let screen_offset = viewport.rect.min.to_vec2();
    
    // 构建当前帧的 Gizmo 上下文
    let mut ctx = build_gizmo_ctx(state, view_proj, screen_offset, entity_pos);
    
    // ── 更新悬停状态 ──
    //
    // 按住左键期间**冻结**悬停，不重新拾取。原因：egui 要等指针移动过一段距离
    // （约 6 px）才判定拖拽开始，而操作柄的命中半径只有 6–14 px —— 等于拖拽成立的
    // 那一刻指针可能已经滑出操作柄了。这时若重新拾取就会得到 `None`，
    // `drag_started` 已经用掉、`gizmo_active` 又设不上，**拖拽永远开始不了**
    // （拖得越快越抓不住）。悬停在"按下那一帧"才是准确的，保持到松手就行。
    if state.gizmo_active.is_none() && !primary_down {
        state.gizmo_hovered = match mouse_pos {
            Some(mp) => gizmo::pick_gizmo(mp, &ctx),
            None => None,
        };
        ctx.hovered = state.gizmo_hovered; // 同步更新 ctx
    }
    
    // ── 开始拖拽（指针已移动一段距离且当前悬停在 gizmo 上）──
    if drag_started && state.gizmo_active.is_none() {
        if let Some(hovered_part) = state.gizmo_hovered {
            if let Some(mp) = mouse_pos {
                // 拖拽前的整场景快照，松手时（`drag_stopped`）才入撤销栈 ——
                // 与 inspector 的拖拽式编辑同一套范式：一次拖拽 = 一步撤销，
                // 中途每帧的中间值不占撤销位。
                state.undo_pending = Some(state.scene.clone());

                let entity = &state.scene.entities[selected_idx];
                let start_value = match state.tool_mode {
                    // Translate 在世界空间进行，起始值用世界位置
                    ToolMode::Translate => entity_world.position,
                    // Scale 定义在物体自身轴上，用局部缩放
                    ToolMode::Scale => entity.transform.scale,
                    ToolMode::Rotate | ToolMode::Rect => Vec3::ZERO,
                };
                
                let start_hit = match hovered_part {
                    GizmoPart::AxisX | GizmoPart::AxisY | GizmoPart::AxisZ => {
                        let axis = gizmo::handle_axis(hovered_part).unwrap_or(Vec3::Y);
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
                    // 旋转环：必须给出**旋转平面**上的交点。
                    //
                    // 这里旧实现落进 `_ => entity_pos`，于是 `start_hit - entity_pos`
                    // 是零向量，`compute_rotate` 里 `.normalize()` 得到 NaN，起始角度、
                    // 增量四元数、实体旋转一路全变 NaN —— 表现为"拖一下环，物体就没了"。
                    GizmoPart::RingX | GizmoPart::RingY | GizmoPart::RingZ => {
                        let axis = gizmo::handle_axis(hovered_part).unwrap_or(Vec3::Y);
                        gizmo::hit_position_on_plane(mp, &ctx, axis).unwrap_or(entity_pos)
                    }
                    GizmoPart::Uniform => entity_pos,
                };

                // 沿轴拖拽（平移/缩放）要的起始**轴坐标**，与 `start_hit` 是两码事：
                // 后者是平面交点，位移永远垂直于轴、投影恒为 0。详见
                // `gizmo::axis_param_under_mouse` 的注释。
                let start_axis = match gizmo::handle_axis(hovered_part) {
                    Some(axis) => {
                        gizmo::axis_param_under_mouse(mp, &ctx, axis).unwrap_or(0.0)
                    }
                    None => 0.0,
                };
                
                state.gizmo_active = Some(crate::state::GizmoDrag {
                    part: hovered_part,
                    start_value,
                    start_hit,
                    start_axis,
                    start_screen_pos: mp,
                    start_rotation: entity_world.rotation,
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
                // 把 gizmo 算出的**世界**增量换算回实体的**局部**变换
                let parent_world = state
                    .scene
                    .parent_index(selected_idx)
                    .map(|p| state.scene.world_matrix(p))
                    .unwrap_or(Mat4::IDENTITY);
                let (_, parent_rot, _) = parent_world.to_scale_rotation_translation();

                let entity = &mut state.scene.entities[selected_idx];
                match delta {
                    gizmo::TransformDelta::Position(new_world_pos) => {
                        entity.transform.position = world_point_to_local(parent_world, new_world_pos);
                    }
                    gizmo::TransformDelta::Rotation(world_delta) => {
                        // 世界旋转 = 增量 × 起始世界旋转；再左乘父旋转的逆得到局部值。
                        // 按起始值重算（而不是往当前值上乘），一次拖拽才不会累积过头。
                        let new_world_rot = world_delta * drag.start_rotation;
                        entity.transform.rotation = parent_rot.inverse() * new_world_rot;
                    }
                    gizmo::TransformDelta::Scale(new_scale) => entity.transform.scale = new_scale,
                }
            }
        }
    }
    
    // ── 释放拖拽：把拖拽前的快照推入撤销栈 ──
    if drag_stopped {
        state.gizmo_active = None;
        if let Some(snapshot) = state.undo_pending.take() {
            state.push_undo_snapshot(snapshot);
        }
    }
    
    // ── 绘制 Gizmo ──
    // ctx 是函数开头建的快照，本帧里 hovered / active 都被改过（刚悬停、刚开始
    // 或刚结束拖拽），画之前同步回最新值，避免比实际状态晚一帧。
    ctx.hovered = state.gizmo_hovered;
    ctx.active = state.gizmo_active.clone();
    gizmo::draw_gizmo(painter, &ctx, state.tool_mode);
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
        format!("Speed: {:.1}", state.editor_camera.pan_speed),
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
        format!("{:.0} FPS | Tool: {}", fps, tool_label),
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
    draw_render_texture(painter, viewport.rect, tex_id);
    
    // 物理调试线框
    draw_physics_debug_lines(state, painter, &view_proj, viewport.rect);
    
    // ── 鼠标信号 ──
    // 拖拽（指针移动过一段距离才算）交给 gizmo；纯点击交给拾取。两者互斥，
    // 别拿 drag_started 当「按下」用。
    let pointer = PointerInput::read(&viewport.response, ui, state.is_editing());

    handle_gizmo_interaction(state, painter, view_proj, &viewport, &pointer);

    // 实体拾取（左键纯点击，且没点在 gizmo 上）
    if pointer.clicked
        && state.gizmo_active.is_none()
        && state.gizmo_hovered.is_none()
    {
        if let Some(mp) = pointer.pos {
            let screen_offset = viewport.rect.min.to_vec2();
            state.selected_entity = pick_entity(state, mp, &view_proj, screen_offset);
        }
    }
    
    // ── 工具切换快捷键（有选中实体且未进入飞行模式时生效）──
    // 飞行模式下 W/A/S/D/Q/E 是相机移动键，必须让位，否则按 W 会同时平移相机和切工具。
    if state.is_editing() && !state.fly_state && state.selected_entity.is_some() {
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
    draw_zoom_indicator(state, painter, viewport.rect);
    draw_bottom_status_bar(state, painter, viewport.rect, ui);
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 视口里的一次合成鼠标脚本。每项是 `(x, y, 按下状态)`：
    /// `None` = 只移动指针，`Some(true)` = 按下左键，`Some(false)` = 松开左键。
    ///
    /// 逐帧喂给一个与 `setup_viewport` 同规格的 `Sense::click_and_drag()` 区域，
    /// 收集每帧的 `(clicked, drag_started_by(Primary))`。
    fn drive(script: &[(f32, f32, Option<bool>)]) -> Vec<(bool, bool)> {
        let ctx = egui::Context::default();
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
        let mut out = Vec::new();

        for &(x, y, press) in script {
            let pos = egui::pos2(x, y);
            let mut input = egui::RawInput {
                screen_rect: Some(rect),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(pos));
            if let Some(pressed) = press {
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                });
            }

            let mut snapshot = None;
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(ctx, |ui| {
                        let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
                        snapshot = Some((
                            resp.clicked(),
                            resp.drag_started_by(egui::PointerButton::Primary),
                        ));
                    });
            });
            out.push(snapshot.expect("每帧都应拿到 response"));
        }
        out
    }

    /// **「视口里点一下选不中物体」的根因测试。**
    ///
    /// egui 对 `click_and_drag` 的控件要等指针**移动过一段距离**才认定拖拽开始
    /// （`Response::drag_started_by` 的官方说明："If the widget also senses clicks,
    /// this will only become true if the pointer has moved a bit"）。
    /// 所以「按下—原地松手」只会给出 `clicked()`，`drag_started_by()` 全程为 false。
    ///
    /// 旧代码把 `drag_started_by(Primary)` 当成"点击"来驱动拾取，于是纯点击永远
    /// 选不中 —— 必须鼠标拖一下才行。这条测试同时锁住两件事：
    /// 1. egui 的这条语义（升级 egui 改动它，这里先炸）；
    /// 2. 我们的用法分工（点击走 `clicked()`，拖拽走 `drag_started_by()`）。
    #[test]
    fn plain_click_yields_clicked_not_drag_started() {
        let frames = drive(&[
            (200.0, 150.0, None),       // 悬停
            (200.0, 150.0, Some(true)), // 按下
            (200.0, 150.0, Some(false)),// 原地松手
        ]);

        assert_eq!(frames.len(), 3);
        for (i, &(_, drag_started)) in frames.iter().enumerate() {
            assert!(
                !drag_started,
                "第 {i} 帧不该被判成拖拽开始：指针没移动过"
            );
        }
        assert!(
            frames[2].0,
            "原地松手必须给出 clicked() —— 视口拾取正是靠它"
        );
        assert!(!frames[0].0 && !frames[1].0, "只有松手那一帧才算点击");
    }

    /// 另一半：真的拖了，就该走 `drag_started_by()`，且不再算点击。
    ///
    /// gizmo 拖拽依赖这个信号。两条路径互斥，所以不能用同一个信号驱动拾取和拖拽。
    #[test]
    fn moving_while_held_yields_drag_started_not_click() {
        let frames = drive(&[
            (200.0, 150.0, None),        // 悬停
            (200.0, 150.0, Some(true)),  // 按下
            (260.0, 150.0, None),        // 按住并移动
            (260.0, 150.0, Some(false)), // 松手
        ]);

        assert!(
            frames.iter().any(|&(_, drag_started)| drag_started),
            "按住移动过一段距离后必须判定为拖拽开始"
        );
        assert!(
            !frames[3].0,
            "拖拽结束的松手不该再算成点击（否则拖完 gizmo 会顺手改一次选中）"
        );
    }

    // -----------------------------------------------------------------------
    // 端到端：合成事件 → 视口 → 拾取
    // -----------------------------------------------------------------------

    /// 造一个临时项目 + 只含一个立方体的场景。
    ///
    /// 返回的项目目录由调用方负责删（测试正常跑完时删，断言失败时留着便于排查）。
    fn test_state(tag: &str) -> (EditorState, std::path::PathBuf) {
        let base = std::env::temp_dir().join(format!("xgame_sceneview_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let handle =
            xgame_core::ProjectHandle::create(&base, "PickTest").expect("创建临时项目");

        let mut state = EditorState::new(handle);
        // 用自己造的干净场景，别依赖默认场景里有什么。
        state.scene = xgame_core::Scene::new("PickTest");
        let mut cube = xgame_core::Entity::new("Box");
        cube.model = "builtin://cube".to_string();
        state.scene.add_entity(cube);
        state.selected_entity = None;
        (state, base)
    }

    /// 跑一帧 `show_content`，把合成事件喂进去。
    ///
    /// 面板用 `Frame::none()`：视口的屏幕偏移就等于「顶栏高度」这一行，
    /// 测试里才好算出该点哪里。
    fn frame(ctx: &egui::Context, state: &mut EditorState, events: Vec<egui::Event>, screen: egui::Rect) {
        let input = egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| show_content(state, ui));
        });
    }

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }

    /// **端到端回归**：在视口正中点一下，应当选中那里的立方体。
    ///
    /// 直接跑 `show_content`，覆盖「纯点击 → 拾取 → `selected_entity`」整条链路。
    /// 这正是「视口里点不中物体」那个 bug 的守卫：只要拾取又改回拿
    /// `drag_started_by` 驱动（而不是 `clicked()`），这条立刻变红。
    #[test]
    fn clicking_the_viewport_selects_the_entity_under_the_cursor() {
        let (mut state, base) = test_state("click_pick");
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));

        // 第 1 帧：空跑，只为量出视口尺寸（顶栏占掉的高度 = 屏幕高 − 视口高）。
        frame(&ctx, &mut state, vec![], screen);
        let vp = state.viewport_logical_size;
        assert!(vp.y > 0.0, "视口应当有非零尺寸");
        assert_eq!(state.selected_entity, None, "起始不该有选中");

        // 立方体就摆在相机正前方，投影落在视口正中。
        let top_bar_h = screen.height() - vp.y;
        let center = egui::pos2(screen.width() * 0.5, top_bar_h + vp.y * 0.5);

        // 第 2~4 帧：移过去 → 按下 → 原地松手（一次纯点击，没有任何拖动）。
        frame(&ctx, &mut state, vec![egui::Event::PointerMoved(center)], screen);
        frame(&ctx, &mut state, vec![button(center, true)], screen);
        frame(&ctx, &mut state, vec![button(center, false)], screen);

        assert_eq!(
            state.selected_entity,
            Some(0),
            "在视口里点一下应当选中立方体（旧实现拿 drag_started 当点击，永远选不中）"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 点视口的空白处应当取消选中；点中立方体再选中 —— 证明拾取是按射线判定，
    /// 而不是"有实体就随便选一个"。
    #[test]
    fn clicking_empty_space_deselects() {
        let (mut state, base) = test_state("click_empty");
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));

        frame(&ctx, &mut state, vec![], screen);
        let vp = state.viewport_logical_size;
        let top_bar_h = screen.height() - vp.y;
        let center = egui::pos2(screen.width() * 0.5, top_bar_h + vp.y * 0.5);

        // 先点中立方体
        frame(&ctx, &mut state, vec![egui::Event::PointerMoved(center)], screen);
        frame(&ctx, &mut state, vec![button(center, true)], screen);
        frame(&ctx, &mut state, vec![button(center, false)], screen);
        assert_eq!(state.selected_entity, Some(0));

        // 再点左上角（视口里的空白，那里没有东西）
        let empty = egui::pos2(8.0, top_bar_h + 8.0);
        frame(&ctx, &mut state, vec![egui::Event::PointerMoved(empty)], screen);
        frame(&ctx, &mut state, vec![button(empty, true)], screen);
        frame(&ctx, &mut state, vec![button(empty, false)], screen);

        assert_eq!(state.selected_entity, None, "点空白处应当取消选中");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 视口在屏幕上的矩形（`Frame::none()` 下：x 从 0 起，y 从顶栏下方起）。
    fn viewport_rect(screen: egui::Rect, vp: egui::Vec2) -> egui::Rect {
        let top_bar_h = screen.height() - vp.y;
        egui::Rect::from_min_size(egui::pos2(0.0, top_bar_h), vp)
    }

    /// 选中场景里唯一的实体（跑一遍"移过去 → 按下 → 松手"的纯点击）。
    fn select_first_entity(
        ctx: &egui::Context,
        state: &mut EditorState,
        screen: egui::Rect,
        at: egui::Pos2,
    ) {
        frame(ctx, state, vec![egui::Event::PointerMoved(at)], screen);
        frame(ctx, state, vec![button(at, true)], screen);
        frame(ctx, state, vec![button(at, false)], screen);
    }

    /// **端到端回归**：拖一下 gizmo 应当产生**一步可撤销的编辑**。
    ///
    /// 旧实现逐帧直接改 `transform`，既不设 `undo_pending`、松手也不入栈，
    /// 于是 gizmo 拖拽完全无法撤销（Ctrl+Z 只回退其它操作）。这条测试盯住
    /// "拖完必须有东西可撤销"，且撤销回到拖之前的位置。
    #[test]
    fn dragging_the_gizmo_pushes_a_single_undo_step() {
        let (mut state, base) = test_state("gizmo_undo");
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));

        frame(&ctx, &mut state, vec![], screen);
        let vp = state.viewport_logical_size;
        let rect = viewport_rect(screen, vp);
        let center = rect.center();

        select_first_entity(&ctx, &mut state, screen, center);
        assert_eq!(state.selected_entity, Some(0), "先得选中才能拖 gizmo");
        assert!(!state.can_undo(), "选本身不该产生撤销点");

        // 沿着 gizmo 的 X 轴取一个屏幕点：轴线中点偏外 70%，避开中心。
        let (w, h) = state.viewport_pixel_size;
        let view_proj = build_view_projection(&state, w, h);
        let entity_pos = state.scene.world_transform(0).position;
        let gz_size = gizmo::gizmo_world_size(state.editor_camera.eye(), entity_pos);
        let on_axis = gizmo::project_to_screen(
            entity_pos + Vec3::X * gz_size * 0.7,
            &view_proj,
            vp,
            rect.min.to_vec2(),
        );
        assert!(rect.contains(on_axis), "轴上的点应当落在视口里: {on_axis:?}");

        // 悬停 → 按下 → 拖动（指针移动过阈值才算拖拽）→ 松手。
        frame(&ctx, &mut state, vec![egui::Event::PointerMoved(on_axis)], screen);
        assert_eq!(
            state.gizmo_hovered,
            Some(GizmoPart::AxisX),
            "指针落在 X 轴线上应当判成悬停 AxisX"
        );

        let before = state.scene.entities[0].transform.position;
        let drag_to = on_axis + egui::vec2(60.0, 0.0);
        let drag_more = on_axis + egui::vec2(120.0, 0.0);

        frame(&ctx, &mut state, vec![button(on_axis, true)], screen);
        // 这一帧才被判成拖拽开始，抓取参考点（`start_hit`）也在这时记录，
        // 所以本帧不会产生位移 —— 头 ~6px 是 egui 的拖拽阈值死区。
        frame(&ctx, &mut state, vec![egui::Event::PointerMoved(drag_to)], screen);
        assert!(state.gizmo_active.is_some(), "移动过阈值后应当进入拖拽态");
        assert_eq!(
            state.scene.entities[0].transform.position, before,
            "拖拽被判定的那一帧只记录参考点，不动位置（避免开拖就跳一下）"
        );

        // 再动一帧才开始真正改位置。
        frame(&ctx, &mut state, vec![egui::Event::PointerMoved(drag_more)], screen);
        let mid = state.scene.entities[0].transform.position;
        assert!(
            (mid - before).length() > 1e-4,
            "继续拖动应当改位置: {before} -> {mid}"
        );
        assert!(
            mid.y.abs() < 1e-5 && mid.z.abs() < 1e-5,
            "拖 X 轴只该改 x（旧实现轴的位移恒为常量、根本不动）: {mid}"
        );

        frame(&ctx, &mut state, vec![button(drag_more, false)], screen);
        assert!(state.gizmo_active.is_none(), "松手应当结束拖拽");
        assert!(
            state.can_undo(),
            "一次 gizmo 拖拽必须在撤销栈里留一步（旧实现什么都不留）"
        );

        // 撤销一次应当回到拖拽前的位置，且只需要撤一次。
        state.undo();
        let after = state.scene.entities[0].transform.position;
        assert!(
            (after - before).length() < 1e-5,
            "撤销一次就该回到拖之前: {before} != {after}"
        );
        assert!(!state.can_undo(), "一次拖拽只该产生一步撤销");

        let _ = std::fs::remove_dir_all(&base);
    }

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    /// 工具切换快捷键在**飞行模式**下必须让位。
    ///
    /// 飞行模式用 W/A/S/D/Q/E 平移相机；其中 W/E 同时又是 Translate/Rotate 的
    /// 快捷键。两个都生效的话，按住右键飞的同时按 W，相机前进、工具也被改掉。
    #[test]
    fn tool_shortcuts_yield_to_fly_camera() {
        let (mut state, base) = test_state("tool_shortcut");
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));

        frame(&ctx, &mut state, vec![], screen);
        let vp = state.viewport_logical_size;
        let center = viewport_rect(screen, vp).center();

        select_first_entity(&ctx, &mut state, screen, center);
        assert_eq!(state.selected_entity, Some(0));

        // 非飞行态：W 切到 Translate。
        state.tool_mode = ToolMode::Rotate;
        state.fly_state = false;
        frame(&ctx, &mut state, vec![key(egui::Key::W)], screen);
        assert_eq!(state.tool_mode, ToolMode::Translate, "非飞行态 W 应当切工具");

        // 飞行态：W 只用来推相机，不该动工具。
        state.tool_mode = ToolMode::Rotate;
        state.fly_state = true;
        frame(&ctx, &mut state, vec![key(egui::Key::W)], screen);
        assert_eq!(
            state.tool_mode,
            ToolMode::Rotate,
            "飞行态按 W 不该把工具切走（否则飞的同时工具被改掉）"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}