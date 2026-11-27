//! Rapier 刚体后端：与旧引擎同一套求解器与参数（rapier3d 0.22 + `simd-stable`）。
//!
//! 为什么不用自研简化碰撞：割接要求**同帧状态可比**。旧端用 rapier 的默认
//! `IntegrationParameters`、`gravity = (0, -9.81, 0)`、每固定帧 `step(fixed_dt)`
//! 一次；这里逐项照搬，落点与翻滚姿态才有机会和旧图对上（简化 AABB 求解器
//! 没有角动量，盒子永远不会翻滚）。
//!
//! 这一刀的范围：`physics.body` 实体（静态 / 动态 / 运动学）+ `physics.character`
//! 实体的**水平** collide-and-slide（`character_kcc`）。角色的竖直 / 坡面 / 贴地 /
//! 台阶仍走 `xgc-sim` 的既有系统，理由见 `character_kcc` 模块文档。

use rapier3d::na::Vector3;
use rapier3d::prelude::*;

use xgc_base::EntityKey;
use xgc_world::{
    CharacterController, Component, GlobalTransform, PhysicsBody, Slope, Transform, Velocity, World,
};

use crate::{System, SystemAccess};

mod bodies;
mod characters;
mod explain;
mod slopes;
use bodies::{isometry, world_transform, BodyLink, BodySignature};
use characters::CharacterLink;
pub use explain::{ColliderOverlap, OverlapBody};
use slopes::{SlopeEntry, SlopeLink, SlopeSignature};

/// 旧引擎 `PhysicsConfig::default()` 的重力。
pub const GRAVITY: [f32; 3] = [0.0, -9.81, 0.0];
/// 旧引擎 `PhysicsConfig::default()` 的单步时长；`Simulation` 也用它。
pub const FIXED_DT: f32 = 1.0 / 60.0;

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
    /// 建体时的输入签名（按 ID 排序）；第一次建表整份重建，之后按 ID 差量修补。
    ///
    /// 必须带 `EntityKey` 而不是只比数量：`Replace` / 组件增删 / 快照恢复只换代不换 ID，
    /// 数量判据不会触发重建，`BodyLink.key` 就停在过期代际——`World::get` 拒绝过期句柄，
    /// 动态体的 Transform / Velocity 回写会静默冻结（角色表 `character_signature` 同款）。
    /// 还必须带上 `PhysicsBody` 的构建参数：只比身份会让"运行期改形状 / 质量 /
    /// 传感器"静默不生效（见 [`BodySignature`]）。
    built_signature: Option<Vec<BodySignature>>,
    /// `ensure_built` 的每帧探针，跨帧复用避免热路径分配。
    body_probe: Vec<BodySignature>,
    /// 建体和运动学同步串行复用父链缓冲，深度增大时才扩容。
    body_transform_chain: Vec<Transform>,
    /// 建体表整份重建的次数（诊断口径）：同一份世界稳态下应恒为 1。
    rebuilds: u64,
    /// 建体表被增量修补的次数（诊断口径）：每次修补各算一次（同帧多处增删合并为一次）。
    body_patches: u64,
    characters: Vec<CharacterLink>,
    /// 角色集合的签名：变了才重建角色刚体。
    ///
    /// 必须带 `EntityKey`（index + generation）：`Replace` / 组件增删 / 快照恢复都会
    /// 让 `EntityKey` 换代而实体 ID 不变（`xgc-world` 的 `alloc_key`）。只看 ID 的话
    /// 签名不变 → 提前返回 → 角色仍指向过期句柄：`World::get` 拒绝过期代际，水平位移
    /// 会静默冻结，而旧胶囊作为幽灵碰撞体留在 rapier 里挡住别人。
    character_signature: Option<Vec<(u64, EntityKey, [u32; 4])>>,
    /// `ensure_characters` 的每帧探针，跨帧复用避免热路径分配（与 `body_probe` 同口径）。
    character_probe: Vec<(u64, EntityKey, [u32; 4])>,
    /// 角色收集阶段的临时表，跨帧复用（同样是为了热路径零分配）。
    character_entries: Vec<(u64, EntityKey, CharacterController)>,
    /// 角色阶段每帧复用的接触法线缓冲：`move_shape` 的事件回调往里写，判"陡坡当墙"用。
    wall_normals: Vec<Vector3<f32>>,
    slopes: Vec<SlopeLink>,
    /// 坡面集合的签名：变了才重建坡面碰撞体（同样必须带 `EntityKey` 代际）。
    slope_signature: Option<SlopeSignature>,
    /// `ensure_slopes` 的每帧探针，跨帧复用。
    slope_probe: SlopeSignature,
    /// 坡面收集阶段的临时表，跨帧复用。
    slope_entries: Vec<SlopeEntry>,
    /// 角色 vs 传感器的上一帧接触对（`(角色, 传感器)`，按 ID 排序）。
    sensor_previous: Vec<(u64, u64)>,
    /// 角色 vs 传感器的本帧接触对；跨帧复用，避免每帧分配。
    sensor_current: Vec<(u64, u64)>,
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
            built_signature: None,
            body_probe: Vec::new(),
            body_transform_chain: Vec::new(),
            rebuilds: 0,
            body_patches: 0,
            characters: Vec::new(),
            character_signature: None,
            character_probe: Vec::new(),
            character_entries: Vec::new(),
            wall_normals: Vec::new(),
            slopes: Vec::new(),
            slope_signature: None,
            slope_probe: Vec::new(),
            slope_entries: Vec::new(),
            sensor_previous: Vec::new(),
            sensor_current: Vec::new(),
        }
    }

    /// 推进一个固定步，并把动态体结果写回世界。
    pub fn step(&mut self, world: &mut World, dt: f32) {
        self.ensure_built(world);
        // 坡面几何要先于角色推进：`sync_character_poses` 与 `move_shape` 都要能看到坡。
        self.ensure_slopes(world);
        self.ensure_characters(world);
        self.sync_character_poses(world);
        // 运动学体：脚本摆好的位置每步推给物理（与旧端 `push_kinematic_bodies` 同义）。
        for link in &self.links {
            if !link.kinematic {
                continue;
            }
            // 旧端 `push_kinematic_bodies` 用的是世界矩阵；子实体的运动学体只有按世界
            // 变换推才算对位置（局部推等于把子物体瞬移回父节点坐标系）。
            let Some(transform) = world_transform(world, link.key, &mut self.body_transform_chain)
            else {
                continue;
            };
            if let Some(body) = self.bodies.get_mut(link.body) {
                body.set_position(isometry(&transform), true);
            }
        }

        // 只结算显式脚本指令，避免把 ECS 在本步临时积分的速度重新注入 Rapier。
        let signatures = self.built_signature.as_deref().unwrap_or_default();
        for (entity, key, linear) in world.drain_body_velocities() {
            let Ok(index) = signatures.binary_search_by_key(&entity.raw(), |signature| signature.0)
            else {
                continue;
            };
            let link = &self.links[index];
            if link.key != key {
                continue;
            }
            if let Some(body) = self.bodies.get_mut(link.body) {
                body.set_linvel(Vector3::from(linear), true);
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

        self.move_characters(world);
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

    fn prepare(&mut self, world: &World) {
        // 建体必须发生在本帧速度积分之前：旧端 `sync_bodies()` 在固定步内先建
        // 刚体，再把本步结果写回；这里换成帧首只读准备，避免新刚体把
        // `VelocitySystem` 本帧的位移当成场景位姿（整条轨迹会多出 g·dt²）。
        self.world.ensure_built(world);
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![
                PhysicsBody::ID,
                GlobalTransform::ID,
                CharacterController::ID,
                // `step` 会在 `ensure_slopes` 里读坡面组件建静态碰撞体；漏声明会让
                // 访问契约与实际读写不符，等 `Schedule` 按声明分批时就会和写者撞车。
                Slope::ID,
            ],
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

    #[test]
    fn kinematic_body_velocity_moves_the_collider_after_scene_pose_sync() {
        let mut world = World::new();
        let platform = spawn(&mut world, [0.0, 3.0, 0.0], [1.0; 3], "kinematic");
        let id = world.entity(platform).unwrap();
        let mut physics = RapierWorld::new();
        physics.step(&mut world, FIXED_DT);
        assert!(physics.ground_under(2.0, 0.0, 4.0).is_none());

        world.queue_body_velocity(id, [120.0, 0.0, 0.0]);
        physics.step(&mut world, FIXED_DT);
        assert_eq!(physics.ground_under(2.0, 0.0, 4.0), Some(3.5));
        assert!(physics.ground_under(0.0, 0.0, 4.0).is_none());
        assert_eq!(world.get::<Transform>(platform).unwrap().position[0], 0.0);
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
