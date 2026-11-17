//! AABB 碰撞求解：动态刚体两两分离 / 动态 vs 静态推出，并在接触时**清掉接近速度**。
//!
//! 不清速度的话，被推出来的盒子下一帧又带着累积的向下速度砸回去：位置在接触面上
//! 来回抖、堆叠永远静不下来（实测贴地后速度仍每帧 +0.16 m/s）。

use xgc_base::{EntityId, EntityKey};
use xgc_world::{Component, PhysicsBody, Transform, Velocity, World};

use crate::{System, SystemAccess};

/// AABB 碰撞：动态刚体两两分离，最小穿透轴推出；static 不动。
#[derive(Clone, Copy, Debug, Default)]
pub struct AabbCollisionSystem;

struct BodyBox {
    key: EntityKey,
    id: EntityId,
    position: [f32; 3],
    /// 动态刚体的线速度（静态为 `None`）。接触求解会沿分离轴把它清零。
    velocity: Option<[f32; 3]>,
    half: [f32; 3],
    sphere: Option<f32>,
    dynamic: bool,
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
        let mut bodies = Vec::new();
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
                dynamic: body.kind == "dynamic",
            });
        });
        bodies.sort_by_key(|body| body.id.raw());
        for i in 0..bodies.len() {
            for j in (i + 1)..bodies.len() {
                if !bodies[i].dynamic && !bodies[j].dynamic {
                    continue;
                }
                if let (Some(ra), Some(rb)) = (bodies[i].sphere, bodies[j].sphere) {
                    let (left, right) = bodies.split_at_mut(j);
                    resolve_spheres(&mut left[i], &mut right[0], ra, rb);
                    continue;
                }
                let Some((axis, penetration)) = overlap(&bodies[i], &bodies[j]) else {
                    continue;
                };
                let direction =
                    if (bodies[i].position[axis] - bodies[j].position[axis]).abs() < f32::EPSILON {
                        1.0
                    } else {
                        (bodies[i].position[axis] - bodies[j].position[axis]).signum()
                    };
                match (bodies[i].dynamic, bodies[j].dynamic) {
                    (true, true) => {
                        bodies[i].position[axis] += direction * penetration * 0.5;
                        bodies[j].position[axis] -= direction * penetration * 0.5;
                        stop_along(&mut bodies[i], axis);
                        stop_along(&mut bodies[j], axis);
                    }
                    (true, false) => {
                        bodies[i].position[axis] += direction * penetration;
                        stop_along(&mut bodies[i], axis);
                    }
                    (false, true) => {
                        bodies[j].position[axis] -= direction * penetration;
                        stop_along(&mut bodies[j], axis);
                    }
                    (false, false) => {}
                }
            }
        }
        for body in &bodies {
            if let Some(transform) = world.get_mut::<Transform>(body.key) {
                transform.position = body.position;
            }
            if let (Some(velocity), Some(linear)) =
                (world.get_mut::<Velocity>(body.key), body.velocity)
            {
                velocity.linear = linear;
            }
        }
    }
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

fn resolve_spheres(a: &mut BodyBox, b: &mut BodyBox, ra: f32, rb: f32) {
    let delta = [
        a.position[0] - b.position[0],
        a.position[1] - b.position[1],
        a.position[2] - b.position[2],
    ];
    let distance = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
    let minimum = ra + rb;
    if distance >= minimum {
        return;
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
