//! 场景里声明的物理属性（纯数据，可序列化）。
//!
//! # 为什么这一层在 core 而不是 physics
//!
//! `xgame-core` 不认识 rapier3d，也不该认识 —— 它只描述"这个实体在场景里
//! 应该是一块什么样的物理物体"，例如「动态、1×1×1 的箱子、摩擦 0.6」。
//! `xgame-physics` 负责把这份声明翻译成 rapier 的 `RigidBody` + `Collider`。
//!
//! 这样分层带来两个实际好处：
//!
//! - **场景文件能表达物理**。旧实现是"凡是带模型的实体一律变成动态盒子"，
//!   玩家连一块地面都放不稳（示例项目里的物体会一直往下掉）。物理属性进场景
//!   之后，地面、平台、角色各自是什么，是**作者**说了算，不是运行时猜。
//! - **core 依然可以被无物理的宿主使用**。只做渲染预览的工具不必拖进 rapier。
//!
//! 数据用 `[f32; 3]` 而不是 `glam::Vec3`：场景文件里 `(1.0, 1.0, 1.0)`
//! 这种写法对 RON 更友好，也避免把 glam 的序列化格式当成文件格式的一部分。

use serde::{Deserialize, Serialize};

/// 刚体的运动学类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BodyKind {
    /// 不动，无限质量。地面、墙壁、平台。
    Static,
    /// 全动力学模拟：受重力、力、碰撞影响。
    #[default]
    Dynamic,
    /// 运动学：由脚本 / 代码直接驱动位置或速度，本身不受力，但会推动别人。
    /// 移动平台、电梯、由脚本控制的角色。
    Kinematic,
}

/// 碰撞体形状（场景可编写的子集）。
///
/// 引擎内部还支持三角网格与高度场（见 `xgame-physics::ColliderShape`），
/// 但那两种形状携带大块数据、通常由导入器生成而非手写，所以不进场景文件。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum CollisionShape {
    /// 轴对齐盒体（半边长）。
    Cuboid { half_extents: [f32; 3] },
    /// 球体。
    Ball { radius: f32 },
    /// 沿 Y 轴的胶囊体。
    Capsule { half_height: f32, radius: f32 },
    /// 沿 Y 轴的圆柱体。
    Cylinder { half_height: f32, radius: f32 },
    /// 由运行时按实体变换的缩放自动生成盒子。
    ///
    /// 这是**默认值**，也是新实体的常见情形：作者摆一个 1×1×1 的方块
    /// 缩放成 4×0.2×4 当平台，`Auto` 直接读它的变换，不用再手填一遍尺寸。
    /// 缩放的绝对值被用作盒子半边长 —— 与"视觉外观即碰撞形状"的直觉一致。
    #[default]
    Auto,
}

/// 一个实体的物理属性。
///
/// 挂在 [`Entity::physics`](crate::Entity) 上；为 `None` 表示这个实体
/// **不参与物理**（纯视觉），这是与 Unity「没有 RigidBody / Collider 就没有物理」
/// 一致的行为。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PhysicsBody {
    /// 运动学类型。
    #[serde(default)]
    pub kind: BodyKind,
    /// 碰撞体形状。
    #[serde(default)]
    pub shape: CollisionShape,
    /// 质量（千克）。`0` 表示按碰撞体密度自动推算。
    #[serde(default)]
    pub mass: f32,
    /// 库仑摩擦（0 无摩擦，1 高摩擦）。
    #[serde(default = "default_friction")]
    pub friction: f32,
    /// 弹性（0 完全非弹性，1 完全弹性）。
    #[serde(default)]
    pub restitution: f32,
    /// 重力缩放（1 = 正常，0 = 失重，负数 = 反重力）。
    #[serde(default = "default_gravity_scale")]
    pub gravity_scale: f32,
    /// 传感器：检测重叠但不产生接触力（触发器）。
    #[serde(default)]
    pub is_sensor: bool,
}

fn default_friction() -> f32 {
    0.5
}

fn default_gravity_scale() -> f32 {
    1.0
}

impl Default for PhysicsBody {
    fn default() -> Self {
        Self {
            kind: BodyKind::Dynamic,
            shape: CollisionShape::Auto,
            mass: 0.0,
            friction: default_friction(),
            restitution: 0.0,
            gravity_scale: default_gravity_scale(),
            is_sensor: false,
        }
    }
}

impl PhysicsBody {
    /// 动态盒子（半边长），常用于可推动的物体。
    pub fn dynamic_box(half_extents: [f32; 3]) -> Self {
        Self {
            shape: CollisionShape::Cuboid { half_extents },
            ..Default::default()
        }
    }

    /// 静态盒子 —— 地面 / 墙。
    pub fn static_box(half_extents: [f32; 3]) -> Self {
        Self {
            kind: BodyKind::Static,
            shape: CollisionShape::Cuboid { half_extents },
            ..Default::default()
        }
    }

    /// 动态球体。
    pub fn dynamic_ball(radius: f32) -> Self {
        Self {
            shape: CollisionShape::Ball { radius },
            ..Default::default()
        }
    }

    /// 运动学胶囊体 —— 角色控制器的载体。
    pub fn kinematic_capsule(half_height: f32, radius: f32) -> Self {
        Self {
            kind: BodyKind::Kinematic,
            shape: CollisionShape::Capsule { half_height, radius },
            ..Default::default()
        }
    }

    /// 这个实体是否会移动（推导：静态体不需要每帧回写变换）。
    pub fn is_static(&self) -> bool {
        matches!(self.kind, BodyKind::Static)
    }

    /// 是否会响应代码施加的速度（运动学或动态）。
    pub fn is_movable(&self) -> bool {
        !self.is_static()
    }
}

impl CollisionShape {
    /// `Auto` 形状的等效半边长（由实体缩放推导时用）。
    ///
    /// 返回 `None` 表示形状不依赖实体缩放（球 / 胶囊 / 圆柱以自身参数为准）。
    pub fn auto_half_extents(scale: [f32; 3]) -> [f32; 3] {
        [
            scale[0].abs().max(1e-3) * 0.5,
            scale[1].abs().max(1e-3) * 0.5,
            scale[2].abs().max(1e-3) * 0.5,
        ]
    }

    pub fn is_auto(&self) -> bool {
        matches!(self, CollisionShape::Auto)
    }
}

/// 运行时组件：实体对应的物理刚体句柄。
///
/// 由运行时在建立物理世界时插进 `Entity::components`（那个容器 `#[serde(skip)]`，
/// 正好是"运行时才有"的东西该待的位置）。放在 core 里，是为了让**脚本层**
/// 能读到它 —— 脚本需要"把这个实体推到那个方向"，但没有它就找不到对应的刚体。
///
/// 句柄本身对 core 是不透明的 `u64`：core 不关心它背后是 rapier 还是别的后端。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicsHandle(pub u64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_dynamic_auto() {
        let b = PhysicsBody::default();
        assert_eq!(b.kind, BodyKind::Dynamic);
        assert!(b.shape.is_auto());
        assert!(b.is_movable());
    }

    #[test]
    fn static_box_is_immovable() {
        let b = PhysicsBody::static_box([2.0, 0.1, 2.0]);
        assert!(b.is_static());
        assert!(!b.is_movable());
    }

    #[test]
    fn serde_roundtrip_keeps_defaults_compact() {
        // 只写 kind，其它字段应落到默认值。
        let text = r#"(kind: Static)"#;
        let body: PhysicsBody = ron::from_str(text).unwrap();
        assert_eq!(body.kind, BodyKind::Static);
        assert_eq!(body.friction, 0.5);
        assert_eq!(body.gravity_scale, 1.0);
    }

    #[test]
    fn auto_half_extents_follows_scale() {
        let h = CollisionShape::auto_half_extents([4.0, 0.2, 4.0]);
        assert_eq!(h, [2.0, 0.1, 2.0]);
    }
}
