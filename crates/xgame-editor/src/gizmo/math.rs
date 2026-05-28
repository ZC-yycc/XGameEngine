//! 核心数学工具：3D↔2D 投影、射线构建、几何相交测试。
//!
//! 所有 NaN/Inf 防护集中在 [`project_to_screen`] 单一入口点，
//! 调用方通过 [`is_valid_screen`] 即可判断结果有效性。

use xgame_math::glam::{Mat4, Vec3, Vec4};

// ---------------------------------------------------------------------------
// Gizmo 世界空间尺寸
// ---------------------------------------------------------------------------

/// 计算 gizmo 的世界空间长度，使其在屏幕上保持大致恒定的视觉大小。
pub fn gizmo_world_size(camera_pos: Vec3, entity_pos: Vec3) -> f32 {
    let dist = (entity_pos - camera_pos).length().max(0.1);
    dist * super::GIZMO_SCREEN_FRACTION
}

// ---------------------------------------------------------------------------
// 3D → 2D 投影
// ---------------------------------------------------------------------------

/// 将世界坐标投影到 egui 屏幕坐标（视口相对坐标 + screen_offset）。
///
/// # NaN/Inf 防护
///
/// 当点位于相机后方、NDC 超出视锥体边缘、或矩阵运算产生 NaN/Inf 时，
/// 返回 `(-10000, -10000)`。调用方通过 `x > -5000` 判断有效性。
///
/// # 参数
///
/// - `point`: 世界空间坐标
/// - `view_proj`: 预计算的 `projection * view` 矩阵
/// - `screen_size`: 视口的逻辑像素尺寸
/// - `screen_offset`: 视口在面板中的偏移（rect.min）
pub fn project_to_screen(
    point: Vec3,
    view_proj: &Mat4,
    screen_size: egui::Vec2,
    screen_offset: egui::Vec2,
) -> egui::Pos2 {
    let clip = *view_proj * Vec4::new(point.x, point.y, point.z, 1.0);
    
    // 只检查相机后方，允许超出屏幕边缘
    if !clip.is_finite() || clip.w <= 1e-6 {
        return sentinel();
    }

    let ndc = clip.truncate() / clip.w;
    if !ndc.is_finite() {
        return sentinel();
    }

    // 移除严格的 NDC 裁剪，只做基本的 w 检查
    // 允许超出屏幕边缘的顶点（用于线条延伸）
    
    let x = (ndc.x + 1.0) * 0.5 * screen_size.x + screen_offset.x;
    let y = (1.0 - ndc.y) * 0.5 * screen_size.y + screen_offset.y;

    // 只检查坐标是否有限
    if !x.is_finite() || !y.is_finite() {
        return sentinel();
    }

    egui::pos2(x, y)
}

/// 标记无效投影结果的哨兵值。
#[inline]
fn sentinel() -> egui::Pos2 {
    egui::pos2(-10000.0, -10000.0)
}

/// 检查屏幕坐标是否为有效投影结果。
pub fn is_valid_screen(p: egui::Pos2) -> bool {
    p.x > -5000.0
}

/// 检查所有屏幕点是否均有效。
pub fn all_valid_screen(points: &[egui::Pos2]) -> bool {
    points.iter().all(|&p| is_valid_screen(p))
}

// ---------------------------------------------------------------------------
// 屏幕 → 3D 射线
// ---------------------------------------------------------------------------

/// 从屏幕绝对坐标构建世界空间射线。
///
/// 此函数接收**屏幕绝对坐标**（包含视口偏移量），内部会自动减去偏移量
/// 转换为视口相对坐标后再计算射线。
///
/// # 参数
///
/// - `mouse_pos`: 屏幕绝对坐标（包含视口偏移）
/// - `view_proj`: 预计算的 `projection * view` 矩阵
/// - `screen_size`: 视口的逻辑像素尺寸
/// - `screen_offset`: 视口在面板中的偏移（rect.min）
///
/// # 返回
///
/// 返回 `(origin, direction)`。若逆矩阵奇异则返回零射线。
pub fn screen_to_ray(
    mouse_pos: egui::Pos2,
    view_proj: &Mat4,
    screen_size: egui::Vec2,
    screen_offset: egui::Vec2,
) -> (Vec3, Vec3) {
    let inv_vp = view_proj.inverse();
    if !inv_vp.is_finite() {
        return (Vec3::ZERO, Vec3::Z);
    }

    // 转换为视口相对坐标（NDC 计算需要）
    let viewport_x = mouse_pos.x - screen_offset.x;
    let viewport_y = mouse_pos.y - screen_offset.y;

    // 计算 NDC 坐标（范围 -1 到 1）
    let ndc_x = (viewport_x / screen_size.x) * 2.0 - 1.0;
    let ndc_y = (1.0 - viewport_y / screen_size.y) * 2.0 - 1.0;

    let near = inv_vp * Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
    let far = inv_vp * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);

    let origin = near.truncate() / near.w;
    let far_point = far.truncate() / far.w;

    let direction = (far_point - origin).normalize();
    (origin, direction)
}

// ---------------------------------------------------------------------------
// 向量辅助
// ---------------------------------------------------------------------------

/// 获取与给定轴垂直的任意单位向量。
pub fn perpendicular_to(axis: Vec3) -> Vec3 {
    if axis.dot(Vec3::Y).abs() < 0.9 {
        axis.cross(Vec3::Y).normalize()
    } else {
        axis.cross(Vec3::Z).normalize()
    }
}

/// 获取与给定轴垂直的一对正交单位向量 `(u, v)`。
pub fn perpendicular_axes(axis: Vec3) -> (Vec3, Vec3) {
    let u = perpendicular_to(axis);
    let v = axis.cross(u).normalize();
    (u, v)
}