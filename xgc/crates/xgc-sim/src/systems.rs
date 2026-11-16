use std::collections::{HashMap, HashSet};

use xgc_base::{EntityId, EntityKey};
use xgc_world::{
    Animator, CharacterController, Component, GlobalTransform, Parent, PhysicsBody, Transform,
    Value, Velocity, World,
};

use super::{System, SystemAccess};

#[derive(Clone, Copy, Debug)]
pub struct PhysicsSystem {
    pub gravity: [f32; 3],
}

impl Default for PhysicsSystem {
    fn default() -> Self {
        Self {
            gravity: [0.0, -9.81, 0.0],
        }
    }
}

impl System for PhysicsSystem {
    fn name(&self) -> &'static str {
        "physics"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![PhysicsBody::ID],
            writes: vec![Velocity::ID],
        }
    }

    fn run(&mut self, world: &mut World, dt: f32) {
        let gravity = self.gravity;
        world.for_each2_mut::<Velocity, PhysicsBody>(|_, velocity, body| {
            if body.kind != "dynamic" {
                return;
            }
            for (axis, value) in gravity.iter().enumerate() {
                velocity.linear[axis] += value * body.gravity_scale * dt;
            }
        });
    }
}

/// 地面碰撞：dynamic 刚体落到 ground_y 时夹住位置并清掉向下速度。
#[derive(Clone, Copy, Debug)]
pub struct GroundCollisionSystem {
    pub ground_y: f32,
}

impl Default for GroundCollisionSystem {
    fn default() -> Self {
        Self { ground_y: 0.0 }
    }
}

impl System for GroundCollisionSystem {
    fn name(&self) -> &'static str {
        "ground"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![PhysicsBody::ID],
            writes: vec![Transform::ID, Velocity::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        let ground = self.ground_y;
        world.for_each3_mut::<Transform, Velocity, PhysicsBody>(|_, transform, velocity, body| {
            if body.kind != "dynamic" {
                return;
            }
            let half = support_half(body);
            if transform.position[1] - half < ground {
                transform.position[1] = ground + half;
                if velocity.linear[1] < 0.0 {
                    velocity.linear[1] = 0.0;
                }
            }
        });
    }
}

fn support_half(body: &PhysicsBody) -> f32 {
    match body.shape.as_str() {
        "ball" => body.radius,
        "capsule" => body.half_height + body.radius,
        _ => body.half_extents[1],
    }
}

/// 角色起跳：消费 `jump` 请求，踩地时写入竖直速度。
#[derive(Clone, Copy, Debug, Default)]
pub struct CharacterJumpSystem;

impl System for CharacterJumpSystem {
    fn name(&self) -> &'static str {
        "character_jump"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![CharacterController::ID],
            writes: vec![Velocity::ID, CharacterController::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        world.for_each2_mut::<Velocity, CharacterController>(|_, velocity, controller| {
            if controller.jump {
                if controller.grounded {
                    velocity.linear[1] = controller.jump_velocity;
                    controller.grounded = false;
                }
                controller.jump = false;
            }
        });
    }
}

/// 角色贴地：角色原点在脚底，落到 y=0 时夹住并清掉向下速度。
#[derive(Clone, Copy, Debug, Default)]
pub struct CharacterSystem;

impl System for CharacterSystem {
    fn name(&self) -> &'static str {
        "character"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![CharacterController::ID],
            writes: vec![Transform::ID, Velocity::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        world.for_each3_mut::<Transform, Velocity, CharacterController>(
            |_, transform, velocity, _controller| {
                if transform.position[1] < 0.0 {
                    transform.position[1] = 0.0;
                    if velocity.linear[1] < 0.0 {
                        velocity.linear[1] = 0.0;
                    }
                }
                _controller.grounded = transform.position[1] <= 0.0;
            },
        );
    }
}

/// AABB 碰撞：动态刚体两两分离，最小穿透轴推出；static 不动。
#[derive(Clone, Copy, Debug, Default)]
pub struct AabbCollisionSystem;

struct BodyBox {
    key: EntityKey,
    id: EntityId,
    position: [f32; 3],
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
            writes: vec![Transform::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        let mut bodies = Vec::new();
        world.for_each2::<Transform, PhysicsBody>(|key, transform, body| {
            let Some(entity) = world.entity(key) else {
                return;
            };
            bodies.push(BodyBox {
                key,
                id: entity,
                position: transform.position,
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
                    }
                    (true, false) => {
                        bodies[i].position[axis] += direction * penetration;
                    }
                    (false, true) => {
                        bodies[j].position[axis] -= direction * penetration;
                    }
                    (false, false) => {}
                }
            }
        }
        for body in &bodies {
            if let Some(transform) = world.get_mut::<Transform>(body.key) {
                transform.position = body.position;
            }
        }
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
        }
        (true, false) => {
            for (axis, normal) in normal.iter().enumerate() {
                a.position[axis] += normal * penetration;
            }
        }
        (false, true) => {
            for (axis, normal) in normal.iter().enumerate() {
                b.position[axis] -= normal * penetration;
            }
        }
        (false, false) => {}
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

/// 动画时钟：自动播放的 Animator 按 speed 推进，duration 有效时循环 / 截断。
#[derive(Clone, Copy, Debug, Default)]
pub struct AnimatorSystem;

impl System for AnimatorSystem {
    fn name(&self) -> &'static str {
        "animation"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: Vec::new(),
            writes: vec![Animator::ID],
        }
    }

    fn run(&mut self, world: &mut World, dt: f32) {
        world.for_each_mut::<Animator>(|_, animator| {
            if !animator.autoplay {
                return;
            }
            animator.time += animator.speed * dt;
            if animator.duration > 0.0 {
                if animator.looping {
                    animator.time = animator.time.rem_euclid(animator.duration);
                } else {
                    animator.time = animator.time.min(animator.duration);
                }
            }
        });
    }
}

/// 第一个内建系统：`position += linear * dt`。
#[derive(Clone, Copy, Debug)]
pub struct VelocitySystem {
    pub threads: usize,
}

impl Default for VelocitySystem {
    fn default() -> Self {
        Self { threads: 1 }
    }
}

impl VelocitySystem {
    pub fn new(threads: usize) -> Self {
        Self {
            threads: threads.max(1),
        }
    }
}

impl System for VelocitySystem {
    fn name(&self) -> &'static str {
        "velocity"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![Velocity::ID],
            writes: vec![Transform::ID],
        }
    }

    fn run(&mut self, world: &mut World, dt: f32) {
        world.par_for_each2_mut::<Transform, Velocity>(self.threads, |_, transform, velocity| {
            for axis in 0..3 {
                transform.position[axis] += velocity.linear[axis] * dt;
            }
        });
    }
}

/// 第二个内建系统：给速度加固定重力；只写 Velocity，不读其它组件。
#[derive(Clone, Copy, Debug)]
pub struct GravitySystem {
    pub gravity: [f32; 3],
}

impl Default for GravitySystem {
    fn default() -> Self {
        Self {
            gravity: [0.0, -9.81, 0.0],
        }
    }
}

impl GravitySystem {
    pub fn new(gravity: [f32; 3]) -> Self {
        Self { gravity }
    }
}

impl System for GravitySystem {
    fn name(&self) -> &'static str {
        "gravity"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: Vec::new(),
            writes: vec![Velocity::ID],
        }
    }

    fn run(&mut self, world: &mut World, dt: f32) {
        let gravity = self.gravity;
        world.for_each_mut::<Velocity>(|_, velocity| {
            for (axis, value) in gravity.iter().enumerate() {
                velocity.linear[axis] += *value * dt;
            }
        });
    }
}

/// 层级系统：按 `Parent` 链把局部 `Transform` 合成 `GlobalTransform`。
/// 层级系统：按 `Parent` 链把局部 `Transform` 合成 `GlobalTransform`。
///
/// 没有 `Parent`、或父节点不带 `GlobalTransform` 的实体按根处理；环里的实体
/// 保持原值，不阻塞其它节点。节点 / 存在集 / 结果表跨帧复用，稳态不再分配。
#[derive(Default)]
pub struct HierarchySystem {
    nodes: Vec<HierarchyNode>,
    present: HashSet<EntityId>,
    globals: HashMap<EntityId, GlobalTransform>,
}

struct HierarchyNode {
    entity: EntityId,
    parent: Option<EntityId>,
    local: Transform,
}

impl System for HierarchySystem {
    fn name(&self) -> &'static str {
        "hierarchy"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![Parent::ID, Transform::ID],
            writes: vec![GlobalTransform::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        self.nodes.clear();
        self.present.clear();
        self.globals.clear();

        world.for_each::<GlobalTransform>(|key, _| {
            let Some(entity) = world.entity(key) else {
                return;
            };
            self.nodes.push(HierarchyNode {
                entity,
                parent: world.get::<Parent>(key).map(|parent| parent.entity),
                local: world.get::<Transform>(key).copied().unwrap_or_default(),
            });
        });

        for node in &self.nodes {
            self.present.insert(node.entity);
        }
        for node in &self.nodes {
            if node
                .parent
                .is_none_or(|parent| !self.present.contains(&parent))
            {
                self.globals
                    .insert(node.entity, GlobalTransform::from_local(&node.local));
            }
        }

        let mut changed = true;
        while changed {
            changed = false;
            for node in &self.nodes {
                if self.globals.contains_key(&node.entity) {
                    continue;
                }
                let Some(parent) = node.parent else {
                    continue;
                };
                let Some(parent_global) = self.globals.get(&parent) else {
                    continue;
                };
                self.globals.insert(
                    node.entity,
                    GlobalTransform::compose(parent_global, &node.local),
                );
                changed = true;
            }
        }

        for (entity, global) in &self.globals {
            let _ = world.set_field(
                *entity,
                GlobalTransform::ID,
                "position",
                &Value::Vec3([
                    f64::from(global.position[0]),
                    f64::from(global.position[1]),
                    f64::from(global.position[2]),
                ]),
            );
            let _ = world.set_field(
                *entity,
                GlobalTransform::ID,
                "rotation",
                &Value::Quat([
                    f64::from(global.rotation[0]),
                    f64::from(global.rotation[1]),
                    f64::from(global.rotation[2]),
                    f64::from(global.rotation[3]),
                ]),
            );
            let _ = world.set_field(
                *entity,
                GlobalTransform::ID,
                "scale",
                &Value::Vec3([
                    f64::from(global.scale[0]),
                    f64::from(global.scale[1]),
                    f64::from(global.scale[2]),
                ]),
            );
        }
    }
}
