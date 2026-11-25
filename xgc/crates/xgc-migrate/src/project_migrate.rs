//! 项目级迁移：把旧 `project.ron` 指向的关卡与内容搬成一份可加载的 XGC 项目。
//!
//! 与 `migrate_dir` 的区别：目录里所有 `.ron` 一把梭会把 `project.ron` /
//! `input.ron` 当场景；这里只认清单，先 `check_old_project` 体检，再迁
//! **入口 + build.scenes**，其余 RON 不碰。
//!
//! 事务边界：所有写入先落在输出目录同级的 staging 目录，全部成功后才 rename；
//! 中途失败删 staging，输出路径从不出现半套项目。干跑只算账，不建目录。
//!
//! 场景仍是 `save_world` 的版本化 JSON（`scenes/*.json`），不是设计文档里的
//! `*.scene.ron`——XGC 的 store 目前只认这一种世界格式，清单也不复制场景内容。

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use xgc_base::{path::normalize_project_relative, EntityId};
use xgc_store::{
    save_project, save_world, ProjectContent, ProjectManifest, PROJECT_FORMAT_VERSION,
};
use xgc_world::WorldData;

use crate::migrate_str;
use crate::prefab::remap_parent;
use crate::project_check::{check_old_project, read_old_project, ProjectCheckError};

const ID_MAP_FILE: &str = "entity-id-map.json";

#[derive(Debug)]
pub enum ProjectMigrationError {
    Check(ProjectCheckError),
    /// 项目体检有问题；迁移拒绝开始，输出目录不会被创建。
    Problems(Vec<String>),
    OutputExists(PathBuf),
    Io {
        path: PathBuf,
        message: String,
    },
    Scene {
        scene: String,
        message: String,
    },
    Write {
        path: PathBuf,
        message: String,
    },
    Manifest(String),
}

impl std::fmt::Display for ProjectMigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Check(err) => write!(f, "{err}"),
            Self::Problems(problems) => {
                writeln!(f, "项目体检发现 {} 个问题，未开始迁移：", problems.len())?;
                for problem in problems {
                    writeln!(f, "  {problem}")?;
                }
                Ok(())
            }
            Self::OutputExists(path) => {
                write!(f, "输出目录已存在，拒绝覆盖：{}", path.display())
            }
            Self::Io { path, message } => write!(f, "IO 错误（{}）：{message}", path.display()),
            Self::Scene { scene, message } => write!(f, "场景 {scene} 迁移失败：{message}"),
            Self::Write { path, message } => write!(f, "写入失败（{}）：{message}", path.display()),
            Self::Manifest(message) => write!(f, "项目清单不合法：{message}"),
        }
    }
}

impl std::error::Error for ProjectMigrationError {}

impl From<ProjectCheckError> for ProjectMigrationError {
    fn from(err: ProjectCheckError) -> Self {
        Self::Check(err)
    }
}

#[derive(Clone, Debug)]
pub struct ProjectMigrationReport {
    pub name: String,
    pub scenes: usize,
    pub entities: usize,
    pub copied_files: usize,
    pub dry_run: bool,
    pub output: PathBuf,
}

impl ProjectMigrationReport {
    pub fn render(&self) -> String {
        let suffix = if self.dry_run {
            "（dry-run，未写盘）"
        } else {
            ""
        };
        format!(
            "migrate: project={} scenes={} entities={} copied={} -> {}{}\n  entity ID map: {ID_MAP_FILE}",
            self.name,
            self.scenes,
            self.entities,
            self.copied_files,
            self.output.display(),
            suffix
        )
    }
}

/// 迁移一个旧工程目录。`dry_run` 为真时只体检 + 算报告，不创建任何目录。
pub fn migrate_old_project(
    root: &Path,
    output: &Path,
    dry_run: bool,
) -> Result<ProjectMigrationReport, ProjectMigrationError> {
    let check = check_old_project(root)?;
    if check.has_problems() {
        return Err(ProjectMigrationError::Problems(check.problems));
    }
    let project = read_old_project(root)?;
    let plan = plan_project(root, &project)?;

    let report = |dry_run: bool| ProjectMigrationReport {
        name: plan.manifest.name.clone(),
        scenes: plan.scenes.len(),
        entities: plan.scenes.iter().map(|scene| scene.ids.len()).sum(),
        copied_files: plan.copied_files,
        dry_run,
        output: output.to_path_buf(),
    };

    if dry_run {
        return Ok(report(true));
    }
    if output.symlink_metadata().is_ok() {
        return Err(ProjectMigrationError::OutputExists(output.to_path_buf()));
    }

    let staging = staging_path(output);
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|err| ProjectMigrationError::Io {
                path: parent.to_path_buf(),
                message: err.to_string(),
            })?;
        }
    }

    // 独占拿到暂存目录后才能在失败时清理，避免删掉另一次迁移正在写的目录。
    fs::create_dir(&staging).map_err(|err| ProjectMigrationError::Io {
        path: staging.clone(),
        message: err.to_string(),
    })?;
    if let Err(err) = write_staging(root, &staging, &plan) {
        let _ = fs::remove_dir_all(&staging);
        return Err(err);
    }
    if let Err(err) = fs::rename(&staging, output) {
        let _ = fs::remove_dir_all(&staging);
        return Err(ProjectMigrationError::Io {
            path: output.to_path_buf(),
            message: err.to_string(),
        });
    }
    Ok(report(false))
}

#[derive(Serialize)]
struct ScenePlan {
    source: String,
    target: String,
    ids: BTreeMap<EntityId, EntityId>,
    #[serde(skip)]
    world: WorldData,
}

#[derive(Serialize)]
struct IdMap<'a> {
    format_version: u32,
    files: &'a [ScenePlan],
}

struct Plan {
    manifest: ProjectManifest,
    scenes: Vec<ScenePlan>,
    copy_dirs: Vec<String>,
    copy_files: Vec<String>,
    copied_files: usize,
}

fn plan_project(
    root: &Path,
    project: &crate::project_check::OldProject,
) -> Result<Plan, ProjectMigrationError> {
    let scenes_dir = normalize(project.content.scenes_dir.trim())?;
    let entry = normalize(&project.entry_scene)?;

    let mut ordered = Vec::new();
    let mut seen = BTreeSet::new();
    if !project
        .build
        .scenes
        .iter()
        .any(|scene| scene.trim() == project.entry_scene.trim())
    {
        ordered.push(entry.clone());
        seen.insert(entry.clone());
    }
    for scene in &project.build.scenes {
        let scene = normalize(scene)?;
        if seen.insert(scene.clone()) {
            ordered.push(scene);
        }
    }
    if ordered.is_empty() {
        ordered.push(entry.clone());
    }

    let mut scenes = Vec::new();
    let mut sources = BTreeMap::new();
    for old in &ordered {
        let path = root.join(old);
        let canonical = fs::canonicalize(&path).map_err(|err| ProjectMigrationError::Io {
            path,
            message: err.to_string(),
        })?;
        if let Some(first) = sources.insert(canonical, old) {
            return Err(ProjectMigrationError::Manifest(format!(
                "{first} 与 {old} 指向同一个源文件"
            )));
        }
        let out = scene_output(old, &scenes_dir)?;
        scenes.push(read_scene(root, old, out)?);
    }
    let entry_out = scenes
        .iter()
        .find(|scene| scene.source == entry)
        .map(|scene| scene.target.clone())
        .ok_or_else(|| ProjectMigrationError::Manifest("入口场景不在迁移清单里".into()))?;

    let name = if project.name.trim().is_empty() {
        root.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into())
    } else {
        project.name.trim().to_string()
    };

    let mut copy_dirs = Vec::new();
    for raw in [
        &project.content.models_dir,
        &project.content.textures_dir,
        &project.content.audio_dir,
        &project.content.shaders_dir,
        &project.content.prefabs_dir,
        &project.content.scripts_dir,
    ] {
        if raw.trim().is_empty() {
            continue;
        }
        let dir = normalize(raw)?;
        if !copy_dirs.contains(&dir) {
            copy_dirs.push(dir);
        }
    }

    let mut copy_files = Vec::new();
    if root.join("input.ron").is_file() {
        copy_files.push("input.ron".to_string());
    }

    let mut search_paths = Vec::new();
    for search in &project.content.search_paths {
        let search = normalize(search)?;
        if !search_paths.contains(&search) {
            search_paths.push(search);
        }
    }

    let shaders = if project.content.shaders_dir.trim().is_empty()
        || !root.join(project.content.shaders_dir.trim()).is_dir()
    {
        None
    } else {
        Some(normalize(&project.content.shaders_dir)?)
    };
    let manifest = ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        name,
        entry_scene: entry_out,
        scenes: scenes.iter().map(|scene| scene.target.clone()).collect(),
        content: ProjectContent {
            models: normalize(&project.content.models_dir)?,
            textures: normalize(&project.content.textures_dir)?,
            audio: normalize(&project.content.audio_dir)?,
            shaders,
            prefabs: normalize(&project.content.prefabs_dir)?,
            scripts: normalize(&project.content.scripts_dir)?,
        },
        search_paths,
        input: copy_files.first().cloned(),
    };
    manifest
        .validate()
        .map_err(|err| ProjectMigrationError::Manifest(err.to_string()))?;

    validate_output_paths(root, &manifest, &copy_dirs)?;
    copy_dirs.retain(|dir| root.join(dir).is_dir());
    let mut copied_files = 0;
    for dir in &copy_dirs {
        copied_files += count_files(&root.join(dir)).map_err(|err| ProjectMigrationError::Io {
            path: root.join(dir),
            message: err.to_string(),
        })?;
    }
    copied_files += copy_files.len();

    remap_scene_ids(&mut scenes);
    Ok(Plan {
        manifest,
        scenes,
        copy_dirs,
        copy_files,
        copied_files,
    })
}

fn validate_output_paths(
    root: &Path,
    manifest: &ProjectManifest,
    copy_dirs: &[String],
) -> Result<(), ProjectMigrationError> {
    let mut outputs = BTreeMap::new();
    for target in [ID_MAP_FILE, "project.xgc"]
        .into_iter()
        .chain(manifest.scenes.iter().map(String::as_str))
    {
        if outputs.insert(output_path_key(target), target).is_some() {
            return Err(ProjectMigrationError::Manifest(format!(
                "输出文件 {target} 重复"
            )));
        }
    }
    let copy_roots: Vec<_> = copy_dirs.iter().map(|dir| output_path_key(dir)).collect();
    let search_roots: Vec<_> = manifest
        .search_paths
        .iter()
        .map(|dir| output_path_key(dir))
        .collect();
    for (key, target) in &outputs {
        let output_overlap = key
            .ancestors()
            .skip(1)
            .any(|path| outputs.contains_key(path));
        let reserved_dir = copy_roots
            .iter()
            .chain(&search_roots)
            .any(|dir| dir.starts_with(key));
        let copied_entry = Path::new(target).ancestors().any(|path| {
            let relative = path.to_str().expect("输出路径是 UTF-8");
            let ancestor = output_path_key(relative);
            copy_roots.iter().any(|dir| ancestor.starts_with(dir))
                && root.join(path).symlink_metadata().is_ok_and(|metadata| {
                    metadata.is_file() || (&ancestor == key && metadata.is_dir())
                })
        });
        if output_overlap || reserved_dir || copied_entry {
            return Err(ProjectMigrationError::Manifest(format!(
                "输出文件 {target} 与项目路径冲突"
            )));
        }
    }
    Ok(())
}

fn output_path_key(path: &str) -> PathBuf {
    // Windows 输出路径按通常的大小写不敏感口径预检，避免 dry-run 漏掉冲突。
    if cfg!(windows) {
        PathBuf::from(path.to_lowercase())
    } else {
        PathBuf::from(path)
    }
}

fn scene_output(old: &str, scenes_dir: &str) -> Result<String, ProjectMigrationError> {
    let prefix = format!("{scenes_dir}/");
    let relative = old
        .strip_prefix(&prefix)
        .unwrap_or_else(|| old.rsplit_once('/').map(|(_, file)| file).unwrap_or(old));
    let stem = relative.strip_suffix(".ron").unwrap_or(relative);
    if stem.is_empty() {
        return Err(ProjectMigrationError::Manifest(format!(
            "场景路径没有文件名：{old}"
        )));
    }
    Ok(format!("scenes/{stem}.json"))
}

fn read_scene(
    root: &Path,
    source: &str,
    target: String,
) -> Result<ScenePlan, ProjectMigrationError> {
    let text =
        fs::read_to_string(root.join(source)).map_err(|err| ProjectMigrationError::Scene {
            scene: source.to_string(),
            message: err.to_string(),
        })?;
    let (world, _) = migrate_str(&text).map_err(|err| ProjectMigrationError::Scene {
        scene: source.to_string(),
        message: err.to_string(),
    })?;
    let ids = world
        .entities
        .iter()
        .map(|entity| (entity.id, entity.id))
        .collect();
    Ok(ScenePlan {
        source: source.to_string(),
        target,
        ids,
        world,
    })
}

fn remap_scene_ids(scenes: &mut [ScenePlan]) {
    // 关卡清单的顺序可以改。用文件路径和文件内的旧 ID 排序，避免关卡改序影响映射。
    scenes.sort_unstable_by(|left, right| left.source.cmp(&right.source));
    let mut next_id = 1;
    for scene in scenes.iter_mut() {
        for mapped in scene.ids.values_mut() {
            *mapped = EntityId::from_raw(next_id);
            next_id += 1;
        }
        for entity in &mut scene.world.entities {
            entity.id = scene.ids[&entity.id];
            remap_parent(entity, &scene.ids);
        }
    }
    // 各场景第一次分配的新 ID 都必须越过本次迁移的全部实体。
    for scene in scenes {
        scene.world.next_id = next_id;
    }
}

fn write_staging(root: &Path, staging: &Path, plan: &Plan) -> Result<(), ProjectMigrationError> {
    for dir in &plan.copy_dirs {
        copy_tree(&root.join(dir), &staging.join(dir))?;
    }
    for file in &plan.copy_files {
        let target = staging.join(file);
        copy_file(&root.join(file), &target)?;
    }
    for scene in &plan.scenes {
        let target = staging.join(&scene.target);
        if target.exists() {
            return Err(ProjectMigrationError::Write {
                path: target,
                message: "场景输出与复制的内容路径冲突".into(),
            });
        }
        save_world(&target, &scene.world).map_err(|err| ProjectMigrationError::Write {
            path: target,
            message: err.to_string(),
        })?;
    }
    write_id_map(staging, &plan.scenes)?;
    let manifest_path = staging.join("project.xgc");
    save_project(&manifest_path, &plan.manifest).map_err(|err| ProjectMigrationError::Write {
        path: manifest_path,
        message: err.to_string(),
    })?;
    Ok(())
}

fn write_id_map(staging: &Path, scenes: &[ScenePlan]) -> Result<(), ProjectMigrationError> {
    let path = staging.join(ID_MAP_FILE);
    let error = |message| ProjectMigrationError::Write {
        path: path.clone(),
        message,
    };
    let mapping = IdMap {
        format_version: 1,
        files: scenes,
    };
    let bytes = serde_json::to_vec_pretty(&mapping).map_err(|err| error(err.to_string()))?;
    // 暂存目录本身就是事务边界。独占创建避免覆盖同名的用户内容。
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut file| file.write_all(&bytes))
        .map_err(|err| error(err.to_string()))
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), ProjectMigrationError> {
    fs::create_dir_all(target).map_err(|err| ProjectMigrationError::Io {
        path: target.to_path_buf(),
        message: err.to_string(),
    })?;
    let entries = fs::read_dir(source).map_err(|err| ProjectMigrationError::Io {
        path: source.to_path_buf(),
        message: err.to_string(),
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| ProjectMigrationError::Io {
            path: source.to_path_buf(),
            message: err.to_string(),
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|err| ProjectMigrationError::Io {
            path: path.clone(),
            message: err.to_string(),
        })?;
        if file_type.is_dir() {
            copy_tree(&path, &target.join(entry.file_name()))?;
        } else if file_type.is_file() {
            copy_file(&path, &target.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn copy_file(source: &Path, target: &Path) -> Result<(), ProjectMigrationError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|err| ProjectMigrationError::Io {
            path: parent.to_path_buf(),
            message: err.to_string(),
        })?;
    }
    fs::copy(source, target)
        .map(|_| ())
        .map_err(|err| ProjectMigrationError::Io {
            path: source.to_path_buf(),
            message: err.to_string(),
        })
}

fn count_files(path: &Path) -> std::io::Result<usize> {
    let mut count = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            count += count_files(&entry.path())?;
        } else if file_type.is_file() {
            count += 1;
        }
    }
    Ok(count)
}

fn normalize(raw: &str) -> Result<String, ProjectMigrationError> {
    normalize_project_relative(raw)
        .map_err(|defect| ProjectMigrationError::Manifest(format!("路径不合法（{raw}）：{defect}")))
}

fn staging_path(output: &Path) -> PathBuf {
    let name = output
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string());
    output.with_file_name(format!(".{name}.staging-{}", std::process::id()))
}
