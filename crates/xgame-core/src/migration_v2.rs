//! 显式组件文件升级：先完整预检，再留下原文和目标文本，最后逐文件原子替换。
//! 中断后使用备份目录恢复；仅在目标仍是原文或本次目标文本时才允许覆盖。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::asset::{
    decode_prefab_references, encode_prefab_v2_references, encode_scene_v2_references,
};
use crate::fsx;
use crate::prefab::Prefab;
use crate::project::{
    normalize_project_relative, normalize_scene_relative, parse_v1_upgrade_text, ProjectHandle,
};
use crate::scene_v2::PrefabV2;

const BACKUP_PREFIX: &str = ".xgame-component-v2-";
const JOURNAL: &str = "migration.ron";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentMigrationReport {
    /// 原文、目标文本和事务记录都保存在项目根目录下这份目录里。
    pub backup: PathBuf,
    pub files: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    format_version: u32,
    state: MigrationState,
    files: Vec<String>,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
enum MigrationState {
    Pending,
    Complete,
    Restored,
}

struct Rewrite {
    relative: String,
    original: Vec<u8>,
    upgraded: Vec<u8>,
}

impl ProjectHandle {
    /// 显式将已知场景和预制体升级为 v2；不修改 project.ron，不修改任何 v2 原文件。
    /// 完整预检失败时不创建备份，也不碰项目文件。成功后保留可恢复的原文副本。
    pub fn migrate_components_v2(&self) -> Result<ComponentMigrationReport, String> {
        // 有未完成事务时先恢复。否则覆盖文件会让旧事务的恢复依据失效。
        for entry in std::fs::read_dir(self.root()).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry
                .file_name()
                .to_string_lossy()
                .starts_with(BACKUP_PREFIX)
                || !entry.path().is_dir()
            {
                continue;
            }
            let journal = entry.path().join(JOURNAL);
            if !journal.exists() {
                return Err(format!(
                    "存在未完成的组件升级备份 {}；请先检查",
                    entry.path().display()
                ));
            }
            let record = read_journal(&journal)?;
            if record.state == MigrationState::Pending {
                return Err(format!(
                    "存在未恢复的组件升级 {}；请先运行恢复",
                    entry.path().display()
                ));
            }
        }

        let mut scenes = BTreeSet::new();
        scenes.insert(normalize_scene_relative(&self.manifest().entry_scene)?);
        for raw in self
            .list_scenes()
            .into_iter()
            .chain(self.manifest().build.scenes.iter().cloned())
        {
            scenes.insert(normalize_scene_relative(&raw)?);
        }
        let mut prefabs: BTreeSet<String> = self.list_prefabs().into_iter().collect();
        let mut scene_rewrites = Vec::new();
        for relative in scenes {
            let original = std::fs::read(self.path(&relative))
                .map_err(|e| format!("无法读取 {relative}: {e}"))?;
            let version = crate::file_format::disk_version(&self.path(&relative))
                .map_err(|e| format!("无法读取 {relative}: {e}"))?;
            let mut scene = match version {
                Some(2) => self.load_scene_v2(&relative)?,
                Some(1) | None => self.preview_scene_v2(&relative)?,
                Some(other) => return Err(format!("{relative} 的版本 {other} 不支持迁移")),
            };
            for entity in &scene.entities {
                if let Some(link) = &entity.prefab {
                    prefabs.insert(
                        normalize_project_relative(&link.source).map_err(|e| e.to_string())?,
                    );
                }
            }
            if version != Some(2) {
                let plain = scene.clone().to_v1()?;
                let defects = crate::validate::scene_defects(&plain);
                if let Some(defect) = defects.first() {
                    return Err(format!("{relative}: {}", defect.describe()));
                }
                encode_scene_v2_references(&mut scene, &self.assets());
                scene_rewrites.push(Rewrite {
                    relative,
                    original,
                    upgraded: scene.to_ron()?.into_bytes(),
                });
            }
        }

        let mut rewrites = Vec::new();
        for relative in prefabs {
            let relative = normalize_project_relative(&relative).map_err(|e| e.to_string())?;
            let original = std::fs::read(self.path(&relative))
                .map_err(|e| format!("无法读取 {relative}: {e}"))?;
            let version = crate::file_format::disk_version(&self.path(&relative))
                .map_err(|e| format!("无法读取 {relative}: {e}"))?;
            if version == Some(2) {
                self.load_prefab_v2(&relative)?;
                continue;
            }
            if !matches!(version, Some(1) | None) {
                return Err(format!("{relative} 的版本 {version:?} 不支持迁移"));
            }
            let text = std::str::from_utf8(&original).map_err(|e| format!("{relative}: {e}"))?;
            let mut old: Prefab = parse_v1_upgrade_text(text, &relative)?;
            if !old.duplicate_ids().is_empty() {
                return Err(format!("{relative}: 预制体包含重复实体 ID"));
            }
            decode_prefab_references(&mut old, &self.assets());
            if let Some(defect) = crate::validate::entity_defects(&old.entities).first() {
                return Err(format!("{relative}: {}", defect.describe()));
            }
            let mut upgraded = PrefabV2::from_v1(&old)?;
            encode_prefab_v2_references(&mut upgraded, &self.assets());
            rewrites.push(Rewrite {
                relative,
                original,
                upgraded: upgraded.to_ron()?.into_bytes(),
            });
        }
        rewrites.extend(scene_rewrites); // 来源资产必须先落盘，场景的来源关联才可读。
        let files: Vec<String> = rewrites.iter().map(|item| item.relative.clone()).collect();
        if files.is_empty() {
            return Err("项目内没有需要从 v1 升级的文件".into());
        }

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let backup = self
            .root()
            .join(format!("{BACKUP_PREFIX}{}-{stamp}", std::process::id()));
        std::fs::create_dir(&backup)
            .map_err(|e| format!("创建备份 {} 失败: {e}", backup.display()))?;
        for item in &rewrites {
            for (folder, bytes) in [("original", &item.original), ("upgraded", &item.upgraded)] {
                let path = backup.join(folder).join(&item.relative);
                fsx::write_atomic_new(&path, bytes)
                    .map_err(|e| format!("保存备份 {} 失败: {e}", path.display()))?;
            }
        }
        let mut journal = Journal {
            format_version: 1,
            state: MigrationState::Pending,
            files: files.clone(),
        };
        write_journal(&backup, &journal, true)?;
        for item in &rewrites {
            let path = self.path(&item.relative);
            let current = std::fs::read(&path).map_err(|e| e.to_string());
            let step = match current {
                Ok(bytes) if bytes == item.original => fsx::write_atomic(&path, &item.upgraded),
                Ok(_) => Err(std::io::Error::other("迁移前文件被外部修改")),
                Err(e) => Err(std::io::Error::other(e)),
            };
            if let Err(error) = step {
                let recovery = self.restore_components_v2(&backup);
                return Err(format!(
                    "迁移 {path:?} 失败: {error}；备份 {}；自动恢复: {recovery:?}",
                    backup.display()
                ));
            }
        }
        journal.state = MigrationState::Complete;
        if let Err(error) = write_journal(&backup, &journal, false) {
            let recovery = self.restore_components_v2(&backup);
            return Err(format!(
                "迁移完成状态记录失败: {error}；备份 {}；自动恢复: {recovery:?}",
                backup.display()
            ));
        }
        Ok(ComponentMigrationReport { backup, files })
    }

    /// 恢复某次迁移的原文；先逐文件核对，外部改动后拒绝整体恢复，避免覆盖用户工作。
    pub fn restore_components_v2(&self, backup: &Path) -> Result<(), String> {
        if backup.parent() != Some(self.root())
            || !backup
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(BACKUP_PREFIX))
        {
            return Err("备份必须是本项目根目录下的组件迁移目录".into());
        }
        let mut journal = read_journal(&backup.join(JOURNAL))?;
        let mut originals = Vec::new();
        for relative in &journal.files {
            let relative = normalize_project_relative(relative).map_err(|e| e.to_string())?;
            let old = std::fs::read(backup.join("original").join(&relative))
                .map_err(|e| e.to_string())?;
            let next = std::fs::read(backup.join("upgraded").join(&relative))
                .map_err(|e| e.to_string())?;
            let current = std::fs::read(self.path(&relative)).map_err(|e| e.to_string())?;
            if current != old && current != next {
                return Err(format!(
                    "{relative} 在迁移后已被修改，拒绝覆盖；备份在 {}",
                    backup.display()
                ));
            }
            originals.push((relative, old, current != next));
        }
        for (relative, old, already_old) in originals {
            if !already_old {
                fsx::write_atomic(&self.path(&relative), old)
                    .map_err(|e| format!("恢复 {relative} 失败: {e}"))?;
            }
        }
        journal.state = MigrationState::Restored;
        write_journal(backup, &journal, false)
    }
}

fn read_journal(path: &Path) -> Result<Journal, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let journal: Journal =
        ron::from_str(&text).map_err(|e| format!("解析 {} 失败: {e}", path.display()))?;
    if journal.format_version != 1 || journal.files.is_empty() {
        return Err(format!("{} 的迁移记录版本或文件列表无效", path.display()));
    }
    Ok(journal)
}

fn write_journal(backup: &Path, journal: &Journal, new: bool) -> Result<(), String> {
    let text = ron::ser::to_string_pretty(journal, ron::ser::PrettyConfig::default())
        .map_err(|e| e.to_string())?;
    let path = backup.join(JOURNAL);
    (if new {
        fsx::write_atomic_new(&path, text)
    } else {
        fsx::write_atomic(&path, text)
    })
    .map_err(|e| format!("写入迁移记录 {} 失败: {e}", path.display()))
}
