//! Gizmo 拾取检测——通过屏幕空间距离和 3D 射线相交判断鼠标悬停在哪个操作柄上。
//!
//! 策略：
//! - **轴线/箭头**：屏幕空间点到线段的距离
//! - **平面**：3D 射线与平面的相交
//! - **圆环**：屏幕空间点到圆环曲线的距离
//! - **方块（Scale）**：屏幕空间点到方块的 2D 包围盒距离
//!
//! 所有距离阈值以逻辑像素为单位。

use xgame_math::glam::Vec3;

use crate::state::GizmoPart;
use super::{
    GizmoCtx, AXIS_DIRS, CUBE_SIZE,
    PLANE_HANDLE_SIZE, RING_RADIUS, RING_SEGMENTS,
};
use super::math::{all_valid_screen, is_valid_screen, project_to_screen, screen_to_ray};

// ---------------------------------------------------------------------------
// 阈值常量
// ---------------------------------------------------------------------------

/// 轴线的拾取阈值（逻辑像素）。
const AXIS_HIT_RADIUS: f32 = 10.0;

/// 箭头头部的拾取阈值（逻辑像素）。
const ARROW_HEAD_HIT_RADIUS: f32 = 14.0;

/// 平面操作柄的拾取阈值（逻辑像素）——检测点是否在投影四边形内部。
const PLANE_HIT_RADIUS: f32 = 6.0;

/// 旋转圆环的拾取阈值（逻辑像素）。
const RING_HIT_RADIUS: f32 = 8.0;

/// 方块（Scale 端部/均匀）的拾取阈值（逻辑像素）。
const CUBE_HIT_RADIUS: f32 = 10.0;

// ---------------------------------------------------------------------------
// 公共拾取入口
// ---------------------------------------------------------------------------

/// 对所有 gizmo 部件进行拾取检测，返回最近的悬停部件（若无则为 `None`）。
///
/// # 重要说明
///
/// `ctx.screen_offset` 必须正确设置为视口在屏幕上的偏移量（通常是视口矩形的 `min` 点）。
/// 鼠标坐标 `mouse_pos` 应该是相对于**整个窗口**的屏幕坐标，函数内部会自动减去偏移量
/// 转换为视口相对坐标进行拾取检测。
pub fn pick_gizmo(mouse_pos: egui::Pos2, ctx: &GizmoCtx) -> Option<GizmoPart> {
    // 将鼠标坐标转换为视口相对坐标（用于投影比较）
    let viewport_mouse = egui::pos2(
        mouse_pos.x - ctx.screen_offset.x,
        mouse_pos.y - ctx.screen_offset.y,
    );

    // 检查鼠标是否在视口有效范围内
    if viewport_mouse.x < 0.0 || viewport_mouse.x > ctx.screen_size.x
        || viewport_mouse.y < 0.0 || viewport_mouse.y > ctx.screen_size.y {
        return None;
    }

    let mut best: Option<(GizmoPart, f32)> = None;

    // 根据工具模式决定检测哪些部件
    // 实际场景中我们由调用方指定 tool_mode，这里检测全部再由外部过滤。
    // 但为了避免冗余，按部件类型分组检测。

    // --- 轴线（Translate + Scale + Rotate 共用轴方向）---
    for (i, &axis) in AXIS_DIRS.iter().enumerate() {
        let part = super::axis_part(i);
        let dist = pick_axis(part, ctx, axis, viewport_mouse);
        update_best(&mut best, part, dist);
    }

    // --- 箭头头部（仅 Translate）---
    for (i, &axis) in AXIS_DIRS.iter().enumerate() {
        let part = super::axis_part(i);
        let dist = pick_arrow_head(ctx, axis, viewport_mouse);
        update_best(&mut best, part, dist);
    }

    // --- 平面操作柄（仅 Translate）---
    for (idx, plane_axes) in [(Vec3::X, Vec3::Y), (Vec3::X, Vec3::Z), (Vec3::Y, Vec3::Z)].iter().enumerate() {
        let part = match idx {
            0 => GizmoPart::PlaneXY,
            1 => GizmoPart::PlaneXZ,
            _ => GizmoPart::PlaneYZ,
        };
        let dist = pick_plane(ctx, *plane_axes, viewport_mouse);
        update_best(&mut best, part, dist);
    }

    // --- 旋转圆环（仅 Rotate）---
    for (i, &axis) in AXIS_DIRS.iter().enumerate() {
        let part = super::ring_part(i);
        let dist = pick_ring(ctx, axis, viewport_mouse);
        update_best(&mut best, part, dist);
    }

    // --- Scale 方块（端部 + 中心）---
    for (i, &axis) in AXIS_DIRS.iter().enumerate() {
        let part = super::axis_part(i);
        let dist = pick_cube_at(ctx, ctx.entity_pos + axis * ctx.gizmo_size, viewport_mouse);
        update_best(&mut best, part, dist);
    }

    // 均匀缩放方块（中心）
    let uniform_dist = pick_cube_at(ctx, ctx.entity_pos, viewport_mouse);
    update_best(&mut best, GizmoPart::Uniform, uniform_dist);

    best.map(|(part, _)| part)
}

/// 更新最佳候选项：更小的距离胜出。
fn update_best(best: &mut Option<(GizmoPart, f32)>, part: GizmoPart, dist: f32) {
    if dist < 0.0 {
        return; // 未命中
    }
    match *best {
        Some((_, best_dist)) if dist < best_dist => {
            *best = Some((part, dist));
        }
        None => {
            *best = Some((part, dist));
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// 轴心点命中检测：沿轴拖拽时使用的平面投影点
// ---------------------------------------------------------------------------

/// 计算鼠标射线与经过实体位置且垂直于指定轴的平面的交点。
/// 用于确定拖拽轴时鼠标在轴上的投影位置。
///
/// 注意：`mouse_pos` 应该是**屏幕绝对坐标**（包含偏移量），
/// 因为 `screen_to_ray` 需要屏幕坐标来计算射线。
pub fn hit_position_on_axis(
    mouse_pos: egui::Pos2,
    ctx: &GizmoCtx,
    axis: Vec3,
) -> Option<Vec3> {
    // 射线计算需要使用屏幕绝对坐标（包含偏移量）
    let (ray_origin, ray_dir) = screen_to_ray(mouse_pos, &ctx.view_proj, ctx.screen_size, ctx.screen_offset);
    if ray_dir.length_squared() < 1e-10 {
        return None;
    }

    // 平面方程: (p - entity_pos) · axis = 0
    // t = (entity_pos - ray_origin) · axis / (ray_dir · axis)
    let denom = ray_dir.dot(axis);
    if denom.abs() < 1e-8 {
        return None; // 射线与平面平行
    }

    let t = (ctx.entity_pos - ray_origin).dot(axis) / denom;
    if t < 0.0 {
        return None; // 交点在射线后方
    }

    Some(ray_origin + ray_dir * t)
}

/// 计算鼠标射线与指定平面的交点（用于平面拖拽）。
///
/// 注意：`mouse_pos` 应该是**屏幕绝对坐标**（包含偏移量）。
pub fn hit_position_on_plane(
    mouse_pos: egui::Pos2,
    ctx: &GizmoCtx,
    normal: Vec3,
) -> Option<Vec3> {
    let (ray_origin, ray_dir) = screen_to_ray(mouse_pos, &ctx.view_proj, ctx.screen_size, ctx.screen_offset);
    if ray_dir.length_squared() < 1e-10 {
        return None;
    }

    let denom = ray_dir.dot(normal);
    if denom.abs() < 1e-8 {
        return None;
    }

    let t = (ctx.entity_pos - ray_origin).dot(normal) / denom;
    if t < 0.0 {
        return None;
    }

    Some(ray_origin + ray_dir * t)
}

// ---------------------------------------------------------------------------
// 各部件拾取实现
// ---------------------------------------------------------------------------

/// 检测屏幕点是否靠近 3D 线段投影。
/// 
/// 注意：`mouse` 参数应该是**视口相对坐标**（不包含偏移量）。
fn pick_axis(
    _part: GizmoPart,
    ctx: &GizmoCtx,
    axis: Vec3,
    mouse: egui::Pos2,
) -> f32 {
    let start = ctx.entity_pos;
    let end = ctx.entity_pos + axis * ctx.gizmo_size;

    // project_to_screen 返回的是屏幕绝对坐标（包含偏移量）
    let a = project_to_screen(start, &ctx.view_proj, ctx.screen_size, ctx.screen_offset);
    let b = project_to_screen(end, &ctx.view_proj, ctx.screen_size, ctx.screen_offset);
    if !is_valid_screen(a) || !is_valid_screen(b) {
        return -1.0;
    }

    // 将投影点转换为视口相对坐标进行比较
    let a_viewport = egui::pos2(a.x - ctx.screen_offset.x, a.y - ctx.screen_offset.y);
    let b_viewport = egui::pos2(b.x - ctx.screen_offset.x, b.y - ctx.screen_offset.y);

    let dist = point_to_segment_distance(mouse, a_viewport, b_viewport);
    if dist < AXIS_HIT_RADIUS {
        dist
    } else {
        -1.0
    }
}

/// 检测屏幕点是否靠近箭头头部投影（箭头头部抽像为端点）。
///
/// 注意：`mouse` 参数应该是**视口相对坐标**（不包含偏移量）。
fn pick_arrow_head(
    ctx: &GizmoCtx,
    axis: Vec3,
    mouse: egui::Pos2,
) -> f32 {
    let tip = ctx.entity_pos + axis * ctx.gizmo_size;
    let tip_screen = project_to_screen(tip, &ctx.view_proj, ctx.screen_size, ctx.screen_offset);
    if !is_valid_screen(tip_screen) {
        return -1.0;
    }

    let tip_viewport = egui::pos2(tip_screen.x - ctx.screen_offset.x, tip_screen.y - ctx.screen_offset.y);
    let dist = mouse.distance(tip_viewport);
    if dist < ARROW_HEAD_HIT_RADIUS {
        dist
    } else {
        -1.0
    }
}

/// 检测屏幕点是否在平面操作柄的投影四边形内部（或边缘附近）。
///
/// 注意：`mouse` 参数应该是**视口相对坐标**（不包含偏移量）。
fn pick_plane(
    ctx: &GizmoCtx,
    axes: (Vec3, Vec3),
    mouse: egui::Pos2,
) -> f32 {
    let size = ctx.gizmo_size * PLANE_HANDLE_SIZE;
    let corners = [
        ctx.entity_pos,
        ctx.entity_pos + axes.0 * size,
        ctx.entity_pos + axes.0 * size + axes.1 * size,
        ctx.entity_pos + axes.1 * size,
    ];

    let screen_corners: Vec<egui::Pos2> = corners
        .iter()
        .map(|v| project_to_screen(*v, &ctx.view_proj, ctx.screen_size, ctx.screen_offset))
        .collect();

    if !all_valid_screen(&screen_corners) {
        return -1.0;
    }

    // 转换为视口相对坐标
    let viewport_corners: Vec<egui::Pos2> = screen_corners
        .iter()
        .map(|p| egui::pos2(p.x - ctx.screen_offset.x, p.y - ctx.screen_offset.y))
        .collect();

    // 检查点是否在四边形内部
    if point_in_convex_quad(mouse, viewport_corners[0], viewport_corners[1], viewport_corners[2], viewport_corners[3]) {
        // 内部：返回一个较小的距离值以确保优先于轴线
        return PLANE_HIT_RADIUS * 0.5;
    }

    // 检查边缘距离
    let edges = [
        (viewport_corners[0], viewport_corners[1]),
        (viewport_corners[1], viewport_corners[2]),
        (viewport_corners[2], viewport_corners[3]),
        (viewport_corners[3], viewport_corners[0]),
    ];
    let mut min_edge_dist = f32::MAX;
    for (a, b) in &edges {
        let d = point_to_segment_distance(mouse, *a, *b);
        if d < min_edge_dist {
            min_edge_dist = d;
        }
    }

    if min_edge_dist < PLANE_HIT_RADIUS {
        min_edge_dist
    } else {
        -1.0
    }
}

/// 检测屏幕点是否靠近旋转圆环的投影。
///
/// 注意：`mouse` 参数应该是**视口相对坐标**（不包含偏移量）。
fn pick_ring(
    ctx: &GizmoCtx,
    axis: Vec3,
    mouse: egui::Pos2,
) -> f32 {
    let radius = ctx.gizmo_size * RING_RADIUS;
    let (u, v) = super::math::perpendicular_axes(axis);

    // 对圆环上的采样点进行检测（使用与绘制相同的分段数）
    let n = RING_SEGMENTS;
    let mut best_dist = f32::MAX;

    for s in 0..=n {
        let theta = (s as f32 / n as f32) * std::f32::consts::TAU;
        let point = ctx.entity_pos + (u * theta.cos() + v * theta.sin()) * radius;
        let screen_pt = project_to_screen(point, &ctx.view_proj, ctx.screen_size, ctx.screen_offset);
        if !is_valid_screen(screen_pt) {
            continue;
        }

        let viewport_pt = egui::pos2(screen_pt.x - ctx.screen_offset.x, screen_pt.y - ctx.screen_offset.y);
        let d = mouse.distance(viewport_pt);
        if d < best_dist {
            best_dist = d;
        }
    }

    if best_dist < RING_HIT_RADIUS {
        best_dist
    } else {
        -1.0
    }
}

/// 检测屏幕点是否靠近 3D 方块中心的屏幕投影。
///
/// 注意：`mouse` 参数应该是**视口相对坐标**（不包含偏移量）。
fn pick_cube_at(
    ctx: &GizmoCtx,
    center_3d: Vec3,
    mouse: egui::Pos2,
) -> f32 {
    let screen_pt = project_to_screen(center_3d, &ctx.view_proj, ctx.screen_size, ctx.screen_offset);
    if !is_valid_screen(screen_pt) {
        return -1.0;
    }

    let viewport_pt = egui::pos2(screen_pt.x - ctx.screen_offset.x, screen_pt.y - ctx.screen_offset.y);
    let half_screen = ctx.gizmo_size * CUBE_SIZE * 3.0;
    let hit_radius = CUBE_HIT_RADIUS.max(half_screen);

    let dist = mouse.distance(viewport_pt);
    if dist < hit_radius {
        dist
    } else {
        -1.0
    }
}

// ---------------------------------------------------------------------------
// 2D 几何辅助函数
// ---------------------------------------------------------------------------

/// 计算点到线段的最短距离。
fn point_to_segment_distance(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let ap = p - a;
    let t = (ap.x * ab.x + ap.y * ab.y) / (ab.x * ab.x + ab.y * ab.y + 1e-10);
    let t = t.clamp(0.0, 1.0);
    let closest = a + ab * t;
    p.distance(closest)
}

/// 使用叉积符号法判断点是否在凸四边形内部（屏幕空间，顺时针或逆时针均可）。
fn point_in_convex_quad(
    p: egui::Pos2,
    a: egui::Pos2,
    b: egui::Pos2,
    c: egui::Pos2,
    d: egui::Pos2,
) -> bool {
    let sign = |p1: egui::Pos2, p2: egui::Pos2, p3: egui::Pos2| -> f32 {
        (p1.x - p3.x) * (p2.y - p3.y) - (p2.x - p3.x) * (p1.y - p3.y)
    };

    let d1 = sign(p, a, b);
    let d2 = sign(p, b, c);
    let d3 = sign(p, c, d);
    let d4 = sign(p, d, a);

    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0 || d4 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0 || d4 > 0.0;

    !(has_neg && has_pos)
}