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
use crate::panels::theme;
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
    state.viewport_rect = rect;
    
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

/// 渲染 3D 场景并返回纹理 ID。
///
/// 渲染在 **UI 线程上同步进行**：输入处理完 → 重画离屏 3D → 同帧上屏。
/// 编辑器视口必须让"输入到像素"零管线延迟 —— 拖 gizmo、转视角时物体要精确钉在
/// 光标下，任何异步/跨帧都会表现为"不跟手"。稳态渲染实测单帧 CPU 仅 ~0.4ms
/// （指纹门控让静态帧整帧跳过），同步渲染完全可承受，且没有第二个线程与窗口的
/// 模态移动循环抢资源。
fn render_scene(state: &mut EditorState, vp_w: u32, vp_h: u32, view_proj: &Mat4) -> Option<egui::TextureId> {
    // 记下"这一帧渲染用的是哪个相机"，供帧序回归测试断言（见 `show_content`
    // 的帧序说明）。渲染必须发生在本帧输入之后 —— 这条不变量决定了视口跟不跟手。
    #[cfg(test)]
    {
        state.last_render_view_proj = Some(*view_proj);
    }

    if vp_w == 0 || vp_h == 0 {
        return None;
    }

    // 画面设置从清单读 —— 编辑器视口 / 独立游戏 / 无头截图吃的是**同一份**
    // `render` 节（视口不搞自己的"调试参数"，否则截图与玩家画面会分叉）。
    // 先取出来再借 `scene_renderer`，免得两个 `state` 借用打架。
    let settings = state.project.manifest().render;
    let renderer = state.scene_renderer.as_mut()?;
    renderer.set_viewport_size(vp_w, vp_h);
    renderer.render(
        &state.scene,
        view_proj,
        &state.editor_camera.eye(),
        xgame_render::scene_pipeline::SceneLighting::new(
            state.scene.ambient,
            &state.scene.lights,
        ),
        settings,
    );
    renderer.texture_id()
}

/// 绘制渲染纹理或空白背景
fn draw_render_texture(painter: &egui::Painter, rect: egui::Rect, tex_id: Option<egui::TextureId>) {
    if let Some(tex_id) = tex_id {
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(tex_id, rect, uv, egui::Color32::WHITE);
    } else {
        // 无渲染纹理时的占位底色：走主题常量，和 UE 一样接近纯黑，
        // 让视口和周围面板在视觉上拉开层次。
        painter.rect_filled(rect, 0.0, crate::panels::theme::colors::VIEWPORT_BG);
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

/// 通过射线检测拾取实体。
///
/// # 为什么用「射线 × 世界 AABB」而不是「射线 × 包围球」
///
/// 旧实现给每个实体造一个包围球，半径取 `scale.length() * 0.5`。对**被拉伸过**
/// 的物体这是灾难性的高估：一块 `16 × 0.5 × 16` 的地面半径会算成 16，而相机
/// 离场景中心只有约 8.7 —— 相机直接落在球**内部**，射线永远与它相交，于是
/// 无论点哪儿都先选中那块地面（真机表现：**想选的物体怎么点都选不中**）。
///
/// 现在改成逐实体拿**网格的本地包围盒**（`GltfMesh::bounds`）经世界矩阵变换成
/// AABB 再做 slab 求交，取最近的交点。范围是"真实几何包住的那一块"，不是球。
/// 拿不到网格数据（没加载过 / 无渲染器）时退回单位立方体 —— 内置图元本来就是
/// 单位尺寸，这个兜底是准确的。
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

        // 本地包围盒 → 世界 AABB（变换 8 个角点再求盒，旋转也能正确处理）。
        // 包围盒由渲染器加载模型后缓存；还没加载过的退回单位盒。
        let local = state
            .scene_renderer
            .as_ref()
            .and_then(|r| r.mesh_bounds(&entity.model))
            .unwrap_or(gizmo::UNIT_BOUNDS);
        let aabb = local.transformed(world);

        if let Some(t) = gizmo::ray_aabb(origin, dir, &aabb) {
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
/// 处理 Gizmo 交互（悬停、拖拽开始、逐帧更新、释放）—— **只改状态，不落笔**。
///
/// # 为什么要和绘制拆开
///
/// `show_content` 的帧序是「先处理输入 → 再重画离屏 3D → 最后画叠加层」。
/// Gizmo 是叠加层，必须画在 3D 之上，所以它得等离屏渲染之后再落笔；而它的
/// **状态更新**必须发生在那之前（否则这一帧的拖拽要等下一帧才看得见，白白多
/// 一帧延迟 —— 拖物体、转视角都会"黏手"）。拆成"更新"与"绘制"两步是唯一
/// 能同时满足这两条的做法。
///
/// 返回本帧最终的 [`GizmoCtx`]；`None` 表示当前不该画（非编辑态 / 没选中）。
///
/// `drag_started` / `drag_stopped` 是**真正判定成拖拽**的那一对信号，与"点击"无关：
/// egui 对 `click_and_drag` 的控件要等指针移动过一段距离才认定拖拽开始，所以
/// "按住不放、原地松手"不会走到这里 —— 那属于拾取（见 `show_content`）。
fn handle_gizmo_interaction(
    state: &mut EditorState,
    view_proj: Mat4,
    viewport: &ViewportInfo,
    pointer: &PointerInput,
) -> Option<gizmo::GizmoCtx> {
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
        return None;
    }

    let Some(selected_idx) = state.selected_entity else {
        // 没有选中实体时清除 gizmo 状态
        state.gizmo_hovered = None;
        if state.gizmo_active.take().is_some() {
            state.undo_pending = None;
        }
        return None;
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

                // 用 `get` 而不是裸索引：选中索引理论上由每帧入口的
                // `clamp_selection` 收敛过，但这里再挡一层，任何漏网都会变成
                // "这一帧不动"而不是"编辑器 panic 退出"。
                let Some(entity) = state.scene.entities.get(selected_idx) else {
                    return Some(build_gizmo_ctx(state, view_proj, screen_offset, entity_pos));
                };
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
                    // 轴拖拽的参考线锚点 = 拖拽开始那一瞬实体的世界位置。
                    // 绝不能让它跟着走 —— `handle_gizmo_interaction` 每帧都会按**当前**
                    // 位置重建 ctx，锚点若取 ctx.entity_pos 就成了自反馈（拖拽抽搐）。
                    axis_anchor: entity_pos,
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

                // 同样用 `get_mut` 兜底（见上面对 `get` 的说明）。
                if let Some(entity) = state.scene.entities.get_mut(selected_idx) {
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
    }
    
    // ── 释放拖拽：把拖拽前的快照推入撤销栈 ──
    if drag_stopped {
        state.gizmo_active = None;
        if let Some(snapshot) = state.undo_pending.take() {
            state.push_undo_snapshot(snapshot);
        }
    }
    
    // ── 返回最终 ctx 供调用方绘制 ──
    // ctx 是函数开头建的快照，本帧里 hovered / active 都被改过（刚悬停、刚开始
    // 或刚结束拖拽），交出去之前同步回最新值，避免比实际状态晚一帧。
    ctx.hovered = state.gizmo_hovered;
    ctx.active = state.gizmo_active.clone();
    Some(ctx)
}

// ============================================================================
// 相机控制相关
// ============================================================================

/// 处理飞行模式相机控制
///
/// # 两个手感上的细节（都踩过）
///
/// 1. **转视角不能每帧都要求"指针压在视口上"。**
///    飞行态是在"右键按下且指针在视口里"那一刻开启的；开启之后若还逐帧用
///    `response.hovered()` 当闸门，用户手一抖把光标带出视口（划过 Hierarchy 或
///    拖到屏幕边上），旋转就会**卡住**，移回来又**跳一下** —— 连续快速转视角时
///    表现就是"抽搐"。飞行态一旦开着就一路转到松手，才和商业引擎一致。
/// 2. **位移用 `stable_dt`（平滑过的真实帧间隔），不用 `predicted_dt`。**
///    `predicted_dt` 是"下一帧大概要多久"的预测，预测不准时同一段按键时长会走出
///    不同的距离，速度跟着一冲一顿 —— 平移和旋转同时进行时最容易看出来。
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

    // 这里**不再**手动 `request_repaint()`：飞行时指针是按着的，`frame_policy`
    // 会因为 `interacting` 持续排帧（见 `frame_policy` 模块头）。把"什么时候出帧"
    // 集中到一处决策，免得散落各处的 `request_repaint` 互相打架、也免得漏掉某一处
    // （漏掉的那一处就表现为"这个操作画面不跟手"）。
    let dt = ui.input(|i| i.stable_dt);
    
    // 不再要求悬停：见上面第 1 条。
    let delta = ui.input(|i| i.pointer.delta());
    state.editor_camera.rotate(delta.x, delta.y);
    
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

/// 视口里的 HUD 文字：画一块半透明暗底再写字。
///
/// 早先只描 1px 黑影，结果在亮天空上白字几乎看不见（示例场景的天顶接近纯白）。
/// 折中方案是"暗色药丸底 + 亮字"：无论背后是天空、地形还是暗色模型都读得清，
/// 观感也接近 UE 视口的 stats 叠加层。
fn hud_text(
    painter: &egui::Painter,
    pos: egui::Pos2,
    align: egui::Align2,
    text: impl ToString,
    font: egui::FontId,
    color: egui::Color32,
) {
    let galley = painter.layout_no_wrap(text.to_string(), font, color);
    let rect = align.anchor_size(pos, galley.size());
    let bg = rect.expand2(egui::vec2(6.0, 3.0));
    // 药丸底的不透明度要压得住**明亮天空**：0.38 的黑叠在 200 级灰的天空上
    // 还剩 124 级 —— 白字压上去对比度只有 2:1，看着像糊了一层雾；0.5 也不够
    // （背景最亮时接近纯白，叠完是 128 级中灰）。0.69 才能保证任何背景下
    // 底色都落在 80 级以下，白字对比度 5:1 以上。
    painter.rect_filled(bg, 3.0, egui::Color32::from_black_alpha(176));
    painter.rect_stroke(
        bg,
        3.0,
        egui::Stroke::new(1.0_f32, egui::Color32::from_black_alpha(72)),
    );
    painter.galley(rect.min, galley, color);
}

/// 绘制缩放速度指示器
fn draw_zoom_indicator(state: &EditorState, painter: &egui::Painter, rect: egui::Rect) {
    if state.zoom_indicator <= 0.0 {
        return;
    }

    hud_text(
        painter,
        egui::pos2(rect.right() - 10.0, rect.bottom() - 26.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("Speed {:.1}", state.editor_camera.pan_speed),
        egui::FontId::monospace(10.5),
        egui::Color32::from_rgb(180, 220, 255),
    );
}

/// 视口 HUD：四个角各放一条信息，全部**药丸底小字**。
///
/// 与面板里的标题不同，这里的文字直接压在半透明/3D 画面上，所以：
/// 字号统一 11（比面板正文小半号，避免和 3D 抢注意力）、一律带
/// `from_black_alpha(128)` 的药丸底 + 1px 暗边、颜色只用「亮 / 暗」两档。
/// 旧实现有一条独立的顶部栏，白白吃掉 22px 视口高度，
/// 而它显示的两条信息（场景名、相机位置）用叠加层表达更省地方也更像 UE。
fn draw_viewport_hud(state: &EditorState, painter: &egui::Painter, rect: egui::Rect, ui: &egui::Ui) {
    use crate::panels::theme::colors;

    // 左上：场景名
    //
    // 这里**不用粗体族**：视口 HUD 可能在没有 `install_fonts` 的裸 egui 上下文里
    // 被画出来（scene_view 的交互测试就是这样），而 `FontFamily::Name` 指向未注册
    // 的族会让 epaint 直接 panic。药丸底 + 亮色已经够醒目，不需要再加字重。
    hud_text(
        painter,
        rect.left_top() + egui::vec2(10.0, 8.0),
        egui::Align2::LEFT_TOP,
        &state.scene.name,
        egui::FontId::new(11.5, egui::FontFamily::Proportional),
        colors::TEXT_BRIGHT,
    );

    // 右上：相机位置（等宽数字，读数不抖）
    //
    // **必须带前缀**：三个裸数字（"5.0 5.0 5.0"）在画面上没有任何含义，
    // 用户只能猜它是坐标、网格间距还是缩放。`Cam` 三个字母就够定语义，
    // 又不至于像 "Camera Position" 那样把药丸拉得比场景名还长。
    let eye = state.editor_camera.eye();
    hud_text(
        painter,
        rect.right_top() + egui::vec2(-10.0, 8.0),
        egui::Align2::RIGHT_TOP,
        format!("Cam  {:.1}  {:.1}  {:.1}", eye.x, eye.y, eye.z),
        egui::FontId::monospace(10.5),
        // 药丸底已经压到 72 级：`TEXT_SECONDARY`（128）压上去只有 2.3:1，
        // 缩到 100% 缩放看不清。HUD 的"次要"只是相对场景名而言，绝对对比度
        // 得过关 —— 用 `TEXT_PRIMARY`（191）刚好 4.9:1。
        colors::TEXT_PRIMARY,
    );

    // 左下：当前工具 + 帧率
    let tool_label = match state.tool_mode {
        ToolMode::Translate => "Translate",
        ToolMode::Rotate => "Rotate",
        ToolMode::Scale => "Scale",
        ToolMode::Rect => "Rect",
    };
    let fps = ui.input(|i| i.stable_dt).recip().round();
    hud_text(
        painter,
        egui::pos2(rect.left() + 10.0, rect.bottom() - 7.0),
        egui::Align2::LEFT_BOTTOM,
        format!("{tool_label}   {fps:.0} fps"),
        egui::FontId::new(10.5, egui::FontFamily::Proportional),
        colors::TEXT_PRIMARY,
    );

    // 右下：本帧绘制读数（draw call / 实例 / 被剔除的实例）。
    //
    // 剔除是**保守**的、画面与不剔除逐位一致，所以"它到底有没有在干活"
    // 只能靠计数看。把读数放在视口里而不是只给日志：相机一转这个数就变，
    // 用户（和接手的开发者）立刻知道剔除生效了，不用去翻基准。
    //
    // 与缩放提示共用右下角：它在 `bottom - 26`，这里在 `bottom - 7`，
    // 同时出现时上下叠放，不会互相压字。
    if let Some(stats) = state.scene_renderer.as_ref().map(|r| r.frame_stats()) {
        hud_text(
            painter,
            egui::pos2(rect.right() - 10.0, rect.bottom() - 7.0),
            egui::Align2::RIGHT_BOTTOM,
            format!(
                "Draws {}   Inst {}   Cull {}",
                stats.draw_calls(),
                stats.opaque_instances + stats.blended_draws,
                stats.culled_instances
            ),
            egui::FontId::monospace(10.5),
            colors::TEXT_PRIMARY,
        );
    }
}

// ============================================================================
// 主入口函数
// ============================================================================

/// 在 `Ui` 内渲染场景视图内容。
pub fn show_content(state: &mut EditorState, ui: &mut egui::Ui) {
    // 视口区域（HUD 是叠加层，不占布局高度）
    let viewport = setup_viewport(ui, state);
    // painter 内部只是一堆 Arc，克隆很便宜；后面要跨越对 `ui` 的多次借用。
    let painter = ui.painter().clone();
    // ── 帧序：这里是整个视口手感的命门 ──
    //
    // 顺序必须是 **先处理本帧输入 → 再按最新状态重画离屏 3D → 最后画叠加层**。
    //
    // 旧顺序是"先重画、后处理输入"，代价是相机与拖拽都白白慢一帧 —— 这一帧画出来的
    // 是上一帧的相机与场景，于是拖物体、转视角时画面永远差一帧，手感就是"黏"。
    // 而叠加层（gizmo / 调试线 / HUD）必须画在 3D 之上，只能排在离屏渲染之后；
    // 这也正是 gizmo 的"更新状态"与"绘制"被拆成两步的原因。

    // 1) 飞行相机：先把本帧的旋转 / 平移落到相机上。
    handle_fly_camera(state, ui, &viewport.response);

    // 2) 用**最新**相机算视图投影矩阵，后面的交互与渲染共用这一份。
    let (vp_w, vp_h) = viewport.pixel_size;
    let view_proj = build_view_projection(state, vp_w, vp_h);

    // 3) 鼠标信号 → gizmo 拖拽 / 拾取 / 工具快捷键（只改状态，不落笔）。
    let pointer = PointerInput::read(&viewport.response, ui, state.is_editing());
    let gizmo_ctx = handle_gizmo_interaction(state, view_proj, &viewport, &pointer);

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

    // 工具切换快捷键（有选中实体且未进入飞行模式时生效）
    // 飞行模式下 W/A/S/D/Q/E 是相机移动键，必须让位，否则按 W 会同时平移相机和切工具。
    // 注意 `fly_state` 已在本帧第 1 步更新过，这里读到的才是**当前**帧的状态。
    if state.is_editing() && !state.fly_state && state.selected_entity.is_some() {
        if ui.input(|i| i.key_pressed(egui::Key::W)) {
            state.tool_mode = ToolMode::Translate;
        } else if ui.input(|i| i.key_pressed(egui::Key::E)) {
            state.tool_mode = ToolMode::Rotate;
        } else if ui.input(|i| i.key_pressed(egui::Key::R)) {
            state.tool_mode = ToolMode::Scale;
        }
    }

    // 4) 现在才重画离屏 3D —— 用的就是本帧最新的相机与场景，同帧上屏。
    let tex_id = render_scene(state, vp_w, vp_h, &view_proj);
    draw_render_texture(&painter, viewport.rect, tex_id);

    // 5) 叠加层：画在 3D 之上。
    if let Some(ctx) = gizmo_ctx {
        gizmo::draw_gizmo(&painter, &ctx, state.tool_mode);
    }
    draw_physics_debug_lines(state, &painter, &view_proj, viewport.rect);
    draw_zoom_indicator(state, &painter, viewport.rect);
    draw_viewport_hud(state, &painter, viewport.rect, ui);

    // 6) 资源拖放（Assets 面板 → 视口）。
    //
    // 落点用"鼠标射线 × y=0 地面平面"：这一版不做表面吸附（命中实体 AABB 再取
    // 上表面），因为项目约定地面就在 y=0 附近，先保证"拖进来就落在看得见的地方"；
    // 射线朝上 / 平行于地面时退回相机前方 5 米（`placement_point_in_front`）。
    let asset_drop = compute_asset_drop(state, ui, &viewport, &view_proj);
    if let Some(drop) = &asset_drop {
        draw_asset_drop_hint(&painter, &viewport, &view_proj, state, drop);
    }
    // 松手当帧放置。`asset_drag` 本身由 `main.rs` 帧末统一清空 ——
    // 面板可能被拖成非活动页签而不绘制，清理放在这里会漏。
    if ui.input(|i| i.pointer.any_released()) {
        if let Some(drop) = asset_drop {
            if xgame_core::AssetKind::from_path(&drop.path)
                == Some(xgame_core::AssetKind::Material)
            {
                // 材质不是"生成实体"的资产：它要贴到已有实体上。优先用鼠标
                // 下面的实体，其次退回当前选中 —— 拖到空白处时仍有一个明确目标。
                let target = pick_entity(
                    state,
                    drop.cursor,
                    &view_proj,
                    viewport.rect.min.to_vec2(),
                )
                .or(state.selected_entity);
                match target {
                    Some(index) => {
                        state.assign_material(index, &drop.path);
                    }
                    None => state.log_warning("把材质拖到实体上，或先选中一个实体"),
                }
            } else {
                state.spawn_asset_at(&drop.path, drop.point);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 资源拖放
// ---------------------------------------------------------------------------

/// 一次"拖资产进视口"的落点（只在指针位于视口内时存在）。
struct AssetDrop {
    path: String,
    point: Vec3,
    /// 指针的屏幕位置（画提示用）。
    cursor: egui::Pos2,
}

/// 射线与 y=0 平面求交。射线朝上或无交点时返回 `None`。
fn ray_ground_point(origin: Vec3, dir: Vec3) -> Option<Vec3> {
    if dir.y.abs() < 1e-5 {
        return None;
    }
    let t = -origin.y / dir.y;
    (t > 0.0).then(|| origin + dir * t)
}

fn compute_asset_drop(
    state: &EditorState,
    ui: &egui::Ui,
    viewport: &ViewportInfo,
    view_proj: &Mat4,
) -> Option<AssetDrop> {
    let path = state.asset_drag.clone()?;
    // `interact_pos` 在拖拽期间比 `hover_pos` 可靠：指针离开控件后
    // hover 会变成 None，而拖拽还在继续。
    let cursor = ui.input(|i| i.pointer.interact_pos())?;
    if !viewport.rect.contains(cursor) {
        return None;
    }
    let screen_offset = viewport.rect.min.to_vec2();
    let (origin, dir) =
        gizmo::screen_to_ray(cursor, view_proj, state.viewport_logical_size, screen_offset);
    let point = ray_ground_point(origin, dir).unwrap_or_else(|| state.placement_point_in_front());
    Some(AssetDrop {
        path,
        point,
        cursor,
    })
}

/// 拖拽中的落点提示：世界坐标处画一个十字与脚环，指针旁浮出资产名。
fn draw_asset_drop_hint(
    painter: &egui::Painter,
    viewport: &ViewportInfo,
    view_proj: &Mat4,
    state: &EditorState,
    drop: &AssetDrop,
) {
    use egui::{Align2, FontId, Pos2};

    let screen = project_to_screen(
        drop.point,
        view_proj,
        state.viewport_logical_size,
        viewport.rect.min.to_vec2(),
    );
    if !viewport.rect.contains(screen) {
        return;
    }
    let arm = 9.0;
    let stroke = egui::Stroke::new(1.5_f32, theme::colors::ACCENT_LIGHT);
    painter.line_segment(
        [
            Pos2::new(screen.x - arm, screen.y),
            Pos2::new(screen.x + arm, screen.y),
        ],
        stroke,
    );
    painter.line_segment(
        [
            Pos2::new(screen.x, screen.y - arm),
            Pos2::new(screen.x, screen.y + arm),
        ],
        stroke,
    );
    painter.circle_stroke(screen, 5.0_f32, stroke);
    let name = asset_display_name(&drop.path);
    let label = if xgame_core::AssetKind::from_path(&drop.path)
        == Some(xgame_core::AssetKind::Material)
    {
        format!("指派材质 {name}")
    } else {
        name
    };
    painter.text(
        drop.cursor + egui::vec2(14.0, 10.0),
        Align2::LEFT_TOP,
        label,
        FontId::proportional(12.0),
        theme::colors::TEXT_BRIGHT,
    );
}

/// 路径 → 显示名（`Resources/Models/Box.glb` → `Box`）。
fn asset_display_name(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.split('.').next().unwrap_or(file).to_string()
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
    /// 场景里**第一个实体**的世界中心投到屏幕上的位置 —— "点它就能选中它"的那个点。
    ///
    /// 别拿"视口正中"当替身：实体不一定正好落在正中，而拾取是按**真实几何范围**
    /// （世界 AABB）判定的，点偏一点就是真的没打中。老测试用视口正中，是靠旧实现
    /// 那个半径 0.866 的包围球侥幸蒙过的 —— 球比单位盒大得多，才把它兜住了。
    fn entity_screen_pos(state: &EditorState, screen: egui::Rect) -> egui::Pos2 {
        let vp = state.viewport_logical_size;
        let rect = viewport_rect(screen, vp);
        let (w, h) = state.viewport_pixel_size;
        let view_proj = build_view_projection(state, w, h);
        let center = state.scene.world_transform(0).position;
        gizmo::project_to_screen(center, &view_proj, vp, rect.min.to_vec2())
    }

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

        // 光标要落到**立方体自己的投影位置**上，而不是"视口正中"这个替身。
        let center = entity_screen_pos(&state, screen);
        assert!(viewport_rect(screen, vp).contains(center), "立方体应当投影在视口内: {center:?}");

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
        let center = entity_screen_pos(&state, screen);

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

        let at = entity_screen_pos(&state, screen);
        select_first_entity(&ctx, &mut state, screen, at);
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

    /// 在 X 轴上拖拽一次，返回物体的 X 位移。    ///
    /// `step_px` 是鼠标的屏幕 X 增量序列（相对抓取点）。开头会先走一个小位移
    /// 跨过 egui 的拖拽阈值 —— 那一帧只记录参考点、不产生位移，所以不计入。
    fn drag_x_by(step_px: &[f32]) -> f32 {
        let (mut state, base) = test_state("gizmo_fps");
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));

        frame(&ctx, &mut state, vec![], screen);
        let vp = state.viewport_logical_size;
        let rect = viewport_rect(screen, vp);
        let at = entity_screen_pos(&state, screen);
        select_first_entity(&ctx, &mut state, screen, at);
        assert_eq!(state.selected_entity, Some(0));

        let (w, h) = state.viewport_pixel_size;
        let view_proj = build_view_projection(&state, w, h);
        let entity_pos = state.scene.world_transform(0).position;
        let gz = gizmo::gizmo_world_size(state.editor_camera.eye(), entity_pos);
        let on_axis = gizmo::project_to_screen(
            entity_pos + Vec3::X * gz * 0.7,
            &view_proj,
            vp,
            rect.min.to_vec2(),
        );

        frame(&ctx, &mut state, vec![egui::Event::PointerMoved(on_axis)], screen);
        frame(&ctx, &mut state, vec![button(on_axis, true)], screen);

        // 跨过拖拽阈值的这一步：只记录参考点。
        let mut cursor = on_axis + egui::vec2(8.0, 0.0);
        frame(&ctx, &mut state, vec![egui::Event::PointerMoved(cursor)], screen);
        assert!(state.gizmo_active.is_some(), "跨过阈值后应当进入拖拽态");
        let at_drag_start = state.scene.entities[0].transform.position;

        for dx in step_px {
            cursor += egui::vec2(*dx, 0.0);
            frame(&ctx, &mut state, vec![egui::Event::PointerMoved(cursor)], screen);
        }
        frame(&ctx, &mut state, vec![button(cursor, false)], screen);

        let after = state.scene.entities[0].transform.position;
        let _ = std::fs::remove_dir_all(&base);
        after.x - at_drag_start.x
    }

    /// **端到端回归（帧率无关性）**：同一段鼠标位移，拆成 1 大步和 6 小步必须落到
    /// **同一个地方**。
    ///
    /// 这是"像商业引擎一样"的基本承诺 —— 拖拽结果只由"鼠标现在在哪"决定，不该
    /// 依赖它被采样成了多少帧。而轴拖拽的正反馈（`Δ_k = A_k − Δ_{k-1}`）恰恰破坏
    /// 这一点：参考线跟着物体跑，多帧的会少走（稳态只剩一半）且中途来回跳。
    ///
    /// 上面那条单测用的是固定 ctx、这条端到端旧版只断言"位置变了" —— 两个盲区
    /// 合起来正好放过了它。
    #[test]
    fn gizmo_drag_is_frame_rate_independent() {
        let one = drag_x_by(&[120.0]);
        let six = drag_x_by(&[20.0, 20.0, 20.0, 20.0, 20.0, 20.0]);

        assert!(one > 0.05, "向右拖 X 轴应当把物体挪到 +X，实得 {one}");
        assert!(
            (one - six).abs() < one.abs() * 0.05 + 1e-3,
            "1 大步位移 {one} 与 6 小步 {six} 必须一致；差距大说明结果依赖帧数（正反馈）"
        );
    }

    /// **回归（帧序）**：同一帧里，离屏渲染必须发生在输入处理**之后**。
    ///
    /// 旧顺序是"先重画、后处理输入" —— 这一帧画出来的是**上一帧**的相机与场景，
    /// 所以拖物体、转视角都永远差一帧，手感发"黏"。这条不变量在无头环境里从渲染
    /// 结果上看不出来，于是把"渲染那一刻用的 view_proj"记进
    /// `EditorState::last_render_view_proj`，断言它等于**本帧输入处理完之后**的相机矩阵。
    #[test]
    fn the_frame_is_rendered_after_this_frames_camera_input() {
        let (mut state, base) = test_state("frame_order");
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));

        frame(&ctx, &mut state, vec![], screen);
        let vp = state.viewport_logical_size;
        let rect = viewport_rect(screen, vp);
        let p0 = rect.center();

        // 右键在视口里按下 → 开启飞行态。
        frame(
            &ctx,
            &mut state,
            vec![
                egui::Event::PointerMoved(p0),
                egui::Event::PointerButton {
                    pos: p0,
                    button: egui::PointerButton::Secondary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            screen,
        );
        assert!(state.fly_state, "右键在视口里按下应当进入飞行态");

        // 下一帧把指针横着挪一段：这一帧的相机应当**立刻**转起来。
        let yaw_before = state.editor_camera.yaw;
        state.last_render_view_proj = None;
        let p1 = p0 + egui::vec2(60.0, 0.0);
        frame(&ctx, &mut state, vec![egui::Event::PointerMoved(p1)], screen);
        assert_ne!(state.editor_camera.yaw, yaw_before, "这一帧应当已经转过视角");

        // 关键断言：渲染用的矩阵必须是**转过之后**那台相机算出来的。
        let (w, h) = state.viewport_pixel_size;
        let expected = build_view_projection(&state, w, h);
        let got = state.last_render_view_proj.expect("这一帧应当渲染过离屏场景");
        let (a, b) = (got.to_cols_array(), expected.to_cols_array());
        assert!(
            a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-6),
            "渲染用的是上一帧的相机 —— 帧序反了（先重画、后处理输入），会白白慢一帧"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// **端到端回归（拾取）**：拾取必须按**真实几何范围**判定，不能按包围球。
    ///
    /// # 旧实现的病灶
    ///
    /// 它给每个实体造一个包围球，半径取 `scale.length() * 0.5`。对**被拉伸过**的
    /// 物体这是灾难性的高估：一块 `16 × 0.5 × 16` 的地面半径会算成 11.3，而相机
    /// 离场景中心只有约 8.7 —— **相机直接落在球内部**，于是任何射线都命中它。
    /// 后果是"点天空也选中地面"，也就是**取消选中根本做不到**；真机表现就是
    /// "想选的选不中、想取消的取消不掉"。
    ///
    /// 现在按网格本地包围盒经世界矩阵变换出的 AABB 求交，空处就是空处。
    /// 下面两条断言合起来才区分得开新旧实现 —— 只断言"点得中方块"是不够的
    /// （旧的包围球在那个位置同样命得中，实测过）。
    #[test]
    fn picking_respects_real_geometry_not_bounding_spheres() {
        let (mut state, base) = test_state("pick_ground");
        // 换成"极扁极宽的地面 + 正上方一根细柱子"，与示例项目同款布局。
        state.scene = xgame_core::Scene::new("PickGround");

        let mut ground = xgame_core::Entity::new("Ground");
        ground.model = "builtin://cube".into();
        ground.transform.scale = Vec3::new(16.0, 0.5, 16.0);
        ground.transform.position = Vec3::new(0.0, -0.25, 0.0);
        state.scene.add_entity(ground);

        let mut block = xgame_core::Entity::new("Block");
        block.model = "builtin://cube".into();
        block.transform.scale = Vec3::new(1.0, 3.0, 1.0);
        block.transform.position = Vec3::new(-1.0, 1.5, 0.0);
        state.scene.add_entity(block);

        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        frame(&ctx, &mut state, vec![], screen);

        let vp = state.viewport_logical_size;
        let rect = viewport_rect(screen, vp);
        let (w, h) = state.viewport_pixel_size;
        let view_proj = build_view_projection(&state, w, h);

        // ① 点在柱子上 → 必须选到柱子。
        let block_center = state.scene.world_transform(1).position;
        let on_block = gizmo::project_to_screen(block_center, &view_proj, vp, rect.min.to_vec2());
        assert!(rect.contains(on_block), "柱子的屏幕位置应当落在视口里: {on_block:?}");
        select_first_entity(&ctx, &mut state, screen, on_block);
        assert_eq!(state.selected_entity, Some(1), "点在柱子上必须选中柱子");

        // ② 点天空（视口左上角，那里只有背景）→ 必须取消选中。
        let sky = egui::pos2(rect.min.x + 16.0, rect.min.y + 16.0);
        select_first_entity(&ctx, &mut state, screen, sky);
        assert_eq!(
            state.selected_entity, None,
            "点空处必须取消选中；仍选中地面说明拾取被那块大包围球吞了"
        );

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
        let rect = viewport_rect(screen, vp);

        let at = entity_screen_pos(&state, screen);
        select_first_entity(&ctx, &mut state, screen, at);
        assert_eq!(state.selected_entity, Some(0), "工具快捷键测试要先选中一个实体");

        // 非飞行态：W 切到 Translate。
        state.tool_mode = ToolMode::Rotate;
        frame(&ctx, &mut state, vec![key(egui::Key::W)], screen);
        assert_eq!(state.tool_mode, ToolMode::Translate, "非飞行态 W 应当切工具");

        // 飞行态：W 只用来推相机，不该动工具。
        //
        // `fly_state` 是**每帧由"右键是否按住"推导**出来的，不是可以随手写死的外部
        // 状态（`handle_fly_camera` 会在每帧开头重新判定），所以这里必须真的造出
        // "右键按住"这件事 —— 否则测的就不是真实路径。
        let p = rect.center();
        frame(
            &ctx,
            &mut state,
            vec![
                egui::Event::PointerMoved(p),
                egui::Event::PointerButton {
                    pos: p,
                    button: egui::PointerButton::Secondary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            screen,
        );
        assert!(state.fly_state, "按住右键应当进入飞行态");

        state.tool_mode = ToolMode::Rotate;
        frame(&ctx, &mut state, vec![key(egui::Key::W)], screen);
        assert_eq!(
            state.tool_mode,
            ToolMode::Rotate,
            "飞行态按 W 不该把工具切走（否则飞的同时工具被改掉）"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
/// 资源拖放的落点计算（纯函数，独立成模块便于单测）。
#[cfg(test)]
mod drop_tests {
    use super::*;

    /// 向下的射线打中 y=0 平面；朝上 / 平行都不算命中（退回相机前方）。
    #[test]
    fn ray_ground_point_rules() {
        let hit = ray_ground_point(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, -1.0, 0.0))
            .expect("垂直向下应命中");
        assert!((hit - Vec3::ZERO).length() < 1e-6);

        let slant = ray_ground_point(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.5, -1.0, 0.0).normalize())
            .expect("斜向下应命中");
        assert!((slant.y).abs() < 1e-6, "命中点必须在地面上");
        assert!(slant.x > 0.0, "应落在射线方向一侧");

        assert!(
            ray_ground_point(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 1.0, 0.0)).is_none(),
            "朝上不该命中"
        );
        assert!(
            ray_ground_point(Vec3::new(0.0, 2.0, 0.0), Vec3::new(1.0, 0.0, 0.0)).is_none(),
            "平行于地面不该命中"
        );
    }

    /// 显示名取第一个点之前（`Box.glb` → `Box`，`A.prefab.ron` → `A`）。
    #[test]
    fn asset_display_name_strips_suffixes() {
        assert_eq!(asset_display_name("Resources/Box.glb"), "Box");
        assert_eq!(asset_display_name("Prefabs/A.prefab.ron"), "A");
        assert_eq!(asset_display_name("Box.glb"), "Box");
    }
}
