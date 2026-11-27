//! 项目清单 `project.xgc`：迁移产物与三个宿主的共同入口。
//!
//! 这里只放**项目级**信息：入口场景、关卡顺序、内容目录、输入映射、画面设置和碰撞矩阵。
//! 单个场景仍是 `scenes/*.json` 的 `WorldData`，两者各自版本化；清单不重复场景内容。
//!
//! 路径一律是项目相对路径（正斜杠），读写边界都走
//! `xgc_base::path::normalize_project_relative`，不在这里另写一套。

use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use xgc_base::layer::CollisionLayers;
use xgc_base::path::normalize_project_relative;
use xgc_base::render_settings::{RenderQuality, RenderSettings};
use xgc_base::EntityId;

use crate::{
    load_world, load_world_with_bytes, replace_world_if_unchanged, write_atomic_checked, Result,
    StoreError,
};
use xgc_world::WorldData;

mod prefab_apply;
pub(crate) use prefab_apply::save_value_prefab_if_unchanged;
pub use prefab_apply::PrefabSceneChange;

/// 清单格式版本；新增必填字段必须同时上调。
pub const PROJECT_FORMAT_VERSION: u32 = 2;
const PROJECT_FILE: &str = "project.xgc";
const PROJECT_LOCK: &str = ".xgc-project.lock";
const PROJECT_CONFLICT: &str = "项目清单已被外部修改；请重新打开项目";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectManifest {
    pub format_version: u32,
    /// 已承诺的项目 ID 区间的上界，`u64::MAX` 表示耗尽。
    pub next_entity_id: u64,
    pub name: String,
    /// 启动场景；必须出现在 `scenes` 里。
    pub entry_scene: String,
    /// 关卡顺序（下标 0 起），也是构建清单。
    pub scenes: Vec<String>,
    pub content: ProjectContent,
    #[serde(default)]
    pub search_paths: Vec<String>,
    /// 输入映射的项目相对路径；没有输入映射时为 None。
    #[serde(default)]
    pub input: Option<String>,
    #[serde(default, skip_serializing_if = "RenderSettings::is_default")]
    pub render: RenderSettings,
    #[serde(default, skip_serializing_if = "RenderQuality::is_default")]
    pub quality: RenderQuality,
    #[serde(default, skip_serializing_if = "CollisionLayers::is_default")]
    pub layers: CollisionLayers,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectContent {
    pub models: String,
    pub textures: String,
    pub audio: String,
    /// 没有着色器目录时为空。
    #[serde(default)]
    pub shaders: Option<String>,
    pub prefabs: String,
    pub scripts: String,
}

impl ProjectManifest {
    /// 读盘后 / 写盘前的唯一校验点。清单不自洽时宁可拒载，也不让宿主
    /// 带着半份路径去猜。
    pub fn validate(&self) -> Result<()> {
        if self.format_version != PROJECT_FORMAT_VERSION {
            return Err(StoreError::Format(format!(
                "project.xgc 版本 {} 不受支持（当前 {PROJECT_FORMAT_VERSION}）",
                self.format_version
            )));
        }
        if self.next_entity_id == 0 {
            return Err(StoreError::Format(
                "project.xgc 的 next_entity_id 必须至少为 1".into(),
            ));
        }
        if self.name.trim().is_empty() {
            return Err(StoreError::Format("project.xgc 的 name 不能为空".into()));
        }
        let entry = self.normalized("entry_scene", &self.entry_scene)?;
        if self.scenes.is_empty() {
            return Err(StoreError::Format("project.xgc 的 scenes 不能为空".into()));
        }
        let mut seen = std::collections::BTreeSet::new();
        for scene in &self.scenes {
            let scene = self.normalized("scenes", scene)?;
            if !seen.insert(scene.clone()) {
                return Err(StoreError::Format(format!(
                    "project.xgc 重复登记场景：{scene}"
                )));
            }
        }
        if !seen.contains(&entry) {
            return Err(StoreError::Format(format!(
                "project.xgc 的 entry_scene 不在 scenes 里：{entry}"
            )));
        }
        self.normalized("content.models", &self.content.models)?;
        self.normalized("content.textures", &self.content.textures)?;
        self.normalized("content.audio", &self.content.audio)?;
        self.normalized("content.prefabs", &self.content.prefabs)?;
        self.normalized("content.scripts", &self.content.scripts)?;
        if let Some(shaders) = &self.content.shaders {
            if !shaders.trim().is_empty() {
                self.normalized("content.shaders", shaders)?;
            }
        }
        for search in &self.search_paths {
            self.normalized("search_paths", search)?;
        }
        if let Some(input) = &self.input {
            self.normalized("input", input)?;
        }
        self.layers
            .validate()
            .map_err(|err| StoreError::Format(format!("project.xgc 的 layers 不合法：{err}")))?;
        Ok(())
    }

    fn normalized(&self, field: &str, raw: &str) -> Result<String> {
        normalize_project_relative(raw).map_err(|defect| {
            StoreError::Format(format!("project.xgc 的 {field} 不合法（{raw}）：{defect}"))
        })
    }
}

pub fn save_project(path: &Path, manifest: &ProjectManifest) -> Result<()> {
    manifest.validate()?;
    let root = project_parent(path);
    fs::create_dir_all(root)?;
    let _lock = lock_recovered_project(root)?;
    match fs::symlink_metadata(path) {
        Ok(_) => {
            let path = checked_manifest_path(path, root)?;
            let bytes = fs::read(&path)?;
            let current = decode_manifest(&bytes)?;
            if manifest.next_entity_id < current.next_entity_id {
                return Err(StoreError::Format("项目 ID 水位不能回退".into()));
            }
            write_manifest(&path, manifest, Some(&bytes))
        }
        Err(err) if err.kind() == ErrorKind::NotFound => write_manifest(path, manifest, None),
        Err(err) => Err(err.into()),
    }
}

pub fn load_project(path: &Path) -> Result<ProjectManifest> {
    decode_manifest(&fs::read(path)?)
}

fn decode_manifest(bytes: &[u8]) -> Result<ProjectManifest> {
    let manifest: ProjectManifest = ron::de::from_bytes(bytes)
        .map_err(|err| StoreError::Format(format!("project.xgc 解析失败：{err}")))?;
    manifest.validate()?;
    Ok(manifest)
}

fn write_manifest(path: &Path, manifest: &ProjectManifest, expected: Option<&[u8]>) -> Result<()> {
    let text = ron::ser::to_string_pretty(manifest, ron::ser::PrettyConfig::default())
        .map_err(|err| StoreError::Format(format!("project.xgc 序列化失败：{err}")))?;
    write_atomic_checked(
        path,
        text.as_bytes(),
        expected.map(|bytes| (bytes, PROJECT_CONFLICT)),
    )
}

fn project_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn checked_manifest_path(path: &Path, root: &Path) -> Result<PathBuf> {
    let canonical = path.canonicalize()?;
    if canonical.parent() != Some(root.canonicalize()?.as_path()) || !canonical.is_file() {
        return Err(StoreError::Format(
            "项目清单必须是项目根目录内的文件".into(),
        ));
    }
    Ok(canonical)
}

fn lock_project(root: &Path) -> Result<File> {
    // 固定锁文件跨越清单的 rename，所有写进程都锁同一文件。关闭句柄释放锁。
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(PROJECT_LOCK))?;
    lock.lock()?;
    Ok(lock)
}

fn lock_recovered_project(root: &Path) -> Result<File> {
    let lock = lock_project(root)?;
    prefab_apply::recover_pending(root)?;
    Ok(lock)
}

/// 已打开的项目：清单 + 根目录。三个宿主都从这里取入口场景与内容根，
/// 不再各自拼 `entry_scene` 的路径。
#[derive(Clone, Debug)]
pub struct ProjectSession {
    root: PathBuf,
    manifest_path: PathBuf,
    manifest: ProjectManifest,
}

impl ProjectSession {
    /// `path` 可以是项目目录，也可以直接是 `project.xgc`。
    pub fn open(path: &Path) -> Result<Self> {
        let manifest_path = if path.is_dir() {
            path.join(PROJECT_FILE)
        } else {
            path.to_path_buf()
        };
        let manifest = load_project(&manifest_path)?;
        Self::from_manifest(&manifest_path, manifest)
    }

    /// 离线工具可先构造候选清单，复用同一项目路径解析和校验。
    pub fn from_manifest(path: &Path, manifest: ProjectManifest) -> Result<Self> {
        manifest.validate()?;
        let root = project_parent(path);
        let manifest_path = checked_manifest_path(path, root)?;
        Ok(Self {
            root: root.to_path_buf(),
            manifest_path,
            manifest,
        })
    }

    /// 先持久化水位，再交出尚未使用的连续 ID。撤销或丢弃草稿可以留下空号。
    pub fn reserve_entity_ids(
        &mut self,
        count: usize,
        world_next_id: EntityId,
    ) -> Result<Range<u64>> {
        self.advance_entity_ids(world_next_id, |_, manifest| {
            u64::try_from(count)
                .ok()
                .and_then(|count| manifest.next_entity_id.checked_add(count))
                .ok_or_else(|| StoreError::Format("项目实体 ID 已耗尽".into()))
        })
    }

    /// 提交端保留计划中显式指定的 ID。跳过的号随本次水位前移一起保留。
    pub fn claim_scene_entity_ids(
        &mut self,
        scene: &str,
        expected_path: &Path,
        first: EntityId,
        last: EntityId,
        world_next_id: EntityId,
    ) -> Result<()> {
        if first > last || last.validate().is_err() {
            return Err(StoreError::Format("项目实体 ID 范围不合法".into()));
        }
        self.advance_entity_ids(world_next_id, |session, manifest| {
            session.current_scene_path(manifest, scene, expected_path)?;
            let start = manifest.next_entity_id;
            if first.raw() < start {
                return Err(StoreError::Format(format!(
                    "实体 ID {first} 低于项目水位 next_entity_id={start}；请重新规划"
                )));
            }
            Ok(last.raw() + 1)
        })?;
        Ok(())
    }

    fn advance_entity_ids(
        &mut self,
        world_next_id: EntityId,
        end: impl FnOnce(&Self, &ProjectManifest) -> Result<u64>,
    ) -> Result<Range<u64>> {
        let _lock = lock_recovered_project(project_parent(&self.manifest_path))?;
        let bytes = fs::read(&self.manifest_path)?;
        let mut manifest = decode_manifest(&bytes)?;
        let start = manifest.next_entity_id;
        if start < world_next_id.raw() || start < self.manifest.next_entity_id {
            return Err(StoreError::Format(
                "项目 ID 水位落后于场景或当前会话".into(),
            ));
        }
        let end = end(self, &manifest)?;
        if end != start {
            manifest.next_entity_id = end;
            write_manifest(&self.manifest_path, &manifest, Some(&bytes))?;
        }
        self.manifest.next_entity_id = end;
        Ok(start..end)
    }

    /// 查找已登记场景。文件缺失时仍按登记路径识别，保存入口才能发现冲突。
    pub fn containing_scene(path: &Path) -> Result<Option<(Self, String)>> {
        let requested = std::path::absolute(path)?;
        let target = resolved_location(&requested)?;
        let roots = requested.ancestors().skip(1).chain(
            (target != requested)
                .then_some(target.as_path())
                .into_iter()
                .flat_map(|path| path.ancestors().skip(1)),
        );
        for root in roots {
            let manifest = root.join(PROJECT_FILE);
            match fs::symlink_metadata(&manifest) {
                Ok(_) => {}
                Err(err) if err.kind() == ErrorKind::NotFound => continue,
                Err(err) => return Err(err.into()),
            }
            let session = Self::open(&manifest)?;
            for scene in &session.manifest.scenes {
                let declared = session.scene_path(scene)?;
                let resolved = resolved_location(&declared)?;
                if same_location(&declared, &requested) || same_location(&resolved, &target) {
                    if !resolved.starts_with(project_parent(&session.manifest_path)) {
                        return Err(StoreError::Format("场景必须位于项目根目录内".into()));
                    }
                    let scene = scene.clone();
                    return Ok(Some((session, scene)));
                }
            }
        }
        Ok(None)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> &ProjectManifest {
        &self.manifest
    }

    pub fn entry_scene(&self) -> &str {
        &self.manifest.entry_scene
    }

    /// 文件经规范化和实际路径校验后才交给宿主，确保目录链接不会把输入读到项目之外。
    pub fn input_path(&self) -> Result<Option<PathBuf>> {
        let Some(input) = &self.manifest.input else {
            return Ok(None);
        };
        let relative = self.manifest.normalized("input", input)?;
        let root = self.root.canonicalize()?;
        let path = root.join(relative).canonicalize()?;
        if !path.starts_with(&root) || !path.is_file() {
            return Err(StoreError::Format(format!(
                "输入映射 {input} 必须是项目内的文件"
            )));
        }
        Ok(Some(path))
    }

    /// 场景路径一律由清单的相对路径拼根目录；未登记的场景直接拒绝，
    /// 避免拼出项目外路径。
    pub fn scene_path(&self, scene: &str) -> Result<PathBuf> {
        let scene = normalize_project_relative(scene)
            .map_err(|defect| StoreError::Format(format!("场景路径不合法（{scene}）：{defect}")))?;
        let registered = self.manifest.scenes.iter().any(|known| {
            normalize_project_relative(known)
                .map(|known| known == scene)
                .unwrap_or(false)
        });
        if !registered {
            return Err(StoreError::Format(format!(
                "场景没有登记在 project.xgc 里：{scene}"
            )));
        }
        Ok(self.root.join(scene))
    }

    /// 作者会话同时保存解析后的路径与磁盘原文，避免链接改指后写到另一关。
    pub fn canonical_scene_path(&self, scene: &str) -> Result<PathBuf> {
        self.canonical_registered_scene(&self.manifest, scene)
    }

    /// 删除文件或解除登记后，对原路径的请求仍属于原作者会话。
    pub fn matches_scene_location(
        &self,
        scene: &str,
        expected_path: &Path,
        requested: &Path,
    ) -> Result<bool> {
        let requested = std::path::absolute(requested)?;
        let declared = std::path::absolute(self.scene_path(scene)?)?;
        Ok(same_location(&declared, &requested)
            || same_location(expected_path, &resolved_location(&requested)?))
    }

    fn current_scene_path(
        &self,
        manifest: &ProjectManifest,
        scene: &str,
        expected_path: &Path,
    ) -> Result<PathBuf> {
        let path = self.canonical_registered_scene(manifest, scene)?;
        if !same_location(&path, expected_path) {
            return Err(StoreError::Format("场景路径已变；请重新打开项目".into()));
        }
        Ok(path)
    }

    fn canonical_registered_scene(
        &self,
        manifest: &ProjectManifest,
        scene: &str,
    ) -> Result<PathBuf> {
        let normalized = manifest.normalized("scenes", scene)?;
        if !manifest.scenes.iter().any(|registered| {
            normalize_project_relative(registered).is_ok_and(|known| known == normalized)
        }) {
            return Err(StoreError::Format("场景登记已变；请重新打开项目".into()));
        }
        let root = project_parent(&self.manifest_path);
        let path = root.join(normalized).canonicalize()?;
        if !path.starts_with(root) || !path.is_file() {
            return Err(StoreError::Format("场景必须是项目根目录内的文件".into()));
        }
        Ok(path)
    }

    /// 已登记场景的作者保存和 ID 分配串行使用同一把项目锁。
    pub fn save_scene_if_unchanged(
        &self,
        scene: &str,
        expected_path: &Path,
        data: &WorldData,
        expected: &[u8],
    ) -> Result<Vec<u8>> {
        let root = project_parent(&self.manifest_path);
        let _lock = lock_recovered_project(root)?;
        let manifest = load_project(&self.manifest_path)?;
        let canonical = self.current_scene_path(&manifest, scene, expected_path)?;
        check_reserved_ids(data, &manifest, "场景")?;
        replace_world_if_unchanged(&canonical, data, expected)
    }

    /// 一次持锁完成恢复、场景登记复核和磁盘基准读取，避免载入尚未提交的内容。
    pub fn load_scene_for_authoring(&self, scene: &str) -> Result<(PathBuf, WorldData, Vec<u8>)> {
        let root = project_parent(&self.manifest_path);
        let _lock = lock_recovered_project(root)?;
        let manifest = load_project(&self.manifest_path)?;
        let path = self.canonical_registered_scene(&manifest, scene)?;
        let (data, bytes) = load_world_with_bytes(&path)?;
        Ok((path, data, bytes))
    }

    pub fn load_scene(&self, scene: &str) -> Result<WorldData> {
        load_world(&self.scene_path(scene)?)
    }

    pub fn load_entry(&self) -> Result<WorldData> {
        self.load_scene(&self.manifest.entry_scene)
    }
}

fn check_reserved_ids(data: &WorldData, manifest: &ProjectManifest, kind: &str) -> Result<()> {
    if data.next_id > manifest.next_entity_id
        || data
            .entities
            .iter()
            .any(|entity| entity.id.raw() >= manifest.next_entity_id)
    {
        return Err(StoreError::Format(format!(
            "{kind}使用了尚未预留的实体 ID（next_entity_id={}）",
            manifest.next_entity_id
        )));
    }
    Ok(())
}

fn resolved_location(path: &Path) -> Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
                return Err(err.into());
            };
            // 不把断开的链接当成可直接创建的新文件。
            match fs::symlink_metadata(path) {
                Err(missing) if missing.kind() == ErrorKind::NotFound => {
                    Ok(resolved_location(parent)?.join(name))
                }
                _ => Err(err.into()),
            }
        }
        Err(err) => Err(err.into()),
    }
}

fn same_location(first: &Path, second: &Path) -> bool {
    if cfg!(windows) {
        first.to_string_lossy().to_lowercase() == second.to_string_lossy().to_lowercase()
    } else {
        first == second
    }
}
