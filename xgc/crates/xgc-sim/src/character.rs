//! 角色台阶：横向移动撞到低矮实体时抬到其顶面。

use xgc_base::EntityKey;
use xgc_world::{CharacterController, Component, PhysicsBody, Transform, Velocity, World};

use super::{System, SystemAccess};

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
                radius: controller.radius,
                height: controller.height,
                step: controller.step_offset,
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
                let overlaps_x = predicted[0] + character.radius > solid.min[0]
                    && predicted[0] - character.radius < solid.max[0];
                let overlaps_z = predicted[2] + character.radius > solid.min[2]
                    && predicted[2] - character.radius < solid.max[2];
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

/// 横向阻挡：高于 step_offset 的实体把角色推出并清掉对应水平速度。
#[derive(Clone, Copy, Debug, Default)]
pub struct CharacterBlockSystem;

impl System for CharacterBlockSystem {
    fn name(&self) -> &'static str {
        "character_block"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![CharacterController::ID, PhysicsBody::ID],
            writes: vec![Transform::ID, Velocity::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
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
                radius: controller.radius,
                height: controller.height,
                step: controller.step_offset,
            });
        });
        for character in characters {
            let mut position = character.position;
            let mut velocity = character.velocity;
            for solid in &solids {
                let feet = position[1];
                let head = feet + character.height;
                if head <= solid.min[1] || feet >= solid.max[1] {
                    continue;
                }
                let rise = solid.max[1] - feet;
                if rise > 0.0 && rise <= character.step {
                    continue;
                }
                let overlap_x = (position[0] + character.radius).min(solid.max[0])
                    - (position[0] - character.radius).max(solid.min[0]);
                let overlap_z = (position[2] + character.radius).min(solid.max[2])
                    - (position[2] - character.radius).max(solid.min[2]);
                if overlap_x <= 0.0 || overlap_z <= 0.0 {
                    continue;
                }
                if overlap_x < overlap_z {
                    if position[0] < (solid.min[0] + solid.max[0]) * 0.5 {
                        position[0] = solid.min[0] - character.radius;
                    } else {
                        position[0] = solid.max[0] + character.radius;
                    }
                    velocity[0] = 0.0;
                } else if position[2] < (solid.min[2] + solid.max[2]) * 0.5 {
                    position[2] = solid.min[2] - character.radius;
                    velocity[2] = 0.0;
                } else {
                    position[2] = solid.max[2] + character.radius;
                    velocity[2] = 0.0;
                }
            }
            if let Some(transform) = world.get_mut::<Transform>(character.key) {
                transform.position = position;
            }
            if let Some(slot) = world.get_mut::<Velocity>(character.key) {
                slot.linear = velocity;
            }
        }
    }
}
