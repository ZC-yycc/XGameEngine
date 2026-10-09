//! 射线与命中结果。
//!
//! 编辑器拾取、脚本里的"看向哪里"、射击判定、角色脚下的地面探测，用的是
//! 同一个表示。放在 core 里是为了让这些模块互相不依赖对方，却能交换同一种数据。

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::id::EntityId;

/// 一条射线。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Ray {
    pub origin: Vec3,
    /// 单位方向向量。构造函数会归一化 —— 这样 `point_at(t)` 里的 `t`
    /// 就等于"沿射线的距离"，而不是被方向长度缩放过的怪值。
    pub direction: Vec3,
}

impl Ray {
    /// 造一条射线。零方向会退化成 `-Z`（约定俗成的前方），
    /// 免得下游到处判 NaN。
    pub fn new(origin: Vec3, direction: Vec3) -> Self {
        let dir = if direction.length_squared() > 1e-12 {
            direction.normalize()
        } else {
            Vec3::NEG_Z
        };
        Self { origin, direction: dir }
    }

    /// 射线上距原点 `t` 处的点。
    pub fn point_at(&self, t: f32) -> Vec3 {
        self.origin + self.direction * t
    }
}

/// 一次射线命中的结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// 被打中的实体（`None` 表示命中了没有实体归属的几何体）。
    pub entity: Option<EntityId>,
    /// 命中点的世界坐标。
    pub point: Vec3,
    /// 命中处的世界法线。
    pub normal: Vec3,
    /// 从射线原点到命中点的距离。
    pub distance: f32,
    /// 命中是否为传感器（触发器）—— 角色移动通常要忽略传感器。
    pub is_sensor: bool,
}

impl RayHit {
    pub fn new(entity: Option<EntityId>, point: Vec3, normal: Vec3, distance: f32) -> Self {
        Self { entity, point, normal, distance, is_sensor: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_is_normalized() {
        let r = Ray::new(Vec3::ZERO, Vec3::new(0.0, 0.0, -10.0));
        assert!((r.direction.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn point_at_uses_world_distance() {
        let r = Ray::new(Vec3::new(0.0, 0.0, 5.0), Vec3::new(0.0, 0.0, -2.0));
        assert_eq!(r.point_at(3.0), Vec3::new(0.0, 0.0, 2.0));
    }

    #[test]
    fn zero_direction_falls_back_to_forward() {
        let r = Ray::new(Vec3::ZERO, Vec3::ZERO);
        assert_eq!(r.direction, Vec3::NEG_Z);
    }
}
