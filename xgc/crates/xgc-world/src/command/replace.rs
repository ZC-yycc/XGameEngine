//! 回滚与实体整份替换的内部实现。

use super::{Command, EntityData, World};

impl World {
    pub(super) fn rollback(&mut self, inverse: &[Command]) -> Result<(), String> {
        for command in inverse.iter().rev() {
            match command {
                Command::SetField {
                    entity,
                    component,
                    field,
                    value,
                } => {
                    self.set_field_raw(*entity, *component, field, value)?;
                }
                Command::Spawn { data } => {
                    let snapshot = self.snapshot_from_data(data)?;
                    self.restore_entity_raw(snapshot)?;
                }
                Command::AddComponent { entity, component } => {
                    self.add_component_raw(*entity, component)?;
                }
                Command::RemoveComponent { entity, component } => {
                    self.remove_component_raw(*entity, *component)?;
                }
                Command::Replace { data } => {
                    let Some(original) = self.entity_data(data.id) else {
                        return Err(format!("回滚时替换目标不存在：{}", data.id));
                    };
                    self.replace_entity_raw(&original, data)?;
                }
                Command::Despawn { entity } => {
                    let Some(key) = self.key_of(*entity) else {
                        return Err(format!("回滚时实体不存在：{entity}"));
                    };
                    if !self.despawn_raw(key) {
                        return Err(format!("回滚 despawn 失败：{entity}"));
                    }
                }
            }
        }
        Ok(())
    }

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
