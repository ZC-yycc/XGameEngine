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

    /// 角色每帧移动：探地 → 竖直重力 → collide-and-slide → 台阶试探 → 贴地吸附 → 写回。
    ///
    /// 顺序照着旧端 `move_character`（`xgame-physics/src/world.rs`）：KCC 只给
    /// `EffectiveCharacterMovement.translation`，`grounded` / `autostep` / `snap_to_ground`
    /// 全部自实现，因为 rapier 0.22 在胶囊角色上这几项都不可用（见 `character_kcc` 模块文档）。
    pub(super) fn move_characters(&mut self, world: &mut World) {
        let dt = self.integration.dt;
        if dt <= 0.0 {
            return;
        }
        for link in &self.characters {
            let Some(mut transform) = world.get::<Transform>(link.key).copied() else {
                continue;
            };
            let Some(def) = world.get::<CharacterController>(link.key).copied() else {
                continue;
            };
            let Some(start) = self.bodies.get(link.body).map(|body| *body.position()) else {
                continue;
            };
            let mut velocity = world.get::<Velocity>(link.key).copied().unwrap_or_default();
            let core = link.center_height;
            let rest = link.rest_gap;
            let prediction = rest * character_kcc::GROUND_PREDICTION_FACTOR;
            let skin = character_kcc::skin_width(&def);
            let snap = def.sanitized().snap_to_ground;
            let step_offset = def.sanitized().step_offset;
            let radius = character_kcc::radius(&def);
            let limit_cos = character_kcc::slope_limit(&def).to_radians().cos();

            // ① 脚下探测：起始就站着才允许累加重力之外的竖直逻辑。
            let before = character_kcc::probe_ground(
                &self.bodies,
                &self.colliders,
                &self.query,
                &link.capsule,
                core,
                &start,
                link.body,
                snap + prediction,
            );
            let grounded_at_start = before.as_ref().is_some_and(|hit| {
                hit.normal.y >= character_kcc::GROUND_NORMAL_MIN_DOT && hit.gap <= prediction + 1e-5
            });
            let steep = before.as_ref().is_some_and(|hit| hit.normal.y < limit_cos);
            // 迟滞带：坡度超过 `slope_limit + 10°` 的坡站不住，角色会沿坡面滑下去；
            // `slope_limit..+10°` 之内只"爬不上去"，不主动下滑（旧端 `min_slope_slide_angle` 同式）。
            let slip_cos = character_kcc::slip_limit(&def).to_radians().cos();
            let slipping =
                grounded_at_start && before.as_ref().is_some_and(|hit| hit.normal.y < slip_cos);

            // ② 竖直速度：着地清掉向下分量，腾空按重力累加（旧端脚本层同口径、无终端速度）。
            if grounded_at_start {
                // 站不住的陡坡别清速度：清了就永远"站"在坡面上，重力再也进不来。
                // 让它继续累加，③ 的坡面投影会把它变成沿坡切向的下滑。
                if slipping {
                    velocity.linear[1] += character_kcc::GRAVITY_Y * dt;
                } else if velocity.linear[1] < 0.0 {
                    velocity.linear[1] = 0.0;
                }
            } else {
                velocity.linear[1] += character_kcc::GRAVITY_Y * dt;
            }

            // ③ 期望位移：陡坡沿切向滑、陷进地面先抬到静息高度。
            let mut desired = Vector3::new(
                velocity.linear[0] * dt,
                velocity.linear[1] * dt,
                velocity.linear[2] * dt,
            );
            if let Some(hit) = before.as_ref() {
                if steep && desired.y < 0.0 {
                    desired -= hit.normal * desired.dot(&hit.normal);
                } else if hit.gap < rest {
                    // 抬离地面：间隙小于静息值（含贴面）时先抬回去，否则 `move_shape` 会
                    // 把"起始间隙 ≤ offset"当成可以放行的穿透（穿墙 / 沉地）。
                    desired.y = desired.y.max(rest - hit.gap);
                }
            }

            self.wall_normals.clear();
            let moved = character_kcc::move_character(
                &self.bodies,
                &self.colliders,
                &self.query,
                &link.controller,
                &link.capsule,
                &start,
                desired,
                link.body,
                dt,
                &mut self.wall_normals,
            );
            let mut next = translate(&start, moved.translation);

            // ④ 陡坡当墙：水平顶陡坡时 KCC 会把接触切向当成"想爬坡"而放行，这里按旧 XGC 合同
            // 降级成竖直墙 —— 去掉抬升、消掉朝墙里的水平分量，角色沿墙横向滑。可行走坡的
            // 法线余弦高于 `slope_limit`，不会走到这里。
            //
            // 两个门禁缺一不可：接触列表里也混着**普通竖直墙**（`n.y ≈ 0`）与天花板。
            // 一旦无条件清竖直位移，跳跃会被吞掉（起跳帧位移被清零、`grounded` 又写成 true，
            // 速度却留着，松开墙才突然弹起），下落会被挂在墙上（`vy` 每帧累加却在释放时
            // 一次性放行）。旧端同样只在 `grounded_at_start && is_steep && desired.y < 0`
            // 时才做坡面投影（`crates/xgame-physics/src/world.rs:571-582`）。
            // 正在沿陡坡下滑时不能走这条分支：把竖直位移清零等于又"站住"了。
            let steep_contact = grounded_at_start
                && !slipping
                && desired.y <= 0.0
                && self.wall_normals.iter().any(|normal| normal.y < limit_cos);
            if steep_contact {
                for normal in &self.wall_normals {
                    if normal.y >= limit_cos {
                        continue;
                    }
                    let mut delta = next.translation.vector - start.translation.vector;
                    delta.y = 0.0;
                    let flat = Vector3::new(normal.x, 0.0, normal.z);
                    let length = flat.norm();
                    if length > 1e-6 {
                        let unit = flat / length;
                        let inward = -delta.dot(&unit);
                        if inward > 0.0 {
                            delta += unit * inward;
                        }
                    }
                    next = translate(&start, delta);
                }
            }

            // ④ 台阶：起始着地、没有向上意图、这一帧确实撞到东西了才做三段试探。
            // 判据不用"水平位移被吃掉多少"：小半径胶囊撞立面时 `move_shape` 仍能挤出七八成
            // 位移（沿接触切向滑），按比例判会漏掉台阶；改用"本帧有接触"当门禁。
            let wanted = Vector3::new(desired.x, 0.0, desired.z);
            let got = Vector3::new(moved.translation.x, 0.0, moved.translation.z);
            let want_len = wanted.norm();
            if grounded_at_start
                && step_offset > 1e-4
                && desired.y <= 1e-5
                && want_len > 1e-4
                && !self.wall_normals.is_empty()
            {
                let dir = wanted / want_len;
                // 扫掠距离是 `want_len + radius + 0.02`（旧端同式）：必须越过台阶前沿多一个
                // 半径，落点才在台面上而不是立面里；因此采纳后这一帧会拿到比请求更大的水平
                // 位移（最多 `radius + 0.02`），这是上台阶的固有代价，不是滑动速度失控。
                if let Some(stepped) = character_kcc::try_step_up(
                    &self.bodies,
                    &self.colliders,
                    &self.query,
                    &link.controller,
                    &link.capsule,
                    &start,
                    dir,
                    want_len,
                    step_offset + rest,
                    radius,
                    rest,
                    link.body,
                    dt,
                ) {
                    let climbed = Vector3::new(
                        stepped.translation.x - start.translation.x,
                        0.0,
                        stepped.translation.z - start.translation.z,
                    )
                    .dot(&dir);
                    // 落点必须是**能站**的面：陡坡沿坡面往上也能"抬 + 前进"成功，
                    // 但那是爬坡不是上台阶，交回 ③ 的陡坡当墙规则处理。
                    let land = character_kcc::probe_ground(
                        &self.bodies,
                        &self.colliders,
                        &self.query,
                        &link.capsule,
                        core,
                        &stepped,
                        link.body,
                        prediction,
                    );
                    let walkable = land.as_ref().is_none_or(|hit| hit.normal.y >= limit_cos);
                    if walkable && climbed > got.dot(&dir) + 1e-4 {
                        next = stepped;
                    }
                }
            }

            // ⑤ 贴地吸附：下坡走快时 KCC 会短暂离地，把间隙拉回静息值。
            if grounded_at_start && !steep_contact && snap > 1e-4 && desired.y <= 1e-5 {
                if let Some(hit) = character_kcc::probe_ground(
                    &self.bodies,
                    &self.colliders,
                    &self.query,
                    &link.capsule,
                    core,
                    &next,
                    link.body,
                    snap + skin,
                ) {
                    if hit.gap > rest + 1e-4 && hit.gap <= snap {
                        next.translation.y -= hit.gap - rest;
                    }
                }
            }

            // ⑥ `y = 0` 是 XGC 的隐式地面（没有碰撞体）：胶囊中心不许低于它。
            let floor = core + rest;
            if next.translation.y < floor {
                next.translation.y = floor;
            }

            // ⑦ 终局地面判定：写回 `grounded`，落地帧顺手清掉向下速度。
            let after = character_kcc::probe_ground(
                &self.bodies,
                &self.colliders,
                &self.query,
                &link.capsule,
                core,
                &next,
                link.body,
                prediction,
            );
            let grounded = after.as_ref().is_some_and(|hit| {
                hit.normal.y >= character_kcc::GROUND_NORMAL_MIN_DOT && hit.gap <= prediction + 1e-5
            });
            // 沿陡坡下滑时不能清：清了下滑速度永远只剩一帧的重力增量，角色变成
            // 每帧挪一两毫米的"慢性蠕动"（实测 180 帧只滑 0.28 m）。
            if grounded && !slipping && velocity.linear[1] < 0.0 {
                velocity.linear[1] = 0.0;
            }

            // ⑧ 写回：实体原点是脚底，刚体原点是胶囊中心（再抬 `rest_gap`）。
            transform.position[0] = next.translation.x;
            transform.position[1] = next.translation.y - core - rest;
            transform.position[2] = next.translation.z;
            velocity.linear[0] = (next.translation.x - start.translation.x) / dt;
            velocity.linear[2] = (next.translation.z - start.translation.z) / dt;
            if let Some(body) = self.bodies.get_mut(link.body) {
                body.set_position(next, true);
            }
            if let Some(slot) = world.get_mut::<Transform>(link.key) {
                *slot = transform;
            }
            if let Some(slot) = world.get_mut::<Velocity>(link.key) {
                slot.linear = velocity.linear;
            }
            if let Some(slot) = world.get_mut::<CharacterController>(link.key) {
                slot.grounded = grounded;
            }
        }
    }
}

/// 平移一个位姿（旋转不动：角色只支持直立）。
fn translate(position: &Isometry3<f32>, translation: Vector3<f32>) -> Isometry3<f32> {
    Isometry3::from_parts(
        Translation3::from(position.translation.vector + translation),
        position.rotation,
    )
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
