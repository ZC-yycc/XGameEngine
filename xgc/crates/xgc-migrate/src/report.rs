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
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MigrationBatch {
    pub scenes: usize,
    pub entities: usize,
    pub with_old_components: usize,
    pub mapped_components: usize,
    pub missing_parents: usize,
    pub failures: Vec<String>,
}
