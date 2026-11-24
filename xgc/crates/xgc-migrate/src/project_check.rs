//! 旧工程的项目级检查：`xgc check <项目目录>`。
//!
//! 迁移工具只认 **场景 RON**，项目目录里还有 `project.ron` / `input.ron` / 预制体 /
//! 脚本 / `.meta`。把这些混在一起，用户拿到的是「拒绝访问」或者「不支持的旧场景
//! 版本」，而不是「第 3 关缺脚本」。这一层只做**只读**体检，不迁移、不写盘；
//! 判定口径与旧端 `xgame-core::check` 对齐：
//! - `project.ron` / 入口场景坏 = 致命错误（调用方 exit 1）；
//! - 其余场景 / 预制体 / 脚本 / 资产问题 = 报告问题（调用方 exit 2）；
//! - `Scenes/` 里有但 `build.scenes` 没列 = hint，不算问题。
//!
//! 资产引用统一走 `xgc-asset` 的只读目录；路径规则统一走
//! `xgc_base::path::normalize_project_relative`，两处都不在这里另抄一份。

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use xgc_asset::{scan, AssetCatalog};
use xgc_base::path::normalize_project_relative;

use crate::old_scene::{OldEntity, OldScene};
use crate::{OldComponentData, OLD_SCENE_FORMAT_VERSION};

/// 旧 `project.ron` 的格式版本；与旧端 `ProjectFile::CURRENT_VERSION` 同值。
pub const OLD_PROJECT_FORMAT_VERSION: u32 = 1;

/// 项目检查的致命错误：这些情况下报告不可信，调用方应 exit 1。
#[derive(Debug)]
pub enum ProjectCheckError {
    /// 目录不存在或不是目录。
    NotADirectory(PathBuf),
    /// `project.ron` 不存在 / 读不了。
    ProjectIo { path: PathBuf, message: String },
    /// `project.ron` 解析失败。
    ProjectParse(String),
    /// 入口场景路径本身不合法。
    InvalidEntry(String),
    /// 入口场景不存在 / 读不了。
    MissingEntry(String),
    /// 入口场景解析失败或版本不是 2。
    EntryParse(String),
}

impl std::fmt::Display for ProjectCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotADirectory(path) => write!(f, "不是项目目录：{}", path.display()),
            Self::ProjectIo { path, message } => {
                write!(f, "读不了 project.ron（{}）：{message}", path.display())
            }
            Self::ProjectParse(message) => write!(f, "project.ron 解析失败：{message}"),
            Self::InvalidEntry(message) => write!(f, "入口场景路径不合法：{message}"),
            Self::MissingEntry(path) => write!(f, "入口场景不存在或读不了：{path}"),
            Self::EntryParse(message) => write!(f, "入口场景解析失败：{message}"),
        }
    }
}

impl std::error::Error for ProjectCheckError {}

/// 项目级只读体检结果。
#[derive(Debug, Default)]
pub struct ProjectCheckReport {
    /// `project.ron` 里的项目名。
    pub name: String,
    /// 成功解析的场景数。
    pub scenes: usize,
    /// 这些场景里的实体总数。
    pub entities: usize,
    /// `Scenes/` 下扫到的预制体文件数。
    pub prefabs: usize,
    /// 去重后的脚本引用数。
    pub scripts: usize,
    /// `.meta` 登记的资源数。
    pub assets: usize,
    /// 必须修的问题（调用方 exit 2）。
    pub problems: Vec<String>,
    /// 不挡割接、但值得提醒的事项（例如 Rust 插件组件不会执行）。
    pub hints: Vec<String>,
}

impl ProjectCheckReport {
    pub fn has_problems(&self) -> bool {
        !self.problems.is_empty()
    }

    pub fn issue_count(&self) -> usize {
        self.problems.len()
    }

    /// 人类可读的报告；问题行带 `[code]` 前缀，便于 CI 日志 grep。
    pub fn render(&self) -> String {
        let mut out = format!(
            "check: project={} scenes={} entities={} prefabs={} scripts={} assets={} problems={}",
            if self.name.is_empty() {
                "?"
            } else {
                self.name.as_str()
            },
            self.scenes,
            self.entities,
            self.prefabs,
            self.scripts,
            self.assets,
            self.problems.len()
        );
        if self.problems.is_empty() {
            out.push_str("\ncheck: OK");
        }
        for problem in &self.problems {
            out.push_str("\n  problem: ");
            out.push_str(problem);
        }
        for hint in &self.hints {
            out.push_str("\n  hint: ");
            out.push_str(hint);
        }
        out
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct OldProject {
    #[serde(default)]
    pub(crate) format_version: u32,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) entry_scene: String,
    #[serde(default)]
    pub(crate) content: OldContent,
    #[serde(default)]
    pub(crate) build: OldBuild,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OldContent {
    #[serde(default = "default_scenes_dir")]
    pub(crate) scenes_dir: String,
    #[serde(default = "default_models_dir")]
    pub(crate) models_dir: String,
    #[serde(default = "default_textures_dir")]
    pub(crate) textures_dir: String,
    #[serde(default = "default_audio_dir")]
    pub(crate) audio_dir: String,
    #[serde(default = "default_shaders_dir")]
    pub(crate) shaders_dir: String,
    #[serde(default = "default_prefabs_dir")]
    pub(crate) prefabs_dir: String,
    #[serde(default = "default_scripts_dir")]
    pub(crate) scripts_dir: String,
    #[serde(default)]
    pub(crate) search_paths: Vec<String>,
}

impl Default for OldContent {
    fn default() -> Self {
        Self {
            scenes_dir: default_scenes_dir(),
            models_dir: default_models_dir(),
            textures_dir: default_textures_dir(),
            audio_dir: default_audio_dir(),
            shaders_dir: default_shaders_dir(),
            prefabs_dir: default_prefabs_dir(),
            scripts_dir: default_scripts_dir(),
            search_paths: Vec::new(),
        }
    }
}

fn default_scenes_dir() -> String {
    "Scenes".to_string()
}

fn default_models_dir() -> String {
    "Resources".to_string()
}

fn default_textures_dir() -> String {
    "Resources".to_string()
}

fn default_audio_dir() -> String {
    "Resources".to_string()
}

fn default_shaders_dir() -> String {
    "Shaders".to_string()
}

fn default_prefabs_dir() -> String {
    "Prefabs".to_string()
}

fn default_scripts_dir() -> String {
    "Scripts".to_string()
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct OldBuild {
    #[serde(default)]
    pub(crate) scenes: Vec<String>,
}

/// 只读 `project.ron`。`check_old_project` 与迁移共用同一份解析。
pub(crate) fn read_old_project(root: &Path) -> Result<OldProject, ProjectCheckError> {
    if !root.is_dir() {
        return Err(ProjectCheckError::NotADirectory(root.to_path_buf()));
    }
    let project_path = root.join("project.ron");
    let text = fs::read_to_string(&project_path).map_err(|err| ProjectCheckError::ProjectIo {
        path: project_path.clone(),
        message: err.to_string(),
    })?;
    ron::from_str(&text).map_err(|err| ProjectCheckError::ProjectParse(err.to_string()))
}

/// 检查一个旧工程目录。只读；不创建 / 不修改任何文件。
pub fn check_old_project(root: &Path) -> Result<ProjectCheckReport, ProjectCheckError> {
    let project = read_old_project(root)?;

    let mut report = ProjectCheckReport {
        name: project.name.clone(),
        ..ProjectCheckReport::default()
    };
    if project.format_version != OLD_PROJECT_FORMAT_VERSION {
        report.problems.push(format!(
            "[project.version] 只认 format_version {OLD_PROJECT_FORMAT_VERSION}，实际 {}",
            project.format_version
        ));
    }

    let content = project.content;
    let scenes_dir = normalize_into(&mut report, &content.scenes_dir, "content.scenes_dir");
    let prefabs_dir = normalize_into(&mut report, &content.prefabs_dir, "content.prefabs_dir");
    for dir in [
        &content.models_dir,
        &content.textures_dir,
        &content.audio_dir,
        &content.shaders_dir,
        &content.scripts_dir,
    ] {
        let _ = normalize_into(&mut report, dir, "content.dir");
    }
    let mut search_paths = Vec::new();
    for search in &content.search_paths {
        if let Some(normalized) = normalize_into(&mut report, search, "content.search_path") {
            search_paths.push(normalized);
        }
    }

    let entry = match normalize_project_relative(&project.entry_scene) {
        Ok(entry) => entry,
        Err(defect) => return Err(ProjectCheckError::InvalidEntry(defect.to_string())),
    };
    let entry_text = fs::read_to_string(root.join(&entry))
        .map_err(|_| ProjectCheckError::MissingEntry(entry.clone()))?;
    let entry_scene: OldScene = ron::from_str(&entry_text)
        .map_err(|err| ProjectCheckError::EntryParse(format!("{entry}: {err}")))?;
    if entry_scene.format_version != OLD_SCENE_FORMAT_VERSION {
        return Err(ProjectCheckError::EntryParse(format!(
            "{entry}: 只认 format_version {OLD_SCENE_FORMAT_VERSION}，实际 {}",
            entry_scene.format_version
        )));
    }

    let catalog = match scan(root) {
        Ok(catalog) => catalog,
        Err(err) => {
            report.problems.push(format!("[asset.scan] {err}"));
            AssetCatalog::default()
        }
    };
    report.assets = catalog.len();
    for issue in catalog.issues() {
        report.problems.push(format!("[asset.meta] {issue}"));
    }

    let on_disk = scene_files(root, scenes_dir.as_deref(), &mut report);
    let mut build = Vec::new();
    let mut seen_build = HashSet::new();
    for raw in &project.build.scenes {
        let Some(rel) = normalize_into(&mut report, raw, "build.scene") else {
            continue;
        };
        if !seen_build.insert(rel.clone()) {
            report
                .problems
                .push(format!("[build.duplicate] 清单重复列出 {rel}"));
            continue;
        }
        if !root.join(&rel).is_file() {
            report
                .problems
                .push(format!("[build.missing] 清单里的场景不存在：{rel}"));
            continue;
        }
        build.push(rel);
    }

    let mut scenes_to_check = vec![(entry.clone(), entry_text, entry_scene)];
    for rel in &build {
        if rel == &entry || scenes_to_check.iter().any(|(checked, _, _)| checked == rel) {
            continue;
        }
        match read_scene(root, rel) {
            Ok((text, scene)) => scenes_to_check.push((rel.clone(), text, scene)),
            Err(problem) => report.problems.push(problem),
        }
    }

    let mut prefab_cache: BTreeMap<PathBuf, Result<HashSet<u64>, String>> = BTreeMap::new();
    let mut script_paths = BTreeSet::new();
    let mut rust_components = 0usize;
    for (rel, scene_text, scene) in &scenes_to_check {
        report.scenes += 1;
        check_scene(
            &mut report,
            root,
            rel,
            scene_text,
            scene,
            &catalog,
            &search_paths,
            &mut prefab_cache,
            &mut script_paths,
            &mut rust_components,
        );
    }
    report.scripts = script_paths.len();
    if rust_components > 0 {
        report.hints.push(format!(
            "[plugin.unsupported] {rust_components} 个 Rust 插件组件在 XGC 里不会执行（无 cdylib 加载）"
        ));
    }

    let listed: HashSet<&String> = build.iter().collect();
    for rel in on_disk {
        if !listed.contains(&rel) {
            report.hints.push(format!(
                "[build.unlisted] {rel} 在 Scenes/ 下但没进 build.scenes"
            ));
        }
    }
    if let Some(prefabs_dir) = &prefabs_dir {
        let prefabs = prefab_files(root, prefabs_dir, &mut report);
        report.prefabs = prefabs.len();
        for path in &prefabs {
            if let Err(message) = prefab_ids(path, &catalog, &mut prefab_cache, &mut report) {
                report
                    .problems
                    .push(format!("[prefab.parse] {}: {message}", path.display()));
            }
        }
    }
    Ok(report)
}

/// 读一个场景；失败返回带 `[scene.*]` 码的问题行，而不是中断整个检查。
fn read_scene(root: &Path, rel: &str) -> Result<(String, OldScene), String> {
    let text = fs::read_to_string(root.join(rel))
        .map_err(|err| format!("[scene.missing] {rel}: {err}"))?;
    let scene: OldScene =
        ron::from_str(&text).map_err(|err| format!("[scene.parse] {rel}: {err}"))?;
    if scene.format_version != OLD_SCENE_FORMAT_VERSION {
        return Err(format!(
            "[scene.version] {rel}: 只认 format_version {OLD_SCENE_FORMAT_VERSION}，实际 {}",
            scene.format_version
        ));
    }
    Ok((text, scene))
}

#[allow(clippy::too_many_arguments)]
fn check_scene(
    report: &mut ProjectCheckReport,
    root: &Path,
    rel: &str,
    text: &str,
    scene: &OldScene,
    catalog: &AssetCatalog,
    search_paths: &[String],
    prefab_cache: &mut BTreeMap<PathBuf, Result<HashSet<u64>, String>>,
    script_paths: &mut BTreeSet<String>,
    rust_components: &mut usize,
) {
    report.entities += scene.entities.len();
    let mut ids = HashSet::new();
    let mut duplicates = BTreeSet::new();
    for entity in &scene.entities {
        if !ids.insert(entity.id.0) {
            duplicates.insert(entity.id.0);
        }
    }
    for id in duplicates {
        report
            .problems
            .push(format!("[scene.duplicate_id] {rel}: 实体 ID {id} 重复"));
    }
    if ids.contains(&u64::MAX) {
        report.problems.push(format!(
            "[scene.bad_id] {rel}: u64::MAX 是坏数据哨兵（读盘会顶死 ID 计数器）"
        ));
    }

    for entity in &scene.entities {
        check_entity(
            report,
            root,
            rel,
            entity,
            &ids,
            catalog,
            search_paths,
            prefab_cache,
            script_paths,
            rust_components,
        );
    }
    for guid in guid_refs(text) {
        if !catalog.contains(&guid) {
            report.problems.push(format!(
                "[asset.dangling] {rel}: guid://{guid} 没有对应的 .meta"
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_entity(
    report: &mut ProjectCheckReport,
    root: &Path,
    rel: &str,
    entity: &OldEntity,
    ids: &HashSet<u64>,
    catalog: &AssetCatalog,
    search_paths: &[String],
    prefab_cache: &mut BTreeMap<PathBuf, Result<HashSet<u64>, String>>,
    script_paths: &mut BTreeSet<String>,
    rust_components: &mut usize,
) {
    if let Some(parent) = entity.parent {
        if !ids.contains(&parent.0) {
            report.problems.push(format!(
                "[scene.parent_missing] {rel}: 实体 {} 的父节点 {} 不在本场景",
                entity.id.0, parent.0
            ));
        }
    }
    if entity.layer > 31 {
        report.problems.push(format!(
            "[scene.bad_layer] {rel}: 实体 {} 的 layer={} 超过 31",
            entity.id.0, entity.layer
        ));
    }
    let transform = &entity.transform;
    if !transform.position.is_finite()
        || !transform.rotation.is_finite()
        || !transform.scale.is_finite()
    {
        report.problems.push(format!(
            "[scene.non_finite] {rel}: 实体 {} 的 Transform 含 NaN / Inf",
            entity.id.0
        ));
    }
    if let Some(prefab) = &entity.prefab {
        check_prefab_ref(
            report,
            root,
            rel,
            prefab.source.as_str(),
            prefab.source_id.0,
            catalog,
            prefab_cache,
        );
    }

    for entry in &entity.components.entries {
        match &entry.data {
            OldComponentData::Lua { path } => {
                if let Some(guid) = path.strip_prefix("guid://") {
                    match catalog.resolve(guid) {
                        Some(resolved) if resolved.is_file() => {
                            script_paths.insert(format!("guid://{guid}"));
                        }
                        _ => report.problems.push(format!(
                            "[script.missing] {rel}: 实体 {} 引用的脚本 GUID 没有对应文件：{path}",
                            entity.id.0
                        )),
                    }
                } else {
                    match normalize_project_relative(path) {
                        Ok(normalized) => {
                            if resolve_read(root, &normalized, search_paths).is_some() {
                                script_paths.insert(normalized);
                            } else {
                                report.problems.push(format!(
                                    "[script.missing] {rel}: 实体 {} 引用的脚本不存在：{path}",
                                    entity.id.0
                                ));
                            }
                        }
                        Err(defect) => report.problems.push(format!(
                            "[script.path] {rel}: 实体 {} 的脚本路径不合法（{}）：{path}",
                            entity.id.0,
                            defect.ascii()
                        )),
                    }
                }
            }
            OldComponentData::Rust { .. } => *rust_components += 1,
            _ => {}
        }
    }
}

mod refs;

use self::refs::{
    check_prefab_ref, guid_refs, normalize_into, prefab_files, prefab_ids, resolve_read,
    scene_files,
};
