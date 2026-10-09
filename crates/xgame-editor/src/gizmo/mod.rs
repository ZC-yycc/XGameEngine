//! Gizmo 变换操作柄系统——在 Scene View 中通过 egui 叠加层绘制并交互。
//!
//! 支持 Translate / Rotate / Scale 三种工具模式，每种模式渲染不同的
//! 3D 操作柄形状（轴线、箭头、圆环、方块、平面），并通过屏幕空间
//! 拾取检测实现鼠标悬停高亮和拖拽交互。
//!
//! # 架构
//!
//! ```text
//! mod.rs    公共 API + 常量 + 辅助函数
//! math.rs   3D↔2D 投影、射线、几何相交
//! shapes.rs 各工具模式的 3D 形状数据
//! draw.rs   将形状投影后绘制为 egui 2D 图元
//! pick.rs   屏幕空间拾取检测
//! drag.rs   拖拽变换计算
//! ```
//!
//! # NaN/Inf 防护策略
//!
//! 所有 3D→2D 投影通过 [`project_to_screen`] 进行，该函数在单一入口点
//! 统一处理 NaN/Inf、相机后方点、NDC 越界等异常情况，返回标记值
//! `(-10000, -10000)`。调用方通过 [`is_valid_screen`] 检查即可。

mod math;
mod shapes;
mod draw;
mod pick;
mod drag;

// ── 公共 API 重导出 ──

pub use math::{gizmo_world_size, project_to_screen, screen_to_ray};
pub use draw::draw_gizmo;
pub use pick::{pick_gizmo, axis_param_under_mouse, hit_position_on_axis, hit_position_on_plane};
pub use drag::{compute_transform, TransformDelta};
use xgame_math::glam::Vec3;
use crate::state::{GizmoDrag, GizmoPart};

/// 操作柄对应的世界轴方向。
///
/// 轴线与旋转环共用同一组方向（环是绕该轴转），平面与中心方块没有单一轴。
/// 类 Unity 的 gizmo 都画在世界轴上、不跟物体自身旋转走，所以这里直接用主轴常量。
pub fn handle_axis(part: GizmoPart) -> Option<Vec3> {
    match part {
        GizmoPart::AxisX | GizmoPart::RingX => Some(Vec3::X),
        GizmoPart::AxisY | GizmoPart::RingY => Some(Vec3::Y),
        GizmoPart::AxisZ | GizmoPart::RingZ => Some(Vec3::Z),
        GizmoPart::PlaneXY | GizmoPart::PlaneXZ | GizmoPart::PlaneYZ | GizmoPart::Uniform => None,
    }
}

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

/// 轴线宽度（逻辑像素）。
pub(crate) const AXIS_LINE_WIDTH: f32 = 3.0;

/// 高亮轴线的宽度（逻辑像素）。
pub(crate) const AXIS_HOVER_WIDTH: f32 = 5.0;

/// 操作柄长度占相机距离的比例。
pub(crate) const GIZMO_SCREEN_FRACTION: f32 = 0.22;

/// 箭头头部长度占操作柄总长的比例。
pub(crate) const ARROW_HEAD_FRACTION: f32 = 0.12;

/// 平面操作柄大小占操作柄总长的比例。
pub(crate) const PLANE_HANDLE_SIZE: f32 = 0.28;

/// 旋转圆环半径占操作柄总长的比例。
pub(crate) const RING_RADIUS: f32 = 0.8;

/// 旋转圆环的细分段数。
pub(crate) const RING_SEGMENTS: u32 = 48;

/// 缩放方块半边长占操作柄总长的比例。
pub(crate) const CUBE_SIZE: f32 = 0.08;

// ── 轴数据 ──

/// 三个主轴方向。
pub(crate) const AXIS_DIRS: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];

/// 轴颜色：X=红, Y=绿, Z=蓝。
pub(crate) const AXIS_COLORS: [egui::Color32; 3] = [
    egui::Color32::from_rgb(255, 48, 48),
    egui::Color32::from_rgb(48, 255, 48),
    egui::Color32::from_rgb(48, 128, 255),
];

/// 悬停高亮颜色（亮黄）。
pub(crate) const HOVER_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 240, 80);

/// 拖拽激活颜色（金色）。
pub(crate) const ACTIVE_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 200, 40);

/// 均匀缩放操作柄颜色。
pub(crate) const UNIFORM_COLOR: egui::Color32 = egui::Color32::from_rgb(200, 200, 200);

/// 平面操作柄默认填充色（半透明白色）。
pub(crate) const PLANE_FILL: egui::Color32 =
    egui::Color32::from_rgba_premultiplied(255, 255, 100, 80);

// ---------------------------------------------------------------------------
// 内部上下文 —— 减少子模块参数传递
// ---------------------------------------------------------------------------

/// 打包 gizmo 绘制所需的视图和状态上下文。
///
/// 仅在 crate 内部使用；公共 API 保持独立参数以兼容现有调用方。
#[derive(Clone)]
pub(crate) struct GizmoCtx {
    pub view_proj: xgame_math::glam::Mat4,
    pub screen_size: egui::Vec2,
    pub screen_offset: egui::Vec2,
    pub entity_pos: Vec3,
    pub gizmo_size: f32,
    pub hovered: Option<GizmoPart>,
    pub active: Option<GizmoDrag>,
}

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

/// 按索引获取轴操作柄类型。
pub(crate) fn axis_part(idx: usize) -> GizmoPart {
    match idx {
        0 => GizmoPart::AxisX,
        1 => GizmoPart::AxisY,
        _ => GizmoPart::AxisZ,
    }
}

/// 按索引获取旋转环操作柄类型。
pub(crate) fn ring_part(idx: usize) -> GizmoPart {
    match idx {
        0 => GizmoPart::RingX,
        1 => GizmoPart::RingY,
        _ => GizmoPart::RingZ,
    }
}

/// 按操作柄类型获取对应的轴方向。
pub(crate) fn part_axis(part: GizmoPart) -> Vec3 {
    match part {
        GizmoPart::AxisX | GizmoPart::RingX => Vec3::X,
        GizmoPart::AxisY | GizmoPart::RingY => Vec3::Y,
        GizmoPart::AxisZ | GizmoPart::RingZ => Vec3::Z,
        _ => Vec3::Y,
    }
}

/// 获取操作柄的显示颜色（考虑悬停/激活状态）。
pub(crate) fn part_color(
    part: GizmoPart,
    hovered: Option<GizmoPart>,
    active: Option<&GizmoDrag>,
) -> egui::Color32 {
    if let Some(d) = active {
        if d.part == part {
            return ACTIVE_COLOR;
        }
    }
    if let Some(h) = hovered {
        if h == part {
            return HOVER_COLOR;
        }
    }
    base_part_color(part)
}

/// 获取操作柄的基础颜色。
pub(crate) fn base_part_color(part: GizmoPart) -> egui::Color32 {
    match part {
        GizmoPart::AxisX | GizmoPart::RingX => AXIS_COLORS[0],
        GizmoPart::AxisY | GizmoPart::RingY => AXIS_COLORS[1],
        GizmoPart::AxisZ | GizmoPart::RingZ => AXIS_COLORS[2],
        // 平面颜色取对应法线轴的颜色
        GizmoPart::PlaneXY => AXIS_COLORS[2], // XY 平面 → Z 轴色（蓝）
        GizmoPart::PlaneXZ => AXIS_COLORS[1], // XZ 平面 → Y 轴色（绿）
        GizmoPart::PlaneYZ => AXIS_COLORS[0], // YZ 平面 → X 轴色（红）
        GizmoPart::Uniform => UNIFORM_COLOR,
    }
}

/// 判断操作柄是否处于高亮或激活状态。
pub(crate) fn is_highlighted(
    part: GizmoPart,
    hovered: Option<GizmoPart>,
    active: Option<&GizmoDrag>,
) -> bool {
    if let Some(h) = hovered {
        if h == part {
            return true;
        }
    }
    if let Some(d) = active {
        if d.part == part {
            return true;
        }
    }
    false
}

/// 依据高亮状态返回线宽。
pub(crate) fn axis_width(highlighted: bool) -> f32 {
    if highlighted {
        AXIS_HOVER_WIDTH
    } else {
        AXIS_LINE_WIDTH
    }
}
