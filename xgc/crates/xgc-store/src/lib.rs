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

const FORMAT_VERSION: u32 = 1;

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
    save(path, "world", data)
}

pub fn load_world(path: &Path) -> Result<WorldData> {
    load(path, "world")
}

pub fn save_prefab(path: &Path, data: &WorldData) -> Result<()> {
    save(path, "prefab", data)
}

pub fn load_prefab(path: &Path) -> Result<WorldData> {
    load(path, "prefab")
}

pub fn save_journal(path: &Path, journal: &Journal) -> Result<()> {
    save(path, "journal", journal)
}

pub fn load_journal(path: &Path) -> Result<Journal> {
    load(path, "journal")
}

fn save<T: Serialize>(path: &Path, kind: &str, payload: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(&Envelope {
        format_version: FORMAT_VERSION,
        kind: kind.to_string(),
        payload,
    })?;
    write_atomic(path, text.as_bytes())
}

fn load<T: DeserializeOwned>(path: &Path, expected_kind: &str) -> Result<T> {
    let text = fs::read_to_string(path)?;
    let envelope: Envelope<T> = serde_json::from_str(&text)?;
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
    if let Ok(existing) = fs::read(path) {
        if existing == bytes {
            return Ok(());
        }
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let temp = temp_path(path);
    let mut file = File::create(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);

    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(err)
            if err.kind() == std::io::ErrorKind::AlreadyExists
                || err.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(remove_err) if remove_err.kind() == std::io::ErrorKind::NotFound => {}
                Err(remove_err) => {
                    let _ = fs::remove_file(&temp);
                    return Err(remove_err.into());
                }
            }
            fs::rename(&temp, path)?;
            Ok(())
        }
        Err(err) => {
            let _ = fs::remove_file(&temp);
            Err(err.into())
        }
    }
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
