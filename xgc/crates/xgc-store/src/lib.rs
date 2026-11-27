//! XGC 持久化：版本化 JSON 包络 + 原子写。
//!
//! 落盘入口只有这里：先写同目录临时文件、`sync_all`、再 rename。Windows 上
//! `rename` 不覆盖已存在文件，因此目标存在时先删再 rename；这个窗口不是掉电
//! 原子交换，和旧引擎 `fsx` 的边界一致，后续有目录事务时再补。

#![deny(unsafe_code)]

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use xgc_world::{Journal, WorldData};

// Lua 挂载按列表顺序保存，世界、预制体和日志共享此字段结构。
const FORMAT_VERSION: u32 = 6;

mod binary;
pub use binary::{decode_world, encode_world, load_world_binary, save_world_binary};

mod project;
pub use project::{
    load_project, save_project, ProjectContent, ProjectManifest, ProjectSession,
    PROJECT_FORMAT_VERSION,
};

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Format(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "IO 错误：{err}"),
            Self::Json(err) => write!(f, "JSON 错误：{err}"),
            Self::Format(message) => write!(f, "格式错误：{message}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Json(err) => Some(err),
            Self::Format(_) => None,
        }
    }
}

impl From<std::io::Error> for StoreError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err)
    }
}

type Result<T> = std::result::Result<T, StoreError>;

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    format_version: u32,
    kind: String,
    payload: T,
}

pub fn save_world(path: &Path, data: &WorldData) -> Result<()> {
    save_world_with_bytes(path, data).map(drop)
}

/// 返回同一次保存的原文，供编辑器建立磁盘基准。
pub fn save_world_with_bytes(path: &Path, data: &WorldData) -> Result<Vec<u8>> {
    save(path, "world", data)
}

pub fn load_world(path: &Path) -> Result<WorldData> {
    load_world_with_bytes(path).map(|(data, _)| data)
}

pub fn load_world_with_bytes(path: &Path) -> Result<(WorldData, Vec<u8>)> {
    load_with_bytes(path, "world")
}

pub fn save_prefab(path: &Path, data: &WorldData) -> Result<()> {
    save(path, "prefab", data).map(drop)
}

/// 用加载时的原文检查提交前的资产版本，再走同一暂存替换入口。
/// 跨进程的写入仍可发生在检查与替换之间。
pub fn save_prefab_if_unchanged(path: &Path, data: &WorldData, expected: &[u8]) -> Result<()> {
    let bytes = encode("prefab", data)?;
    // serde_json 会把非有限浮点写成 null；必须在替换前确认写出的资产仍能加载。
    decode::<WorldData>(&bytes, "prefab")?;
    write_atomic_checked(path, &bytes, Some(expected))
}

pub fn load_prefab(path: &Path) -> Result<WorldData> {
    load_prefab_with_bytes(path).map(|(data, _)| data)
}

pub fn load_prefab_with_bytes(path: &Path) -> Result<(WorldData, Vec<u8>)> {
    load_with_bytes(path, "prefab")
}

pub fn save_journal(path: &Path, journal: &Journal) -> Result<()> {
    save(path, "journal", journal).map(drop)
}

pub fn load_journal(path: &Path) -> Result<Journal> {
    load_with_bytes(path, "journal").map(|(data, _)| data)
}

fn save<T: Serialize>(path: &Path, kind: &str, payload: &T) -> Result<Vec<u8>> {
    let bytes = encode(kind, payload)?;
    write_atomic(path, &bytes)?;
    Ok(bytes)
}

fn encode<T: Serialize>(kind: &str, payload: &T) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(&Envelope {
        format_version: FORMAT_VERSION,
        kind: kind.to_string(),
        payload,
    })?)
}

fn load_with_bytes<T: DeserializeOwned>(path: &Path, expected_kind: &str) -> Result<(T, Vec<u8>)> {
    let bytes = fs::read_to_string(path)?.into_bytes();
    let data = decode(&bytes, expected_kind)?;
    Ok((data, bytes))
}

fn decode<T: DeserializeOwned>(bytes: &[u8], expected_kind: &str) -> Result<T> {
    let envelope: Envelope<T> = serde_json::from_slice(bytes)?;
    if envelope.format_version != FORMAT_VERSION {
        return Err(StoreError::Format(format!(
            "格式版本 {} 不受支持（当前 {FORMAT_VERSION}）",
            envelope.format_version
        )));
    }
    if envelope.kind != expected_kind {
        return Err(StoreError::Format(format!(
            "文件类型是 {}，期望 {expected_kind}",
            envelope.kind
        )));
    }
    Ok(envelope.payload)
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_checked(path, bytes, None)
}

fn write_atomic_checked(path: &Path, bytes: &[u8], expected: Option<&[u8]>) -> Result<()> {
    if let Some(expected) = expected {
        check_prefab_bytes(path, expected)?;
        if expected == bytes {
            return Ok(());
        }
    } else if fs::read(path).is_ok_and(|existing| existing == bytes) {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let temp = temp_path(path);
    let mut file = File::create(&temp)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);

        if let Some(expected) = expected {
            check_prefab_bytes(path, expected)?;
        }
        match fs::rename(&temp, path) {
            // Apply 的条件替换失败时，旧资产仍须完整留在原处。
            Err(err)
                if expected.is_none()
                    && matches!(
                        err.kind(),
                        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
                    ) =>
            {
                match fs::remove_file(path) {
                    Ok(()) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => return Err(err.into()),
                }
                fs::rename(&temp, path)?;
            }
            result => result?,
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn check_prefab_bytes(path: &Path, expected: &[u8]) -> Result<()> {
    if fs::read(path)? != expected {
        return Err(StoreError::Format(
            "预制体已被外部修改；请重试 Apply".into(),
        ));
    }
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "xgc".to_string());
    path.with_file_name(format!("{name}.tmp.{}.{counter}", std::process::id()))
}
