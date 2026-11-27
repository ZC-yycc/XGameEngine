//! 普通刚体和坡面发布 Rapier 窄相边沿，角色继续使用自身查询的传感器接触对。

use std::{collections::HashMap, sync::Mutex};

use rapier3d::prelude::{
    ColliderHandle, ColliderSet, CollisionEvent as RapierCollisionEvent, ContactPair, EventHandler,
    RigidBodySet,
};
use xgc_base::EntityId;
use xgc_world::{CollisionEvent, World};

#[derive(Default)]
pub(super) struct ContactEvents {
    entities: HashMap<ColliderHandle, EntityId>,
    // prepare 也可能拆刚体，Rapier 直到 step 才发出旧句柄的退出。
    retired: Vec<ColliderHandle>,
    pending: Mutex<Vec<ContactEdge>>,
}

struct ContactEdge {
    pair: (EntityId, EntityId),
    sequence: usize,
    enter: bool,
    removed: bool,
}

impl ContactEvents {
    pub(super) fn register(&mut self, handle: ColliderHandle, entity: EntityId) {
        self.entities.insert(handle, entity);
    }

    pub(super) fn retire(&mut self, handle: ColliderHandle) {
        self.retired.push(handle);
    }

    pub(super) fn publish(&mut self, world: &mut World) {
        let pending = self.pending.get_mut().expect("接触收集锁已中毒");
        // 对与对之间按持久 ID 排序，同一对保留 Rapier 的边沿顺序。
        pending.sort_unstable_by_key(|edge| (edge.pair, edge.sequence));
        let mut edges = pending.drain(..).peekable();
        while let Some(edge) = edges.next() {
            // 每个实体只登记一个当前碰撞体，删除后同一对的进入来自新句柄。
            // 持续接触的换代对脚本仍是同一对，抵消这两个结构性边沿。
            if edge.removed
                && edges
                    .next_if(|next| next.pair == edge.pair && next.enter)
                    .is_some()
            {
                continue;
            }
            emit_pair(world, edge.pair, edge.enter);
        }
        for handle in self.retired.drain(..) {
            self.entities.remove(&handle);
        }
    }
}

impl EventHandler for ContactEvents {
    fn handle_collision_event(
        &self,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        event: RapierCollisionEvent,
        _contact_pair: Option<&ContactPair>,
    ) {
        // 完整 ColliderHandle 含代际。角色不登记，避免同一事件由角色路径重发。
        let (Some(&a), Some(&b)) = (
            self.entities.get(&event.collider1()),
            self.entities.get(&event.collider2()),
        ) else {
            return;
        };
        if a == b {
            return;
        }
        let mut pending = self.pending.lock().expect("接触收集锁已中毒");
        let sequence = pending.len();
        pending.push(ContactEdge {
            pair: (a.min(b), a.max(b)),
            sequence,
            enter: event.started(),
            removed: event.removed(),
        });
    }

    fn handle_contact_force_event(
        &self,
        _dt: f32,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        _contact_pair: &ContactPair,
        _total_force_magnitude: f32,
    ) {
    }
}

/// 角色查询只返回当前接触，和上一步的有序接触对做差才得到进入和退出。
pub(super) fn emit_sensor_contacts(
    world: &mut World,
    previous: &[(u64, u64)],
    current: &[(u64, u64)],
) {
    let (mut left, mut right) = (0, 0);
    while left < previous.len() || right < current.len() {
        let (pair, enter) = match (previous.get(left), current.get(right)) {
            (Some(old), Some(new)) if old == new => {
                left += 1;
                right += 1;
                continue;
            }
            (Some(old), Some(new)) if old < new => {
                left += 1;
                (*old, false)
            }
            (_, Some(new)) => {
                right += 1;
                (*new, true)
            }
            (Some(old), None) => {
                left += 1;
                (*old, false)
            }
            (None, None) => break,
        };
        emit_pair(
            world,
            (EntityId::from_raw(pair.0), EntityId::from_raw(pair.1)),
            enter,
        );
    }
}

fn emit_pair(world: &mut World, (a, b): (EntityId, EntityId), enter: bool) {
    world.push_collision_event(CollisionEvent {
        entity: a,
        other: b,
        enter,
    });
    world.push_collision_event(CollisionEvent {
        entity: b,
        other: a,
        enter,
    });
}
