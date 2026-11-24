//! 瞬态碰撞事件：由物理系统写入、脚本宿主持有，不进 `WorldData` / `state_hash`。
//!
//! 事件是**有向**的：一对接触会分别给 `a` 记 `other = b`、给 `b` 记 `other = a`，
//! 这样脚本按自己的实体查事件时不用再做方向判断。持续接触只在接触开始/结束那帧
//! 出现，中间帧不刷新（与旧端 `on_collision_enter/exit` 同义）。

use xgc_base::EntityId;

use crate::World;

/// 一条碰撞事件：`entity` 的接触对象是 `other`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollisionEvent {
    pub entity: EntityId,
    pub other: EntityId,
    /// true = 本帧开始接触，false = 本帧结束接触。
    pub enter: bool,
}

impl World {
    /// 清掉上一帧的碰撞事件。每个固定步开始时由 `Simulation` 调一次。
    pub fn clear_collision_events(&mut self) {
        self.collision_events.clear();
    }

    /// 物理系统写入一条本帧事件；同一对接触只由生产者去重，不在这里判重。
    pub fn push_collision_event(&mut self, event: CollisionEvent) {
        self.collision_events.push(event);
    }

    /// 本帧事件（只读）；脚本宿主在宿主阶段消费。
    pub fn collision_events(&self) -> &[CollisionEvent] {
        &self.collision_events
    }
}
