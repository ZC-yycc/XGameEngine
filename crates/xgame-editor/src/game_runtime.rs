//! 编辑器无关的游戏运行时。
//!
//! 封装物理引擎状态管理和生命期，可在任何二进制中复用。
//! 使用 `GameClock` 驱动固定步长物理模拟，与渲染帧率解耦。

use std::collections::HashMap;

use xgame_core::{Entity, GameClock, Transform};
use xgame_math::glam::Vec3;
use xgame_physics::{Collider, DebugLine, PhysicsConfig, PhysicsWorld, RigidBody};

/// 游戏运行时，拥有物理世界并管理实体-物理体映射。
pub struct GameRuntime {
    physics: PhysicsWorld,
    entity_to_body: HashMap<usize, u64>,
    saved_states: Vec<Transform>,
    /// 固定步长时钟（默认 60Hz）。
    clock: GameClock,
}

impl GameRuntime {
    /// 进入 Play 模式：保存实体状态，创建物理世界。
    pub fn start(entities: &[Entity]) -> Self {
        let saved_states = entities.iter().map(|e| e.transform).collect();
        let mut physics = PhysicsWorld::new(PhysicsConfig::default());
        let mut entity_to_body = HashMap::new();

        for (i, entity) in entities.iter().enumerate() {
            if entity.model.is_empty() {
                continue;
            }
            let body = RigidBody::dynamic_at(entity.transform);
            let body_id = physics.add_body(body);

            let half_extents = (entity.transform.scale * 0.5).max(Vec3::splat(0.01));
            let collider = Collider::cuboid(half_extents);
            physics.add_collider(body_id, collider);

            entity_to_body.insert(i, body_id);
        }

        Self {
            physics,
            entity_to_body,
            saved_states,
            clock: GameClock::new().with_fixed_dt(1.0 / 60.0),
        }
    }

    /// 使用固定步长推进物理模拟，与渲染帧率解耦。
    pub fn fixed_update(&mut self, dt: f32, entities: &mut [Entity]) {
        self.clock.tick(dt);
        while self.clock.consume_fixed_update() {
            self.physics.step(self.clock.fixed_dt());
        }
        self.clock.reset_step_counter();

        self.sync_transforms(entities);
    }

    fn sync_transforms(&self, entities: &mut [Entity]) {
        let mapping: Vec<(usize, u64)> =
            self.entity_to_body.iter().map(|(&i, &b)| (i, b)).collect();

        for (entity_idx, body_id) in mapping {
            if let Some(phys_transform) = self.physics.body_transform(body_id) {
                if let Some(entity) = entities.get_mut(entity_idx) {
                    entity.transform.position = phys_transform.position;
                    entity.transform.rotation = phys_transform.rotation;
                }
            }
        }
    }

    /// 获取物理调试线条。
    pub fn debug_lines(&self) -> Vec<DebugLine> {
        self.physics.debug_lines()
    }

    /// 返回刚体和碰撞体数量统计。
    pub fn stats(&self) -> (usize, usize) {
        self.physics.stats()
    }

    /// 退出 Play 模式。
    pub fn stop(self, entities: &mut [Entity]) {
        for (i, transform) in self.saved_states.iter().enumerate() {
            if let Some(entity) = entities.get_mut(i) {
                entity.transform = *transform;
            }
        }
    }
}
