use glam::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

/// 层级空间变换（位置、旋转、缩放）。
///
/// 局部空间矩阵：`T * R * S`。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Transform {
    pub const IDENTITY: Self = Self {
        position: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub fn from_translation(position: Vec3) -> Self {
        Self {
            position,
            ..Self::IDENTITY
        }
    }

    /// 局部空间变换矩阵：T * R * S。
    pub fn local_matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }

    /// 从 4x4 矩阵反解出 TRS（位置 / 旋转 / 缩放）。
    ///
    /// 层级操作里经常会碰上"已知世界矩阵、要倒推局部变换"的场景——重新挂载
    /// 父节点（保留世界变换）、物理回写（物理世界给的是世界变换，而实体的
    /// `transform` 存的是局部）、gizmo 在世界空间拖动后回填局部。这三处都走这条。
    ///
    /// `glam` 的分解对退化矩阵（行列式为 0、含 NaN 等）会退化成单位四元数或
    /// 零缩放，不会 panic——层级编辑最忌讳因为一个坏矩阵把整个编辑器带崩。
    pub fn from_matrix(matrix: Mat4) -> Self {
        let (scale, rotation, position) = matrix.to_scale_rotation_translation();
        Self {
            position,
            rotation,
            scale,
        }
    }

    /// 对另一个变换矩阵应用本变换（`self * other`），返回组合矩阵。
    ///
    /// 名字刻意不叫 `mul`，避免和"分量相乘"混淆——这里的语义是矩阵相乘，
    /// 也就是"先按 `other` 变换、再按 `self` 变换"的坐标系级联。
    pub fn then(&self, other: &Mat4) -> Mat4 {
        self.local_matrix() * *other
    }

    pub fn forward(&self) -> Vec3 {
        self.rotation * Vec3::Z
    }

    pub fn right(&self) -> Vec3 {
        self.rotation * Vec3::X
    }

    pub fn up(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}
