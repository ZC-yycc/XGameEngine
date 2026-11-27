//! Apply 的项目提交边界。只有当前草稿接收新增来源关联，场景磁盘只同步已删关联。

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use xgc_asset::{resolve_read_path, AssetCatalog, SearchPaths};
use xgc_base::EntityId;
use xgc_store::project::PrefabSceneChange;
use xgc_store::{
    load_prefab, load_prefab_with_bytes, load_world_with_bytes, save_prefab_if_unchanged,
    ProjectSession, StoreError,
};
use xgc_world::{
    apply_prefab_values, detach_prefab_sources, plan_prefab_apply, Command, ComponentData, World,
    WorldData,
};

use crate::{AssetKind, EditorState, SceneFile};

impl EditorState {
    /// 从选中实例写回来源资产。项目提交可新增一个叶节点或删除完整来源分支。
    pub fn apply_selected_prefab(&mut self) -> Result<usize, String> {
        if self.is_playing() {
            return Err("Play 期间不能 Apply 预制体".into());
        }
        let file = self.file.as_ref().ok_or("请先保存场景再 Apply 预制体")?;
        let (selected, path) = self.selected_prefab_path()?;
        if let (Some(project), Some(scene)) = (&self.project, &self.scene) {
            let mut project =
                ProjectSession::open(project.root()).map_err(|err| err.to_string())?;
            project
                .recover_prefab_apply()
                .map_err(|err| err.to_string())?;
            let (asset, original) = load_prefab_with_bytes(&path).map_err(|err| err.to_string())?;
            let (saved, bytes) =
                load_world_with_bytes(&file.path).map_err(|err| err.to_string())?;
            check_scene_baseline(file, &bytes)?;
            let plan = plan_prefab_apply(&self.world, &saved, &asset, selected)?;
            let removed = plan.removed_sources().clone();
            let minimum = plan.source_id_minimum();
            let mut sources = if removed.is_empty() {
                None
            } else {
                Some(Sources::new(project.root(), &path)?)
            };
            let mut scenes = vec![SceneEdit::new(
                scene.clone(),
                file.path.clone(),
                bytes,
                saved,
                sources.as_mut(),
                &removed,
            )?];
            if minimum.is_some() || !removed.is_empty() {
                for other in &project.manifest().scenes {
                    if other == scene {
                        continue;
                    }
                    let path = project
                        .canonical_scene_path(other)
                        .map_err(|err| err.to_string())?;
                    let (saved, bytes) =
                        load_world_with_bytes(&path).map_err(|err| err.to_string())?;
                    scenes.push(SceneEdit::new(
                        other.clone(),
                        path,
                        bytes,
                        saved,
                        sources.as_mut(),
                        &removed,
                    )?);
                }
            }

            let source_id = if let Some(minimum) = minimum {
                // 来源 ID 归资产所有。CAS 失败也不能把已发行的号还给项目。
                let ids = project
                    .reserve_entity_ids(1, minimum)
                    .map_err(|err| format!("新增预制体叶节点领号失败：{err}"))?;
                Some(EntityId::from_raw(ids.start))
            } else {
                None
            };
            let (candidate, binding) = plan.finish(source_id)?;
            let mut updated_world = None;
            let mut new_undo = None;
            let mut detached_history = None;
            if let Some((entity, binding)) = binding {
                let before = self.world.world_data();
                let mut world = World::new();
                world.restore_world_data(&before)?;
                let bind = world.plan(vec![Command::AddComponent {
                    entity,
                    component: ComponentData::from_component(&binding),
                }]);
                world
                    .apply(&bind)
                    .map_err(|err| format!("无法添加预制体来源关联：{err:?}"))?;
                updated_world = Some(world);
                new_undo = Some(before);
            } else if let Some(sources) = &mut sources {
                let mut draft = self.world.world_data();
                if sources.detach(&mut draft, &removed)? {
                    let mut world = World::new();
                    world.restore_world_data(&draft)?;
                    updated_world = Some(world);
                }
                let mut undo = self.undo.clone();
                let mut redo = self.redo.clone();
                for state in undo.iter_mut().chain(&mut redo) {
                    // 撤销能还原旧实体，其来源关联已经随资产删除。
                    sources.detach(state, &removed)?;
                }
                detached_history = Some((undo, redo));
            }

            let changes: Vec<_> = scenes.iter().map(SceneEdit::change).collect();
            let bytes = project
                .apply_prefab_if_unchanged(&path, &candidate, &original, &changes, || {
                    if removed.is_empty() {
                        return Ok(());
                    }
                    Sources::new(project.root(), &path)
                        .and_then(|mut sources| sources.check_dependencies(&removed))
                        .map_err(StoreError::Format)
                })
                .map_err(|err| format!("写回预制体失败：{err}"))?;
            self.file.as_mut().expect("活动场景已经保存").bytes =
                bytes.into_iter().next().expect("活动场景排在首位");
            if let Some(world) = updated_world {
                self.world = world;
            }
            if let Some(before) = new_undo {
                self.record_undo(before);
                self.dirty = true;
            }
            if let Some((undo, redo)) = detached_history {
                self.undo = undo;
                self.redo = redo;
            }
            Ok(candidate.entities.len())
        } else {
            let (asset, expected) =
                load_prefab_with_bytes(&path).map_err(|err| format!("读取预制体失败：{err}"))?;
            let candidate = apply_prefab_values(&self.world, &asset, selected)?;
            let bytes =
                std::fs::read(&file.path).map_err(|err| format!("读取当前场景失败：{err}"))?;
            check_scene_baseline(file, &bytes)?;
            save_prefab_if_unchanged(&path, &candidate, &expected)
                .map_err(|err| format!("写回预制体失败：{err}"))?;
            Ok(candidate.entities.len())
        }
    }
}

fn check_scene_baseline(file: &SceneFile, bytes: &[u8]) -> Result<(), String> {
    if bytes != file.bytes {
        return Err("当前场景已被外部修改；请重新打开后再 Apply".into());
    }
    Ok(())
}

struct SceneEdit {
    scene: String,
    path: PathBuf,
    original: Vec<u8>,
    replacement: Option<WorldData>,
}

impl SceneEdit {
    fn new(
        scene: String,
        path: PathBuf,
        original: Vec<u8>,
        mut data: WorldData,
        sources: Option<&mut Sources<'_>>,
        removed: &BTreeSet<EntityId>,
    ) -> Result<Self, String> {
        let replacement = match sources {
            Some(sources) => sources.detach(&mut data, removed)?.then_some(data),
            None => None,
        };
        Ok(Self {
            scene,
            path,
            original,
            replacement,
        })
    }

    fn change(&self) -> PrefabSceneChange<'_> {
        PrefabSceneChange {
            scene: &self.scene,
            path: &self.path,
            original: &self.original,
            replacement: self.replacement.as_ref(),
        }
    }
}

struct Sources<'a> {
    root: PathBuf,
    path: &'a Path,
    catalog: AssetCatalog,
    matches: HashMap<String, bool>,
}

impl<'a> Sources<'a> {
    fn new(root: &Path, path: &'a Path) -> Result<Self, String> {
        let root = root.canonicalize().map_err(|err| err.to_string())?;
        let catalog = xgc_asset::scan(&root).map_err(|err| err.to_string())?;
        Ok(Self {
            root,
            path,
            catalog,
            matches: HashMap::new(),
        })
    }

    fn detach(
        &mut self,
        data: &mut WorldData,
        removed: &BTreeSet<EntityId>,
    ) -> Result<bool, String> {
        detach_prefab_sources(data, removed, |source| {
            if let Some(&matches) = self.matches.get(source) {
                return Ok(matches);
            }
            let matches = resolve_read_path(
                &self.root,
                source,
                &SearchPaths::default(),
                Some(&self.catalog),
            )
            .is_ok_and(|path| path.canonicalize().is_ok_and(|path| path == self.path));
            self.matches.insert(source.to_owned(), matches);
            Ok(matches)
        })
    }

    fn check_dependencies(&mut self, removed: &BTreeSet<EntityId>) -> Result<(), String> {
        let prefabs = xgc_asset::find_files(&self.root, |path| {
            path.to_str()
                .is_some_and(|path| AssetKind::from_path(path) == AssetKind::Prefab)
        })
        .map_err(|err| format!("读取项目预制体失败：{err}"))?;
        for path in prefabs {
            if path == self.path {
                continue;
            }
            let mut prefab = load_prefab(&path)
                .map_err(|err| format!("读取预制体 {} 失败：{err}", path.display()))?;
            if self.detach(&mut prefab, removed)? {
                return Err(format!("预制体 {} 仍引用将删除的来源实体", path.display()));
            }
        }
        Ok(())
    }
}
