//! 刚体表：建体签名、整表重建与按 ID 的增量修补。
//!
//! 从 `rapier_world.rs` 拆出来（单文件软上限 600 行）：这里只放"建体输入变了怎么同步
//! 刚体表"这一块，`impl RapierWorld` 直接操作父模块的 rapier 资源表，与
//! `characters.rs` / `slopes.rs` 同一分工。

use rapier3d::na::{Isometry3, Matrix3, Matrix4, Rotation3, Translation3, UnitQuaternion};
use rapier3d::prelude::*;

use xgc_base::EntityKey;
use xgc_world::{BodyKind, Parent, PhysicsBody, Transform, World};

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
    ///
    /// 位置 / 旋转 / Auto 尺寸一律按**世界变换**建体（旧端 `body_from_scene(def, world)`）；
    /// 世界变换由父链现算再经矩阵分解，因此非单位四元数在这里被归一化 —— 旧端同样
    /// 走 `Transform::from_matrix`（glam 的 `to_scale_rotation_translation`），
    /// 场景里手写的 `(0.2, 0, 0, 0.98)` 这类非单位旋转到旧端是合法姿态、到 rapier
    /// 直接当 `UnitQuaternion` 用会得到非正交矩阵（落地接触分叉的根因）。
    fn insert_body(&mut self, world: &World, key: EntityKey) -> Option<BodyLink> {
        let def = world.get::<PhysicsBody>(key)?.clone();
        let world_transform = world_transform(world, key)?;
        let (body, collider) = build_body(&def, &world_transform);
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
///
/// `world` 是父链合成并分解过的世界变换（见 [`world_transform`]）：位置、旋转与
/// `Auto` 形状尺寸都取它，不再用局部 `Transform`。
fn build_body(def: &PhysicsBody, world: &Transform) -> (RigidBody, Collider) {
    let body_type = match def.kind() {
        BodyKind::Static => RigidBodyType::Fixed,
        BodyKind::Kinematic => RigidBodyType::KinematicVelocityBased,
        BodyKind::Dynamic => RigidBodyType::Dynamic,
    };
    let dynamic = body_type == RigidBodyType::Dynamic;
    let (shape, unit_mass) = collider_shape(def, world.scale);
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
    let iso = isometry(world);
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
    (body, collider)
}

/// 沿父链把局部变换合成世界变换，再按旧端 `Transform::from_matrix` 的口径分解回 TRS。
///
/// 为什么不用实体的 `GlobalTransform`：迁移世界里子实体的那份派生数据初始等于局部值
/// （`HierarchySystem` 还没跑过），建体若读它会退化成"局部位置建体"；而且旧端建体读的
/// 是当场算出来的世界矩阵（`Scene::world_transform`），不是缓存。
///
/// 分解按 glam 语义：平移取第四列；缩放取三列长度（行列式为负时翻转 X 轴缩放）；
/// 旋转 = 归一化后的列基（`Rotation3` → 单位四元数）。这一步顺带把非单位四元数
/// 归一化，与旧端建体口径一致。
pub(super) fn world_transform(world: &World, key: EntityKey) -> Option<Transform> {
    let mut chain = Vec::new();
    let mut cursor = key;
    loop {
        chain.push(world.get::<Transform>(cursor).copied()?);
        let Some(parent) = world.get::<Parent>(cursor) else {
            break;
        };
        // 环检测：正常场景层级不可能超过这个深度；超过按"没有父节点"处理，
        // 免得坏数据把建体拖进死循环。
        if chain.len() >= 64 {
            break;
        }
        cursor = world.key(parent.entity)?;
    }
    chain.reverse();
    let mut matrix = Matrix4::identity();
    for transform in &chain {
        matrix *= local_matrix(transform);
    }
    Some(decompose(&matrix))
}

/// 局部 TRS → 4x4 矩阵；四元数 → 旋转基的公式与 glam `Mat3::from_quat` 同式
/// （旧端 `Transform::local_matrix` 用的就是 glam）。
fn local_matrix(transform: &Transform) -> Matrix4<f32> {
    let [x, y, z, w] = transform.rotation;
    let x2 = x + x;
    let y2 = y + y;
    let z2 = z + z;
    let xx = x * x2;
    let xy = x * y2;
    let xz = x * z2;
    let yy = y * y2;
    let yz = y * z2;
    let zz = z * z2;
    let wx = w * x2;
    let wy = w * y2;
    let wz = w * z2;
    let scale = transform.scale;
    let mut matrix = Matrix4::identity();
    matrix[(0, 0)] = (1.0 - (yy + zz)) * scale[0];
    matrix[(1, 0)] = (xy + wz) * scale[0];
    matrix[(2, 0)] = (xz - wy) * scale[0];
    matrix[(0, 1)] = (xy - wz) * scale[1];
    matrix[(1, 1)] = (1.0 - (xx + zz)) * scale[1];
    matrix[(2, 1)] = (yz + wx) * scale[1];
    matrix[(0, 2)] = (xz + wy) * scale[2];
    matrix[(1, 2)] = (yz - wx) * scale[2];
    matrix[(2, 2)] = (1.0 - (xx + yy)) * scale[2];
    matrix[(0, 3)] = transform.position[0];
    matrix[(1, 3)] = transform.position[1];
    matrix[(2, 3)] = transform.position[2];
    matrix
}

/// 4x4 矩阵 → TRS（glam `to_scale_rotation_translation` 的等价实现）。
fn decompose(matrix: &Matrix4<f32>) -> Transform {
    let position = [matrix[(0, 3)], matrix[(1, 3)], matrix[(2, 3)]];
    let mut columns = [
        [matrix[(0, 0)], matrix[(1, 0)], matrix[(2, 0)]],
        [matrix[(0, 1)], matrix[(1, 1)], matrix[(2, 1)]],
        [matrix[(0, 2)], matrix[(1, 2)], matrix[(2, 2)]],
    ];
    let mut scale = [
        length(&columns[0]),
        length(&columns[1]),
        length(&columns[2]),
    ];
    // 行列式为负 = 含镜像：glam 把负号折进 X 轴缩放，旋转仍是右手系。
    let determinant = dot(&columns[0], &cross(&columns[1], &columns[2]));
    if determinant < 0.0 {
        scale[0] = -scale[0];
        columns[0] = [-columns[0][0], -columns[0][1], -columns[0][2]];
    }
    for (column, length) in columns.iter_mut().zip(scale.iter()) {
        let divisor = length.abs();
        if divisor > 0.0 && divisor.is_finite() {
            column[0] /= divisor;
            column[1] /= divisor;
            column[2] /= divisor;
        }
    }
    let basis = Matrix3::from_columns(&[
        rapier3d::na::Vector3::new(columns[0][0], columns[0][1], columns[0][2]),
        rapier3d::na::Vector3::new(columns[1][0], columns[1][1], columns[1][2]),
        rapier3d::na::Vector3::new(columns[2][0], columns[2][1], columns[2][2]),
    ]);
    let rotation = UnitQuaternion::from_rotation_matrix(&Rotation3::from_matrix_unchecked(basis));
    Transform {
        position,
        rotation: [rotation.i, rotation.j, rotation.k, rotation.w],
        scale,
    }
}

fn length(v: &[f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn dot(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: &[f32; 3], b: &[f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
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
fn collider_shape(def: &PhysicsBody, scale: [f32; 3]) -> (SharedShape, f32) {
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
        // `Auto`：用世界缩放（绝对值，兜 1e-3）当边长的一半，与旧端同式。
        _ => {
            let half = [
                scale[0].abs().max(1e-3) * 0.5,
                scale[1].abs().max(1e-3) * 0.5,
                scale[2].abs().max(1e-3) * 0.5,
            ];
            let shape = SharedShape::cuboid(half[0], half[1], half[2]);
            (shape, 8.0 * half[0] * half[1] * half[2])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgc_world::GlobalTransform;

    fn entity(
        world: &mut World,
        position: [f32; 3],
        rotation: [f32; 4],
        scale: [f32; 3],
    ) -> EntityKey {
        world.spawn((
            Transform {
                position,
                rotation,
                scale,
            },
            GlobalTransform {
                position,
                rotation,
                scale,
            },
        ))
    }

    /// 子实体的刚体建在世界位置：父在 (-5,0.5,0)、子局部 (0,1.05,0) → 世界 (-5,1.55,0)。
    #[test]
    fn world_transform_composes_the_parent_chain() {
        let mut world = World::new();
        let parent = entity(&mut world, [-5.0, 0.5, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0; 3]);
        let parent_id = world.entity(parent).expect("父实体存在");
        let child = world.spawn((
            Transform {
                position: [0.0, 1.05, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [0.8; 3],
            },
            GlobalTransform {
                position: [0.0, 1.05, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [0.8; 3],
            },
            Parent { entity: parent_id },
        ));
        let world_transform = super::world_transform(&world, child).expect("子实体有变换");
        assert!(
            (world_transform.position[0] + 5.0).abs() < 1e-5,
            "{world_transform:?}"
        );
        assert!(
            (world_transform.position[1] - 1.55).abs() < 1e-5,
            "{world_transform:?}"
        );
        assert!(
            (world_transform.scale[0] - 0.8).abs() < 1e-5,
            "{world_transform:?}"
        );
    }

    /// 非单位四元数经矩阵分解后旋转是单位四元数，缩放带上 |q| 的污染 —— 这是旧端
    /// `Transform::from_matrix` 的口径（不是"直接把四元数归一化"）。
    #[test]
    fn decompose_normalizes_a_non_unit_rotation_like_the_old_engine() {
        let near_unit = local_matrix(&Transform {
            position: [1.0, 2.0, 3.0],
            rotation: [0.2, 0.0, 0.0, 0.98],
            scale: [1.2; 3],
        });
        let decomposed = decompose(&near_unit);
        let norm = (decomposed.rotation[0] * decomposed.rotation[0]
            + decomposed.rotation[1] * decomposed.rotation[1]
            + decomposed.rotation[2] * decomposed.rotation[2]
            + decomposed.rotation[3] * decomposed.rotation[3])
            .sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "旋转应归一化：{decomposed:?}");
        assert!((decomposed.position[1] - 2.0).abs() < 1e-5);
        assert!((decomposed.scale[0] - 1.2).abs() < 1e-5, "{decomposed:?}");
        assert!(
            (decomposed.scale[1] - 1.200032).abs() < 1e-4,
            "{decomposed:?}"
        );

        // 极端非单位四元数：旧端分解出的旋转角 ≈41°、Y/Z 缩放 ≈1.2192，
        // 而不是"归一化四元数"的 11.4° —— 钉住这层语义。
        let far = local_matrix(&Transform {
            position: [0.0; 3],
            rotation: [0.2, 0.0, 0.0, 2.0],
            scale: [1.0; 3],
        });
        let decomposed = decompose(&far);
        assert!(
            (decomposed.rotation[3] - 0.9366).abs() < 1e-3,
            "{decomposed:?}"
        );
        assert!(
            (decomposed.scale[1] - 1.21918).abs() < 1e-4,
            "{decomposed:?}"
        );
    }
}
