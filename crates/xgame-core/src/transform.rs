use glam::{EulerRot, Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

/// 欧拉角（度）→ 四元数：**全工程唯一的欧拉换算入口**，分解顺序固定 `YXZ`。
///
/// # 为什么必须只有一处
///
/// 欧拉角不是姿态的唯一表示：同一组 `(x, y, z)` 在 `XYZ` / `YXZ` / `ZXY` 下是
/// **三个不同的姿态**。当不同入口各写一份 `from_euler`，症状不会是报错，而是
/// 「面板上写 `(30, 45, 60)`、脚本里读回来是 `(20.3, 41.9, 55.1)`」—— 作者
/// 只会以为其中一处算错了。这个 bug 真实存在过：MCP 的 `rotation_euler_deg`
/// 用 `XYZ`，而 Inspector 与脚本 API 用 `YXZ`，单轴旋转看不出差别（两种顺序
/// 在单轴上恰好同解），多轴一混就分叉。
///
/// # 为什么是 `YXZ`
///
/// 它把偏航（Y）/ 俯仰（X）/ 滚转（Z）分开，对相机与角色这类"先转头再低头"
/// 的物体给出人类读得懂的三元组；`XYZ` 在同样位姿下会折成
/// `(-46°, 153°, 160°)` 这种读不出来的值。**别改成 `XYZ`**：改了等于把所有
/// 已落盘的作者意图重新解释一遍。
///
/// 分量顺序始终是 `(x, y, z)` —— `YXZ` 是**施加顺序**，不是分量排列。
pub fn euler_deg_to_quat(deg: Vec3) -> Quat {
    Quat::from_euler(
        EulerRot::YXZ,
        deg.y.to_radians(),
        deg.x.to_radians(),
        deg.z.to_radians(),
    )
}

/// 四元数 → 欧拉角（度），与 [`euler_deg_to_quat`] 互为逆，分量顺序 `(x, y, z)`。
///
/// 往返不是恒等（欧拉角有万向锁与前后的等价解），只保证**姿态**等价 ——
/// 任何"存下来的欧拉角将来还要读回同一个数"的需求都应存四元数。
pub fn quat_to_euler_deg(q: Quat) -> Vec3 {
    let (y, x, z) = q.to_euler(EulerRot::YXZ);
    Vec3::new(x.to_degrees(), y.to_degrees(), z.to_degrees())
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// 钉住顺序就是 `YXZ`：**这条测试是在钉契约，不是在测数学**。
    ///
    /// 混合轴是关键 —— 单轴时 `XYZ` 与 `YXZ` 恰好同解，所以只用 `(0, 90, 0)`
    /// 做断言的话，MCP 那份 `XYZ` 实现混进来也照样绿（这正是它长期没被发现的
    /// 原因）。这里的期望值用 `YXZ` 显式写出来，改成 `XYZ` 立刻红。
    #[test]
    fn euler_conversion_uses_yxz_order() {
        let deg = Vec3::new(30.0, 45.0, 60.0);
        let got = euler_deg_to_quat(deg);
        let yxz = Quat::from_euler(
            EulerRot::YXZ,
            deg.y.to_radians(),
            deg.x.to_radians(),
            deg.z.to_radians(),
        );
        assert!(
            (0..4).all(|i| (got.to_array()[i] - yxz.to_array()[i]).abs() < 1e-6),
            "必须是 YXZ：{got:?} != {yxz:?}"
        );
        // 负对照：同一组度数在 XYZ 下是**另一个姿态**，两者必须分得开。
        let xyz = Quat::from_euler(
            EulerRot::XYZ,
            deg.x.to_radians(),
            deg.y.to_radians(),
            deg.z.to_radians(),
        );
        let diff = (0..4)
            .map(|i| (got.to_array()[i] - xyz.to_array()[i]).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            diff > 0.1,
            "混合轴下 YXZ 与 XYZ 必须明显不同（实测差 {diff}）"
        );
    }

    /// 相机式的偏航 + 俯仰要能原样读回来（`YXZ` 的实际理由）。
    #[test]
    fn yaw_and_pitch_read_back_as_humans_expect() {
        let pose = euler_deg_to_quat(Vec3::new(20.0, 30.0, 0.0));
        let back = quat_to_euler_deg(pose);
        assert!((back.x - 20.0).abs() < 1e-3, "pitch: {back:?}");
        assert!((back.y - 30.0).abs() < 1e-3, "yaw: {back:?}");
        assert!(back.z.abs() < 1e-3, "roll 应当为 0: {back:?}");
    }

    /// 往返只在**姿态**上等价；同一象限的小角度会逐分量还原。
    #[test]
    fn euler_roundtrip_preserves_pose() {
        let original = Vec3::new(12.0, 45.0, -30.0);
        let back = quat_to_euler_deg(euler_deg_to_quat(original));
        for (got, want) in [
            (back.x, original.x),
            (back.y, original.y),
            (back.z, original.z),
        ] {
            assert!((got - want).abs() < 1e-3, "{back:?} vs {original:?}");
        }
        // 反过来也要成立：姿态本身必须逐分量一致。
        let a = euler_deg_to_quat(original);
        let b = euler_deg_to_quat(back);
        assert!(
            (0..4).all(|i| (a.to_array()[i] - b.to_array()[i]).abs() < 1e-5),
            "四元数往返必须稳定：{a:?} vs {b:?}"
        );
    }
}
