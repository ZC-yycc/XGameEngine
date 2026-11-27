//! 脚本宿主与刚体固定步之间的瞬态输入，不进世界数据、快照或状态哈希。

use xgc_base::{EntityId, EntityKey};

use crate::World;

impl World {
    /// 脚本批次提交后按调用顺序交给下一固定步。记录句柄代际，避免已换代的刚体收到旧输入。
    pub fn queue_body_velocity(&mut self, entity: EntityId, linear: [f32; 3]) {
        if let Some(key) = self.key(entity) {
            self.body_velocities.push((entity, key, linear));
        }
    }

    /// 物理后端在求解前取走本批输入，保留缓冲供之后的步骤复用。
    pub fn drain_body_velocities(
        &mut self,
    ) -> impl Iterator<Item = (EntityId, EntityKey, [f32; 3])> + '_ {
        self.body_velocities.drain(..)
    }
}
