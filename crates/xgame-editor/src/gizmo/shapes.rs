//! 各工具模式的 3D 操作柄形状生成。
//!
//! 所有顶点在世界空间中计算，由 `draw` 模块投影后渲染。
//! 形状数据与工具模式和实体位置绑定，每帧重新生成。

use xgame_math::glam::Vec3;

use super::math;
use super::{ARROW_HEAD_FRACTION, AXIS_DIRS, PLANE_HANDLE_SIZE, RING_RADIUS, RING_SEGMENTS};

// ---------------------------------------------------------------------------
// Translate 形状
// ---------------------------------------------------------------------------

/// Translate 工具的操作柄 3D 形状。
pub struct TranslateShape {
    /// 轴线（不含箭头头部）：[(起点, 终点); 3]。
    pub axes: [(Vec3, Vec3); 3],
    /// 箭头头部三角形：[X, Y, Z]。
    pub arrow_heads: [ArrowHead; 3],
    /// 平面操作柄：[XY, XZ, YZ]。
    pub planes: [PlaneHandle; 3],
}

/// 箭头头部形状（从侧面看是三角形）。
pub struct ArrowHead {
    pub tip: Vec3,
}

/// 平面操作柄（四个角点构成凸四边形）。
pub struct PlaneHandle {
    pub corners: [Vec3; 4],
}

impl TranslateShape {
    /// 根据实体位置和 gizmo 尺寸生成形状。
    pub fn new(entity_pos: Vec3, gizmo_size: f32) -> Self {
        let arrow_len = gizmo_size * (1.0 - ARROW_HEAD_FRACTION);
        let plane_size = gizmo_size * PLANE_HANDLE_SIZE;

        let mut axes = [(Vec3::ZERO, Vec3::ZERO); 3];
        let mut arrow_heads = [
            ArrowHead { tip: Vec3::ZERO },
            ArrowHead { tip: Vec3::ZERO },
            ArrowHead { tip: Vec3::ZERO },
        ];

        for (i, &axis) in AXIS_DIRS.iter().enumerate() {
            let tip = entity_pos + axis * gizmo_size;
            let arrow_base = entity_pos + axis * arrow_len;

            axes[i] = (entity_pos, arrow_base);

            arrow_heads[i] = ArrowHead { tip };
        }

        let planes = [
            PlaneHandle {
                corners: plane_corners(entity_pos, Vec3::X, Vec3::Y, plane_size),
            },
            PlaneHandle {
                corners: plane_corners(entity_pos, Vec3::X, Vec3::Z, plane_size),
            },
            PlaneHandle {
                corners: plane_corners(entity_pos, Vec3::Y, Vec3::Z, plane_size),
            },
        ];

        Self {
            axes,
            arrow_heads,
            planes,
        }
    }
}

/// 从原点沿两个方向生成平面操作柄的四个角点。
fn plane_corners(origin: Vec3, a1: Vec3, a2: Vec3, size: f32) -> [Vec3; 4] {
    [
        origin,
        origin + a1 * size,
        origin + a1 * size + a2 * size,
        origin + a2 * size,
    ]
}

// ---------------------------------------------------------------------------
// Rotate 形状
// ---------------------------------------------------------------------------

/// Rotate 工具的操作柄形状：每个轴的圆环。
pub struct RotateShape {
    /// 每个轴的世界空间圆环顶点：[X, Y, Z]，各 `ring_segments + 1` 个点。
    pub rings: [Vec<Vec3>; 3],
}

impl RotateShape {
    pub fn new(entity_pos: Vec3, gizmo_size: f32) -> Self {
        let radius = gizmo_size * RING_RADIUS;
        let n = RING_SEGMENTS;

        let mut rings: [Vec<Vec3>; 3] = [
            Vec::with_capacity(n as usize + 1),
            Vec::with_capacity(n as usize + 1),
            Vec::with_capacity(n as usize + 1),
        ];

        for (i, &axis) in AXIS_DIRS.iter().enumerate() {
            let (u, v) = math::perpendicular_axes(axis);
            for s in 0..=n {
                let theta = (s as f32 / n as f32) * std::f32::consts::TAU;
                rings[i].push(entity_pos + (u * theta.cos() + v * theta.sin()) * radius);
            }
        }

        Self { rings }
    }
}

// ---------------------------------------------------------------------------
// Scale 形状
// ---------------------------------------------------------------------------

/// Scale 工具的操作柄形状。
pub struct ScaleShape {
    /// 轴线：[X, Y, Z]。
    pub axes: [(Vec3, Vec3); 3],
    /// 轴线端点方块的世界空间中心：[X, Y, Z]。
    pub cube_centers: [Vec3; 3],
    /// 中心均匀缩放方块的世界空间中心。
    pub center: Vec3,
}

impl ScaleShape {
    pub fn new(entity_pos: Vec3, gizmo_size: f32) -> Self {
        let mut axes = [(Vec3::ZERO, Vec3::ZERO); 3];
        let mut cube_centers = [Vec3::ZERO; 3];

        for (i, &axis) in AXIS_DIRS.iter().enumerate() {
            let tip = entity_pos + axis * gizmo_size;
            axes[i] = (entity_pos, tip);
            cube_centers[i] = tip;
        }

        Self {
            axes,
            cube_centers,
            center: entity_pos,
        }
    }
}
