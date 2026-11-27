//! 场景与预制体共用实体投影。原文件内 ID 只在规划期间用于重写引用。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use xgc_base::EntityId;
use xgc_world::{remap_parent, ComponentData, PrefabInstance, WorldData};

use super::files::{copy_file, path_key};
use super::{normalize, ProjectMigrationError};
use crate::old_scene::{OldPrefabRef, OldScene};
use crate::project_check::refs::{prefab_files, resolve_prefab_path};

#[derive(Serialize)]
pub(super) struct WorldPlan {
    pub source: String,
    pub target: String,
    pub ids: BTreeMap<EntityId, EntityId>,
    #[serde(skip)]
    pub world: WorldData,
    #[serde(skip)]
    pub unmatched: Vec<String>,
    #[serde(skip)]
    references: Vec<(usize, OldPrefabRef)>,
}

impl WorldPlan {
    pub(super) fn read(
        root: &Path,
        source: &str,
        target: String,
    ) -> Result<Self, ProjectMigrationError> {
        let error = |message| ProjectMigrationError::Document {
            source: source.to_string(),
            message,
        };
        let text = fs::read_to_string(root.join(source)).map_err(|err| error(err.to_string()))?;
        let scene: OldScene = ron::from_str(&text).map_err(|err| error(err.to_string()))?;
        let (world, _) = crate::migrate_parsed(&scene).map_err(|err| error(err.to_string()))?;
        let ids = world
            .entities
            .iter()
            .map(|entity| (entity.id, entity.id))
            .collect();
        let unmatched = unmatched_fields(source, &scene);
        let references = scene
            .entities
            .into_iter()
            .enumerate()
            .filter_map(|(index, entity)| entity.prefab.map(|reference| (index, reference)))
            .collect();
        Ok(Self {
            source: source.to_string(),
            target,
            ids,
            world,
            unmatched,
            references,
        })
    }

    pub(super) fn assign_ids(&mut self, next: &mut u64) {
        for mapped in self.ids.values_mut() {
            *mapped = EntityId::from_raw(*next);
            *next += 1;
        }
        for entity in &mut self.world.entities {
            entity.id = self.ids[&entity.id];
            remap_parent(entity, &self.ids);
        }
    }

    pub(super) fn save(
        &self,
        staging: &Path,
        save: impl FnOnce(&Path, &WorldData) -> Result<(), xgc_store::StoreError>,
    ) -> Result<(), ProjectMigrationError> {
        let path = staging.join(&self.target);
        save(&path, &self.world).map_err(|err| ProjectMigrationError::Write {
            path,
            message: err.to_string(),
        })
    }
}

fn unmatched_fields(source: &str, scene: &OldScene) -> Vec<String> {
    use crate::OldComponentData;

    let mut notes = Vec::new();
    for entity in &scene.entities {
        let id = entity.id.0;
        if entity.layer != 0 {
            notes.push(format!(
                "[migration.layer] {source} 实体 {id} 的 layer={} 需要重设碰撞过滤",
                entity.layer
            ));
        }
        if source.ends_with(".prefab.ron") && entity.prefab.is_some() {
            notes.push(format!(
                "[migration.nested_prefab] {source} 实体 {id} 保留展开内容，来源链接需要重建"
            ));
        }
        for entry in &entity.components.entries {
            match &entry.data {
                OldComponentData::Mesh(mesh) if !mesh.morph_weights.is_empty() => {
                    notes.push(format!(
                        "[migration.morph] {source} 实体 {id} 的 morph_weights 需要重设"
                    ));
                }
                OldComponentData::Rust {
                    registered_name, ..
                } => {
                    notes.push(format!(
                        "[plugin.unsupported] {source} 实体 {id} 的 {registered_name} 仅保留字段，需移植运行逻辑"
                    ));
                }
                _ => {}
            }
        }
    }
    notes
}

pub(super) struct PrefabPlans {
    pub documents: BTreeMap<PathBuf, PrefabPlan>,
    references: BTreeMap<String, PathBuf>,
}

pub(super) struct PrefabPlan {
    pub document: WorldPlan,
    identity: String,
    meta: Option<(String, String)>,
}

impl PrefabPlans {
    pub(super) fn read(
        root: &Path,
        directory: &str,
        scenes: &[WorldPlan],
    ) -> Result<Self, ProjectMigrationError> {
        let root = canonical(root)?;
        let catalog = xgc_asset::scan(&root).map_err(|err| ProjectMigrationError::Io {
            path: root.clone(),
            message: err.to_string(),
        })?;
        let directory = root.join(directory);
        let paths = if directory.is_dir() {
            prefab_files(&directory).map_err(|err| ProjectMigrationError::Io {
                path: directory,
                message: err.to_string(),
            })?
        } else {
            Vec::new()
        };
        let mut sources = paths
            .into_iter()
            .map(|path| canonical(&path))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let mut references = BTreeMap::new();
        for (scene, reference) in scenes.iter().flat_map(|scene| {
            scene
                .references
                .iter()
                .map(move |(_, reference)| (scene, reference))
        }) {
            if !references.contains_key(&reference.source) {
                let path =
                    resolve_prefab_path(&root, &reference.source, &catalog).map_err(|message| {
                        ProjectMigrationError::Document {
                            source: scene.source.clone(),
                            message,
                        }
                    })?;
                let path = canonical(&path)?;
                sources.insert(path.clone());
                references.insert(reference.source.clone(), path);
            }
        }
        let mut documents = BTreeMap::new();
        for path in sources {
            let source = path
                .strip_prefix(&root)
                .ok()
                .and_then(Path::to_str)
                .ok_or_else(|| {
                    ProjectMigrationError::Manifest(format!(
                        "预制体路径超出项目或无法编码：{}",
                        path.display()
                    ))
                })?;
            let source = normalize(source)?;
            let stem = source.strip_suffix(".prefab.ron").ok_or_else(|| {
                ProjectMigrationError::Manifest(format!(
                    "预制体文件须以 .prefab.ron 结尾：{source}"
                ))
            })?;
            let target = format!("{stem}.prefab.json");
            let identity = catalog
                .guid_for_path(&path)
                .map_or_else(|| target.clone(), |guid| format!("guid://{guid}"));
            let source_meta = format!("{source}.meta");
            let meta = root
                .join(&source_meta)
                .is_file()
                .then(|| (source_meta, format!("{target}.meta")));
            let document = WorldPlan::read(&root, &source, target)?;
            documents.insert(
                path,
                PrefabPlan {
                    document,
                    identity,
                    meta,
                },
            );
        }
        Ok(Self {
            documents,
            references,
        })
    }

    pub(super) fn remap_instances(
        &self,
        scenes: &mut [WorldPlan],
    ) -> Result<(), ProjectMigrationError> {
        for scene in scenes {
            for (index, reference) in &scene.references {
                let prefab = &self.documents[&self.references[&reference.source]];
                let source_id = prefab
                    .document
                    .ids
                    .get(&EntityId::from_raw(reference.source_id.0))
                    .ok_or_else(|| ProjectMigrationError::Document {
                        source: scene.source.clone(),
                        message: format!(
                            "预制体 {} 里没有 source_id {}",
                            prefab.document.source, reference.source_id.0
                        ),
                    })?;
                let instance = PrefabInstance {
                    source: prefab.identity.clone(),
                    source_id: source_id.raw(),
                    root: reference.is_root,
                };
                scene.world.entities[*index]
                    .components
                    .push(ComponentData::from_component(&instance));
            }
        }
        Ok(())
    }

    pub(super) fn replaced_paths(&self) -> BTreeSet<PathBuf> {
        self.documents
            .values()
            .flat_map(|prefab| {
                std::iter::once(prefab.document.source.as_str())
                    .chain(prefab.meta.iter().map(|(source, _)| source.as_str()))
            })
            .map(|path| path_key(Path::new(path)))
            .collect()
    }

    pub(super) fn targets(&self) -> impl Iterator<Item = &str> {
        self.documents.values().flat_map(|prefab| {
            std::iter::once(prefab.document.target.as_str())
                .chain(prefab.meta.iter().map(|(_, target)| target.as_str()))
        })
    }

    pub(super) fn meta_count(&self) -> usize {
        self.documents
            .values()
            .filter(|prefab| prefab.meta.is_some())
            .count()
    }

    pub(super) fn write(&self, root: &Path, staging: &Path) -> Result<(), ProjectMigrationError> {
        for prefab in self.documents.values() {
            prefab.document.save(staging, xgc_store::save_prefab)?;
            if let Some((source, target)) = &prefab.meta {
                copy_file(&root.join(source), &staging.join(target))?;
            }
        }
        Ok(())
    }
}

pub(super) fn canonical(path: &Path) -> Result<PathBuf, ProjectMigrationError> {
    fs::canonicalize(path).map_err(|err| ProjectMigrationError::Io {
        path: path.to_path_buf(),
        message: err.to_string(),
    })
}
