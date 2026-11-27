//! Agent 世界快照与恢复。

use serde_json::{json, Value as Json};

use super::{query::QueryEpoch, Agent};
use xgc_world::{Journal, World, WorldData};

impl Agent {
    pub(crate) fn snapshot(&self) -> Result<Json, String> {
        Ok(json!({
            "revision": self.world.revision(),
            "data": self.world.world_data(),
        }))
    }

    pub(crate) fn restore(&mut self, params: &Json) -> Result<Json, String> {
        let data = params
            .get("data")
            .ok_or_else(|| "world.restore 缺少 data".to_string())?
            .clone();
        let data: WorldData =
            serde_json::from_value(data).map_err(|err| format!("快照解析失败：{err}"))?;
        self.replace_edit_world(&data, "恢复快照失败")
    }

    pub(super) fn replace_edit_world(
        &mut self,
        data: &WorldData,
        failure: &str,
    ) -> Result<Json, String> {
        // 组件与实体身份全部恢复成功后，才换掉编辑会话和它的历史基线。
        let mut world = World::new();
        world
            .restore_world_data(data)
            .map_err(|message| format!("{failure}：{message}"))?;
        self.base = world.snapshot();
        self.world = world;
        self.query_epoch = QueryEpoch::new();
        self.project_scene = None;
        self.plans.clear();
        self.applied.clear();
        self.apply_receipts.clear();
        self.journal = Journal::new();
        Ok(json!({
            "revision": self.world.revision(),
            "entities": self.world.len(),
        }))
    }
}
