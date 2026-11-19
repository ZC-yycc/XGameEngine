//! 刚体表：建体签名、整表重建与按 ID 的增量修补。
//!
//! 从 `rapier_world.rs` 拆出来（单文件软上限 600 行）：这里只放"建体输入变了怎么同步
//! 刚体表"这一块，`impl RapierWorld` 直接操作父模块的 rapier 资源表，与
//! `characters.rs` / `slopes.rs` 同一分工。

use rapier3d::na::{Isometry3, Translation3, UnitQuaternion};
use rapier3d::prelude::*;

use xgc_base::EntityKey;
use xgc_world::{BodyKind, GlobalTransform, PhysicsBody, Transform, World};

use super::RapierWorld;

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
pub(super) type BodySignature = (u64, EntityKey, [u32; 16]);

#[derive(Clone, Copy)]
pub(super) struct BodyLink {
    pub(super) key: EntityKey,
    pub(super) body: RigidBodyHandle,
    pub(super) dynamic: bool,
    pub(super) kinematic: bool,
}

impl RapierWorld {
    /// 本帧刚体输入若与建体时不同（增删 / `EntityKey` 换代 / 改了 `PhysicsBody` 的
    /// 构建参数）就同步建体表。位置、旋转、缩放的变化不触发同步（见 [`BodySignature`]）。
    /// 稳态（同一份世界连续跑帧）下不动。
    ///
    /// 第一次建表走整份 [`Self::rebuild`]，之后只按 ID 差量修补受影响的条目：
    /// rapier 的岛屿构建顺序与插入顺序有关，整表重建会让所有刚体的 handle 重排、
    /// 在飞的速度与接触缓存全丢；增量修补保留未变化刚体的 handle，只替换真的变了的那几个。
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
        if self.built_signature.is_none() {
            self.rebuild(world, probe);
            return;
        }
        self.patch_bodies(world, probe);
    }

    /// 建体表整份重建过多少次（累计）；稳态下只应增长一次。
    pub fn rebuild_count(&self) -> u64 {
        self.rebuilds
    }

    /// 建体表被增量修补的次数（诊断口径）：每次 `patch_bodies` 各算一次，同一帧里
    /// 多处增删合并成一次；诊断不区分一次修补里的条目数。
    pub fn body_patch_count(&self) -> u64 {
        self.body_patches
    }

    fn rebuild(&mut self, world: &World, signature: Vec<BodySignature>) {
        debug_assert_eq!(
            self.bodies.len(),
            0,
            "rebuild 只允许在空表上跑：它整份换掉刚体表与碰撞体，不清 islands / 关节 / 查询 BVH",
        );
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
        for (_, key, _) in entries {
            if let Some(link) = self.insert_body(world, key) {
                self.links.push(link);
            }
        }
        // 首次建表同样要刷新查询结构：`query` 的 BVH 不会自己知道新增的碰撞体。
        self.query.update(&self.colliders);
    }

    /// 按当前世界状态给一个实体建刚体 + 碰撞体，返回映射（没有 `Transform` 时为空）。
    fn insert_body(&mut self, world: &World, key: EntityKey) -> Option<BodyLink> {
        let def = world.get::<PhysicsBody>(key)?.clone();
        let transform = world.get::<Transform>(key).copied()?;
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
        Some(BodyLink {
            key,
            body: handle,
            dynamic: kind == BodyKind::Dynamic,
            kinematic: kind == BodyKind::Kinematic,
        })
    }

    fn remove_body(&mut self, body: RigidBodyHandle) {
        self.bodies.remove(
            body,
            &mut self.islands,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            true,
        );
    }

    /// 只重建与旧签名不同的条目（增删 / 换代 / 改构建参数），未变化的刚体保留 handle。
    fn patch_bodies(&mut self, world: &World, signature: Vec<BodySignature>) {
        let old_links = std::mem::take(&mut self.links);
        let old_signature = self.built_signature.take().unwrap_or_default();
        let mut links = Vec::with_capacity(signature.len());
        let mut changed = false;
        let mut index = 0usize;
        for probe in &signature {
            let id = probe.0;
            while index < old_signature.len() && old_signature[index].0 < id {
                self.remove_body(old_links[index].body);
                changed = true;
                index += 1;
            }
            if index < old_signature.len() && old_signature[index].0 == id {
                if &old_signature[index] == probe {
                    links.push(old_links[index]);
                    index += 1;
                    continue;
                }
                self.remove_body(old_links[index].body);
                changed = true;
                index += 1;
            }
            // 新实体，或换了代际 / 改了构建参数：只按当前世界状态重建这一个。
            let key = probe.1;
            match self.insert_body(world, key) {
                Some(link) => links.push(link),
                None => {
                    debug_assert!(
                        false,
                        "探针已过滤掉没有 Transform 的实体，insert_body 不该失败"
                    );
                    continue;
                }
            }
            changed = true;
        }
        while index < old_signature.len() {
            self.remove_body(old_links[index].body);
            changed = true;
            index += 1;
        }
        debug_assert_eq!(
            links.len(),
            signature.len(),
            "刚体表与签名表必须逐条对应，否则下一次修补会把不同实体的句柄配对"
        );
        self.links = links;
        self.built_signature = Some(signature);
        // 旧签名直接当下一帧的探针缓冲：否则每帧都要为探针重新分配一次。
        self.body_probe = old_signature;
        if changed {
            self.body_patches += 1;
            // 碰撞体增删后查询结构必须显式重建，否则射线 / 形状查询继续用旧 BVH。
            self.query.update(&self.colliders);
        }
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

pub(super) fn rotation(transform: &Transform) -> UnitQuaternion<f32> {
    let [x, y, z, w] = transform.rotation;
    UnitQuaternion::from_quaternion(rapier3d::na::Quaternion::new(w, x, y, z))
}

pub(super) fn isometry(transform: &Transform) -> Isometry3<f32> {
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
