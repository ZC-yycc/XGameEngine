//! 旧 XGameEngine `SceneV2` RON → XGC `WorldData` 的第一层迁移。
//!
//! 这一层只搬实体身份、名称、父子关系、`Transform`，并补默认
//! `GlobalTransform` 让层级系统能工作。Mesh / Material / Camera 等旧组件先不搬，
//! 迁移报告里给出数量，后续按组件逐步补齐，不造"看起来完整"的假数据。

#![deny(unsafe_code)]

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use xgc_base::EntityId;
use xgc_world::{
    Component, ComponentData, ComponentId, EntityData, GlobalTransform, Name, Parent, Transform,
    Value, WorldData,
};

pub const OLD_SCENE_FORMAT_VERSION: u32 = 2;

#[derive(Debug)]
pub enum MigrationError {
    Parse(String),
    UnsupportedVersion(u32),
    DuplicateId(u64),
    Io(String),
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(message) => write!(f, "场景解析失败：{message}"),
            Self::UnsupportedVersion(version) => {
                write!(
                    f,
                    "不支持的旧场景版本：{version}（只认 {OLD_SCENE_FORMAT_VERSION}）"
                )
            }
            Self::DuplicateId(id) => write!(f, "旧场景实体 ID 重复：{id}"),
            Self::Io(message) => write!(f, "迁移 I/O 失败：{message}"),
        }
    }
}

impl std::error::Error for MigrationError {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MigrationReport {
    pub entities: usize,
    pub with_old_components: usize,
    pub missing_parents: usize,
}

#[derive(Debug, Deserialize)]
struct OldScene {
    format_version: u32,
    #[serde(default)]
    #[allow(dead_code)]
    name: String,
    #[serde(default)]
    entities: Vec<OldEntity>,
}

#[derive(Debug, Deserialize)]
struct OldEntity {
    id: OldId,
    #[serde(default)]
    name: String,
    #[serde(default)]
    parent: Option<OldId>,
    transform: OldTransform,
    #[allow(dead_code)]
    components: ron::Value,
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct OldId(u64);

#[derive(Debug, Deserialize)]
struct OldTransform {
    position: glam::Vec3,
    rotation: glam::Quat,
    scale: glam::Vec3,
}

pub fn migrate_str(text: &str) -> Result<(WorldData, MigrationReport), MigrationError> {
    let scene: OldScene =
        ron::from_str(text).map_err(|err| MigrationError::Parse(err.to_string()))?;
    if scene.format_version != OLD_SCENE_FORMAT_VERSION {
        return Err(MigrationError::UnsupportedVersion(scene.format_version));
    }

    let mut seen = HashSet::new();
    for entity in &scene.entities {
        if !seen.insert(entity.id.0) {
            return Err(MigrationError::DuplicateId(entity.id.0));
        }
    }

    let mut report = MigrationReport {
        entities: scene.entities.len(),
        ..MigrationReport::default()
    };
    let mut entities = Vec::with_capacity(scene.entities.len());
    let mut max_id = 0;

    for old in &scene.entities {
        max_id = max_id.max(old.id.0);
        report.with_old_components += 1;

        let position = old.transform.position.to_array();
        let rotation = old.transform.rotation.to_array();
        let scale = old.transform.scale.to_array();
        let mut components = vec![
            ComponentData {
                component: Transform::ID,
                fields: vec![
                    (
                        "position".to_string(),
                        Value::Vec3([
                            f64::from(position[0]),
                            f64::from(position[1]),
                            f64::from(position[2]),
                        ]),
                    ),
                    (
                        "rotation".to_string(),
                        Value::Quat([
                            f64::from(rotation[0]),
                            f64::from(rotation[1]),
                            f64::from(rotation[2]),
                            f64::from(rotation[3]),
                        ]),
                    ),
                    (
                        "scale".to_string(),
                        Value::Vec3([
                            f64::from(scale[0]),
                            f64::from(scale[1]),
                            f64::from(scale[2]),
                        ]),
                    ),
                ],
            },
            ComponentData {
                component: Name::ID,
                fields: vec![("value".to_string(), Value::String(old.name.clone()))],
            },
            ComponentData {
                component: GlobalTransform::ID,
                fields: Vec::new(),
            },
        ];

        if let Some(parent) = old.parent {
            if !seen.contains(&parent.0) {
                report.missing_parents += 1;
            } else {
                components.push(ComponentData {
                    component: Parent::ID,
                    fields: vec![("entity".to_string(), Value::Entity(parent.0))],
                });
            }
        }

        entities.push(EntityData {
            id: EntityId::from_raw(old.id.0),
            revision: 0,
            components,
        });
    }

    Ok((
        WorldData {
            revision: 0,
            next_id: max_id.saturating_add(1),
            entities,
        },
        report,
    ))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MigrationBatch {
    pub scenes: usize,
    pub entities: usize,
    pub with_old_components: usize,
    pub missing_parents: usize,
    pub failures: Vec<String>,
}

/// 批量迁移目录里的 `.ron`；单场景失败只记入 `failures`，不中断其它场景。
pub fn migrate_dir(input: &Path, output: &Path) -> Result<MigrationBatch, MigrationError> {
    let entries = std::fs::read_dir(input)
        .map_err(|err| MigrationError::Io(format!("读取目录 {}：{err}", input.display())))?;
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "ron"))
        .collect();
    files.sort();

    std::fs::create_dir_all(output)
        .map_err(|err| MigrationError::Io(format!("创建目录 {}：{err}", output.display())))?;

    let mut batch = MigrationBatch::default();
    for path in files {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                batch
                    .failures
                    .push(format!("{}：读取失败 {err}", path.display()));
                continue;
            }
        };
        match migrate_str(&text) {
            Ok((data, report)) => {
                let stem = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "scene".to_string());
                let target = output.join(format!("{stem}.json"));
                if let Err(err) = xgc_store::save_world(&target, &data) {
                    batch
                        .failures
                        .push(format!("{}：写入失败 {err}", path.display()));
                    continue;
                }
                batch.scenes += 1;
                batch.entities += report.entities;
                batch.with_old_components += report.with_old_components;
                batch.missing_parents += report.missing_parents;
            }
            Err(err) => batch.failures.push(format!("{}：{err}", path.display())),
        }
    }
    Ok(batch)
}

/// 只给测试 / 诊断用：组件 ID 是稳定协议，不要在迁移层重新分配。
pub fn component_id_name(id: ComponentId) -> &'static str {
    match id {
        Transform::ID => "core.transform",
        Name::ID => "core.name",
        Parent::ID => "core.parent",
        GlobalTransform::ID => "core.global_transform",
        _ => "unknown",
    }
}
