//! v2 预制体结构性 Apply：资产与所有引用场景共用一份可恢复的多文件事务。
//!
//! 多个独立文件不可能靠 rename 获得真正的跨文件原子性。这里把可承诺的语义说清：
//! 先按路径排序拿齐合作进程锁、逐字节复核全部输入，再保存原文 / 目标文本和 Pending
//! 日志；中途失败自动恢复。进程被强杀后，下次 Apply 会先按同一日志恢复，绝不把
//! “写到一半”冒充成功，也绝不覆盖事务后又被用户改过的文件。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::asset::{
    decode_prefab_v2_references, decode_scene_v2_references, encode_prefab_v2_references,
    encode_scene_v2_references,
};
use crate::fsx;
use crate::project::{normalize_project_relative, normalize_scene_relative, ProjectHandle};
use crate::scene_v2::{PrefabV2, SceneV2};

const BACKUP_PREFIX: &str = ".xgame-prefab-v2-apply-";
const PREPARING_PREFIX: &str = ".xgame-prefab-v2-prepare-";
const JOURNAL: &str = "apply.ron";
static BACKUP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 一次结构性 Apply 的持久记录；`backup` 可交给
/// [`ProjectHandle::restore_prefab_apply_v2`] 恢复。
#[derive(Debug, Clone)]
pub struct PrefabApplyReport {
    pub backup: PathBuf,
    pub files: Vec<String>,
    /// 编辑器要求保护的当前场景在事务后的解码值；不含用户尚未保存的值草稿。
    pub guarded_scene: Option<SceneV2>,
    /// 与 `guarded_scene` 对应的精确磁盘字节，用作下一次条件保存的前置版本。
    pub guarded_scene_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    format_version: u32,
    state: ApplyState,
    source: String,
    files: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum ApplyState {
    Pending,
    Complete,
    Restored,
}

struct Rewrite {
    relative: String,
    original: Vec<u8>,
    applied: Vec<u8>,
}

struct PreparedScene {
    relative: String,
    original: Vec<u8>,
    scene: SceneV2,
    changed: bool,
    guarded: bool,
}

impl ProjectHandle {
    /// 原子到“可恢复”粒度地发布一份 v2 预制体结构，并同步所有已知 v2 引用场景。
    ///
    /// `expected_asset` 与 `guarded_scene` 都是编辑会话打开时读到的**完整原文字节**。
    /// 后者即使尚未引用该资产也会进入同一锁集，保证 Apply 不会跨过当前场景的外部写入。
    pub fn apply_prefab_v2_transaction(
        &self,
        source: &str,
        prefab: &PrefabV2,
        expected_asset: &[u8],
        guarded_scene: Option<(&str, &[u8])>,
    ) -> Result<PrefabApplyReport, String> {
        let _ = self.recover_pending_prefab_applies_v2()?;
        let source = normalize_project_relative(source).map_err(|error| error.to_string())?;
        prefab.to_ron()?;
        let current_asset = std::fs::read(self.path(&source))
            .map_err(|error| format!("无法读取 '{source}': {error}"))?;
        if current_asset != expected_asset {
            return Err(format!(
                "预制体 '{source}' 已被外部修改；请重新读取后再 Apply"
            ));
        }
        let old_text = std::str::from_utf8(expected_asset)
            .map_err(|error| format!("预制体 '{source}' 不是 UTF-8: {error}"))?;
        let mut old_prefab = PrefabV2::from_ron(old_text)
            .map_err(|error| format!("无法解析预制体 '{source}': {error}"))?;
        decode_prefab_v2_references(&mut old_prefab, &self.assets());

        let guarded = guarded_scene
            .map(|(relative, expected)| {
                normalize_scene_relative(relative).map(|relative| (relative, expected.to_vec()))
            })
            .transpose()?;

        let mut scene_paths = BTreeSet::new();
        scene_paths.insert(normalize_scene_relative(&self.manifest().entry_scene)?);
        for relative in self
            .list_scenes()
            .into_iter()
            .chain(self.manifest().build.scenes.iter().cloned())
        {
            scene_paths.insert(normalize_scene_relative(&relative)?);
        }
        if let Some((relative, _)) = &guarded {
            scene_paths.insert(relative.clone());
        }

        let mut prepared_scenes = Vec::new();
        for relative in scene_paths {
            let original = std::fs::read(self.path(&relative))
                .map_err(|error| format!("无法读取场景 '{relative}': {error}"))?;
            let is_guarded = guarded
                .as_ref()
                .is_some_and(|(guarded, _)| guarded == &relative);
            if let Some((_, expected)) = guarded.as_ref().filter(|(path, _)| path == &relative) {
                if &original != expected {
                    return Err(format!(
                        "当前场景 '{relative}' 已被外部修改；请重新打开后再 Apply"
                    ));
                }
            }
            let text = std::str::from_utf8(&original)
                .map_err(|error| format!("场景 '{relative}' 不是 UTF-8: {error}"))?;
            let mut scene = SceneV2::from_ron(text)
                .map_err(|error| format!("无法解析场景 '{relative}': {error}"))?;
            decode_scene_v2_references(&mut scene, &self.assets());
            let before = scene.to_ron()?;
            let references_target = scene.entities.iter().any(|entity| {
                entity
                    .prefab
                    .as_ref()
                    .is_some_and(|reference| reference.source == source)
            });
            let old_sources = prefab_sources(self, &scene, &source, &old_prefab)?;
            scene
                .validate_prefab_sources(&old_sources)
                .map_err(|error| format!("场景 '{relative}' 的旧预制体来源无效: {error}"))?;
            if references_target {
                scene.synchronize_prefab_source_metadata(&source, prefab)?;
            }
            let new_sources = prefab_sources(self, &scene, &source, prefab)?;
            scene.validate_prefab_sources(&new_sources).map_err(|error| {
                format!("场景 '{relative}' 无法同步新预制体来源: {error}")
            })?;
            let changed = scene.to_ron()? != before;
            if references_target || is_guarded {
                prepared_scenes.push(PreparedScene {
                    relative,
                    original,
                    scene,
                    changed,
                    guarded: is_guarded,
                });
            }
        }

        // 全部场景预检通过后才允许补 `.meta`；陈旧 / 坏场景不能留下旁路写入。
        {
            let mut assets = self.assets();
            assets.refresh();
            assets.ensure_metas()?;
        }
        let mut encoded_prefab = prefab.clone();
        encode_prefab_v2_references(&mut encoded_prefab, &self.assets());
        let mut rewrites = vec![Rewrite {
            relative: source.clone(),
            original: expected_asset.to_vec(),
            applied: encoded_prefab.to_ron()?.into_bytes(),
        }];
        let mut guarded_scene_value = None;
        let mut guarded_scene_bytes = None;
        for prepared in prepared_scenes {
            let applied = if prepared.changed {
                let mut encoded = prepared.scene.clone();
                encode_scene_v2_references(&mut encoded, &self.assets());
                encoded.to_ron()?.into_bytes()
            } else {
                prepared.original.clone()
            };
            if prepared.guarded {
                guarded_scene_value = Some(prepared.scene.clone());
                guarded_scene_bytes = Some(applied.clone());
            }
            rewrites.push(Rewrite {
                relative: prepared.relative,
                original: prepared.original,
                applied,
            });
        }
        let transaction = commit(self, &source, &rewrites)?;
        Ok(PrefabApplyReport {
            backup: transaction,
            files: rewrites
                .iter()
                .map(|rewrite| rewrite.relative.clone())
                .collect(),
            guarded_scene: guarded_scene_value,
            guarded_scene_bytes,
        })
    }

    /// 从一次完成或中断的结构性 Apply 恢复全部原文字节。
    pub fn restore_prefab_apply_v2(&self, backup: &Path) -> Result<(), String> {
        self.restore_prefab_apply_v2_inner(backup, false)
            .map(|_| ())
    }

    fn restore_prefab_apply_v2_inner(
        &self,
        backup: &Path,
        only_if_pending: bool,
    ) -> Result<bool, String> {
        let backup = validate_backup(self, backup)?;
        let before_lock = read_journal(&backup.join(JOURNAL))?;
        let targets = sorted_targets(&before_lock.files)?;
        let _locks = lock_targets(self, &targets)?;
        // 另一进程可能正持锁执行同一事务：等待期间它会把 Pending 改成 Complete。
        // 拿锁后必须重读，否则恢复者会把刚成功的事务误回滚。
        let mut journal = read_journal(&backup.join(JOURNAL))?;
        if journal.files != before_lock.files || journal.source != before_lock.source {
            return Err("等待文件锁期间事务目标列表发生变化，拒绝恢复".into());
        }
        if only_if_pending && journal.state != ApplyState::Pending {
            return Ok(false);
        }
        restore_locked(self, &backup, &journal.files)?;
        journal.state = ApplyState::Restored;
        write_journal(&backup, &journal, false)?;
        Ok(true)
    }

    /// 恢复项目根目录下所有仍为 Pending 的结构性 Apply；`--check` 不调用，保持只读。
    pub fn recover_pending_prefab_applies_v2(&self) -> Result<usize, String> {
        let mut recovered = 0;
        for entry in std::fs::read_dir(self.root()).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            if !entry.path().is_dir()
                || !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(BACKUP_PREFIX)
            {
                continue;
            }
            let journal_path = entry.path().join(JOURNAL);
            if !journal_path.is_file() {
                return Err(format!(
                    "结构性 Apply 目录 {} 缺少事务日志，请先人工检查",
                    entry.path().display()
                ));
            }
            if read_journal(&journal_path)?.state == ApplyState::Pending {
                let restored = self
                    .restore_prefab_apply_v2_inner(&entry.path(), true)
                    .map_err(|error| {
                        format!(
                            "恢复上次中断的结构性 Apply {} 失败: {error}",
                            entry.path().display()
                        )
                    })?;
                recovered += usize::from(restored);
            }
        }
        Ok(recovered)
    }
}

fn prefab_sources(
    project: &ProjectHandle,
    scene: &SceneV2,
    target_source: &str,
    target: &PrefabV2,
) -> Result<BTreeMap<String, PrefabV2>, String> {
    let mut prefabs = BTreeMap::new();
    for entity in &scene.entities {
        let Some(reference) = &entity.prefab else {
            continue;
        };
        if prefabs.contains_key(&reference.source) {
            continue;
        }
        let prefab = if reference.source == target_source {
            target.clone()
        } else {
            project.load_prefab_v2(&reference.source)?
        };
        prefabs.insert(reference.source.clone(), prefab);
    }
    Ok(prefabs)
}

fn commit(project: &ProjectHandle, source: &str, rewrites: &[Rewrite]) -> Result<PathBuf, String> {
    if rewrites.is_empty() {
        return Err("结构性 Apply 没有事务文件".into());
    }
    let files: Vec<_> = rewrites
        .iter()
        .map(|rewrite| rewrite.relative.clone())
        .collect();
    let targets = sorted_targets(&files)?;
    let _locks = lock_targets(project, &targets)?;
    for rewrite in rewrites {
        let current = std::fs::read(project.path(&rewrite.relative))
            .map_err(|error| format!("复核 '{}' 失败: {error}", rewrite.relative))?;
        if current != rewrite.original {
            return Err(format!(
                "'{}' 在事务准备后被外部修改；结构性 Apply 未写盘",
                rewrite.relative
            ));
        }
    }

    let (preparing, backup) = create_backup_dirs(project)?;
    let prepare = (|| {
        for rewrite in rewrites {
            for (folder, bytes) in [
                ("original", &rewrite.original),
                ("applied", &rewrite.applied),
            ] {
                let path = preparing.join(folder).join(&rewrite.relative);
                fsx::write_atomic_new(&path, bytes)
                    .map_err(|error| format!("保存事务副本 {} 失败: {error}", path.display()))?;
            }
        }
        write_journal(
            &preparing,
            &Journal {
                format_version: 1,
                state: ApplyState::Pending,
                source: source.to_owned(),
                files: files.clone(),
            },
            true,
        )
    })();
    if let Err(error) = prepare {
        let _ = std::fs::remove_dir_all(&preparing);
        return Err(error);
    }
    // 事务目录只有在原文、目标文本和 Pending 日志都齐全后才进入恢复扫描范围。
    // 否则并发 Apply 会把另一个进程尚在准备的目录误判成损坏事务。
    if let Err(error) = std::fs::rename(&preparing, &backup) {
        let _ = std::fs::remove_dir_all(&preparing);
        return Err(format!(
            "公布结构性 Apply 事务 {} 失败: {error}",
            backup.display()
        ));
    }

    let mut journal = read_journal(&backup.join(JOURNAL))?;
    for rewrite in rewrites {
        if rewrite.original == rewrite.applied {
            continue;
        }
        if let Err(error) = fsx::write_atomic(&project.path(&rewrite.relative), &rewrite.applied) {
            let recovery = restore_locked(project, &backup, &files).and_then(|()| {
                journal.state = ApplyState::Restored;
                write_journal(&backup, &journal, false)
            });
            return Err(format!(
                "写入 '{}' 失败: {error}；事务 {}；自动恢复: {recovery:?}",
                rewrite.relative,
                backup.display()
            ));
        }
    }
    journal.state = ApplyState::Complete;
    if let Err(error) = write_journal(&backup, &journal, false) {
        let recovery = restore_locked(project, &backup, &files).and_then(|()| {
            journal.state = ApplyState::Restored;
            write_journal(&backup, &journal, false)
        });
        return Err(format!(
            "记录结构性 Apply 完成状态失败: {error}；事务 {}；自动恢复: {recovery:?}",
            backup.display()
        ));
    }
    Ok(backup)
}

fn create_backup_dirs(project: &ProjectHandle) -> Result<(PathBuf, PathBuf), String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    for _ in 0..100 {
        let sequence = BACKUP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let suffix = format!("{}-{stamp}-{sequence}", std::process::id());
        let preparing = project.root().join(format!("{PREPARING_PREFIX}{suffix}"));
        let backup = project.root().join(format!("{BACKUP_PREFIX}{suffix}"));
        if backup.exists() {
            continue;
        }
        match std::fs::create_dir(&preparing) {
            Ok(()) => return Ok((preparing, backup)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "创建事务准备目录 {} 失败: {error}",
                    preparing.display()
                ));
            }
        }
    }
    Err("无法分配结构性 Apply 事务目录".into())
}

fn validate_backup(project: &ProjectHandle, backup: &Path) -> Result<PathBuf, String> {
    let backup = std::fs::canonicalize(backup)
        .map_err(|error| format!("无法定位事务目录 {}: {error}", backup.display()))?;
    let root = std::fs::canonicalize(project.root())
        .map_err(|error| format!("无法定位项目根目录 {}: {error}", project.root().display()))?;
    if backup.parent() != Some(root.as_path())
        || !backup
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(BACKUP_PREFIX))
    {
        return Err("事务目录必须位于本项目根目录且名称合法".into());
    }
    Ok(backup)
}

fn sorted_targets(files: &[String]) -> Result<Vec<String>, String> {
    let mut targets = files
        .iter()
        .map(|relative| normalize_project_relative(relative).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    targets.sort_by_key(|path| path.to_ascii_lowercase());
    if targets
        .iter()
        .map(|path| path.to_ascii_lowercase())
        .collect::<BTreeSet<_>>()
        .len()
        != targets.len()
    {
        return Err("事务含重复文件路径，拒绝重复持锁".into());
    }
    Ok(targets)
}

fn lock_targets(project: &ProjectHandle, targets: &[String]) -> Result<Vec<std::fs::File>, String> {
    targets
        .iter()
        .map(|relative| {
            project
                .lock_component_file(relative)
                .map_err(|error| error.to_string())
        })
        .collect()
}

fn restore_locked(project: &ProjectHandle, backup: &Path, files: &[String]) -> Result<(), String> {
    let mut originals = Vec::with_capacity(files.len());
    for relative in files {
        let relative = normalize_project_relative(relative).map_err(|error| error.to_string())?;
        let original = std::fs::read(backup.join("original").join(&relative))
            .map_err(|error| format!("读取 '{relative}' 的事务原文失败: {error}"))?;
        let applied = std::fs::read(backup.join("applied").join(&relative))
            .map_err(|error| format!("读取 '{relative}' 的事务目标失败: {error}"))?;
        let current = std::fs::read(project.path(&relative))
            .map_err(|error| format!("读取当前 '{relative}' 失败: {error}"))?;
        if current != original && current != applied {
            return Err(format!(
                "'{relative}' 在结构性 Apply 后又被修改，拒绝覆盖；事务在 {}",
                backup.display()
            ));
        }
        let needs_restore = current != original;
        originals.push((relative, original, needs_restore));
    }
    for (relative, original, needs_restore) in originals {
        if needs_restore {
            fsx::write_atomic(&project.path(&relative), original)
                .map_err(|error| format!("恢复 '{relative}' 失败: {error}"))?;
        }
    }
    Ok(())
}

fn read_journal(path: &Path) -> Result<Journal, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("读取事务日志 {} 失败: {error}", path.display()))?;
    let journal: Journal = ron::from_str(&text)
        .map_err(|error| format!("解析事务日志 {} 失败: {error}", path.display()))?;
    if journal.format_version != 1 || journal.files.is_empty() {
        return Err(format!("事务日志 {} 的版本或文件列表无效", path.display()));
    }
    normalize_project_relative(&journal.source).map_err(|error| error.to_string())?;
    sorted_targets(&journal.files)?;
    Ok(journal)
}

fn write_journal(backup: &Path, journal: &Journal, new: bool) -> Result<(), String> {
    let text = ron::ser::to_string_pretty(journal, ron::ser::PrettyConfig::default())
        .map_err(|error| error.to_string())?;
    let path = backup.join(JOURNAL);
    (if new {
        fsx::write_atomic_new(&path, text)
    } else {
        fsx::write_atomic(&path, text)
    })
    .map_err(|error| format!("写入事务日志 {} 失败: {error}", path.display()))
}
