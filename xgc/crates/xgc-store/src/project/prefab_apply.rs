//! 多文件 Apply 与其他作者写入共用项目锁。目标改动前先发布完整原文，
//! 以 Pending 是否存在区分需要恢复的事务与已完成的事务。

use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    check_reserved_ids, load_project, lock_project, lock_recovered_project, project_parent,
    same_location, ProjectSession, PROJECT_FILE, PROJECT_LOCK,
};
use crate::{
    check_bytes, checked_document_bytes, decode, encode, stage_and_replace, write_atomic_checked,
    Result, StoreError,
};
use xgc_base::path::normalize_project_relative;
use xgc_world::WorldData;

const PENDING_FILE: &str = ".xgc-prefab-apply.pending";
const PENDING_KIND: &str = "prefab_apply";
const ASSET_CONFLICT: &str = "预制体已被外部修改；请重试 Apply";
const SCENE_CONFLICT: &str = "场景已被外部修改；请重新打开场景";

/// 已登记场景的磁盘基准。`None` 只校验原文，`Some` 同时声明要落盘的新世界。
pub struct PrefabSceneChange<'a> {
    pub scene: &'a str,
    pub path: &'a Path,
    pub original: &'a [u8],
    pub replacement: Option<&'a WorldData>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingApply {
    prefab: FileChange,
    scenes: Vec<FileChange>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileChange {
    relative: String,
    original: String,
    applied: String,
}

pub(crate) fn save_value_prefab_if_unchanged(
    path: &Path,
    asset: &WorldData,
    original: &[u8],
) -> Result<()> {
    let canonical = path.canonicalize()?;
    let bytes = checked_document_bytes("prefab", asset)?;
    for root in canonical.ancestors().skip(1) {
        let manifest_path = root.join(PROJECT_FILE);
        match fs::symlink_metadata(&manifest_path) {
            Ok(_) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
        }
        let _lock = lock_recovered_project(root)?;
        let project = ProjectSession::open(&manifest_path)?;
        check_bytes(&canonical, original, ASSET_CONFLICT)?;
        let previous = decode::<WorldData>(original, "prefab")?;
        if entity_ids(&previous) != entity_ids(asset) {
            return Err(StoreError::Format(
                "结构性 Apply 需要已登记场景的磁盘基准".into(),
            ));
        }
        check_reserved_ids(asset, project.manifest(), "预制体")?;
        return write_atomic_checked(&canonical, &bytes, Some((original, ASSET_CONFLICT)));
    }
    write_atomic_checked(&canonical, &bytes, Some((original, ASSET_CONFLICT)))
}

impl ProjectSession {
    /// 在项目锁下核对磁盘基准并提交。返回的原文按 `scenes` 的输入顺序排列。
    /// 资产实体集合有变化时，核对全部登记场景，并在持锁期间执行只读依赖检查。
    pub fn apply_prefab_if_unchanged(
        &self,
        path: &Path,
        asset: &WorldData,
        original: &[u8],
        scenes: &[PrefabSceneChange<'_>],
        check_dependencies: impl FnOnce() -> Result<()>,
    ) -> Result<Vec<Vec<u8>>> {
        require_prefab_name(path)?;
        let root = project_parent(&self.manifest_path);
        let _lock = lock_recovered_project(root)?;
        let manifest = load_project(&self.manifest_path)?;
        let prefab_path = file_in_project(root, path)?;
        require_prefab_name(&prefab_path)?;
        check_bytes(&prefab_path, original, ASSET_CONFLICT)?;
        let previous = decode::<WorldData>(original, "prefab")?;
        check_reserved_ids(asset, &manifest, "预制体")?;
        let prefab_bytes = checked_document_bytes("prefab", asset)?;
        let mut pending = PendingApply {
            prefab: FileChange::new(root, &prefab_path, original, &prefab_bytes)?,
            scenes: Vec::new(),
        };
        let mut paths = vec![self.manifest_path.clone(), prefab_path];
        let mut names = BTreeSet::new();
        let mut scene_bytes = Vec::with_capacity(scenes.len());
        for scene in scenes {
            let path = self.current_scene_path(&manifest, scene.scene, scene.path)?;
            insert_unique_path(&mut paths, &path)?;
            names.insert(manifest.normalized("scenes", scene.scene)?);
            check_bytes(&path, scene.original, SCENE_CONFLICT)?;
            decode::<WorldData>(scene.original, "world")?;
            let bytes = if let Some(data) = scene.replacement {
                check_reserved_ids(data, &manifest, "场景")?;
                let bytes = checked_document_bytes("world", data)?;
                if bytes != scene.original {
                    pending
                        .scenes
                        .push(FileChange::new(root, &path, scene.original, &bytes)?);
                }
                bytes
            } else {
                scene.original.to_vec()
            };
            scene_bytes.push(bytes);
        }
        if names.is_empty() {
            return Err(StoreError::Format(
                "Apply 至少需要一个登记场景的磁盘基准".into(),
            ));
        }
        if entity_ids(&previous) != entity_ids(asset) {
            let registered = manifest
                .scenes
                .iter()
                .map(|name| manifest.normalized("scenes", name))
                .collect::<Result<BTreeSet<_>>>()?;
            if names != registered {
                return Err(StoreError::Format(
                    "结构性 Apply 必须校验 project.xgc 中所有登记场景；请重新打开项目".into(),
                ));
            }
            check_dependencies()?;
        }
        if pending.prefab.original != pending.prefab.applied || !pending.scenes.is_empty() {
            commit(root, &pending)?;
        }
        Ok(scene_bytes)
    }

    /// 作者宿主建立磁盘基准前可恢复一次。没有 Pending 时连锁文件也不创建。
    pub fn recover_prefab_apply(&self) -> Result<bool> {
        let root = project_parent(&self.manifest_path);
        if !has_pending(root)? {
            return Ok(false);
        }
        let _lock = lock_project(root)?;
        recover_pending(root)
    }
}

impl FileChange {
    fn new(root: &Path, path: &Path, original: &[u8], applied: &[u8]) -> Result<Self> {
        let relative = path
            .strip_prefix(root)
            .ok()
            .and_then(Path::to_str)
            .ok_or_else(|| {
                StoreError::Format(format!(
                    "Apply 目标不是项目内的 UTF-8 路径：{}",
                    path.display()
                ))
            })?;
        let relative = if cfg!(windows) {
            relative.replace('\\', "/")
        } else {
            relative.to_owned()
        };
        let declared = journal_target(root, &relative)?;
        if !same_location(path, &declared) {
            return Err(StoreError::Format(format!(
                "Apply 目标路径不一致：{relative}"
            )));
        }
        Ok(Self {
            relative,
            original: document_text(original)?,
            applied: document_text(applied)?,
        })
    }
}

fn entity_ids(data: &WorldData) -> BTreeSet<xgc_base::EntityId> {
    data.entities.iter().map(|entity| entity.id).collect()
}

fn document_text(bytes: &[u8]) -> Result<String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|err| StoreError::Format(format!("Apply 文件必须是 UTF-8：{err}")))
}

fn file_in_project(root: &Path, path: &Path) -> Result<PathBuf> {
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(root) || !canonical.is_file() {
        return Err(StoreError::Format(format!(
            "Apply 目标必须是项目根目录内的文件：{}",
            path.display()
        )));
    }
    Ok(canonical)
}

fn require_prefab_name(path: &Path) -> Result<()> {
    if !path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".prefab.json"))
    {
        return Err(StoreError::Format(format!(
            "Apply 资产必须是 .prefab.json 文件：{}",
            path.display()
        )));
    }
    Ok(())
}

fn insert_unique_path(paths: &mut Vec<PathBuf>, path: &Path) -> Result<()> {
    if paths.iter().any(|used| same_location(used, path)) {
        return Err(StoreError::Format(format!(
            "Apply 目标路径重复：{}",
            path.display()
        )));
    }
    paths.push(path.to_path_buf());
    Ok(())
}

fn journal_target(root: &Path, relative: &str) -> Result<PathBuf> {
    if !normalize_project_relative(relative).is_ok_and(|normalized| normalized == relative) {
        return Err(StoreError::Format(format!(
            "Apply 日志目标路径不合法：{relative}"
        )));
    }
    let path = root.join(relative);
    if [PROJECT_FILE, PROJECT_LOCK, PENDING_FILE]
        .iter()
        .any(|reserved| same_location(&path, &root.join(reserved)))
    {
        return Err(StoreError::Format(format!(
            "Apply 日志目标是项目保留文件：{relative}"
        )));
    }
    Ok(path)
}

fn commit(root: &Path, pending: &PendingApply) -> Result<()> {
    let path = root.join(PENDING_FILE);
    let bytes = encode(PENDING_KIND, pending)?;
    stage_and_replace(&path, &bytes, |temp| {
        if has_pending(root)? {
            return Err(StoreError::Format("上一次预制体 Apply 仍待恢复".into()));
        }
        fs::rename(temp, &path)?;
        Ok(())
    })?;

    let result: Result<()> = (|| {
        for (file, conflict) in std::iter::once((&pending.prefab, ASSET_CONFLICT))
            .chain(pending.scenes.iter().map(|file| (file, SCENE_CONFLICT)))
        {
            write_atomic_checked(
                &root.join(&file.relative),
                file.applied.as_bytes(),
                Some((file.original.as_bytes(), conflict)),
            )?;
        }
        // 移走 Pending 是提交点。移除失败时仍可用原文回滚全部目标。
        fs::remove_file(&path)?;
        Ok(())
    })();
    if let Err(write) = result {
        if let Err(restore) = recover_pending(root) {
            return Err(StoreError::Format(format!(
                "预制体 Apply 失败：{write}；恢复未完成：{restore}"
            )));
        }
        return Err(write);
    }
    Ok(())
}

fn has_pending(root: &Path) -> Result<bool> {
    let metadata = match fs::symlink_metadata(root.join(PENDING_FILE)) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err.into()),
    };
    if !metadata.is_file() {
        return Err(StoreError::Format("预制体 Apply 日志必须是普通文件".into()));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(StoreError::Format("预制体 Apply 日志不能是重解析点".into()));
        }
    }
    Ok(true)
}

/// 调用方持有项目锁。先验证所有目标现状，遇外部修改时保留全部现场。
pub(super) fn recover_pending(root: &Path) -> Result<bool> {
    if !has_pending(root)? {
        return Ok(false);
    }
    let root = root.canonicalize()?;
    let path = root.join(PENDING_FILE);
    let pending = decode::<PendingApply>(&fs::read(&path)?, PENDING_KIND)?;
    require_prefab_name(Path::new(&pending.prefab.relative))?;
    let mut paths = Vec::new();
    let mut restore = Vec::new();
    for (file, kind) in std::iter::once((&pending.prefab, "prefab"))
        .chain(pending.scenes.iter().map(|file| (file, "world")))
    {
        let declared = journal_target(&root, &file.relative)?;
        let canonical = file_in_project(&root, &declared)?;
        if !same_location(&declared, &canonical) {
            return Err(StoreError::Format(format!(
                "Apply 恢复时目标路径已变：{}",
                file.relative
            )));
        }
        insert_unique_path(&mut paths, &canonical)?;
        decode::<WorldData>(file.original.as_bytes(), kind)?;
        decode::<WorldData>(file.applied.as_bytes(), kind)?;
        let current = fs::read(&canonical)?;
        if current != file.original.as_bytes() {
            if current != file.applied.as_bytes() {
                return Err(StoreError::Format(format!(
                    "Apply 恢复冲突：{} 已被外部修改",
                    file.relative
                )));
            }
            restore.push((canonical, file));
        }
    }
    for (path, file) in restore {
        write_atomic_checked(
            &path,
            file.original.as_bytes(),
            Some((file.applied.as_bytes(), "Apply 恢复时文件已被外部修改")),
        )?;
    }
    fs::remove_file(&path)?;
    Ok(true)
}
