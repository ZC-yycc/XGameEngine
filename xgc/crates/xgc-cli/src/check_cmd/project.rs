//! 原生项目只读体检，使用项目清单、世界和预制体的正式读盘边界。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use xgc_asset::{find_files, resolve_read_path, AssetCatalog, SearchPaths};
use xgc_base::{path::normalize_project_relative, EntityId};
use xgc_migrate::ProjectCheckReport;
use xgc_reflect::{AssetKind, Component, Registry, Value};
use xgc_sim::InputMap;
use xgc_store::{load_prefab, load_world, ProjectSession, StoreError};
use xgc_world::{ComponentData, Parent, PluginComponent, PrefabInstance, World, WorldData};

pub(super) fn check(path: &Path, registry: &Registry) -> Result<ProjectCheckReport, String> {
    let project = ProjectSession::open(path).map_err(|err| format!("{}：{err}", path.display()))?;
    check_session(&project, registry).map(|audit| audit.report)
}

pub(crate) struct ProjectAudit {
    pub report: ProjectCheckReport,
    pub next_entity_id: u64,
}

pub(crate) fn check_session(
    project: &ProjectSession,
    registry: &Registry,
) -> Result<ProjectAudit, String> {
    let manifest = project.manifest();
    let root = project
        .root()
        .canonicalize()
        .map_err(|err| err.to_string())?;
    let entry = project.entry_scene();
    let (entry_path, entry_data) =
        load_scene(project, &root, entry).map_err(|err| format!("入口场景 {entry}：{err}"))?;

    let mut report = ProjectCheckReport {
        name: manifest.name.clone(),
        ..ProjectCheckReport::default()
    };
    let catalog = match xgc_asset::scan(&root) {
        Ok(catalog) => catalog,
        Err(err) => {
            report.problems.push(format!("[asset.scan] {err}"));
            AssetCatalog::default()
        }
    };
    report.assets = catalog.len();
    report.problems.extend(
        catalog
            .issues()
            .iter()
            .map(|issue| format!("[asset.meta] {issue}")),
    );
    let mut audit = Audit {
        root,
        registry,
        catalog,
        search_paths: SearchPaths::new(&manifest.search_paths),
        report,
        project_next_id: manifest.next_entity_id,
        next_entity_id: 1,
        scripts: BTreeSet::new(),
        seen_ids: BTreeMap::new(),
        prefabs: BTreeMap::new(),
        pending: Vec::new(),
    };
    audit.discover_prefabs(&manifest.content.prefabs);
    audit.scene(entry, &entry_data);
    let mut scenes = BTreeSet::from([entry_path]);
    let entry_key = normalize_project_relative(entry).ok();
    for scene in &manifest.scenes {
        if normalize_project_relative(scene).ok() == entry_key {
            continue;
        }
        match load_scene(project, &audit.root, scene) {
            Ok((path, data)) => {
                if scenes.insert(path) {
                    audit.scene(scene, &data);
                } else {
                    audit.problem("scene.duplicate_file", scene, "同一场景文件被重复登记");
                }
            }
            Err(err) => {
                let code = match &err {
                    StoreError::Io(err) if err.kind() == ErrorKind::NotFound => "scene.missing",
                    _ => "scene.read",
                };
                audit.problem(code, scene, &err.to_string());
            }
        }
    }
    // 一个预制体可能再引用另一个预制体。先登记源 ID，再查引用，避免循环装载。
    while let Some((document, data)) = audit.pending.pop() {
        audit.document(&document, "prefab", &data);
    }
    if let Some(input) = &manifest.input {
        audit.input(input, project.input_path());
    }
    audit.check_scripts();
    Ok(ProjectAudit {
        report: audit.report,
        next_entity_id: audit.next_entity_id,
    })
}

fn load_scene(
    project: &ProjectSession,
    root: &Path,
    scene: &str,
) -> Result<(PathBuf, WorldData), StoreError> {
    let path = checked_file(root, &project.scene_path(scene)?)?;
    let data = load_world(&path)?;
    Ok((path, data))
}

struct Audit<'a> {
    root: PathBuf,
    registry: &'a Registry,
    catalog: AssetCatalog,
    search_paths: SearchPaths,
    report: ProjectCheckReport,
    project_next_id: u64,
    next_entity_id: u64,
    scripts: BTreeSet<PathBuf>,
    seen_ids: BTreeMap<EntityId, String>,
    prefabs: BTreeMap<PathBuf, Option<BTreeSet<EntityId>>>,
    pending: Vec<(String, WorldData)>,
}

impl Audit<'_> {
    fn problem(&mut self, code: &str, subject: &str, message: &str) {
        self.report
            .problems
            .push(format!("[{code}] {subject}：{message}"));
    }

    fn resolve(&self, kind: AssetKind, reference: &str) -> Result<PathBuf, String> {
        let direct = SearchPaths::default();
        let paths = if kind.uses_search_paths() {
            &self.search_paths
        } else {
            &direct
        };
        let path = resolve_read_path(&self.root, reference, paths, Some(&self.catalog))
            .map_err(|err| err.to_string())?;
        checked_file(&self.root, &path).map_err(|err| err.to_string())
    }

    fn discover_prefabs(&mut self, directory: &str) {
        // 清单已经校验过路径；允许项目没有预制体目录。
        let Ok(relative) = normalize_project_relative(directory) else {
            return;
        };
        let path = match self.root.join(relative).canonicalize() {
            Ok(path) => path,
            Err(err) if err.kind() == ErrorKind::NotFound => return,
            Err(err) => {
                self.problem("prefab.scan", directory, &err.to_string());
                return;
            }
        };
        let listing = in_project(&self.root, &path).and_then(|()| {
            find_files(&path, |file| {
                file.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".prefab.json"))
            })
        });
        match listing {
            Ok(files) => {
                for file in files {
                    self.prefab(&file);
                }
            }
            Err(err) => self.problem("prefab.scan", directory, &err.to_string()),
        }
    }

    fn prefab(&mut self, path: &Path) -> Option<&BTreeSet<EntityId>> {
        if !self.prefabs.contains_key(path) {
            self.report.prefabs += 1;
            let name = path
                .strip_prefix(&self.root)
                .unwrap_or(path)
                .display()
                .to_string();
            let ids = match load_prefab(path) {
                Ok(data) => {
                    let ids = data.entities.iter().map(|entity| entity.id).collect();
                    self.pending.push((name, data));
                    Some(ids)
                }
                Err(err) => {
                    self.problem("prefab.parse", &name, &err.to_string());
                    None
                }
            };
            self.prefabs.insert(path.to_path_buf(), ids);
        }
        self.prefabs.get(path).and_then(Option::as_ref)
    }

    fn scene(&mut self, document: &str, data: &WorldData) {
        self.report.scenes += 1;
        self.report.entities += data.entities.len();
        self.document(document, "scene", data);
    }

    fn document(&mut self, document: &str, kind: &str, data: &WorldData) {
        for diagnostic in World::validate_data(data, self.registry) {
            self.report
                .problems
                .push(format!("{document}：{diagnostic}"));
        }
        self.next_entity_id = self.next_entity_id.max(data.next_id);
        if data.next_id > self.project_next_id {
            self.problem(
                "project.id_watermark",
                document,
                &format!(
                    "文件 ID 水位 {} 超出项目 next_entity_id {}",
                    data.next_id, self.project_next_id
                ),
            );
        }
        let ids: BTreeSet<_> = data.entities.iter().map(|entity| entity.id).collect();
        for id in ids.iter().copied() {
            if id.validate().is_ok() {
                self.next_entity_id = self.next_entity_id.max(id.raw() + 1);
                if id.raw() >= self.project_next_id {
                    self.problem(
                        "project.id_watermark",
                        document,
                        &format!(
                            "实体 ID {id} 未低于项目 next_entity_id {}",
                            self.project_next_id
                        ),
                    );
                }
            }
            if let Some(first) = self.seen_ids.get(&id) {
                let message = format!("实体 ID {id} 已出现在 {first}");
                self.problem("project.duplicate_id", document, &message);
            } else {
                self.seen_ids.insert(id, document.to_string());
            }
        }
        let mut parents = BTreeMap::new();
        for entity in &data.entities {
            let location = format!("{document} 实体 {}", entity.id);
            for component in &entity.components {
                match component.component {
                    Parent::ID => {
                        if let Some(Value::Entity(parent)) = field(component, "entity") {
                            let parent = EntityId::from_raw(*parent);
                            if ids.contains(&parent) {
                                parents.insert(entity.id, parent);
                            } else {
                                self.problem(
                                    &format!("{kind}.parent_missing"),
                                    &location,
                                    &format!("父实体 {parent} 不在当前文件"),
                                );
                            }
                        }
                    }
                    PrefabInstance::ID => self.prefab_source(&location, component),
                    PluginComponent::ID => self.report.hints.push(format!(
                        "[plugin.inactive] {location} 的 Rust 插件组件当前不会执行"
                    )),
                    _ => self.asset_field(&location, component),
                }
            }
        }
        self.parent_cycles(document, kind, parents);
    }

    fn parent_cycles(
        &mut self,
        document: &str,
        kind: &str,
        mut parents: BTreeMap<EntityId, EntityId>,
    ) {
        // 每条边只从待检查集合移出一次，长父链也不用递归。
        while let Some((&start, _)) = parents.first_key_value() {
            let mut visiting = BTreeSet::new();
            let mut cursor = start;
            while let Some(parent) = parents.remove(&cursor) {
                visiting.insert(cursor);
                cursor = parent;
            }
            if visiting.contains(&cursor) {
                self.problem(
                    &format!("{kind}.parent_cycle"),
                    document,
                    &format!("实体 {cursor} 的父链成环"),
                );
            }
        }
    }

    fn prefab_source(&mut self, location: &str, component: &ComponentData) {
        let reference = field(component, "source")
            .and_then(Value::as_string)
            .unwrap_or_default();
        let path = match self.resolve(AssetKind::Prefab, reference) {
            Ok(path) => path,
            Err(err) => {
                self.problem("prefab.source", location, &format!("{reference}：{err}"));
                return;
            }
        };
        let id = match field(component, "source_id") {
            Some(Value::Entity(id)) => EntityId::from_raw(*id),
            _ => EntityId::from_raw(0),
        };
        if self.prefab(&path).is_some_and(|ids| !ids.contains(&id)) {
            self.problem(
                "prefab.source_id",
                location,
                &format!("{reference} 没有源实体 {id}"),
            );
        }
    }

    fn asset_field(&mut self, location: &str, component: &ComponentData) {
        if let Some(schema) = self.registry.get(component.component) {
            schema.visit_asset_references(&component.fields, |kind, name, reference| {
                self.asset_reference(location, name, reference, kind);
            });
        }
    }

    fn asset_reference(&mut self, location: &str, name: &str, reference: &str, kind: AssetKind) {
        if reference.is_empty()
            || (kind == AssetKind::Model
                && reference.starts_with("builtin://")
                && xgc_render::builtin_mesh(reference).is_some())
        {
            return;
        }
        let script = kind == AssetKind::Script;
        match self.resolve(kind, reference) {
            Ok(path) if script => {
                self.scripts.insert(path);
            }
            Ok(_) => {}
            Err(err) => {
                let code = if script {
                    "script.missing"
                } else {
                    "asset.missing"
                };
                self.problem(code, location, &format!("{name} {reference}：{err}"));
            }
        }
    }

    fn check_scripts(&mut self) {
        self.report.scripts = self.scripts.len();
        for path in &self.scripts {
            let name = path
                .strip_prefix(&self.root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            match fs::read_to_string(path) {
                Ok(source) => {
                    if let Err(diagnostic) = xgc_script::check_source(&name, &source) {
                        self.report.problems.push(diagnostic.to_string());
                    }
                }
                Err(err) => self
                    .report
                    .problems
                    .push(format!("[script.read] {name}：{err}")),
            }
        }
    }

    fn input(&mut self, reference: &str, path: Result<Option<PathBuf>, StoreError>) {
        match path.and_then(|path| Ok(path.map(fs::read_to_string).transpose()?)) {
            Ok(Some(text)) => {
                if let Err(err) = InputMap::parse_ron(&text) {
                    self.problem("input.parse", reference, &err);
                }
            }
            Ok(None) => {}
            Err(err) => self.problem("input.read", reference, &err.to_string()),
        }
    }
}

fn field<'a>(component: &'a ComponentData, name: &str) -> Option<&'a Value> {
    component
        .fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

fn checked_file(root: &Path, path: &Path) -> io::Result<PathBuf> {
    let path = path.canonicalize()?;
    in_project(root, &path)?;
    if !path.is_file() {
        return Err(io::Error::new(ErrorKind::InvalidInput, "不是普通文件"));
    }
    Ok(path)
}

fn in_project(root: &Path, path: &Path) -> io::Result<()> {
    if !path.starts_with(root) {
        return Err(io::Error::new(
            ErrorKind::PermissionDenied,
            "路径越出项目根目录",
        ));
    }
    Ok(())
}
