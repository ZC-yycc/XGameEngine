//! Rapier 刚体后端：与旧引擎同一套求解器与参数（rapier3d 0.22 + `simd-stable`）。
//!
//! 为什么不用自研简化碰撞：割接要求**同帧状态可比**。旧端用 rapier 的默认
//! `IntegrationParameters`、`gravity = (0, -9.81, 0)`、每固定帧 `step(fixed_dt)`
//! 一次；这里逐项照搬，落点与翻滚姿态才有机会和旧图对上（简化 AABB 求解器
//! 没有角动量，盒子永远不会翻滚）。
//!
//! 这一刀的范围：只接管 `physics.body` 实体（静态 / 动态 / 运动学）。
//! 角色控制器、斜坡与脚本驱动的通用速度仍走 `xgc-sim` 的既有系统。

use rapier3d::na::{Isometry3, Translation3, UnitQuaternion, Vector3};
use rapier3d::prelude::*;

use xgc_base::EntityKey;
use xgc_world::{Component, GlobalTransform, PhysicsBody, Transform, Velocity, World};

use crate::{System, SystemAccess};

/// 旧引擎 `PhysicsConfig::default()` 的重力。
pub const GRAVITY: [f32; 3] = [0.0, -9.81, 0.0];
/// 旧引擎 `PhysicsConfig::default()` 的单步时长；`Simulation` 也用它。
pub const FIXED_DT: f32 = 1.0 / 60.0;

struct BodyLink {
    key: EntityKey,
    body: RigidBodyHandle,
    dynamic: bool,
    kinematic: bool,
}

/// 一套 rapier 世界 + 实体映射。
pub struct RapierWorld {
    bodies: RigidBodySet,
    colliders: ColliderSet,
    integration: IntegrationParameters,
    pipeline: PhysicsPipeline,
    islands: IslandManager,
    broad_phase: DefaultBroadPhase,
    narrow_phase: NarrowPhase,
    query: QueryPipeline,
    ccd: CCDSolver,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    links: Vec<BodyLink>,
    /// 已建体的 `physics.body` 数量；与当前世界不一致时整表重建。
    built_count: Option<usize>,
}

impl Default for RapierWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl RapierWorld {
    pub fn new() -> Self {
        Self {
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            integration: IntegrationParameters::default(),
            pipeline: PhysicsPipeline::new(),
            islands: IslandManager::new(),
            broad_phase: DefaultBroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            query: QueryPipeline::new(),
            ccd: CCDSolver::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            links: Vec::new(),
            built_count: None,
        }
    }

    /// 本帧实体集合若与建体时不同就整表重建（演示场景够用；增量增删留给后续切片）。
    pub fn ensure_built(&mut self, world: &World) {
        let mut count = 0usize;
        world.for_each::<PhysicsBody>(|_, _| count += 1);
        if self.built_count == Some(count) {
            return;
        }
        self.rebuild(world, count);
    }

    fn rebuild(&mut self, world: &World, count: usize) {
        self.bodies = RigidBodySet::new();
        self.colliders = ColliderSet::new();
        self.links.clear();
        self.built_count = Some(count);
        // 按实体 ID 排序建体：rapier 的岛屿构建顺序与插入顺序有关，固定顺序
        // 才能保证同一份场景两次运行逐位一致。
        let mut entries = Vec::new();
        world.for_each::<PhysicsBody>(|key, body| {
            if let Some(id) = world.entity(key) {
                entries.push((id, key, body.clone()));
            }
        });
        entries.sort_by_key(|(id, _, _)| id.raw());
        for (_, key, def) in entries {
            let Some(transform) = world.get::<Transform>(key).copied() else {
                continue;
            };
            let global = world
                .get::<GlobalTransform>(key)
                .copied()
                .unwrap_or(GlobalTransform {
                    position: transform.position,
                    rotation: transform.rotation,
                    scale: transform.scale,
                });
            let (body, collider) = build_body(&def, &transform, &global);
            let handle = self.bodies.insert(body);
            self.colliders
                .insert_with_parent(collider, handle, &mut self.bodies);
            self.links.push(BodyLink {
                key,
                body: handle,
                dynamic: def.kind == "dynamic",
                kinematic: def.kind == "kinematic",
            });
        }
    }

    /// 推进一个固定步，并把动态体结果写回世界。
    pub fn step(&mut self, world: &mut World, dt: f32) {
        self.ensure_built(world);
        // 运动学体：脚本摆好的位置每步推给物理（与旧端 `push_kinematic_bodies` 同义）。
        for link in &self.links {
            if !link.kinematic {
                continue;
            }
            let Some(transform) = world.get::<Transform>(link.key).copied() else {
                continue;
            };
            if let Some(body) = self.bodies.get_mut(link.body) {
                body.set_position(isometry(&transform), true);
            }
        }

        self.integration.dt = if dt.is_finite() && dt > 0.0 {
            dt
        } else {
            FIXED_DT
        };
        let gravity = Vector3::new(GRAVITY[0], GRAVITY[1], GRAVITY[2]);
        self.pipeline.step(
            &gravity,
            &self.integration,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.ccd,
            Some(&mut self.query),
            &(),
            &(),
        );

        for link in &self.links {
            if !link.dynamic {
                continue;
            }
            let Some(body) = self.bodies.get(link.body) else {
                continue;
            };
            let position = body.position();
            let rotation = position.rotation;
            let linear = body.linvel();
            if let Some(transform) = world.get_mut::<Transform>(link.key) {
                transform.position = [
                    position.translation.x,
                    position.translation.y,
                    position.translation.z,
                ];
                transform.rotation = [rotation.i, rotation.j, rotation.k, rotation.w];
            }
            if let Some(velocity) = world.get_mut::<Velocity>(link.key) {
                velocity.linear = [linear.x, linear.y, linear.z];
            }
        }
    }
}

/// 场景声明 → rapier 刚体 + 碰撞体；逐项对齐旧端 `body_from_scene`。
fn build_body(
    def: &PhysicsBody,
    local: &Transform,
    global: &GlobalTransform,
) -> (RigidBody, Collider) {
    let body_type = match def.kind.as_str() {
        "static" => RigidBodyType::Fixed,
        "kinematic" => RigidBodyType::KinematicVelocityBased,
        _ => RigidBodyType::Dynamic,
    };
    let dynamic = body_type == RigidBodyType::Dynamic;
    let (shape, unit_mass) = collider_shape(def, global);
    // 旧端语义：场景 mass 是总质量，优先按"体积 × 密度"兑现；mass 为空 / 0 时
    // 密度取 1（几何质量）。角惯量始终由同一形状算出。
    let (density, extra_mass) = if dynamic && def.mass.is_finite() && def.mass > 0.0 {
        let density = def.mass / unit_mass;
        if unit_mass.is_finite() && unit_mass > 0.0 && density.is_finite() {
            (density, 0.0)
        } else {
            (1.0, def.mass)
        }
    } else {
        (1.0, 0.0)
    };
    let collider = ColliderBuilder::new(shape)
        .friction(def.friction)
        .restitution(def.restitution)
        .sensor(def.is_sensor)
        .density(density)
        .build();
    let iso = isometry(local);
    let body = match body_type {
        RigidBodyType::Fixed => RigidBodyBuilder::fixed(),
        RigidBodyType::Dynamic => RigidBodyBuilder::dynamic(),
        RigidBodyType::KinematicVelocityBased => RigidBodyBuilder::kinematic_velocity_based(),
        RigidBodyType::KinematicPositionBased => RigidBodyBuilder::kinematic_position_based(),
    }
    .position(iso)
    .linear_damping(0.0)
    .angular_damping(0.05)
    .gravity_scale(if dynamic { def.gravity_scale } else { 0.0 })
    .additional_mass(extra_mass)
    .can_sleep(true)
    .build();
    let _ = global;
    (body, collider)
}

fn isometry(transform: &Transform) -> Isometry3<f32> {
    let [x, y, z, w] = transform.rotation;
    let rotation = UnitQuaternion::from_quaternion(rapier3d::na::Quaternion::new(w, x, y, z));
    Isometry3::from_parts(
        Translation3::new(
            transform.position[0],
            transform.position[1],
            transform.position[2],
        ),
        rotation,
    )
}

/// 形状 + 单位密度质量（旧端用 `mass_properties(1.0).mass()`，这里解析求解）。
fn collider_shape(def: &PhysicsBody, global: &GlobalTransform) -> (SharedShape, f32) {
    match def.shape.as_str() {
        "ball" => {
            let shape = SharedShape::ball(def.radius);
            let mass = 4.0 / 3.0 * std::f32::consts::PI * def.radius.powi(3);
            (shape, mass)
        }
        "capsule" => {
            let shape = SharedShape::capsule_y(def.half_height, def.radius);
            let cylinder = std::f32::consts::PI * def.radius.powi(2) * 2.0 * def.half_height;
            let sphere = 4.0 / 3.0 * std::f32::consts::PI * def.radius.powi(3);
            (shape, cylinder + sphere)
        }
        "cylinder" => {
            let shape = SharedShape::cylinder(def.half_height, def.radius);
            let mass = std::f32::consts::PI * def.radius.powi(2) * 2.0 * def.half_height;
            (shape, mass)
        }
        "cuboid" => {
            let [hx, hy, hz] = def.half_extents;
            let shape = SharedShape::cuboid(hx, hy, hz);
            (shape, 8.0 * hx * hy * hz)
        }
        // `Auto`：用实体缩放（绝对值，兜 1e-3）当边长的一半，与旧端同式。
        _ => {
            let half = [
                global.scale[0].abs().max(1e-3) * 0.5,
                global.scale[1].abs().max(1e-3) * 0.5,
                global.scale[2].abs().max(1e-3) * 0.5,
            ];
            let shape = SharedShape::cuboid(half[0], half[1], half[2]);
            (shape, 8.0 * half[0] * half[1] * half[2])
        }
    }
}

/// 把 [`RapierWorld`] 接进固定步调度。
#[derive(Default)]
pub struct RapierPhysicsSystem {
    world: RapierWorld,
}

impl System for RapierPhysicsSystem {
    fn name(&self) -> &'static str {
        "rapier"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![PhysicsBody::ID, GlobalTransform::ID],
            writes: vec![Transform::ID, Velocity::ID],
        }
    }

    fn run(&mut self, world: &mut World, dt: f32) {
        self.world.step(world, dt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgc_world::World;

    fn body(kind: &str) -> PhysicsBody {
        PhysicsBody {
            kind: kind.to_string(),
            shape: "auto".to_string(),
            ..PhysicsBody::default()
        }
    }

    fn spawn(world: &mut World, position: [f32; 3], scale: [f32; 3], kind: &str) -> EntityKey {
        world.spawn((
            Transform {
                position,
                scale,
                ..Transform::default()
            },
            GlobalTransform {
                position,
                scale,
                ..GlobalTransform::default()
            },
            Velocity::default(),
            body(kind),
        ))
    }

    /// 旧端行为：动态立方体落到静态地面上，y = 半高、速度归零。
    #[test]
    fn dynamic_cube_lands_on_static_ground() {
        let mut world = World::new();
        spawn(&mut world, [0.0, -0.5, 0.0], [20.0, 1.0, 20.0], "static");
        let cube = spawn(&mut world, [0.0, 5.0, 0.0], [1.0; 3], "dynamic");
        let mut physics = RapierWorld::new();
        for _ in 0..240 {
            physics.step(&mut world, FIXED_DT);
        }
        let transform = *world.get::<Transform>(cube).unwrap();
        assert!(
            (transform.position[1] - 0.5).abs() < 0.02,
            "立方体应停在 y = 0.5，实际 {}",
            transform.position[1]
        );
        let velocity = *world.get::<Velocity>(cube).unwrap();
        assert!(velocity.linear[1].abs() < 0.01, "落地后竖直速度应归零");
    }

    /// 翻滚能力：带初始倾角的动态盒子落地后姿态必须变化（简化 AABB 做不到）。
    #[test]
    fn tilted_cube_tumbles_on_landing() {
        let mut world = World::new();
        spawn(&mut world, [0.0, -0.5, 0.0], [20.0, 1.0, 20.0], "static");
        let angle = 0.35f32;
        let cube = world.spawn((
            Transform {
                position: [0.0, 4.0, 0.0],
                rotation: [angle.sin(), 0.0, 0.0, angle.cos()],
                ..Transform::default()
            },
            GlobalTransform::default(),
            Velocity::default(),
            body("dynamic"),
        ));
        let mut physics = RapierWorld::new();
        for _ in 0..240 {
            physics.step(&mut world, FIXED_DT);
        }
        let rotation = world.get::<Transform>(cube).unwrap().rotation;
        let changed = (rotation[0] - angle.sin()).abs() > 1e-3
            || (rotation[2]).abs() > 1e-3
            || (rotation[3] - angle.cos()).abs() > 1e-3;
        assert!(changed, "落地后姿态应被接触力矩改变：{rotation:?}");
    }

    /// 同一场景两次运行必须逐位一致（截图 / 割接可比的前提）。
    #[test]
    fn stepping_is_deterministic() {
        let run = || {
            let mut world = World::new();
            spawn(&mut world, [0.0, -0.5, 0.0], [20.0, 1.0, 20.0], "static");
            spawn(&mut world, [0.0, 5.0, 0.0], [1.0; 3], "dynamic");
            spawn(&mut world, [0.7, 6.0, 0.2], [0.8; 3], "dynamic");
            let mut physics = RapierWorld::new();
            for _ in 0..150 {
                physics.step(&mut world, FIXED_DT);
            }
            world.for_each::<Transform>(|key, transform| {
                let _ = (key, transform);
            });
            for_each_positions(&world)
        };
        assert_eq!(run(), run());
    }

    fn for_each_positions(world: &World) -> Vec<[u32; 3]> {
        let mut out = Vec::new();
        world.for_each::<Transform>(|_, transform| {
            out.push(transform.position.map(f32::to_bits));
        });
        out.sort();
        out
    }
}
