//! Gizmo 绘制模块——将 3D 操作柄形状投影后渲染为 egui 2D 图元。
//!
//! 所有绘制通过 `egui::Painter` 进行，绘制目标为视口之上的叠加层。

use xgame_math::glam::Vec3;

use super::{
    GizmoCtx,
    axis_part, axis_width, ring_part, part_axis,
    part_color, is_highlighted,
    AXIS_DIRS, CUBE_SIZE, PLANE_FILL, ARROW_HEAD_FRACTION
};
use super::math::{all_valid_screen, is_valid_screen, project_to_screen};
use super::shapes::{TranslateShape, RotateShape, ScaleShape};
use crate::state::{GizmoPart, ToolMode};

// ---------------------------------------------------------------------------
// 公共绘制入口
// ---------------------------------------------------------------------------

/// 绘制当前工具模式下的 gizmo。
///
/// 只收「当帧算好的 ctx + 工具模式」：投影矩阵、视口尺寸、实体世界位置、gizmo 大小
/// 这些输入本来就都在 [`GizmoCtx`] 里，早先从 ctx 里拆成 9 个参数传进来、进门第一件事
/// 又是把它们原样装回一个 ctx —— 纯属来回倒手，还留了个"参数和 ctx 不同步"的口子。
pub fn draw_gizmo(painter: &egui::Painter, ctx: &GizmoCtx, tool_mode: ToolMode) {
    match tool_mode {
        ToolMode::Translate => draw_translate(painter, ctx),
        ToolMode::Rotate => draw_rotate(painter, ctx),
        ToolMode::Scale => draw_scale(painter, ctx),
        ToolMode::Rect => {}
    }
}

// ---------------------------------------------------------------------------
// 低层绘制图元
// ---------------------------------------------------------------------------

/// 最小面积阈值：小于此值视为退化，跳过绘制
const MIN_AREA: f32 = 4.0;

/// 绘制 3D 线段（投影后以 2D 线段渲染）。
fn draw_line_3d(
    painter:                    &egui::Painter,
    from:                       Vec3,
    to:                         Vec3,
    ctx:                        &GizmoCtx,
    color:                      egui::Color32,
    width:                      f32,
) {
    let a = project_to_screen(from, &ctx.view_proj, ctx.screen_size, ctx.screen_offset);
    let b = project_to_screen(to, &ctx.view_proj, ctx.screen_size, ctx.screen_offset);
    if !is_valid_screen(a) || !is_valid_screen(b) {
        return;
    }
    painter.add(egui::Shape::line_segment([a, b], egui::Stroke::new(width, color)));
}

/// 绘制填充凸多边形（3D → 2D 投影），带退化检测。
///
/// 额外检查投影后多边形的包围盒：若超过屏幕尺寸的 1.5 倍，说明顶点
/// 位于屏幕远端边缘且构成巨型三角形，egui 不做视口裁剪，会污染全屏。
fn draw_convex_poly_3d(
    painter:                    &egui::Painter,
    vertices_3d:                &[Vec3],
    ctx:                        &GizmoCtx,
    fill:                       egui::Color32,
    stroke:                     egui::Stroke,
) {
    let points: Vec<egui::Pos2> = vertices_3d
        .iter()
        .map(|v| project_to_screen(*v, &ctx.view_proj, ctx.screen_size, ctx.screen_offset))
        .collect();
    if !all_valid_screen(&points) {
        return;
    }
    
    // 检查多边形是否退化（面积太小或接近线形）
    let area = polygon_area(&points);
    if area < MIN_AREA {
        return;
    }
    
    // 包围盒防护：防止顶点虽然各自"有效"但多边形跨屏
    if polygon_bbox_too_large(&points, ctx.screen_size) {
        return;
    }
    painter.add(egui::Shape::convex_polygon(points, fill, stroke));
}

/// 计算屏幕空间三角形面积
fn triangle_area(a: egui::Pos2, b: egui::Pos2, c: egui::Pos2) -> f32 {
    let ab = b - a;
    let ac = c - a;
    (ab.x * ac.y - ab.y * ac.x).abs() * 0.5
}

/// 三角形面积足够大时才绘制
fn draw_triangle_if_valid(
    painter:                    &egui::Painter,
    a:                          egui::Pos2,
    b:                          egui::Pos2,
    c:                          egui::Pos2,
    fill:                       egui::Color32,
    min_area:                   f32,
) {
    if triangle_area(a, b, c) > min_area {
        painter.add(egui::Shape::convex_polygon(
            vec![a, b, c],
            fill,
            egui::Stroke::NONE,
        ));
    }
}

/// 四边形面积足够大时才绘制（拆成两个三角形判断）
fn draw_quad_if_valid(
    painter:                    &egui::Painter,
    a:                          egui::Pos2,
    b:                          egui::Pos2,
    c:                          egui::Pos2,
    d:                          egui::Pos2,
    fill:                       egui::Color32,
    min_area:                   f32,
) {
    let area = triangle_area(a, b, c) + triangle_area(a, c, d);
    if area > min_area {
        painter.add(egui::Shape::convex_polygon(
            vec![a, b, c, d],
            fill,
            egui::Stroke::NONE,
        ));
    }
}

/// 计算多边形面积（屏幕空间）。
fn polygon_area(points: &[egui::Pos2]) -> f32 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut area = 0.0;
    for i in 0..points.len() {
        let p1 = points[i];
        let p2 = points[(i + 1) % points.len()];
        area += p1.x * p2.y - p2.x * p1.y;
    }
    area.abs() * 0.5
}

/// 检查多边形 2D 包围盒是否过大（> 屏幕尺寸的 1.5 倍）。
fn polygon_bbox_too_large(points: &[egui::Pos2], screen_size: egui::Vec2) -> bool {
    if points.len() < 2 {
        return false;
    }
    let mut min_x = f32::MAX;
    let mut max_x = f32::MIN;
    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;
    for p in points {
        min_x = min_x.min(p.x);
        max_x = max_x.max(p.x);
        min_y = min_y.min(p.y);
        max_y = max_y.max(p.y);
    }
    let w = max_x - min_x;
    let h = max_y - min_y;
    w > screen_size.x * 1.5 || h > screen_size.y * 1.5
}

/// 在屏幕坐标绘制方块（3D 中心点 → 屏幕方块），带退化检测。
fn draw_cube_at(
    painter:                    &egui::Painter,
    center_3d:                  Vec3,
    ctx:                        &GizmoCtx,
    fill:                       egui::Color32,
    stroke_color:               egui::Color32,
) {
    let s_center = project_to_screen(center_3d, &ctx.view_proj, ctx.screen_size, ctx.screen_offset);
    if !is_valid_screen(s_center) {
        return;
    }
    let half = ctx.gizmo_size * CUBE_SIZE * 3.0;
    
    // 检查退化：如果方块在屏幕上太小，跳过填充只画十字线
    if half < 0.5 {
        // 退化时画一个小十字线代替
        let stroke = egui::Stroke::new(1.5_f32, stroke_color);
        painter.add(egui::Shape::line_segment(
            [egui::pos2(s_center.x - 2.0, s_center.y), egui::pos2(s_center.x + 2.0, s_center.y)],
            stroke,
        ));
        painter.add(egui::Shape::line_segment(
            [egui::pos2(s_center.x, s_center.y - 2.0), egui::pos2(s_center.x, s_center.y + 2.0)],
            stroke,
        ));
        return;
    }
    
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(s_center.x - half, s_center.y - half),
            egui::pos2(s_center.x + half, s_center.y - half),
            egui::pos2(s_center.x + half, s_center.y + half),
            egui::pos2(s_center.x - half, s_center.y + half),
        ],
        fill,
        egui::Stroke::new(1.5_f32, stroke_color),
    ));
}

// ---------------------------------------------------------------------------
// Translate 绘制
// ---------------------------------------------------------------------------

fn draw_translate(painter: &egui::Painter, ctx: &GizmoCtx) {
    let shape = TranslateShape::new(ctx.entity_pos, ctx.gizmo_size);

    // 1. 平面操作柄（最底层）
    for (idx, plane) in shape.planes.iter().enumerate() {
        let part = match idx {
            0 => GizmoPart::PlaneXY,
            1 => GizmoPart::PlaneXZ,
            _ => GizmoPart::PlaneYZ,
        };
        let color = part_color(part, ctx.hovered, ctx.active.as_ref());
        let hl = is_highlighted(part, ctx.hovered, ctx.active.as_ref());
        let fill = if hl {
            egui::Color32::from_rgba_premultiplied(color.r(), color.g(), color.b(), 160)
        } else {
            PLANE_FILL
        };
        let sw = if hl { 2.5_f32 } else { 1.5_f32 };
        draw_convex_poly_3d(painter, &plane.corners, ctx, fill, egui::Stroke::new(sw, color));
    }

    // 2. 轴线 + 箭头头部
    for (i, &_axis) in AXIS_DIRS.iter().enumerate() {
        let part = axis_part(i);
        let color = part_color(part, ctx.hovered, ctx.active.as_ref());
        let hl = is_highlighted(part, ctx.hovered, ctx.active.as_ref());
        let lw = axis_width(hl);

        // 轴线
        let (axis_start, axis_end) = shape.axes[i];
        draw_line_3d(painter, axis_start, axis_end, ctx, color, lw);

        // 箭头头部
        draw_arrow_head(painter, ctx, &shape.arrow_heads[i], part, lw);
    }
}

/// 绘制箭头头部，跳过退化面
fn draw_arrow_head(
    painter:                    &egui::Painter,
    ctx:                        &GizmoCtx,
    ah:                         &super::shapes::ArrowHead,
    part:                       GizmoPart,
    lw:                         f32,
) {
    let axis_dir = part_axis(part);
    let half_base = ctx.gizmo_size * ARROW_HEAD_FRACTION * 0.4;
    let back_center = ah.tip - axis_dir * ctx.gizmo_size * ARROW_HEAD_FRACTION * 0.8;

    // 计算底面局部坐标系
    let up = if axis_dir.x.abs() < 0.99 { Vec3::X } else { Vec3::Y };
    let right = axis_dir.cross(up).normalize() * half_base;
    let up = right.cross(axis_dir).normalize() * half_base;

    // 底面四个角
    let back_tl = back_center + up - right;
    let back_tr = back_center + up + right;
    let back_bl = back_center - up - right;
    let back_br = back_center - up + right;

    // 投影所有点到屏幕
    let tip = project_to_screen(ah.tip, &ctx.view_proj, ctx.screen_size, ctx.screen_offset);
    let b_tl = project_to_screen(back_tl, &ctx.view_proj, ctx.screen_size, ctx.screen_offset);
    let b_tr = project_to_screen(back_tr, &ctx.view_proj, ctx.screen_size, ctx.screen_offset);
    let b_bl = project_to_screen(back_bl, &ctx.view_proj, ctx.screen_size, ctx.screen_offset);
    let b_br = project_to_screen(back_br, &ctx.view_proj, ctx.screen_size, ctx.screen_offset);

    // 所有点都必须有效
    if !all_valid_screen(&[tip, b_tl, b_tr, b_bl, b_br]) {
        return;
    }

    let color = part_color(part, ctx.hovered, ctx.active.as_ref());
    let arrow_fill = egui::Color32::from_rgba_premultiplied(color.r(), color.g(), color.b(), 180);
    let stroke = egui::Stroke::new(lw * 0.8, color);

    // 四个侧面（三角形），逐个检查是否退化
    draw_triangle_if_valid(painter, tip, b_tl, b_tr, arrow_fill, MIN_AREA);
    draw_triangle_if_valid(painter, tip, b_tr, b_br, arrow_fill, MIN_AREA);
    draw_triangle_if_valid(painter, tip, b_br, b_bl, arrow_fill, MIN_AREA);
    draw_triangle_if_valid(painter, tip, b_bl, b_tl, arrow_fill, MIN_AREA);

    // 底面（矩形），检查是否退化
    draw_quad_if_valid(painter, b_tl, b_tr, b_br, b_bl, arrow_fill, MIN_AREA);

    // 轮廓线始终绘制（线段不会退化）
    painter.add(egui::Shape::line_segment([tip, b_tl], stroke));
    painter.add(egui::Shape::line_segment([tip, b_tr], stroke));
    painter.add(egui::Shape::line_segment([tip, b_br], stroke));
    painter.add(egui::Shape::line_segment([tip, b_bl], stroke));
    painter.add(egui::Shape::line_segment([b_tl, b_tr], stroke));
    painter.add(egui::Shape::line_segment([b_tr, b_br], stroke));
    painter.add(egui::Shape::line_segment([b_br, b_bl], stroke));
    painter.add(egui::Shape::line_segment([b_bl, b_tl], stroke));
}

// ---------------------------------------------------------------------------
// Rotate 绘制
// ---------------------------------------------------------------------------

fn draw_rotate(painter: &egui::Painter, ctx: &GizmoCtx) {
    let shape = RotateShape::new(ctx.entity_pos, ctx.gizmo_size);

    for (i, ring) in shape.rings.iter().enumerate() {
        let part = ring_part(i);
        let color = part_color(part, ctx.hovered, ctx.active.as_ref());
        let hl = is_highlighted(part, ctx.hovered, ctx.active.as_ref());
        let lw = axis_width(hl);

        // 投影所有顶点
        let pts: Vec<egui::Pos2> = ring
            .iter()
            .map(|p| project_to_screen(*p, &ctx.view_proj, ctx.screen_size, ctx.screen_offset))
            .collect();

        // 绘制线段，跳过退化段
        for w in 0..(pts.len() - 1) {
            if !is_valid_screen(pts[w]) || !is_valid_screen(pts[w + 1]) {
                continue;
            }
            // 检查线段是否退化（长度太小）
            let seg_len = pts[w].distance(pts[w + 1]);
            if seg_len < 0.5 {
                continue;
            }
            painter.add(egui::Shape::line_segment(
                [pts[w], pts[w + 1]],
                egui::Stroke::new(lw, color),
            ));
        }

        // 如果高亮且没有退化，绘制填充环面（半透明填充）
        if hl && pts.len() >= 3 {
            let area = polygon_area(&pts);
            if area > MIN_AREA * 10.0 {
                let fill = egui::Color32::from_rgba_premultiplied(color.r(), color.g(), color.b(), 40);
                // 绘制环面填充 - 通过将 ring 视为多边形带
                // 由于 ring 是环形，需要特殊处理。这里简化为只在面积足够大时绘制。
                // 实际上环形填充比较复杂，先保持线段绘制为主。
                let _ = fill; // 暂时不使用填充，避免视觉混乱
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Scale 绘制
// ---------------------------------------------------------------------------

fn draw_scale(painter: &egui::Painter, ctx: &GizmoCtx) {
    let shape = ScaleShape::new(ctx.entity_pos, ctx.gizmo_size);

    // 1. 均匀缩放方块（中心）
    let center_color = part_color(GizmoPart::Uniform, ctx.hovered, ctx.active.as_ref());
    draw_cube_at(painter, shape.center, ctx, center_color, center_color);

    // 2. 轴线 + 端部方块
    for (i, &_axis) in AXIS_DIRS.iter().enumerate() {
        let part = axis_part(i);
        let color = part_color(part, ctx.hovered, ctx.active.as_ref());
        let hl = is_highlighted(part, ctx.hovered, ctx.active.as_ref());
        let lw = axis_width(hl);

        // 轴线（线段不会退化，直接绘制）
        let (start, end) = shape.axes[i];
        draw_line_3d(painter, start, end, ctx, color, lw);

        // 端部方块（内部有退化检测）
        let fill = egui::Color32::from_rgba_premultiplied(color.r(), color.g(), color.b(), 200);
        draw_cube_at(painter, shape.cube_centers[i], ctx, fill, color);
    }
}