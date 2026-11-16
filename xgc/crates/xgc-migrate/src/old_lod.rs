//! 旧 `LodGroup` RON → `render.lod` 组件数据。

use serde::{Deserialize, Serialize};
use xgc_world::{Component, ComponentData, LodGroup, Value};

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct OldLodGroup {
    levels: Vec<OldLodLevel>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct OldLodLevel {
    model: String,
    max_distance: f32,
}

pub fn convert(value: ron::Value) -> Result<Option<ComponentData>, String> {
    let old: OldLodGroup = value
        .into_rust()
        .map_err(|err| format!("render.lod 解析失败：{err}"))?;
    if old.levels.iter().all(|level| level.model.is_empty()) {
        return Ok(None);
    }
    let levels =
        ron::to_string(&old.levels).map_err(|err| format!("render.lod 序列化失败：{err}"))?;
    Ok(Some(ComponentData {
        component: LodGroup::ID,
        fields: vec![("levels".to_string(), Value::String(levels))],
    }))
}
