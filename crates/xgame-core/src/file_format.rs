//! 场景与预制体各自携带文件版本；旧版 serde 入口不能把未来组件静默丢弃。

use std::path::Path;

use serde::{Deserialize, Deserializer};

pub const V1: u32 = 1;

pub const fn is_v1(version: &u32) -> bool {
    *version == V1
}

pub fn deserialize_v1<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if is_v1(&version) {
        Ok(version)
    } else {
        Err(serde::de::Error::custom(format!(
            "不支持文件格式版本 {version}；请使用支持该版本的引擎"
        )))
    }
}

#[derive(Deserialize)]
struct VersionProbe {
    #[serde(default = "v1")]
    format_version: u32,
}

pub const fn v1() -> u32 {
    V1
}

/// 编辑器可能在文件被外部升级后仍持有旧的内存副本，写前重新检查磁盘版本。
pub fn disk_version(path: &Path) -> Result<Option<u32>, std::io::Error> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    // 损坏的旧文件仍可由编辑器手动修复；只在能读到明确版本时禁止旧格式覆写。
    Ok(ron::from_str::<VersionProbe>(&text)
        .ok()
        .map(|probe| probe.format_version))
}
