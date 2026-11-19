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

use rapier3d::na::{Isometry3, Translation3, UnitQuaternion, Vector3};
use rapier3d::prelude::*;

use xgc_base::EntityKey;
use xgc_world::{
    BodyKind, CharacterController, Component, GlobalTransform, PhysicsBody, Slope, Transform,
    Velocity, World,
};

use crate::{System, SystemAccess};

mod characters;
mod slopes;
use characters::CharacterLink;
use slopes::{SlopeEntry, SlopeLink, SlopeSignature};

/// 旧引擎 `PhysicsConfig::default()` 的重力。
pub const GRAVITY: [f32; 3] = [0.0, -9.81, 0.0];
/// 旧引擎 `PhysicsConfig::default()` 的单步时长；`Simulation` 也用它。
pub const FIXED_DT: f32 = 1.0 / 60.0;

/// 建体输入签名：实体身份 + 全部会改变刚体 / 碰撞体几何与物性的参数。
///
/// 只比 `(EntityId, EntityKey)` 时，运行期改 `PhysicsBody` 的 `kind` / `shape` /
/// 尺寸 / 质量 / 摩擦 / 传感器都不会重建，编辑静默不生效（`Replace` 换代那条已经靠
/// `EntityKey` 抓住了，但字段编辑不换代）。
///
/// 位置 / 旋转 / 缩放**都不进**签名：位置与旋转动态体每帧都在动，进了签名等于每帧整表重建；
/// 缩放不进是沿用旧端的取舍 —— Auto 形状只在建体时读一次缩放，而脚本常用缩放做脉冲动画
/// （证据世界里的展示盒就在动缩放），把缩放算进签名会变成「动画期间每帧重建 + 每帧清速度」。
/// 所以「运行期换几何」只有一条路：先把 `shape` 从缺省的 `"auto"` 改成显式形状
/// （`shape` 在签名里，会触发重建），再改 `half_extents` / `radius` / `half_height`；
/// `"auto"` 的几何只由缩放决定，改那三个字段只会白白重建一次、形状不变。
///
/// 12 个槽有效，尾部 4 个槽留 0 给后续新增的构建参数。
type BodySignature = (u64, EntityKey, [u32; 16]);

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
    /// 建体时的输入签名（按 ID 排序）；与当前世界不一致时整表重建。
    ///
    /// 必须带 `EntityKey` 而不是只比数量：`Replace` / 组件增删 / 快照恢复只换代不换 ID，
    /// 数量判据不会触发重建，`BodyLink.key` 就停在过期代际——`World::get` 拒绝过期句柄，
    /// 动态体的 Transform / Velocity 回写会静默冻结（角色表 `character_signature` 同款）。
    /// 还必须带上 `PhysicsBody` 的构建参数：只比身份会让"运行期改形状 / 质量 /
    /// 传感器"静默不生效（见 [`BodySignature`]）。
    built_signature: Option<Vec<BodySignature>>,
    /// `ensure_built` 的每帧探针，跨帧复用避免热路径分配。
    body_probe: Vec<BodySignature>,
    /// 建体表整份重建的次数（诊断口径）：同一份世界稳态下应恒为 1，
    /// 只有实体增删 / 换代 / 改构建参数才会增长。
    rebuilds: u64,
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
            rebuilds: 0,
            characters: Vec::new(),
            character_signature: None,
            character_probe: Vec::new(),
            character_entries: Vec::new(),
            wall_normals: Vec::new(),
            slopes: Vec::new(),
            slope_signature: None,
            slope_probe: Vec::new(),
            slope_entries: Vec::new(),
        }
    }

    /// 本帧刚体输入若与建体时不同（增删 / `EntityKey` 换代 / 改了 `PhysicsBody` 的
    /// 构建参数）就整表重建。位置、旋转、缩放的变化不触发重建（见 [`BodySignature`]）。
    /// 稳态（同一份世界连续跑帧）下不重建。
    pub fn ensure_built(&mut self, world: &World) {
        let mut probe = std::mem::take(&mut self.body_probe);
        probe.clear();
        world.for_each::<PhysicsBody>(|key, def| {
            // 没有 `Transform` 的实体根本建不出刚体，也不该进签名：否则之后补上
            // `Transform` 时签名不变、永远不建体，留下一个看不见的刚体。
            // 现有所有组件增删都经过 `replace_entity_raw`、`EntityKey` 必换代，所以这条
            // 过滤按设计不可观测；留着是与 `rapier_world/characters.rs` 对齐的防御性写法。
            if world.get::<Transform>(key).is_none() {
                return;
            }
            if let Some(id) = world.entity(key) {
                probe.push(body_signature(id.raw(), key, def));
            }
        });
        probe.sort_unstable_by_key(|(id, _, _)| *id);
        if self.built_signature.as_ref() == Some(&probe) {
            self.body_probe = probe;
            return;
        }
        self.rebuild(world, probe);
    }

    /// 建体表整份重建过多少次（累计）；稳态下只应增长一次。
    pub fn rebuild_count(&self) -> u64 {
        self.rebuilds
    }

    fn rebuild(&mut self, world: &World, signature: Vec<BodySignature>) {
        self.bodies = RigidBodySet::new();
        self.colliders = ColliderSet::new();
        self.links.clear();
        self.rebuilds += 1;
        self.built_signature = Some(signature);
        // 刚体表整份换新，角色刚体也随之消失，签名清掉让 `ensure_characters` 重建。
        self.characters.clear();
        self.character_signature = None;
        // 坡面碰撞体与角色同理：刚体表换新后它们一起没了。
        self.slopes.clear();
        self.slope_signature = None;
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
            let kind = def.kind();
            self.links.push(BodyLink {
                key,
                body: handle,
                dynamic: kind == BodyKind::Dynamic,
                kinematic: kind == BodyKind::Kinematic,
            });
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

        self.move_characters(world);
    }
}

/// 建体输入签名：`kind` / `shape` 用 FNV-1a 折叠成 32 位，数值逐项取位模式。
///
/// 哈希只用来做"同 / 不同"判据，不跨进程持久化，所以不需要稳定算法；只折 `kind` /
/// `shape` 两个闭集字符串，32 位 FNV-1a 的 2^-32 碰撞概率可以接受。数值仍逐项进签名，
/// 避免尺寸 / 质量这类连续参数被哈希碰撞吃掉。`[u32; 16]` 里 12 槽有效、尾部 4 槽留 0。
fn body_signature(id: u64, key: EntityKey, def: &PhysicsBody) -> BodySignature {
    fn hash(text: &str) -> u32 {
        let mut state = 0x811c_9dc5u32;
        for byte in text.as_bytes() {
            state ^= u32::from(*byte);
            state = state.wrapping_mul(0x0100_0193);
        }
        state
    }
    let bits = [
        hash(&def.kind),
        hash(&def.shape),
        def.half_extents[0].to_bits(),
        def.half_extents[1].to_bits(),
        def.half_extents[2].to_bits(),
        def.radius.to_bits(),
        def.half_height.to_bits(),
        def.friction.to_bits(),
        def.restitution.to_bits(),
        def.mass.to_bits(),
        def.gravity_scale.to_bits(),
        u32::from(def.is_sensor),
        0,
        0,
        0,
        0,
    ];
    (id, key, bits)
}

/// 场景声明 → rapier 刚体 + 碰撞体；逐项对齐旧端 `body_from_scene`。
fn build_body(
    def: &PhysicsBody,
    local: &Transform,
    global: &GlobalTransform,
) -> (RigidBody, Collider) {
    let body_type = match def.kind() {
        BodyKind::Static => RigidBodyType::Fixed,
        BodyKind::Kinematic => RigidBodyType::KinematicVelocityBased,
        BodyKind::Dynamic => RigidBodyType::Dynamic,
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

fn rotation(transform: &Transform) -> UnitQuaternion<f32> {
    let [x, y, z, w] = transform.rotation;
    UnitQuaternion::from_quaternion(rapier3d::na::Quaternion::new(w, x, y, z))
}

fn isometry(transform: &Transform) -> Isometry3<f32> {
    Isometry3::from_parts(
        Translation3::new(
            transform.position[0],
            transform.position[1],
            transform.position[2],
        ),
        rotation(transform),
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
