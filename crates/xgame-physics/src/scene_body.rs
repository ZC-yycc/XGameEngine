//! 把场景里声明的物理属性（[`xgame_core::PhysicsBody`]）翻译成 rapier 能用的
//! [`RigidBody`] + [`Collider`]。
//!
//! 翻译放在这里而不是 core，是因为 core 不该认识 rapier；放在这里而不是 runtime，
//! 是因为"声明 → 刚体"的映射规则属于物理层自己的知识，编辑器预览、独立游戏、
//! 未来的物理调试工具都应该共用同一份规则 —— 否则三处各写一遍，
//! 迟早出现"编辑器里能站住、游戏里穿模"这种前后不一致。

use glam::Vec3;
use xgame_core::physics::{BodyKind, CollisionShape, PhysicsBody};
use xgame_math::Transform;

use crate::collider::{Collider, ColliderShape};
use crate::rigid_body::{RigidBody, RigidBodyType};

/// 按场景声明生成一对 (刚体, 碰撞体)。
///
/// `world` 是实体的**世界变换**：碰撞体的尺寸取自它（`Auto` 形状直接读缩放），
/// 刚体的初始位置也取自它。传局部变换会让子物体在建体的一瞬间就掉到错误位置。
pub fn body_from_scene(def: &PhysicsBody, world: Transform) -> (RigidBody, Collider) {
    let body_type = match def.kind {
        BodyKind::Static => RigidBodyType::Static,
        BodyKind::Dynamic => RigidBodyType::Dynamic,
        BodyKind::Kinematic => RigidBodyType::KinematicVelocity,
    };

    let shape = collider_shape(def.shape, world.scale);
    let collider = Collider {
        shape,
        friction: def.friction,
        restitution: def.restitution,
        is_sensor: def.is_sensor,
        density: 1.0,
        offset: Vec3::ZERO,
        // 层不在这里决定：`PhysicsBody` 是纯物理声明，层属于实体（`Entity.layer`），
        // 由运行时在建体时按项目矩阵算好再写进来（`World::sync_bodies`）。
        // 这里给全通，保证"不配层"的老场景行为与以前一致。
        memberships: u32::MAX,
        filter: u32::MAX,
    };

    let rigid_body = RigidBody {
        body_type,
        transform: Transform {
            position: world.position,
            rotation: world.rotation,
            // rapier 的刚体没有缩放概念；缩放已经体现在碰撞体尺寸里了。
            scale: Vec3::ONE,
        },
        linear_velocity: Vec3::ZERO,
        angular_velocity: Vec3::ZERO,
        mass: def.mass,
        angular_damping: 0.05,
        linear_damping: 0.0,
        gravity_enabled: def.kind == BodyKind::Dynamic,
        gravity_scale: def.gravity_scale,
        can_sleep: true,
    };

    (rigid_body, collider)
}

/// 场景形状 → rapier 碰撞体形状。
fn collider_shape(shape: CollisionShape, scale: Vec3) -> ColliderShape {
    match shape {
        CollisionShape::Cuboid { half_extents } => ColliderShape::Cuboid {
            half_extents: Vec3::from_array(half_extents),
        },
        CollisionShape::Ball { radius } => ColliderShape::Ball { radius },
        CollisionShape::Capsule { half_height, radius } => {
            ColliderShape::Capsule { half_height, radius }
        }
        CollisionShape::Cylinder { half_height, radius } => {
            ColliderShape::Cylinder { half_height, radius }
        }
        // `Auto`：用实体缩放的绝对值当边长。
        //
        // 取**绝对值**而不是原值：负缩放是镜像（很常见的一种"翻转模型"技巧），
        // 直接拿去建盒子会得到一个行列式为负的畸形碰撞体 —— rapier 对负数
        // 半边长没有明确行为保障。
        CollisionShape::Auto => {
            let h = CollisionShape::auto_half_extents(scale.to_array());
            ColliderShape::Cuboid {
                half_extents: Vec3::from_array(h),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_box_keeps_world_pose() {
        let def = PhysicsBody::static_box([2.0, 0.1, 2.0]);
        let world = Transform::from_translation(Vec3::new(1.0, -0.5, 2.0));
        let (rb, col) = body_from_scene(&def, world);

        assert_eq!(rb.body_type, RigidBodyType::Static);
        assert_eq!(rb.transform.position, Vec3::new(1.0, -0.5, 2.0));
        assert!(!rb.gravity_enabled, "静态体不该吃重力");
        assert_eq!(col.friction, 0.5);
        match col.shape {
            ColliderShape::Cuboid { half_extents } => {
                assert_eq!(half_extents, Vec3::new(2.0, 0.1, 2.0));
            }
            other => panic!("期望 Cuboid，得到 {other:?}"),
        }
    }

    #[test]
    fn auto_shape_takes_absolute_scale() {
        let def = PhysicsBody::default();
        let mut world = Transform::IDENTITY;
        world.scale = Vec3::new(-4.0, 0.2, 4.0);
        let (_, col) = body_from_scene(&def, world);
        match col.shape {
            ColliderShape::Cuboid { half_extents } => {
                assert_eq!(half_extents, Vec3::new(2.0, 0.1, 2.0));
            }
            other => panic!("期望 Cuboid，得到 {other:?}"),
        }
    }

    #[test]
    fn dynamic_body_is_gravity_driven() {
        let (rb, _) = body_from_scene(&PhysicsBody::dynamic_ball(0.5), Transform::IDENTITY);
        assert_eq!(rb.body_type, RigidBodyType::Dynamic);
        assert!(rb.gravity_enabled);
        assert_eq!(rb.gravity_scale, 1.0);
    }

    #[test]
    fn sensor_flag_survives_translation() {
        let def = PhysicsBody {
            is_sensor: true,
            ..PhysicsBody::default()
        };
        let (_, col) = body_from_scene(&def, Transform::IDENTITY);
        assert!(col.is_sensor);
    }
}
