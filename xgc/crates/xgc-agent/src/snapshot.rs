//! Agent 世界快照与恢复。

use serde_json::{json, Value as Json};

use super::Agent;
use xgc_world::{Journal, WorldData};

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
        self.world
            .restore_world_data(&data)
            .map_err(|message| format!("恢复快照失败：{message}"))?;
        self.plans.clear();
        self.applied.clear();
        self.journal = Journal::new();
        self.base = self.world.snapshot();
        Ok(json!({
            "revision": self.world.revision(),
            "entities": self.world.len(),
        }))
    }
}
