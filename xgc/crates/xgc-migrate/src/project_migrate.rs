//! 项目级迁移先规划清单、场景与预制体，再在同级暂存目录写全套输出。
//! 干跑执行同一份规划，输出只在全部写入成功后出现。

mod documents;
mod files;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use xgc_base::path::normalize_project_relative;
use xgc_store::{
    save_project, save_world, ProjectContent, ProjectManifest, PROJECT_FORMAT_VERSION,
};
use xgc_world::{builtin_registry, AssetKind, Value};

use self::documents::{canonical, PrefabPlans, WorldPlan};
use self::files::Copies;
use crate::project_check::{check_old_project, read_old_project, OldProject, ProjectCheckError};

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
    Document {
        source: String,
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
            Self::Document { source, message } => write!(f, "文件 {source} 迁移失败：{message}"),
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
    pub prefabs: usize,
    pub prefab_entities: usize,
    pub copied_files: usize,
    /// 目标场景和预制体里实际写入的组件，包括身份、变换和运行所需的缺省组件。
    pub components: usize,
    /// 目标组件的项目文件引用按出现次数计数，忽略空引用和内建模型。
    pub asset_references: usize,
    /// 父节点和预制体来源等实体 ID 引用按出现次数计数。
    pub entity_references: usize,
    /// 需要移植的行为和需要重新配置的字段，使用旧文件与实体 ID 定位。
    pub unmatched: Vec<String>,
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
        let mut text = format!(
            "migrate: project={} scenes={} entities={} copied={} prefabs={} prefab_entities={} components={} asset_references={} entity_references={} unmatched={} -> {}{}\n  entity ID map: {ID_MAP_FILE}",
            self.name,
            self.scenes,
            self.entities,
            self.copied_files,
            self.prefabs,
            self.prefab_entities,
            self.components,
            self.asset_references,
            self.entity_references,
            self.unmatched.len(),
            self.output.display(),
            suffix
        );
        for note in &self.unmatched {
            text.push_str("\n  migration: ");
            text.push_str(note);
        }
        text
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
    let plan = plan_project(root, &read_old_project(root)?)?;
    let report = plan.report(output, dry_run);

    if dry_run {
        return Ok(report);
    }
    if output.symlink_metadata().is_ok() {
        return Err(ProjectMigrationError::OutputExists(output.to_path_buf()));
    }

    let staging = staging_path(output);
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|err| ProjectMigrationError::Io {
            path: parent.to_path_buf(),
            message: err.to_string(),
        })?;
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
    Ok(report)
}

#[derive(Serialize)]
struct IdMap<'a> {
    format_version: u32,
    files: Vec<&'a WorldPlan>,
}

struct Plan {
    manifest: ProjectManifest,
    scenes: Vec<WorldPlan>,
    prefabs: PrefabPlans,
    copies: Copies,
}

impl Plan {
    fn report(&self, output: &Path, dry_run: bool) -> ProjectMigrationReport {
        let prefabs = self
            .prefabs
            .documents
            .values()
            .map(|prefab| &prefab.document);
        let mut report = ProjectMigrationReport {
            name: self.manifest.name.clone(),
            scenes: self.scenes.len(),
            entities: self.scenes.iter().map(|scene| scene.ids.len()).sum(),
            prefabs: self.prefabs.documents.len(),
            prefab_entities: prefabs.clone().map(|document| document.ids.len()).sum(),
            copied_files: self.copies.len() + self.prefabs.meta_count(),
            components: 0,
            asset_references: 0,
            entity_references: 0,
            unmatched: Vec::new(),
            dry_run,
            output: output.to_path_buf(),
        };
        let registry = builtin_registry();
        for document in self.scenes.iter().chain(prefabs) {
            report.unmatched.extend(document.unmatched.iter().cloned());
            for component in document
                .world
                .entities
                .iter()
                .flat_map(|entity| &entity.components)
            {
                report.components += 1;
                report.entity_references += component
                    .fields
                    .iter()
                    .filter(|(_, value)| matches!(value, Value::Entity(_)))
                    .count();
                if let Some(schema) = registry.get(component.component) {
                    schema.visit_asset_references(&component.fields, |kind, _, reference| {
                        if !reference.is_empty()
                            && !(kind == AssetKind::Model && reference.starts_with("builtin://"))
                        {
                            report.asset_references += 1;
                        }
                    });
                }
            }
        }
        report.unmatched.sort_unstable();
        report
    }
}

fn plan_project(root: &Path, project: &OldProject) -> Result<Plan, ProjectMigrationError> {
    let scenes_dir = normalize(&project.content.scenes_dir)?;
    let entry = normalize(&project.entry_scene)?;
    let mut ordered = Vec::new();
    let mut seen = BTreeSet::new();
    for scene in &project.build.scenes {
        let scene = normalize(scene)?;
        if seen.insert(scene.clone()) {
            ordered.push(scene);
        }
    }
    if !seen.contains(&entry) {
        ordered.insert(0, entry.clone());
    }

    let mut scenes = Vec::new();
    let mut sources = BTreeMap::new();
    for old in &ordered {
        if let Some(first) = sources.insert(canonical(&root.join(old))?, old) {
            return Err(ProjectMigrationError::Manifest(format!(
                "{first} 与 {old} 指向同一个源文件"
            )));
        }
        scenes.push(WorldPlan::read(root, old, scene_output(old, &scenes_dir)?)?);
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
    let shaders = if project.content.shaders_dir.trim().is_empty() {
        None
    } else {
        let path = normalize(&project.content.shaders_dir)?;
        root.join(&path).is_dir().then_some(path)
    };
    let search_paths = project
        .content
        .search_paths
        .iter()
        .map(|search| normalize(search))
        .collect::<Result<Vec<_>, _>>()?;
    // 搜索根按首见顺序去重，解析优先级跟旧项目一致。
    let mut seen_search = BTreeSet::new();
    let search_paths = search_paths
        .into_iter()
        .filter(|path| seen_search.insert(path.clone()))
        .collect();
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
        input: root
            .join("input.ron")
            .is_file()
            .then(|| "input.ron".to_string()),
    };
    manifest
        .validate()
        .map_err(|err| ProjectMigrationError::Manifest(err.to_string()))?;

    let mut prefabs = PrefabPlans::read(root, &manifest.content.prefabs, &scenes)?;
    // 随原目录复制兜底资源和伴生文件，场景里的逻辑路径仍按清单顺序解析。
    let directories: Vec<_> = [
        &manifest.content.models,
        &manifest.content.textures,
        &manifest.content.audio,
        &manifest.content.prefabs,
        &manifest.content.scripts,
    ]
    .into_iter()
    .chain(manifest.content.shaders.iter())
    .chain(&manifest.search_paths)
    .cloned()
    .collect();
    let copies = Copies::plan(
        root,
        &directories,
        manifest.input.as_deref(),
        &prefabs.replaced_paths(),
    )?;
    copies.validate(
        [ID_MAP_FILE, "project.xgc"]
            .into_iter()
            .chain(manifest.scenes.iter().map(String::as_str))
            .chain(prefabs.targets()),
        directories.iter().map(String::as_str),
    )?;
    remap_document_ids(&mut scenes, &mut prefabs);
    prefabs.remap_instances(&mut scenes)?;
    Ok(Plan {
        manifest,
        scenes,
        prefabs,
        copies,
    })
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

fn remap_document_ids(scenes: &mut [WorldPlan], prefabs: &mut PrefabPlans) {
    // 用文件路径和旧 ID 排序，关卡改序或新增预制体不改变既有场景映射。
    scenes.sort_unstable_by(|left, right| left.source.cmp(&right.source));
    let mut prefab_documents: Vec<_> = prefabs
        .documents
        .values_mut()
        .map(|prefab| &mut prefab.document)
        .collect();
    prefab_documents.sort_unstable_by(|left, right| left.source.cmp(&right.source));
    let mut next_id = 1;
    for document in scenes
        .iter_mut()
        .chain(prefab_documents.iter_mut().map(|doc| &mut **doc))
    {
        document.assign_ids(&mut next_id);
    }
    // 第一次运行时分配越过本次迁移的所有 ID，包括预制体模板。
    for document in scenes.iter_mut().chain(prefab_documents) {
        document.world.next_id = next_id;
    }
}

fn write_staging(root: &Path, staging: &Path, plan: &Plan) -> Result<(), ProjectMigrationError> {
    plan.copies.write(root, staging)?;
    for scene in &plan.scenes {
        scene.save(staging, save_world)?;
    }
    plan.prefabs.write(root, staging)?;
    write_id_map(staging, plan)?;
    let manifest_path = staging.join("project.xgc");
    save_project(&manifest_path, &plan.manifest).map_err(|err| ProjectMigrationError::Write {
        path: manifest_path,
        message: err.to_string(),
    })?;
    Ok(())
}

fn write_id_map(staging: &Path, plan: &Plan) -> Result<(), ProjectMigrationError> {
    let path = staging.join(ID_MAP_FILE);
    let error = |message| ProjectMigrationError::Write {
        path: path.clone(),
        message,
    };
    let mut prefabs: Vec<_> = plan
        .prefabs
        .documents
        .values()
        .map(|prefab| &prefab.document)
        .collect();
    prefabs.sort_unstable_by(|left, right| left.source.cmp(&right.source));
    let mapping = IdMap {
        format_version: 1,
        files: plan.scenes.iter().chain(prefabs).collect(),
    };
    let bytes = serde_json::to_vec_pretty(&mapping).map_err(|err| error(err.to_string()))?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut file| file.write_all(&bytes))
        .map_err(|err| error(err.to_string()))
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
