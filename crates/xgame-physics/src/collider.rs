//! 碰撞体组件类型。

use glam::Vec3;
use serde::{Deserialize, Serialize};

/// 支持的碰撞体形状。
///
/// 所有尺寸使用半边长（例如单位立方体的 `half_extents = (0.5, 0.5, 0.5)`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ColliderShape {
    /// 轴对齐盒体，使用半边长。
    Cuboid { half_extents: Vec3 },
    /// 球体，由半径定义。
    Ball { radius: f32 },
    /// 胶囊体，沿 Y 轴对齐。
    Capsule { half_height: f32, radius: f32 },
    /// 圆柱体，沿 Y 轴对齐。
    Cylinder { half_height: f32, radius: f32 },
    /// 任意三角形网格（必须不自交）。
    Trimesh {
        vertices: Vec<[f32; 3]>,
        indices: Vec<[u32; 3]>,
    },
    /// 高度场碰撞体（用于地形）。
    HeightField {
        heights: Vec<f32>,
        nx: u32,
        nz: u32,
        scale: Vec3,
    },
}

impl ColliderShape {
    pub fn cuboid(half_extents: Vec3) -> Self {
        Self::Cuboid { half_extents }
    }

    pub fn ball(radius: f32) -> Self {
        Self::Ball { radius }
    }

    pub fn capsule(half_height: f32, radius: f32) -> Self {
        Self::Capsule { half_height, radius }
    }

    pub fn cylinder(half_height: f32, radius: f32) -> Self {
        Self::Cylinder { half_height, radius }
    }
}

/// 描述附加到刚体上的碰撞形状。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collider {
    pub shape: ColliderShape,
    /// 库仑摩擦系数（0 = 无摩擦，1 = 高摩擦）。
    pub friction: f32,
    /// 弹性系数（弹力）。0 = 完全非弹性，1 = 完全弹性。
    pub restitution: f32,
    /// 是否为传感器（检测重叠但不产生接触力）。
    pub is_sensor: bool,
    /// 密度，用于自动质量计算。当父刚体手动指定 mass > 0 时忽略。
    pub density: f32,
    /// 相对于父刚体原点的局部偏移。
    pub offset: Vec3,
}

impl Default for Collider {
    fn default() -> Self {
        Self {
            shape: ColliderShape::Cuboid {
                half_extents: Vec3::splat(0.5),
            },
            friction: 0.5,
            restitution: 0.0,
            is_sensor: false,
            density: 1.0,
            offset: Vec3::ZERO,
        }
    }
}

impl Collider {
    pub fn cuboid(half_extents: Vec3) -> Self {
        Self {
            shape: ColliderShape::Cuboid { half_extents },
            ..Default::default()
        }
    }

    pub fn ball(radius: f32) -> Self {
        Self {
            shape: ColliderShape::Ball { radius },
            ..Default::default()
        }
    }

    pub fn capsule(half_height: f32, radius: f32) -> Self {
        Self {
            shape: ColliderShape::Capsule { half_height, radius },
            ..Default::default()
        }
    }
}
