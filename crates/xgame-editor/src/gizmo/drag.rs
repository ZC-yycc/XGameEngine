//! Gizmo 拖拽变换计算——根据 GizmoDrag 状态和当前鼠标位置计算实体的新变换值。
//!
//! 通过 `compute_transform` 入口执行，该函数根据工具模式和被拖拽的操作柄类型
//! 选择不同的计算策略：轴拖拽、平面拖拽、旋转拖拽、均匀缩放。

use xgame_math::glam::{Quat, Vec3};
use crate::state::{GizmoDrag, GizmoPart, ToolMode};
use super::GizmoCtx;
use super::pick::{hit_position_on_axis, hit_position_on_plane};

/// 旋转拖拽的灵敏度倍率。
const ROTATE_SENSITIVITY: f32 = 1.0;

/// 均匀缩放灵敏度（屏幕像素到缩放因子的映射）。
const UNIFORM_SCALE_SENSITIVITY: f32 = 0.005;

// ---------------------------------------------------------------------------
// 公共入口
// ---------------------------------------------------------------------------

/// 根据拖拽状态和当前鼠标位置计算实体的新 Transform 值（translation / scale / rotation）。
#[derive(Debug, Clone)]
pub enum TransformDelta {
    /// 位置变化（Translate 工具）。
    Position(Vec3),
    /// 旋转变化（Rotate 工具），四元数表示。
    Rotation(Quat, Vec3), // 第二个 Vec3 用于传递旋转轴（仅在 Rotate 模式下使用）
    /// 缩放变化（Scale 工具）。
    Scale(Vec3),
}

/// 计算拖拽产生的变换变化量。
///
/// 返回 `Some(delta)` 表示变换需要更新，`None` 表示无效状态。
pub fn compute_transform(
    drag:                       &GizmoDrag,
    current_mouse_pos:          egui::Pos2,
    ctx:                        &GizmoCtx,
    tool_mode:                  ToolMode,
) -> Option<TransformDelta> {
    match tool_mode {
        ToolMode::Translate => compute_translate(drag, current_mouse_pos, ctx),
        ToolMode::Rotate => compute_rotate(drag, current_mouse_pos, ctx),
        ToolMode::Scale => compute_scale(drag, current_mouse_pos, ctx),
        ToolMode::Rect => None,
    }
}

// ---------------------------------------------------------------------------
// Translate 计算
// ---------------------------------------------------------------------------

fn compute_translate(
    drag:                       &GizmoDrag,
    mouse_pos:                  egui::Pos2,
    ctx:                        &GizmoCtx,
) -> Option<TransformDelta> {
    match drag.part {
        // ── 轴拖拽 ──
        GizmoPart::AxisX | GizmoPart::AxisY | GizmoPart::AxisZ => {
            let axis = part_to_axis(drag.part);
            let hit = hit_position_on_axis(mouse_pos, ctx, axis)?;

            // 将 hit 点投影到轴上（相对于 entity_pos 的标量偏移）
            let offset = hit - ctx.entity_pos;
            let t = offset.dot(axis);

            // 从 start_value 重新构建起始位置，加上沿轴的 delta
            let start_pos = drag.start_value;
            let new_pos = start_pos + axis * (t - drag.start_hit.dot(axis));

            Some(TransformDelta::Position(new_pos))
        }

        // ── 平面拖拽 ──
        GizmoPart::PlaneXY | GizmoPart::PlaneXZ | GizmoPart::PlaneYZ => {
            let normal = plane_normal(drag.part);
            let hit = hit_position_on_plane(mouse_pos, ctx, normal)?;

            let delta = hit - drag.start_hit;
            let new_pos = drag.start_value + delta;

            Some(TransformDelta::Position(new_pos))
        }

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Rotate 计算
// ---------------------------------------------------------------------------

fn compute_rotate(
    drag: &GizmoDrag,
    mouse_pos: egui::Pos2,
    ctx: &GizmoCtx,
) -> Option<TransformDelta> {
    let axis = match drag.part {
        GizmoPart::RingX => Vec3::X,
        GizmoPart::RingY => Vec3::Y,
        GizmoPart::RingZ => Vec3::Z,
        _ => return None,
    };

    // 获取垂直于轴的两个正交向量（定义旋转平面）
    let (u, v) = super::math::perpendicular_axes(axis);

    // 将鼠标射线投影到旋转平面（经过 entity_pos）
    let hit = hit_position_on_plane(mouse_pos, ctx, axis)?;

    // 计算从 entity_pos 到 hit 的方向向量
    let to_hit = (hit - ctx.entity_pos).normalize();

    // 在旋转平面上分解方向
    let u_comp = to_hit.dot(u);
    let v_comp = to_hit.dot(v);
    let angle = v_comp.atan2(u_comp); // 在旋转平面上的角度

    // 从起始命中点获取起始角度
    let start_dir = (drag.start_hit - ctx.entity_pos).normalize();
    let start_u = start_dir.dot(u);
    let start_v = start_dir.dot(v);
    let start_angle = start_v.atan2(start_u);

    // 计算角度变化量
    let mut delta_angle = angle - start_angle;

    // 处理角度环绕：确保 delta 在 [-PI, PI] 范围内
    if delta_angle > std::f32::consts::PI {
        delta_angle -= std::f32::consts::TAU;
    } else if delta_angle < -std::f32::consts::PI {
        delta_angle += std::f32::consts::TAU;
    }

    delta_angle *= ROTATE_SENSITIVITY;

    // 构建旋转四元数
    let rot = Quat::from_axis_angle(axis, delta_angle);

    // 返回旋转变化量（相对旋转）
    Some(TransformDelta::Rotation(rot, Vec3::ZERO))
}

// ---------------------------------------------------------------------------
// Scale 计算
// ---------------------------------------------------------------------------

fn compute_scale(
    drag: &GizmoDrag,
    mouse_pos: egui::Pos2,
    ctx: &GizmoCtx,
) -> Option<TransformDelta> {
    match drag.part {
        // ── 均匀缩放 ──
        GizmoPart::Uniform => {
            // 使用屏幕空间距离变化来驱动均匀缩放
            let start_screen = drag.start_screen_pos;
            let delta_screen = mouse_pos - start_screen;

            // 从屏幕 Y 方向变化计算缩放因子
            let scale_factor = 1.0 + delta_screen.y * UNIFORM_SCALE_SENSITIVITY;
            let scale_factor = scale_factor.max(0.01); // 不能缩放到负值

            let start_scale = drag.start_value;
            let new_scale = start_scale * scale_factor;

            Some(TransformDelta::Scale(new_scale))
        }

        // ── 轴缩放 ──
        GizmoPart::AxisX | GizmoPart::AxisY | GizmoPart::AxisZ => {
            let axis = part_to_axis(drag.part);
            let hit = hit_position_on_axis(mouse_pos, ctx, axis)?;

            let start_scale = drag.start_value;

            // 计算 hit 沿轴从中心向外的距离
            let offset = hit - ctx.entity_pos;
            let t = offset.dot(axis);

            // 起始偏移量
            let start_t = drag.start_hit.dot(axis);

            // 缩放比值
            let ratio = if start_t.abs() > 1e-8 {
                (t / start_t).max(0.01)
            } else {
                1.0
            };

            // 构建新的缩放值：对应轴分量乘以 ratio，其他分量保持起始值
            let mut new_scale = start_scale;
            if axis.x.abs() > 0.5 {
                new_scale.x = start_scale.x * ratio;
            } else if axis.y.abs() > 0.5 {
                new_scale.y = start_scale.y * ratio;
            } else {
                new_scale.z = start_scale.z * ratio;
            }

            Some(TransformDelta::Scale(new_scale))
        }

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

/// 将 GizmoPart 映射到对应的轴方向。
fn part_to_axis(part: GizmoPart) -> Vec3 {
    match part {
        GizmoPart::AxisX | GizmoPart::RingX => Vec3::X,
        GizmoPart::AxisY | GizmoPart::RingY => Vec3::Y,
        GizmoPart::AxisZ | GizmoPart::RingZ => Vec3::Z,
        _ => Vec3::Y,
    }
}

/// 获取平面操作柄的法线方向。
fn plane_normal(part: GizmoPart) -> Vec3 {
    match part {
        GizmoPart::PlaneXY => Vec3::Z,  // XY 平面法线为 Z
        GizmoPart::PlaneXZ => Vec3::Y,  // XZ 平面法线为 Y
        GizmoPart::PlaneYZ => Vec3::X,  // YZ 平面法线为 X
        _ => Vec3::Y,
    }
}