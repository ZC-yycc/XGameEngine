//! 新建实体在编辑世界提交前向项目领取身份。

use xgc_base::EntityId;
use xgc_world::{Name, Transform};

use crate::EditorState;

impl EditorState {
    /// 新建实体（Name + Transform），返回持久 ID。
    pub fn spawn_entity(&mut self, name: &str) -> Result<EntityId, String> {
        if self.is_playing() {
            return Err("Play 期间不能新建实体".into());
        }
        let local = self.world.next_entity_id();
        let id = match &mut self.project {
            Some(project) => {
                let ids = project
                    .reserve_entity_ids(1, local)
                    .map_err(|err| format!("新建实体失败：{err}"))?;
                EntityId::from_raw(ids.start)
            }
            None => local,
        };
        id.validate()
            .map_err(|err| format!("新建实体失败：{err}"))?;
        let before = self.world.world_data();
        // 共享水位可能已被另一场景推进，先跳过这些项目内已分配的 ID。
        self.world.reserve_entity_ids(id.raw() - local.raw());
        let key = self
            .world
            .spawn((Name { value: name.into() }, Transform::default()));
        debug_assert_eq!(self.world.entity(key), Some(id));
        self.record_undo(before);
        self.selected = Some(id);
        self.dirty = true;
        Ok(id)
    }
}
