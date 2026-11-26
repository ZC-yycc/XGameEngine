//! 项目文件引用的读取入口，供运行时、体检和打包共用查找顺序。

use std::fmt;
use std::path::{Path, PathBuf};

use xgc_base::path::{normalize_project_relative, PathDefect};

use crate::AssetCatalog;

/// 清单声明的读取兜底根。宿主打开项目时准备一次，查找时只探测磁盘。
#[derive(Clone, Debug, Default)]
pub struct SearchPaths {
    directories: Vec<String>,
}

impl SearchPaths {
    /// 按声明顺序规范化目录，跳过非法项。合法目录可以在查找时才出现。
    pub fn new(paths: &[String]) -> Self {
        Self {
            directories: paths
                .iter()
                .filter_map(|path| normalize_project_relative(path).ok())
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadPathError {
    Invalid(PathDefect),
    Missing {
        relative: String,
        /// 项目相对候选，首项为直达路径，其余按清单顺序排列。
        tried: Vec<String>,
    },
    GuidRequiresCatalog,
    UnknownGuid(String),
    MissingGuidFile {
        reference: String,
        path: PathBuf,
    },
}

impl fmt::Display for ReadPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(defect) => write!(f, "{defect}"),
            Self::Missing { relative, tried } => {
                write!(f, "文件不存在：{relative}")?;
                if tried.len() > 1 {
                    write!(f, "（已按 search_paths 试过：{}）", tried.join(", "))?;
                }
                Ok(())
            }
            Self::GuidRequiresCatalog => write!(f, "guid:// 需要资产目录"),
            Self::UnknownGuid(reference) => write!(f, "资产目录没有 {reference}"),
            Self::MissingGuidFile { reference, path } => {
                write!(f, "{reference} 对应的文件不存在：{}", path.display())
            }
        }
    }
}

impl std::error::Error for ReadPathError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid(defect) => Some(defect),
            _ => None,
        }
    }
}

/// 查找项目中的文件。普通引用先规范化，再探测直达路径和有序的兜底根。
///
/// `catalog` 可借用宿主对同一项目的扫描结果。`guid://` 直接用目录登记的路径，
/// 因为登记的是文件的物理位置。返回值已含项目根，保留根的相对或绝对形式，
/// 调用方可直接读盘或用 `strip_prefix(root)` 取得打包用的相对路径。
/// 路径规则为词法校验，文件探测沿用 `Path::is_file` 的链接语义。
pub fn resolve_read_path(
    root: &Path,
    reference: &str,
    search_paths: &SearchPaths,
    catalog: Option<&AssetCatalog>,
) -> Result<PathBuf, ReadPathError> {
    if reference.starts_with("guid://") {
        let catalog = catalog.ok_or(ReadPathError::GuidRequiresCatalog)?;
        let path = catalog
            .resolve(reference)
            .ok_or_else(|| ReadPathError::UnknownGuid(reference.to_string()))?;
        return if path.is_file() {
            Ok(path.to_path_buf())
        } else {
            Err(ReadPathError::MissingGuidFile {
                reference: reference.to_string(),
                path: path.to_path_buf(),
            })
        };
    }

    let relative = normalize_project_relative(reference).map_err(ReadPathError::Invalid)?;
    let mut path = root.join(&relative);
    if path.is_file() {
        return Ok(path);
    }
    for directory in &search_paths.directories {
        path.clear();
        path.push(root);
        path.push(directory);
        path.push(&relative);
        if path.is_file() {
            return Ok(path);
        }
    }

    // 命中时无需分配诊断候选，缺失时才恢复按尝试顺序排列的相对路径。
    let tried = std::iter::once(relative.clone())
        .chain(
            search_paths
                .directories
                .iter()
                .map(|directory| format!("{directory}/{relative}")),
        )
        .collect();
    Err(ReadPathError::Missing { relative, tried })
}
