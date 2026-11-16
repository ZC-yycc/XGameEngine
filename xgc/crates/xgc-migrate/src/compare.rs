//! 旧引擎 `--headless` JSON 与 XGC 世界的统计对照。

use serde::Serialize;
use xgc_world::WorldData;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Stats {
    pub entities: usize,
    pub bodies: usize,
    pub colliders: usize,
    pub emitters: usize,
    pub audio_active: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CompareReport {
    pub old: Stats,
    pub new: Stats,
    pub differences: Vec<String>,
}

/// 读取旧引擎 headless JSON 的 `entities / bodies / colliders / particles.emitters / audio.active`。
pub fn compare_headless(old_json: &str, world: &WorldData) -> Result<CompareReport, Vec<String>> {
    let old_json = old_json.trim_start_matches('\u{feff}');
    let value: serde_json::Value = serde_json::from_str(old_json)
        .map_err(|err| vec![format!("旧 headless JSON 解析失败：{err}")])?;
    let old = Stats {
        entities: number(&value, "entities")?,
        bodies: number(&value, "bodies")?,
        colliders: number(&value, "colliders")?,
        emitters: number(&value["particles"], "emitters")?,
        audio_active: number(&value["audio"], "active")?,
    };
    let new = world_stats(world);
    let mut differences = Vec::new();
    for (name, old, new) in [
        ("entities", old.entities, new.entities),
        ("bodies", old.bodies, new.bodies),
        ("colliders", old.colliders, new.colliders),
        ("emitters", old.emitters, new.emitters),
        ("audio_active", old.audio_active, new.audio_active),
    ] {
        if old != new {
            differences.push(format!("{name}: 旧 {old} / 新 {new}"));
        }
    }
    Ok(CompareReport {
        old,
        new,
        differences,
    })
}

pub fn world_stats(world: &WorldData) -> Stats {
    let mut stats = Stats {
        entities: world.entities.len(),
        bodies: 0,
        colliders: 0,
        emitters: 0,
        audio_active: 0,
    };
    for entity in &world.entities {
        for component in &entity.components {
            match component.component.0 {
                0x0401 => {
                    stats.bodies += 1;
                    stats.colliders += 1;
                }
                0x0701 => stats.emitters += 1,
                0x0302 => stats.audio_active += 1,
                _ => {}
            }
        }
    }
    stats
}

fn number(value: &serde_json::Value, field: &str) -> Result<usize, Vec<String>> {
    value
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .map(|value| value as usize)
        .ok_or_else(|| vec![format!("旧 headless JSON 缺少数值字段 {field}")])
}
