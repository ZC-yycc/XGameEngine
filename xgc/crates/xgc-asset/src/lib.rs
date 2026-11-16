//! 资产目录：扫描项目里的 `.meta`，把 `guid://…` 映射到实际文件路径。
//!
//! 只读、确定性：按路径排序遍历，重复 GUID 记录 issue 并保留第一条；不生成 /
//! 改写 `.meta`。

#![deny(unsafe_code)]

mod cook;
pub use cook::{cook_project, CookManifest, CookedFile};

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug)]
pub enum AssetError {
    Io(String),
}

impl fmt::Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) => write!(f, "资产目录读取失败：{message}"),
        }
    }
}

impl std::error::Error for AssetError {}

#[derive(Clone, Debug, Default)]
pub struct AssetCatalog {
    by_guid: BTreeMap<String, PathBuf>,
    issues: Vec<String>,
}

impl AssetCatalog {
    pub fn len(&self) -> usize {
        self.by_guid.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_guid.is_empty()
    }

    pub fn issues(&self) -> &[String] {
        &self.issues
    }

    /// `guid://<hex>` 或裸 `<hex>`；只返回已登记资产。
    pub fn resolve(&self, reference: &str) -> Option<&Path> {
        let guid = reference.strip_prefix("guid://").unwrap_or(reference);
        self.by_guid.get(guid).map(PathBuf::as_path)
    }

    pub fn contains(&self, guid: &str) -> bool {
        let guid = guid.strip_prefix("guid://").unwrap_or(guid);
        self.by_guid.contains_key(guid)
    }
}

pub fn scan(root: &Path) -> Result<AssetCatalog, AssetError> {
    let mut catalog = AssetCatalog::default();
    let mut metas = Vec::new();
    collect_meta(root, &mut metas)
        .map_err(|err| AssetError::Io(format!("{}：{err}", root.display())))?;
    metas.sort();

    for meta_path in metas {
        let text = match std::fs::read_to_string(&meta_path) {
            Ok(text) => text,
            Err(err) => {
                catalog
                    .issues
                    .push(format!("{}：读取失败 {err}", meta_path.display()));
                continue;
            }
        };
        let meta: MetaFile = match ron::from_str(&text) {
            Ok(meta) => meta,
            Err(err) => {
                catalog
                    .issues
                    .push(format!("{}：解析失败 {err}", meta_path.display()));
                continue;
            }
        };
        let asset_path = meta_path.with_extension("");
        if let Some(existing) = catalog.by_guid.get(&meta.guid) {
            catalog.issues.push(format!(
                "GUID {} 重复：{} 与 {}",
                meta.guid,
                existing.display(),
                asset_path.display()
            ));
            continue;
        }
        catalog.by_guid.insert(meta.guid, asset_path);
    }
    Ok(catalog)
}

#[derive(Debug, Deserialize)]
struct MetaFile {
    guid: String,
}

fn collect_meta(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_meta(&path, out)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "meta")
        {
            out.push(path);
        }
    }
    Ok(())
}
