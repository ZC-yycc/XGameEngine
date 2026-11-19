//! 角色台阶：横向移动撞到低矮实体时抬到其顶面。
//!
//! 横向阻挡（`CharacterBlockSystem`）已由 rapier 的角色控制器 collide-and-slide
//! 取代（`character_kcc` / `RapierWorld::ensure_characters`），旧实现同片拆除。

use xgc_base::EntityKey;
use xgc_world::{CharacterController, Component, PhysicsBody, Transform, Velocity, World};

use super::{System, SystemAccess};
use crate::character_kcc;

/// 台阶判据比 KCC 停位间隙再多要的一圈余量（米）。
///
/// `rest_gap = skin_width × 1.1` 已经大于 KCC 的停位间隙（`skin_width` + nudge），
/// 这 1 mm 只是保证浮点比较留出严格大于的余量。
const PROBE_MARGIN: f32 = 1e-3;

#[derive(Clone, Copy, Debug, Default)]
pub struct CharacterStepSystem;

struct Solid {
    min: [f32; 3],
    max: [f32; 3],
}

struct Character {
    key: EntityKey,
    position: [f32; 3],
    velocity: [f32; 3],
    radius: f32,
    height: f32,
    step: f32,
    /// 台阶判据的横向外扩量：KCC 停位留的间隙 + 一圈余量。
    ///
    /// rapier 0.22 的 `check_and_fix_penetrations` 是空实现，所以 KCC 会停在距立面
    /// `offset = skin_width` 处（再被 `normal_nudge_factor` 顶回一点），胶囊永远碰不到
    /// 台阶面：判据不外扩的话，撞住那一帧速度已被回写成 0、预测位移也归零，角色就卡在
    /// `offset` 外面一动不动（0.3 半径能靠"胶囊肚皮比台阶顶宽"的巧合多吃进一点，小半径
    /// 或低速度就立刻暴露）。外扩的代价是贴着低矮实体也会被抬上去，这正是"压住台阶就上"。
    probe: f32,
}

impl System for CharacterStepSystem {
    fn name(&self) -> &'static str {
        "character_step"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![CharacterController::ID, PhysicsBody::ID],
            writes: vec![Transform::ID, Velocity::ID],
        }
    }

    fn run(&mut self, world: &mut World, dt: f32) {
        let mut solids = Vec::new();
        world.for_each::<PhysicsBody>(|key, body| {
            if body.is_sensor {
                return;
            }
            let Some(transform) = world.get::<Transform>(key) else {
                return;
            };
            let half = half_extents(body);
            solids.push(Solid {
                min: [
                    transform.position[0] - half[0],
                    transform.position[1] - half[1],
                    transform.position[2] - half[2],
                ],
                max: [
                    transform.position[0] + half[0],
                    transform.position[1] + half[1],
                    transform.position[2] + half[2],
                ],
            });
        });
        let mut characters = Vec::new();
        world.for_each::<CharacterController>(|key, controller| {
            // 与 `character_kcc::owned_by_kcc` 同口径：带 `physics.body` 的实体由刚体表托管，
            // 台阶抬升不能再写它的 `Transform`（新调度里 rapier 先跑，写会落在刚体回写之后）。
            if world.get::<PhysicsBody>(key).is_some() {
                return;
            }
            let Some(transform) = world.get::<Transform>(key) else {
                return;
            };
            let Some(velocity) = world.get::<Velocity>(key) else {
                return;
            };
            characters.push(Character {
                key,
                position: transform.position,
                velocity: velocity.linear,
                // 与 KCC 共用同一套边界消毒：组件里的 NaN / 越界值在 `character_kcc`
                // 收口，否则这里会拿 NaN 去算重叠（`overlaps_y` 恒假 → 台阶静默失效）。
                radius: character_kcc::radius(controller),
                height: character_kcc::height(controller),
                step: controller.step_offset.max(0.0),
                probe: character_kcc::rest_gap(controller) + PROBE_MARGIN,
            });
        });
        for character in characters {
            let predicted = [
                character.position[0] + character.velocity[0] * dt,
                character.position[1],
                character.position[2] + character.velocity[2] * dt,
            ];
            let feet = character.position[1];
            let head = feet + character.height;
            let mut top = None;
            for solid in &solids {
                let reach = character.radius + character.probe;
                let overlaps_x =
                    predicted[0] + reach > solid.min[0] && predicted[0] - reach < solid.max[0];
                let overlaps_z =
                    predicted[2] + reach > solid.min[2] && predicted[2] - reach < solid.max[2];
                let overlaps_y = head > solid.min[1] && feet < solid.max[1];
                if !(overlaps_x && overlaps_z && overlaps_y) {
                    continue;
                }
                let rise = solid.max[1] - feet;
                if rise > 0.0 && rise <= character.step {
                    top = Some(top.map_or(solid.max[1], |value: f32| value.max(solid.max[1])));
                }
            }
            if let Some(top) = top {
                if let Some(transform) = world.get_mut::<Transform>(character.key) {
                    transform.position[1] = top;
                }
                if let Some(velocity) = world.get_mut::<Velocity>(character.key) {
                    velocity.linear[1] = 0.0;
                }
            }
        }
    }
}

fn half_extents(body: &PhysicsBody) -> [f32; 3] {
    match body.shape.as_str() {
        "ball" => [body.radius; 3],
        "capsule" => [body.radius, body.half_height + body.radius, body.radius],
        _ => body.half_extents,
    }
}
