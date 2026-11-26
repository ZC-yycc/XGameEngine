//! 资产目录：扫描项目里的 `.meta`，把 `guid://…` 映射到实际文件路径。
//!
//! 只读、确定性：按路径排序遍历，重复 GUID 记录 issue 并保留第一条；不生成 /
//! 改写 `.meta`。

#![deny(unsafe_code)]

mod cook;
mod read_path;

pub use cook::{
    cook_project, cook_project_filtered, cook_project_with_dependencies, cook_project_with_runtime,
    file_entry, verify_manifest, write_manifest, CookManifest, CookRuntime, CookedFile,
};
pub use read_path::{resolve_read_path, AssetPaths, ReadPathError, SearchPaths};

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

    /// 按扫描时的路径查找对应资产身份，供从文件浏览器选中的路径使用。
    pub fn guid_for_path(&self, path: &Path) -> Option<&str> {
        self.by_guid
            .iter()
            .find_map(|(guid, asset)| (asset == path).then_some(guid.as_str()))
    }

    pub fn contains(&self, guid: &str) -> bool {
        let guid = guid.strip_prefix("guid://").unwrap_or(guid);
        self.by_guid.contains_key(guid)
    }
}

pub fn scan(root: &Path) -> Result<AssetCatalog, AssetError> {
    let metas = find_files(root, is_meta)
        .map_err(|err| AssetError::Io(format!("{}：{err}", root.display())))?;
    Ok(catalog_from_metas(metas.iter().map(PathBuf::as_path)))
}

fn is_meta(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "meta")
}

fn catalog_from_metas<'a>(metas: impl IntoIterator<Item = &'a Path>) -> AssetCatalog {
    let mut catalog = AssetCatalog::default();
    for meta_path in metas {
        let text = match std::fs::read_to_string(meta_path) {
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
    catalog
}

#[derive(Debug, Deserialize)]
struct MetaFile {
    guid: String,
}

/// 按路径排序枚举普通资产文件。链接可能越出项目或形成环，扫描不跟随。
pub fn find_files(dir: &Path, matches: impl Fn(&Path) -> bool) -> std::io::Result<Vec<PathBuf>> {
    find_files_in(dir, |_| true, matches)
}

fn find_files_in(
    dir: &Path,
    descend: impl Fn(&Path) -> bool,
    matches: impl Fn(&Path) -> bool,
) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut directories = vec![dir.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                // junction 可以绕过 is_symlink，目录和文件都检查属性。
                const REPARSE_POINT: u32 = 0x400;
                if entry.metadata()?.file_attributes() & REPARSE_POINT != 0 {
                    continue;
                }
            }
            let path = entry.path();
            if kind.is_dir() && descend(&path) {
                directories.push(path);
            } else if kind.is_file() && matches(&path) {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}
