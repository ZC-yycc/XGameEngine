use std::fmt;

use crate::OLD_SCENE_FORMAT_VERSION;

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
    pub mapped_components: usize,
    pub missing_parents: usize,
    unattached_lua: Vec<UnattachedLua>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct UnattachedLua {
    entity: u64,
    retained: String,
    skipped: Vec<String>,
}

impl MigrationReport {
    pub(crate) fn map_lua<'a>(&mut self, entity: u64, paths: &[&'a str]) -> Option<&'a str> {
        // 和现有世界投影一样保留最后一项。报告由同一次选择生成，避免和落盘结果分叉。
        let (&retained, skipped) = paths.split_last()?;
        self.mapped_components += 1;
        if !skipped.is_empty() {
            self.unattached_lua.push(UnattachedLua {
                entity,
                retained: retained.to_string(),
                skipped: skipped.iter().map(|path| (*path).to_string()).collect(),
            });
        }
        Some(retained)
    }

    pub fn unmatched<'a>(&'a self, source: &'a str) -> impl Iterator<Item = String> + 'a {
        self.unattached_lua.iter().map(move |loss| {
            format!(
                "[migration.lua.attachment] {source} 实体 {} 保留 {:?}，未挂载 {:?}。需将遗漏的行为合并到保留脚本",
                loss.entity, loss.retained, loss.skipped
            )
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MigrationBatch {
    pub scenes: usize,
    pub entities: usize,
    pub with_old_components: usize,
    pub mapped_components: usize,
    pub missing_parents: usize,
    pub unmatched: Vec<String>,
    pub failures: Vec<String>,
}
