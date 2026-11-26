//! AABB 碰撞求解：动态刚体两两分离 / 动态 vs 静态推出，并在接触时**清掉接近速度**。
//!
//! 不清速度的话，被推出来的盒子下一帧又带着累积的向下速度砸回去：位置在接触面上
//! 来回抖、堆叠永远静不下来（实测贴地后速度仍每帧 +0.16 m/s）。

mod broad_phase;

use self::broad_phase::BroadPhase;
use xgc_base::{EntityId, EntityKey};
use xgc_world::{CollisionEvent, Component, PhysicsBody, Transform, Velocity, World};

use crate::{System, SystemAccess};

/// AABB 碰撞：动态刚体两两分离，最小穿透轴推出；static 不动。
///
/// 接触对跨帧保存，用来产出 `on_collision_enter/exit`：只在开始 / 结束那帧
/// 各发一次，持续接触不刷新。传感器只记录接触、不做分离（rapier 的 sensor
/// 也不挡路，AABB 不能比它更硬）。
#[derive(Clone, Debug, Default)]
pub struct AabbCollisionSystem {
    bodies: Vec<BodyBox>,
    broad_phase: BroadPhase,
    previous: Vec<(u64, u64)>,
    current: Vec<(u64, u64)>,
}

#[derive(Clone, Debug)]
struct BodyBox {
    key: EntityKey,
    id: EntityId,
    position: [f32; 3],
    /// 动态刚体的线速度（静态为 `None`）。接触求解会沿分离轴把它清零。
    velocity: Option<[f32; 3]>,
    half: [f32; 3],
    sphere: Option<f32>,
    dynamic: bool,
    sensor: bool,
}

impl System for AabbCollisionSystem {
    fn name(&self) -> &'static str {
        "aabb"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![PhysicsBody::ID],
            writes: vec![Transform::ID, Velocity::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        self.current.clear();
        let bodies = &mut self.bodies;
        bodies.clear();
        world.for_each2::<Transform, PhysicsBody>(|key, transform, body| {
            let Some(entity) = world.entity(key) else {
                return;
            };
            let velocity = world.get::<Velocity>(key).map(|velocity| velocity.linear);
            bodies.push(BodyBox {
                key,
                id: entity,
                position: transform.position,
                velocity,
                half: box_half(body),
                sphere: (body.shape == "ball").then_some(body.radius),
                dynamic: body.is_dynamic(),
                sensor: body.is_sensor,
            });
        });
        // ID 唯一，原地排序可保留求解顺序并省去排序临时区。
        bodies.sort_unstable_by_key(|body| body.id.raw());
        solve_contacts(bodies, &mut self.broad_phase, &mut self.current);
        for body in bodies.iter() {
            if let Some(transform) = world.get_mut::<Transform>(body.key) {
                transform.position = body.position;
            }
            if let (Some(velocity), Some(linear)) =
                (world.get_mut::<Velocity>(body.key), body.velocity)
            {
                velocity.linear = linear;
            }
        }

        self.current.sort_unstable();
        self.current.dedup();
        let previous = std::mem::take(&mut self.previous);
        let current = std::mem::take(&mut self.current);
        emit_contacts(world, &previous, &current);
        // 两块缓冲互换：上一帧的 current 变成下一帧写接触的 scratch，避免每帧分配。
        self.previous = current;
        self.current = previous;
        self.current.clear();
    }
}

fn solve_contacts(
    bodies: &mut [BodyBox],
    broad_phase: &mut BroadPhase,
    pairs: &mut Vec<(u64, u64)>,
) {
    const MAX_SPATIAL_QUERIES: usize = 4;
    let body_count = bodies.len();
    if body_count < 2 || !bodies.iter().any(|body| body.dynamic) {
        return;
    }
    broad_phase.rebuild(bodies);
    for i in 0..body_count - 1 {
        let dynamic = bodies[i].dynamic;
        broad_phase.find_after(i, i, dynamic);
        let mut queries = 1;
        let mut scan_suffix = false;
        let mut next = 0;
        loop {
            let candidate = if scan_suffix {
                (next < body_count).then_some(next)
            } else {
                broad_phase.candidates.get(next).copied()
            };
            let Some(j) = candidate else { break };
            next += 1;
            let (before, after) = bodies.split_at_mut(j);
            let left = &mut before[i];
            let right = &mut after[0];
            let left_position = left.position.map(f32::to_bits);
            let right_position = right.position.map(f32::to_bits);
            if !resolve_pair(left, right) {
                continue;
            }
            pairs.push((left.id.raw(), right.id.raw()));
            if right.position.map(f32::to_bits) != right_position {
                broad_phase.update(j, right);
            }
            if left.position.map(f32::to_bits) != left_position
                && broad_phase.update(i, left)
                && !scan_suffix
            {
                // 分离会把新接触带进范围。多次推出后按 ID 扫完余下对，避免在稠密处
                // 反复收集、排序同一段候选。只有动态的左体会移动，余下右体均可参与。
                if queries == MAX_SPATIAL_QUERIES {
                    debug_assert!(dynamic);
                    scan_suffix = true;
                    next = j + 1;
                } else {
                    broad_phase.find_after(i, j, dynamic);
                    queries += 1;
                    next = 0;
                }
            }
        }
    }
}

fn resolve_pair(left: &mut BodyBox, right: &mut BodyBox) -> bool {
    let separate = !left.sensor && !right.sensor;
    if let (Some(ra), Some(rb)) = (left.sphere, right.sphere) {
        return resolve_spheres(left, right, ra, rb, separate);
    }
    let Some((axis, penetration)) = overlap(left, right) else {
        return false;
    };
    if separate {
        let delta = left.position[axis] - right.position[axis];
        let direction = if delta.abs() < f32::EPSILON {
            1.0
        } else {
            delta.signum()
        };
        match (left.dynamic, right.dynamic) {
            (true, true) => {
                left.position[axis] += direction * penetration * 0.5;
                right.position[axis] -= direction * penetration * 0.5;
                stop_along(left, axis);
                stop_along(right, axis);
            }
            (true, false) => {
                left.position[axis] += direction * penetration;
                stop_along(left, axis);
            }
            (false, true) => {
                right.position[axis] -= direction * penetration;
                stop_along(right, axis);
            }
            (false, false) => {}
        }
    }
    true
}

/// 有向事件：每对接触给双方各发一条，脚本按自己的实体查即可。
pub(crate) fn emit_contacts(world: &mut World, previous: &[(u64, u64)], current: &[(u64, u64)]) {
    let (mut left, mut right) = (0, 0);
    while left < previous.len() || right < current.len() {
        match (previous.get(left), current.get(right)) {
            (Some(old), Some(new)) if old == new => {
                left += 1;
                right += 1;
            }
            (Some(old), Some(new)) if old < new => {
                push_pair(world, *old, false);
                left += 1;
            }
            (Some(_), Some(new)) => {
                push_pair(world, *new, true);
                right += 1;
            }
            (Some(old), None) => {
                push_pair(world, *old, false);
                left += 1;
            }
            (None, Some(new)) => {
                push_pair(world, *new, true);
                right += 1;
            }
            (None, None) => break,
        }
    }
}

fn push_pair(world: &mut World, pair: (u64, u64), enter: bool) {
    world.push_collision_event(CollisionEvent {
        entity: EntityId::from_raw(pair.0),
        other: EntityId::from_raw(pair.1),
        enter,
    });
    world.push_collision_event(CollisionEvent {
        entity: EntityId::from_raw(pair.1),
        other: EntityId::from_raw(pair.0),
        enter,
    });
}

/// 沿 `axis` 把动态刚体的接近速度清零（完全非弹性接触）。
///
/// 不清速度的话，被推出来的盒子下一帧又带着累积的向下速度砸回去：位置在
/// 接触面上来回抖、堆叠永远静不下来（实测贴地后速度仍每帧 +0.16 m/s）。
fn stop_along(body: &mut BodyBox, axis: usize) {
    if !body.dynamic {
        return;
    }
    if let Some(linear) = &mut body.velocity {
        linear[axis] = 0.0;
    }
}

fn resolve_spheres(a: &mut BodyBox, b: &mut BodyBox, ra: f32, rb: f32, separate: bool) -> bool {
    let delta = [
        a.position[0] - b.position[0],
        a.position[1] - b.position[1],
        a.position[2] - b.position[2],
    ];
    let distance = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
    let minimum = ra + rb;
    if distance >= minimum {
        return false;
    }
    if !separate {
        return true;
    }
    let normal = if distance > f32::EPSILON {
        [
            delta[0] / distance,
            delta[1] / distance,
            delta[2] / distance,
        ]
    } else {
        [1.0, 0.0, 0.0]
    };
    let penetration = minimum - distance;
    match (a.dynamic, b.dynamic) {
        (true, true) => {
            for (axis, normal) in normal.iter().enumerate() {
                a.position[axis] += normal * penetration * 0.5;
                b.position[axis] -= normal * penetration * 0.5;
            }
            stop_along_normal(a, &normal);
            stop_along_normal(b, &normal);
        }
        (true, false) => {
            for (axis, normal) in normal.iter().enumerate() {
                a.position[axis] += normal * penetration;
            }
            stop_along_normal(a, &normal);
        }
        (false, true) => {
            for (axis, normal) in normal.iter().enumerate() {
                b.position[axis] -= normal * penetration;
            }
            stop_along_normal(b, &normal);
        }
        (false, false) => {}
    }
    true
}

/// 只去掉沿法向的接近速度分量，切向（摩擦前）保留。
fn stop_along_normal(body: &mut BodyBox, normal: &[f32; 3]) {
    if !body.dynamic {
        return;
    }
    let Some(linear) = &mut body.velocity else {
        return;
    };
    let closing = linear[0] * normal[0] + linear[1] * normal[1] + linear[2] * normal[2];
    if closing < 0.0 {
        for (axis, normal) in normal.iter().enumerate() {
            linear[axis] -= normal * closing;
        }
    }
}

fn box_half(body: &PhysicsBody) -> [f32; 3] {
    match body.shape.as_str() {
        "ball" => [body.radius; 3],
        "capsule" => [body.radius, body.half_height + body.radius, body.radius],
        _ => body.half_extents,
    }
}

fn overlap(a: &BodyBox, b: &BodyBox) -> Option<(usize, f32)> {
    let mut penetration = [0.0f32; 3];
    for (axis, penetration) in penetration.iter_mut().enumerate() {
        *penetration = a.half[axis] + b.half[axis] - (a.position[axis] - b.position[axis]).abs();
        if *penetration <= 0.0 {
            return None;
        }
    }
    let mut best = 0;
    for axis in 1..3 {
        if penetration[axis] < penetration[best] {
            best = axis;
        }
    }
    Some((best, penetration[best]))
}
