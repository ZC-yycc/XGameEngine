//! 相机组件：**纯数据层**（投影参数 + 主相机标记，不含任何渲染代码）。
//!
//! # 分层
//!
//! ```text
//! core（本模块：Camera 声明 + 朝向数学）
//!   ↑
//! runtime（CameraView：把实体变换解成眼睛 / 朝向 / 投影，给三宿主共用）
//!   ↑                    ↑
//! editor / game        xgame-editor headless（--screenshot）
//! ```
//!
//! 与 [`crate::particle::ParticleEmitter`] / [`crate::audio::AudioSource`] 同款
//! 理由：**组件是场景数据**，要能 RON 序列化、能进撤销快照、能被脚本读改。
//!
//! # 为什么不学 `Light` 放进 `Scene`
//!
//! `Light` 是环境（"这个世界怎么被照亮"），相机是**物体**（"谁在看、从哪看"）。
//! 相机必须能被 Transform 层级带动 —— 挂在角色下面做第三人称肩视角、挂在上层
//! 节点上做整体摇镜、被脚本每帧推着跟随目标。做成场景级数据就丢了这一整层
//! 能力（`Light` 已经付过这个代价：点光位置只能手填，见其模块文档）。
//!
//! # 朝向约定：本地方向 **+Z** 是镜头朝向
//!
//! 与引擎其它地方保持一致，别改：
//!
//! - glTF 的模型正面就是 +Z（规范如此），`Transform::forward()` 是 `rotation * Z`；
//! - 所以一个 `rotation` 为单位四元数的相机**看向世界 +Z**，`up` 是世界 +Y。
//!
//! "看向 +Z"和"视图空间看向 −Z"是同一个事实的两种表述：视图矩阵（右手系 +
//! 0..1 深度）本来就把**镜头前方**映射到 −Z，这一步换算在 `runtime::CameraView`
//! 里做，本模块只给"世界空间的眼睛 / 前 / 上"。
//!
//! # 多相机
//!
//! 引擎单帧只渲染**一台**主相机（不做 Unity 那种多相机叠画）。选择规则只有
//! 一处实现：[`crate::Scene::active_camera_index`] —— 先看 `active`，都是
//! inactive 就退回"场景里第一个相机"，一台都没有就 `None`（宿主各自退回旧行为，
//! 老场景因此完全不受影响）。

use glam::{Mat3, Quat, Vec3};
use serde::{Deserialize, Serialize};

fn default_fov() -> f32 {
    60.0
}

fn default_near() -> f32 {
    0.1
}

fn default_far() -> f32 {
    1000.0
}

fn default_active() -> bool {
    true
}

/// 透视相机参数。挂在实体上就表示"这台相机在这份场景里的机位"。
///
/// 只有透视投影（`orthographic` 没做，见下）；宽高比**不在组件里** ——
/// 它属于视口而不属于场景：同一个相机在编辑器视口、独立游戏窗口、
/// 无头截图的尺寸各不相同，写进场景文件反而是错的数据。
///
/// 刻意**不做正交投影**：等真的出现塔防 / 等距玩法再补 `projection` 枚举，
/// 现在加只会让 Inspector 多一个永远默认的字段、视图矩阵多一个分支。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    /// 垂直视场角（**度**）。水平视角由宽高比推出来（`2·atan(tan(fov/2)·aspect)`）。
    #[serde(default = "default_fov")]
    pub fov: f32,
    /// 近裁剪面（米）。
    #[serde(default = "default_near")]
    pub near: f32,
    /// 远裁剪面（米）。
    #[serde(default = "default_far")]
    pub far: f32,
    /// 是否作为主相机候选。多机位切换时脚本只翻这一个开关。
    #[serde(default = "default_active")]
    pub active: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            fov: default_fov(),
            near: default_near(),
            far: default_far(),
            active: default_active(),
        }
    }
}

impl Camera {
    /// 一台 60° / 0.1 ~ 1000 的标准透视相机。
    pub fn new() -> Self {
        Self::default()
    }

    /// 垂直视场角（弧度）。零 / 负数 / 超范围值会被 [`Self::sanitized`] 钳住。
    pub fn fov_radians(&self) -> f32 {
        self.sanitized().fov.to_radians()
    }

    /// 钳掉会让渲染坏掉的参数。
    ///
    /// 为什么在 core 里钳而不是渲染层：这些值是场景数据（人手改 RON、脚本写、
    /// MCP 写），坏值会直接变成"整屏黑"或"所有东西被裁掉"，而**在那之前**
    /// 没有任何地方会报错。与其让 GPU 侧吃 NaN，不如读的时候就给一个确定值。
    ///
    /// - `fov` → 1°..179°（0° 与 180° 会让投影矩阵退化）；
    /// - `near` → ≥ 0.001（0 会让 1/near 变 inf）；
    /// - `far` → ≥ near + 0.001（far ≤ near 时整条深度范围反向 / 为空）。
    pub fn sanitized(&self) -> Self {
        let fov = if self.fov.is_finite() {
            self.fov.clamp(1.0, 179.0)
        } else {
            default_fov()
        };
        let near = if self.near.is_finite() {
            self.near.max(0.001)
        } else {
            default_near()
        };
        let far = if self.far.is_finite() {
            self.far.max(near + 0.001)
        } else {
            default_far().max(near + 0.001)
        };
        Self {
            fov,
            near,
            far,
            active: self.active,
        }
    }

    /// 世界空间镜头朝向（由实体的世界旋转给出）。
    pub fn forward(rotation: Quat) -> Vec3 {
        rotation * Vec3::Z
    }

    /// 世界空间上方向（由实体的世界旋转给出）。
    pub fn up(rotation: Quat) -> Vec3 {
        rotation * Vec3::Y
    }
}

/// 由"看向哪里"反解出旋转：让实体本地的 **+Z** 指向 `forward`。
///
/// `up_hint` 是用来定滚转的参考世界上方向，通常传 `Vec3::Y`。
///
/// 退化处理（都会返回一个**有限**的四元数，绝不给 NaN）：
/// - `forward` 为零 / 非有限 → 单位旋转（看向 +Z）；
/// - `up_hint` 与 `forward` 平行（正上方俯视）→ 换 `Z` 或 `X` 当参考轴，
///   否则叉乘退化成零向量，`normalize()` 出来是 NaN，整帧变黑。
pub fn look_rotation(forward: Vec3, up_hint: Vec3) -> Quat {
    if !forward.is_finite() || forward.length_squared() < 1e-12 {
        return Quat::IDENTITY;
    }
    let dir = forward.normalize();
    let mut hint = if up_hint.is_finite() && up_hint.length_squared() > 1e-12 {
        up_hint.normalize()
    } else {
        Vec3::Y
    };
    if hint.cross(dir).length_squared() < 1e-8 {
        hint = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
        if hint.cross(dir).length_squared() < 1e-8 {
            hint = Vec3::X;
        }
    }
    let right = hint.cross(dir).normalize();
    let up = dir.cross(right).normalize();
    // 列向量 = 本地 X / Y / Z 在世界空间的方向（行列式 +1，右手系）。
    Quat::from_mat3(&Mat3::from_cols(right, up, dir)).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_quat_eq(a: Quat, b: Quat, tol: f32) {
        // q 与 -q 是同一个旋转，比的时候取绝对值。
        let d = a.dot(b).abs();
        assert!((d - 1.0).abs() < tol, "四元数不等: {a:?} vs {b:?}");
    }

    #[test]
    fn default_camera_is_perspective_60() {
        let c = Camera::default();
        assert_eq!(c.fov, 60.0);
        assert!(c.active);
        assert!(c.near > 0.0 && c.far > c.near);
        assert!((c.fov_radians() - 60f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn sanitized_clamps_broken_projection() {
        // 0° 视场角会让投影矩阵退化；near = 0 会让 1/near 变 inf；
        // far < near 会让深度范围反向。三种坏值都要被钳成可用值。
        let c = Camera {
            fov: 0.0,
            near: 0.0,
            far: -5.0,
            active: false,
        }
        .sanitized();
        assert_eq!(c.fov, 1.0);
        assert!(c.near >= 0.001);
        assert!(c.far > c.near);
        // active 不是"坏值"，不能被顺手改掉。
        assert!(!c.active);
    }

    #[test]
    fn sanitized_replaces_non_finite() {
        let c = Camera {
            fov: f32::NAN,
            near: f32::INFINITY,
            far: f32::NEG_INFINITY,
            active: true,
        }
        .sanitized();
        assert_eq!(c.fov, 60.0);
        assert!(c.near.is_finite() && c.far.is_finite());
        assert!(c.far > c.near);
    }

    #[test]
    fn look_rotation_of_plus_z_is_identity() {
        // 约定：本地 +Z 是镜头朝向 → 看向 +Z 等于不旋转。
        let q = look_rotation(Vec3::Z, Vec3::Y);
        assert_quat_eq(q, Quat::IDENTITY, 1e-5);
        assert!((Camera::forward(q) - Vec3::Z).length() < 1e-5);
        assert!((Camera::up(q) - Vec3::Y).length() < 1e-5);
    }

    #[test]
    fn look_rotation_points_forward_along_target_direction() {
        for dir in [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(1.0, -2.0, 3.0),
            Vec3::new(-0.3, 0.9, 0.2),
        ] {
            let q = look_rotation(dir, Vec3::Y);
            let f = Camera::forward(q);
            assert!(
                (f - dir.normalize()).length() < 1e-4,
                "forward 没对准 {dir:?}，实际 {f:?}"
            );
            // up 必须与 forward 正交，且保持在提示上方向的同侧（没有翻转）。
            assert!(f.dot(Camera::up(q)).abs() < 1e-4);
            assert!(Camera::up(q).y > 0.0, "上方向被翻到下面去了: {dir:?}");
        }
    }

    #[test]
    fn look_rotation_survives_vertical_look() {
        // 正上方 / 正下方俯视：up_hint 与前方向平行，叉乘会退化成零向量。
        // 这条测试盯的是"不许出 NaN"（出 NaN 就是整帧变黑）。
        for dir in [Vec3::Y, Vec3::NEG_Y] {
            let q = look_rotation(dir, Vec3::Y);
            assert!(q.is_finite() && q.is_normalized(), "退化方向给出了坏四元数");
            assert!((Camera::forward(q) - dir).length() < 1e-4);
            assert!(Camera::up(q).is_finite());
        }
    }

    #[test]
    fn look_rotation_handles_zero_and_non_finite() {
        assert_quat_eq(look_rotation(Vec3::ZERO, Vec3::Y), Quat::IDENTITY, 1e-6);
        let q = look_rotation(Vec3::new(f32::NAN, 0.0, 0.0), Vec3::Y);
        assert_eq!(q, Quat::IDENTITY);
        // up_hint 坏了也不该污染结果。
        let q = look_rotation(Vec3::X, Vec3::ZERO);
        assert!((Camera::forward(q) - Vec3::X).length() < 1e-4);
    }

    #[test]
    fn camera_ron_roundtrip_and_defaults() {
        let c = Camera {
            fov: 75.0,
            near: 0.05,
            far: 200.0,
            active: false,
        };
        let text = ron::ser::to_string(&c).expect("序列化失败");
        let back: Camera = ron::from_str(&text).expect("反序列化失败");
        assert_eq!(c, back);

        // 手写块只给一部分字段：其余走默认值（老文件 / 人手改 RON 都靠这个）。
        let partial: Camera = ron::from_str("(fov: 90.0)").expect("缺字段应走默认");
        assert_eq!(partial.fov, 90.0);
        assert_eq!(partial.near, default_near());
        assert_eq!(partial.far, default_far());
        assert!(partial.active);
    }
}
