//! 编辑器变换操作：位置 / 缩放 / 旋转；无父节点时同步 GlobalTransform。

use xgc_base::EntityId;
use xgc_world::{GlobalTransform, Parent, Transform};

use crate::EditorState;

impl EditorState {
    pub fn entity_position(&self, id: EntityId) -> Option<[f32; 3]> {
        let key = self.world.key(id)?;
        if let Some(transform) = self.world.get::<GlobalTransform>(key) {
            return Some(transform.position);
        }
        self.world
            .get::<Transform>(key)
            .map(|transform| transform.position)
    }

    pub fn set_entity_position(&mut self, id: EntityId, position: [f32; 3]) -> Result<(), String> {
        self.mutate(
            id,
            |transform| transform.position = position,
            |global| global.position = position,
        )
    }

    pub fn entity_scale(&self, id: EntityId) -> Option<[f32; 3]> {
        let key = self.world.key(id)?;
        self.world
            .get::<Transform>(key)
            .map(|transform| transform.scale)
    }

    pub fn set_entity_scale(&mut self, id: EntityId, scale: [f32; 3]) -> Result<(), String> {
        self.mutate(
            id,
            |transform| transform.scale = scale,
            |global| global.scale = scale,
        )
    }

    pub fn entity_rotation(&self, id: EntityId) -> Option<[f32; 4]> {
        let key = self.world.key(id)?;
        self.world
            .get::<Transform>(key)
            .map(|transform| transform.rotation)
    }

    pub fn set_entity_rotation(&mut self, id: EntityId, rotation: [f32; 4]) -> Result<(), String> {
        self.mutate(
            id,
            |transform| transform.rotation = rotation,
            |global| global.rotation = rotation,
        )
    }

    /// 绕世界轴旋转；本地四元数左乘轴角。
    pub fn rotate_entity(
        &mut self,
        id: EntityId,
        axis: [f32; 3],
        degrees: f32,
    ) -> Result<(), String> {
        let current = self.entity_rotation(id).unwrap_or([0.0, 0.0, 0.0, 1.0]);
        let rotation = quat_mul(axis_angle(axis, degrees.to_radians()), current);
        self.set_entity_rotation(id, rotation)
    }

    fn mutate(
        &mut self,
        id: EntityId,
        write: impl FnOnce(&mut Transform),
        write_global: impl FnOnce(&mut GlobalTransform),
    ) -> Result<(), String> {
        if self.play.is_some() {
            return Err("Play 期间不能编辑变换".to_string());
        }
        let key = self
            .world
            .key(id)
            .ok_or_else(|| format!("实体 {} 不存在", id.raw()))?;
        let Some(transform) = self.world.get_mut::<Transform>(key) else {
            return Err(format!("实体 {} 没有 Transform", id.raw()));
        };
        write(transform);
        if self.world.get::<Parent>(key).is_none() {
            if let Some(global) = self.world.get_mut::<GlobalTransform>(key) {
                write_global(global);
            }
        }
        self.dirty = true;
        Ok(())
    }
}

fn axis_angle(axis: [f32; 3], radians: f32) -> [f32; 4] {
    let length = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    if length <= 1e-8 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let half = radians * 0.5;
    let sin = half.sin() / length;
    [axis[0] * sin, axis[1] * sin, axis[2] * sin, half.cos()]
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// 吸附到最近的 step 倍数；step <= 0 时原样返回。
pub fn snap(value: f32, step: f32) -> f32 {
    if !step.is_finite() || step <= 0.0 {
        return value;
    }
    (value / step).round() * step
}
