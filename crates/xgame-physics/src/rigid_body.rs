//! 刚体组件类型。

use glam::Vec3;
use serde::{Deserialize, Serialize};
use xgame_math::Transform;

/// 刚体类型，决定其与力的交互方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RigidBodyType {
    /// 不可移动，无限质量。不响应力。
    Static,
    /// 全动力学模拟——受重力、力和碰撞影响。
    Dynamic,
    /// 用户控制速度。影响动态体但不受其影响。典型用于移动平台或玩家角色。
    KinematicVelocity,
    /// 位置控制。类似 KinematicVelocity，但通过每帧直接设置位置更新。
    KinematicPosition,
}

/// 描述刚体的物理属性。
///
/// 存储在 CPU 端；添加到物理世界时转换为 rapier3d `RigidBody`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RigidBody {
    pub body_type: RigidBodyType,
    /// 创建时或最后一次同步时的变换。
    pub transform: Transform,
    /// 初始线速度（世界空间）。用于 Dynamic 类型。
    pub linear_velocity: Vec3,
    /// 初始角速度。用于 Dynamic 类型。
    pub angular_velocity: Vec3,
    /// 质量（千克）。若 `body_type` 为 Static，则忽略。
    pub mass: f32,
    /// 角惯性阻尼（0 = 无阻尼，1 = 完全停止）。
    pub angular_damping: f32,
    /// Linear damping.
    pub linear_damping: f32,
    /// 重力是否影响该刚体。
    pub gravity_enabled: bool,
    /// 重力缩放系数（1 = 正常，0 = 失重，负数 = 反重力）。
    ///
    /// 与 `gravity_enabled` 并存而不是取代它：后者是给"要不要重力"这个
    /// 二元问题的显式开关，前者负责连续调节。实际生效的重力 =
    /// `gravity_enabled as f32 * gravity_scale * 世界重力`。
    pub gravity_scale: f32,
    /// 该刚体是否可以休眠（性能优化）。
    pub can_sleep: bool,
}

impl Default for RigidBody {
    fn default() -> Self {
        Self {
            body_type: RigidBodyType::Dynamic,
            transform: Transform::IDENTITY,
            linear_velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            mass: 1.0,
            angular_damping: 0.1,
            linear_damping: 0.1,
            gravity_enabled: true,
            gravity_scale: 1.0,
            can_sleep: true,
        }
    }
}

impl RigidBody {
    /// 便捷方法：在给定变换处创建静态刚体。
    pub fn static_at(transform: Transform) -> Self {
        Self {
            body_type: RigidBodyType::Static,
            transform,
            mass: 0.0,
            gravity_enabled: false,
            can_sleep: true,
            ..Default::default()
        }
    }

    /// 便捷方法：在给定变换处创建动态刚体。
    pub fn dynamic_at(transform: Transform) -> Self {
        Self {
            body_type: RigidBodyType::Dynamic,
            transform,
            ..Default::default()
        }
    }

    /// 便捷方法：创建**位置驱动**的运动学刚体（角色控制器专用）。
    ///
    /// 与 `KinematicVelocity` 的区别在 rapier 侧的接触求解语义：position-based
    /// 是"硬挡"（无限质量，动态物体被锋利的投影推开，接近 Unity 的
    /// CharacterController）；velocity-based 则会把速度交换给动态物体
    /// （为移动平台设计）。角色要的是前者。
    ///
    /// `gravity_enabled: false` + `can_sleep: false`：重力由角色控制器自己算
    /// （脚本层的 `character_simple_move`），不走 rapier 的重力积分；休眠对
    /// 一个每帧都要 `move_shape` 的刚体没有意义，还可能在唤醒边界上抖一帧。
    pub fn kinematic_position_at(transform: Transform) -> Self {
        Self {
            body_type: RigidBodyType::KinematicPosition,
            transform,
            mass: 0.0,
            gravity_enabled: false,
            can_sleep: false,
            ..Default::default()
        }
    }
}
