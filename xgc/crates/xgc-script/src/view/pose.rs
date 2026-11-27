use xgc_base::EntityId;
use xgc_world::{to_local_position, to_world_delta, GlobalTransform};

use super::ScriptView;

impl ScriptView {
    pub(crate) fn world_position(&mut self, id: u64) -> Option<[f32; 3]> {
        let row = self.entity(id)?;
        let Some(local) = row.position else {
            return Some([0.0; 3]);
        };
        let Some(parent) = row.parent else {
            return Some(local);
        };
        // 逐级作用于点，祖先的非均匀缩放不会被压成一个 TRS 后丢掉剪切项。
        Some(self.parents(parent).iter().fold(local, |point, parent| {
            let offset = to_world_delta(parent, point);
            std::array::from_fn(|axis| parent.position[axis] + offset[axis])
        }))
    }

    pub(crate) fn local_position(&mut self, id: u64, world: [f32; 3]) -> [f32; 3] {
        let Some(parent) = self.entity(id).and_then(|row| row.parent) else {
            return world;
        };
        let parents = self.parents(parent);
        // 和旧端的世界点逆变换同样处理奇异父链。用 f64 做判据，避免中间缩放的 f32 溢出。
        let determinant: f64 = parents
            .iter()
            .flat_map(|pose| pose.scale)
            .map(f64::from)
            .product();
        if !determinant.is_finite() || determinant.abs() <= 1e-12 {
            return world;
        }
        parents
            .iter()
            .rev()
            .fold(world, |point, parent| to_local_position(parent, point))
    }

    pub(crate) fn local_rotation(&mut self, id: u64, world: [f32; 4]) -> [f32; 4] {
        let Some(parent) = self.entity(id).and_then(|row| row.parent) else {
            return world;
        };
        // 相机读取层级传播的四元数，所以这里按同一旋转顺序抵消父级。
        self.parents(parent)
            .iter()
            .rev()
            .fold(world, |rotation, parent| parent.to_local_rotation(rotation))
    }

    fn parents(&mut self, mut id: EntityId) -> &[GlobalTransform] {
        // 读当前暂存的局部姿态。派生 GlobalTransform 要等脚本宿主结束之后才更新。
        self.parent_transforms.clear();
        // Brent 判环复用正在遍历的父链，遇到小环时无需按世界大小扩展缓冲。
        let mut checkpoint = id;
        let mut span = 1_usize;
        let mut hops = 0;
        while let Some(row) = self.entity(id.raw()) {
            let Some(position) = row.position else {
                break;
            };
            let pose = GlobalTransform {
                position,
                rotation: row.rotation,
                scale: row.scale.unwrap_or([1.0; 3]),
            };
            let next = row.parent;
            self.parent_transforms.push(pose);
            let Some(parent) = next else {
                break;
            };
            if parent == checkpoint {
                // 环上没有世界基准，沿用调用目标的局部坐标。
                self.parent_transforms.clear();
                break;
            }
            hops += 1;
            if hops == span {
                checkpoint = parent;
                span = span.saturating_mul(2);
                hops = 0;
            }
            id = parent;
        }
        &self.parent_transforms
    }
}
