//! 旧 `LodGroup` RON → `render.lod` 组件数据。

use serde::Deserialize;
use xgc_world::{Component, ComponentData, LodGroup, LodLevel};

#[derive(Default, Deserialize)]
#[serde(default)]
struct OldLodGroup {
    levels: Vec<OldLodLevel>,
}

#[derive(Default, Deserialize)]
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
    let lod = LodGroup {
        levels: old
            .levels
            .into_iter()
            .map(|level| LodLevel {
                model: level.model,
                // 旧端把非有限距离当作无限级，JSON 用同义的 0 表示。
                max_distance: if level.max_distance.is_finite() {
                    level.max_distance
                } else {
                    0.0
                },
            })
            .collect(),
    };
    Ok(Some(ComponentData {
        component: LodGroup::ID,
        fields: vec![("levels".into(), lod.get_field("levels").unwrap())],
    }))
}
