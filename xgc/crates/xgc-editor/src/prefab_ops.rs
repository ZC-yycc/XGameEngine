//! 编辑器预制体：读取旧 PrefabV2 → 分配新 ID → 一次性 Spawn。

use std::path::Path;

use xgc_base::EntityId;
use xgc_migrate::{instantiate_prefab, migrate_str, revert_prefab};

use crate::EditorState;

impl EditorState {
    pub fn instantiate_prefab_file(&mut self, path: &Path) -> Result<EntityId, String> {
        if self.play.is_some() {
            return Err("Play 期间不能实例化预制体".to_string());
        }
        let text = std::fs::read_to_string(path).map_err(|err| format!("读取预制体失败：{err}"))?;
        let (data, _report) =
            migrate_str(&text).map_err(|err| format!("预制体迁移失败：{err:?}"))?;
        let root_old = data
            .entities
            .iter()
            .find(|entity| {
                !entity
                    .components
                    .iter()
                    .any(|component| component.component.0 == 0x0104)
            })
            .map(|entity| entity.id)
            .ok_or_else(|| "预制体没有根实体".to_string())?;
        self.checkpoint();
        let mapping = instantiate_prefab(&mut self.world, &data, Some(&path.to_string_lossy()))?;
        let root = mapping
            .iter()
            .find(|(old, _)| *old == root_old)
            .map(|(_, new)| *new)
            .ok_or_else(|| "预制体根实体没有新 ID".to_string())?;
        self.selected = Some(root);
        self.dirty = true;
        Ok(root)
    }
    pub fn revert_prefab_file(&mut self, path: &Path) -> Result<usize, String> {
        if self.play.is_some() {
            return Err("Play 期间不能 Revert 预制体".to_string());
        }
        let text = std::fs::read_to_string(path).map_err(|err| format!("读取预制体失败：{err}"))?;
        let (data, _report) =
            migrate_str(&text).map_err(|err| format!("预制体迁移失败：{err:?}"))?;
        let source = path.to_string_lossy().to_string();
        self.checkpoint();
        let mapping = revert_prefab(&mut self.world, &data, &source)?;
        if let Some((_, root)) = mapping.first() {
            self.selected = Some(*root);
        }
        self.dirty = true;
        Ok(mapping.len())
    }
}
