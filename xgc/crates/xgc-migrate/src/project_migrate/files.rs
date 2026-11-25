//! 迁移计划里的文件清单。预检和发布使用同一批路径，避免干跑漏掉已存在的内容。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::ProjectMigrationError;

pub(super) struct Copies {
    directories: BTreeSet<PathBuf>,
    files: BTreeMap<PathBuf, PathBuf>,
}

impl Copies {
    pub(super) fn plan(
        root: &Path,
        directories: &[String],
        input: Option<&str>,
        excluded: &BTreeSet<PathBuf>,
    ) -> Result<Self, ProjectMigrationError> {
        let mut plan = Self {
            directories: BTreeSet::new(),
            files: BTreeMap::new(),
        };
        for dir in directories {
            if root.join(dir).is_dir() {
                plan.collect(root, Path::new(dir), excluded)?;
            }
        }
        if let Some(input) = input {
            let path = Path::new(input);
            plan.files.insert(path_key(path), path.to_path_buf());
        }
        Ok(plan)
    }

    fn collect(
        &mut self,
        root: &Path,
        dir: &Path,
        excluded: &BTreeSet<PathBuf>,
    ) -> Result<(), ProjectMigrationError> {
        if !self.directories.insert(dir.to_path_buf()) {
            return Ok(());
        }
        let absolute = root.join(dir);
        let io = |err: std::io::Error| ProjectMigrationError::Io {
            path: absolute.clone(),
            message: err.to_string(),
        };
        for entry in fs::read_dir(&absolute).map_err(io)? {
            let entry = entry.map_err(io)?;
            let child = dir.join(entry.file_name());
            let kind = entry.file_type().map_err(io)?;
            if kind.is_dir() {
                self.collect(root, &child, excluded)?;
            } else if kind.is_file() {
                let key = path_key(&child);
                if !excluded.contains(&key) {
                    self.files.entry(key).or_insert(child);
                }
            }
        }
        Ok(())
    }

    pub(super) fn validate<'a>(
        &self,
        outputs: impl IntoIterator<Item = &'a str>,
        reserved: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), ProjectMigrationError> {
        let mut occupied = BTreeMap::new();
        for target in outputs {
            if occupied
                .insert(path_key(Path::new(target)), target)
                .is_some()
            {
                return Err(ProjectMigrationError::Manifest(format!(
                    "输出文件 {target} 重复"
                )));
            }
        }
        let reserved: BTreeSet<_> = self
            .directories
            .iter()
            .cloned()
            .chain(reserved.into_iter().map(PathBuf::from))
            .flat_map(|path| {
                path_key(&path)
                    .ancestors()
                    .map(Path::to_path_buf)
                    .collect::<Vec<_>>()
            })
            .collect();
        for (key, target) in &occupied {
            let blocked = reserved.contains(key)
                || self.files.contains_key(key)
                || key
                    .ancestors()
                    .skip(1)
                    .any(|parent| self.files.contains_key(parent) || occupied.contains_key(parent));
            if blocked {
                return Err(ProjectMigrationError::Manifest(format!(
                    "输出文件 {target} 与项目路径冲突"
                )));
            }
        }
        Ok(())
    }

    pub(super) fn len(&self) -> usize {
        self.files.len()
    }

    pub(super) fn write(&self, root: &Path, staging: &Path) -> Result<(), ProjectMigrationError> {
        for dir in &self.directories {
            let path = staging.join(dir);
            fs::create_dir_all(&path).map_err(|err| ProjectMigrationError::Io {
                path,
                message: err.to_string(),
            })?;
        }
        for path in self.files.values() {
            copy_file(&root.join(path), &staging.join(path))?;
        }
        Ok(())
    }
}

pub(super) fn copy_file(source: &Path, target: &Path) -> Result<(), ProjectMigrationError> {
    let result = (|| {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, target)
    })();
    result.map(|_| ()).map_err(|err| ProjectMigrationError::Io {
        path: source.to_path_buf(),
        message: err.to_string(),
    })
}

pub(super) fn path_key(path: &Path) -> PathBuf {
    // Windows 的通常目录不区分大小写，预检时按同一目标比较。
    if cfg!(windows) {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    } else {
        path.to_path_buf()
    }
}
