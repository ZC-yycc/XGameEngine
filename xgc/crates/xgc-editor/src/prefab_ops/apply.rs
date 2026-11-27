//! Apply 的项目提交边界。场景草稿和其他实例的覆盖值不随来源删除落盘。

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
    apply_prefab_removals, apply_prefab_values, detach_prefab_sources, World, WorldData,
};

use crate::{AssetKind, EditorState, SceneFile};

impl EditorState {
    /// 从选中实例写回来源资产。项目可删除已保存的来源分支，仍保留场景草稿。
    pub fn apply_selected_prefab(&mut self) -> Result<usize, String> {
        if self.is_playing() {
            return Err("Play 期间不能 Apply 预制体".into());
        }
        let file = self.file.as_ref().ok_or("请先保存场景再 Apply 预制体")?;
        let (selected, path) = self.selected_prefab_path()?;
        if let (Some(project), Some(scene)) = (&self.project, &self.scene) {
            let project = ProjectSession::open(project.root()).map_err(|err| err.to_string())?;
            project
                .recover_prefab_apply()
                .map_err(|err| err.to_string())?;
            let (asset, original) = load_prefab_with_bytes(&path).map_err(|err| err.to_string())?;
            let (saved, bytes) =
                load_world_with_bytes(&file.path).map_err(|err| err.to_string())?;
            check_scene_baseline(file, &bytes)?;
            let (candidate, removed) =
                apply_prefab_removals(&self.world, &saved, &asset, selected)?;
            let count = candidate.entities.len();
            if removed.is_empty() {
                project
                    .apply_prefab_if_unchanged(
                        &path,
                        &candidate,
                        &original,
                        &[PrefabSceneChange {
                            scene,
                            path: &file.path,
                            original: &file.bytes,
                            replacement: None,
                        }],
                        || Ok(()),
                    )
                    .map_err(|err| format!("写回预制体失败：{err}"))?;
            } else {
                let mut sources = Sources::new(project.root(), &path)?;
                let active = SceneEdit::new(
                    scene.clone(),
                    file.path.clone(),
                    bytes,
                    saved,
                    &mut sources,
                    &removed,
                )?;
                let mut scenes = vec![active];
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
                        &mut sources,
                        &removed,
                    )?);
                }

                let mut draft = self.world.world_data();
                let mut undo = self.undo.clone();
                let mut redo = self.redo.clone();
                let mut updated_world = None;
                if sources.detach(&mut draft, &removed)? {
                    let mut world = World::new();
                    world.restore_world_data(&draft)?;
                    updated_world = Some(world);
                }
                for state in undo.iter_mut().chain(&mut redo) {
                    // 撤销能还原已删除的实体，但关联的来源已从项目资产移除。
                    sources.detach(state, &removed)?;
                }

                let changes: Vec<_> = scenes.iter().map(SceneEdit::change).collect();
                let bytes = project
                    .apply_prefab_if_unchanged(&path, &candidate, &original, &changes, || {
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
                self.undo = undo;
                self.redo = redo;
            }
            Ok(count)
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
        sources: &mut Sources<'_>,
        removed: &BTreeSet<EntityId>,
    ) -> Result<Self, String> {
        let changed = sources.detach(&mut data, removed)?;
        Ok(Self {
            scene,
            path,
            original,
            replacement: changed.then_some(data),
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
