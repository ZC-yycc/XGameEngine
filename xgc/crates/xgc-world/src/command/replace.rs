//! 实体整份替换的内部实现。

use super::{EntityData, World};

impl World {
    pub(super) fn replace_entity_raw(
        &mut self,
        original: &EntityData,
        updated: &EntityData,
    ) -> Result<(), String> {
        let Some(key) = self.key_of(original.id) else {
            return Err(format!("实体不存在：{}", original.id));
        };
        let snapshot = self.snapshot_from_data(updated)?;
        if !self.despawn_raw(key) {
            return Err(format!("移除旧实体失败：{}", original.id));
        }
        if let Err(message) = self.restore_entity_raw(snapshot) {
            if let Ok(original_snapshot) = self.snapshot_from_data(original) {
                let _ = self.restore_entity_raw(original_snapshot);
            }
            return Err(message);
        }
        Ok(())
    }
}
