//! 角色（`physics.character`）在 rapier 里的托管：运动学胶囊 + 水平 collide-and-slide。
//!
//! 为什么从 `rapier_world.rs` 拆出来：那个文件已经贴着 600 行软上限，而角色这一片
//! 有自己的生命周期（建 / 删 / 每帧推进），和"`physics.body` 刚体表"混在一起时
//! 两边都不好读。刚体表仍然是 `RapierWorld` 的私有字段，本模块是它的子模块，能直接读写。

use rapier3d::control::KinematicCharacterController;
use rapier3d::na::{Isometry3, Translation3, Vector3};

use xgc_base::EntityKey;
use xgc_world::{CharacterController, Transform, Velocity, World};

use super::{rotation, RapierWorld};
use crate::character_kcc;

/// 一个角色实体的运行时托管：刚体 + 胶囊 + 本帧要用的控制器。
///
/// 刚体是 position-based 运动学体，位置每帧由 ECS 的脚底高度算出来再推给 rapier；
/// 控制器与胶囊按组件值预算好，避免每帧重建（`move_shape` 是只读取值的）。
pub(super) struct CharacterLink {
    pub(super) key: EntityKey,
    pub(super) body: rapier3d::prelude::RigidBodyHandle,
    pub(super) center_height: f32,
    pub(super) rest_gap: f32,
    pub(super) capsule: rapier3d::prelude::Capsule,
    pub(super) controller: KinematicCharacterController,
}

impl RapierWorld {
    /// 按当前世界建 / 重建角色刚体（`physics.character` 且没有 `physics.body` 的实体）。
    ///
    /// 一个实体只给一套托管：同时挂了 `physics.body` 与 `physics.character` 时按刚体走
    /// （`physics.body` 是显式动力学声明，角色不该再被第二套移动逻辑抢 Transform）。
    pub(super) fn ensure_characters(&mut self, world: &World) {
        let mut entries = std::mem::take(&mut self.character_entries);
        entries.clear();
        world.for_each::<CharacterController>(|key, def| {
            // 同一判据由 `character_kcc::owned_by_kcc` 收口（带刚体 / 没有 `Transform` 的实体
            // 不建胶囊、也**不进签名**：之后补上 `Transform` 时签名才会变、才会重建）。
            if !character_kcc::owned_by_kcc(world, key) {
                return;
            }
            if let Some(id) = world.entity(key) {
                entries.push((id.raw(), key, *def));
            }
        });
        // 与刚体表同一条确定性理由：按实体 ID 排序建体，句柄顺序不随 archetype 变。
        entries.sort_unstable_by_key(|(id, _, _)| *id);
        // 签名里必须放"会进 `CharacterLink` 的每一个输入"：胶囊尺寸、抬升量（skin）、
        // 两个坡角（slope_limit）以及句柄代际。漏任何一个，运行期改字段就是静默 no-op。
        let mut signature = std::mem::take(&mut self.character_probe);
        signature.clear();
        signature.extend(entries.iter().map(|(id, key, def)| {
            (
                *id,
                *key,
                [
                    character_kcc::radius(def).to_bits(),
                    character_kcc::half_height(def).to_bits(),
                    character_kcc::skin_width(def).to_bits(),
                    character_kcc::slope_limit(def).to_bits(),
                ],
            )
        }));
        if self.character_signature.as_ref() == Some(&signature) {
            self.character_probe = signature;
            self.character_entries = entries;
            return;
        }
        // 只重排角色这一片：`physics.body` 的刚体表不动。
        for link in std::mem::take(&mut self.characters) {
            self.bodies.remove(
                link.body,
                &mut self.islands,
                &mut self.colliders,
                &mut self.impulse_joints,
                &mut self.multibody_joints,
                true,
            );
        }
        for (_, key, def) in &entries {
            let Some(transform) = world.get::<Transform>(*key).copied() else {
                continue;
            };
            let center_height = character_kcc::center_height(def);
            let rest_gap = character_kcc::rest_gap(def);
            let position = character_isometry(&transform, center_height, rest_gap);
            let body = self.bodies.insert(
                rapier3d::prelude::RigidBodyBuilder::kinematic_position_based()
                    .position(position)
                    .build(),
            );
            self.colliders.insert_with_parent(
                rapier3d::prelude::ColliderBuilder::capsule_y(
                    character_kcc::half_height(def),
                    character_kcc::radius(def),
                )
                .build(),
                body,
                &mut self.bodies,
            );
            self.characters.push(CharacterLink {
                key: *key,
                body,
                center_height,
                rest_gap,
                capsule: character_kcc::character_capsule(def),
                controller: character_kcc::build_character_controller(def),
            });
        }
        // 新签名接管旧的探针缓冲（`replace` 把新签名挪进字段，同形的旧签名继续当探针），
        // 重建这条路也不另外分配。
        self.character_probe = self
            .character_signature
            .replace(signature)
            .unwrap_or_default();
        self.character_entries = entries;
        // 新加 / 删过碰撞体，查询加速结构要显式重建，否则 `move_shape` 看不到它们。
        self.query.update(&self.colliders);
    }

    /// 把 ECS 的脚底高度推给角色胶囊：刚体原点是胶囊中心（再抬 `rest_gap`）。
    ///
    /// 为什么抬高：rapier 0.22 的 `check_and_fix_penetrations` 是空函数、
    /// `stop_at_penetration` 为 false，起始间隙 ≤ offset 时会放行整段位移（穿墙），
    /// 详见 `character_kcc` 模块文档。
    pub(super) fn sync_character_poses(&mut self, world: &World) {
        for link in &self.characters {
            let Some(transform) = world.get::<Transform>(link.key).copied() else {
                continue;
            };
            let position = character_isometry(&transform, link.center_height, link.rest_gap);
            if let Some(body) = self.bodies.get_mut(link.body) {
                body.set_position(position, true);
            }
        }
    }

    /// 角色水平移动：`move_shape` 只读刚体 / 碰撞体 / 查询结构，放在 `pipeline.step`
    /// 之后做，本帧其它刚体的位置已经落定，扫掠看到的是同一帧的世界。
    pub(super) fn move_characters(&mut self, world: &mut World) {
        let dt = self.integration.dt;
        for link in &self.characters {
            if world.get::<Transform>(link.key).is_none() {
                continue;
            }
            let Some(velocity) = world.get::<Velocity>(link.key).copied() else {
                continue;
            };
            // 竖直不参与：本刀由 ECS 决定脚底高度（见 `character_kcc` 模块文档）。
            let desired = Vector3::new(velocity.linear[0] * dt, 0.0, velocity.linear[2] * dt);
            if desired == Vector3::zeros() {
                continue;
            }
            let start = match self.bodies.get(link.body) {
                Some(body) => *body.position(),
                None => continue,
            };
            let moved = character_kcc::move_horizontal(
                &self.bodies,
                &self.colliders,
                &self.query,
                &link.controller,
                &link.capsule,
                &start,
                desired,
                link.body,
                dt,
            );
            let next = Isometry3::from_parts(
                Translation3::new(
                    start.translation.x + moved.translation.x,
                    start.translation.y + moved.translation.y,
                    start.translation.z + moved.translation.z,
                ),
                start.rotation,
            );
            if let Some(body) = self.bodies.get_mut(link.body) {
                body.set_position(next, true);
            }
            if let Some(transform) = world.get_mut::<Transform>(link.key) {
                // 只写水平：竖直与朝向仍归 ECS（角色只支持直立）。
                transform.position[0] += moved.translation.x;
                transform.position[2] += moved.translation.z;
            }
            if let Some(slot) = world.get_mut::<Velocity>(link.key) {
                // 与旧端同口径：水平速度回写成"实际位移 / dt"，撞墙归零、沿墙滑动被压短。
                slot.linear[0] = moved.translation.x / dt;
                slot.linear[2] = moved.translation.z / dt;
            }
        }
    }
}

/// 角色胶囊刚体的位姿：实体原点是脚底，刚体原点是胶囊中心（再抬 `rest_gap`）。
fn character_isometry(transform: &Transform, center_height: f32, rest_gap: f32) -> Isometry3<f32> {
    Isometry3::from_parts(
        Translation3::new(
            transform.position[0],
            transform.position[1] + center_height + rest_gap,
            transform.position[2],
        ),
        rotation(transform),
    )
}
