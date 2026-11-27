//! 用场景快照正常建体后的形状解释实体对的几何重叠。

use rapier3d::{parry::query::intersection_test, prelude::RigidBodyType};
use xgc_base::EntityKey;
use xgc_world::{BodyKind, World};

use super::{BodyLink, RapierWorld};

#[derive(Clone, Copy, Debug)]
pub struct OverlapBody {
    pub kind: BodyKind,
    pub sensor: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct ColliderOverlap {
    pub body: Option<OverlapBody>,
    pub other_body: Option<OverlapBody>,
    /// 任一刚体未建成，或 Parry 不支持这对形状时为空。
    pub intersects: Option<bool>,
}

impl RapierWorld {
    /// 仅按当前快照为指定的两个 `physics.body` 实体建体。结果只描述初始几何，
    /// 因此运行中保留的刚体位姿、接触缓存和求解结果需要从仿真轨迹读取。
    pub fn snapshot_overlap(world: &World, body: EntityKey, other: EntityKey) -> ColliderOverlap {
        let mut rapier = Self::new();
        let body = rapier.insert_body(world, body);
        let other = rapier.insert_body(world, other);
        let collider = |link: BodyLink| &rapier.colliders[link.collider];
        let observe = |link: BodyLink| OverlapBody {
            kind: match rapier.bodies[link.body].body_type() {
                RigidBodyType::Fixed => BodyKind::Static,
                RigidBodyType::Dynamic => BodyKind::Dynamic,
                RigidBodyType::KinematicPositionBased | RigidBodyType::KinematicVelocityBased => {
                    BodyKind::Kinematic
                }
            },
            sensor: collider(link).is_sensor(),
        };
        let intersects = match (body, other) {
            (Some(a), Some(b)) => {
                let a = collider(a);
                let b = collider(b);
                // 有父节点的 Auto 碰撞体也用实际建成的形状与绝对位姿。
                intersection_test(a.position(), a.shape(), b.position(), b.shape()).ok()
            }
            _ => None,
        };
        ColliderOverlap {
            body: body.map(observe),
            other_body: other.map(observe),
            intersects,
        }
    }
}
