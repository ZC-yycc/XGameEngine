//! 编辑器从原生预制体文件实例化，按选中实体的来源还原或写回。

use std::path::{Path, PathBuf};

use xgc_base::{path::normalize_project_relative, EntityId};
use xgc_store::{load_prefab, load_prefab_with_bytes, save_prefab_if_unchanged};
use xgc_world::{
    apply_prefab_values, instantiate_prefab, instantiate_prefab_with_ids, revert_prefab,
    revert_prefab_with_ids, Component, Parent, PrefabInstance,
};

use crate::{AssetKind, EditorState};

impl EditorState {
    pub fn instantiate_prefab_file(&mut self, path: &Path) -> Result<EntityId, String> {
        if self.is_playing() {
            return Err("Play 期间不能实例化预制体".to_string());
        }
        let (path, source) = match self.prefab_project_root()? {
            Some(root) => {
                let input = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    root.join(project_relative(path)?)
                };
                let path = project_prefab_path(&root, &input)?;
                let source = project_source(&root, &path)?;
                (path, source)
            }
            None => {
                let path = canonical_prefab_path(path)?;
                let source = path_text(&path)?.to_string();
                (path, source)
            }
        };
        let data = load_prefab(&path).map_err(|err| format!("读取预制体失败：{err}"))?;
        let root_asset = data
            .entities
            .iter()
            .find(|entity| {
                !entity
                    .components
                    .iter()
                    .any(|component| component.component == Parent::ID)
            })
            .map(|entity| entity.id)
            .ok_or_else(|| "预制体没有根实体".to_string())?;
        let before = self.world.world_data();
        let mapping = match &mut self.project {
            Some(project) => instantiate_prefab_with_ids(
                &mut self.world,
                &data,
                Some(&source),
                |local, count| {
                    project
                        .reserve_entity_ids(count, local)
                        .map_err(|err| err.to_string())
                },
            ),
            None => instantiate_prefab(&mut self.world, &data, Some(&source)),
        }?;
        let root = mapping
            .iter()
            .find_map(|(asset, instance)| (*asset == root_asset).then_some(*instance))
            .ok_or_else(|| "预制体根实体没有新 ID".to_string())?;
        self.record_undo(before);
        self.selected = Some(root);
        self.dirty = true;
        Ok(root)
    }

    pub(crate) fn can_revert_prefab(&self) -> bool {
        !self.is_playing()
            && self
                .selected
                .and_then(|entity| self.prefab_instance(entity))
                .is_some_and(|instance| !instance.source.is_empty())
    }

    pub fn revert_selected_prefab(&mut self) -> Result<usize, String> {
        if self.is_playing() {
            return Err("Play 期间不能 Revert 预制体".to_string());
        }
        let (selected, path) = self.selected_prefab_path()?;
        let data = load_prefab(&path).map_err(|err| format!("读取预制体失败：{err}"))?;
        let before = self.world.world_data();
        let mapping = match &mut self.project {
            Some(project) => {
                revert_prefab_with_ids(&mut self.world, &data, selected, |local, count| {
                    project
                        .reserve_entity_ids(count, local)
                        .map_err(|err| err.to_string())
                })
            }
            None => revert_prefab(&mut self.world, &data, selected),
        }?;
        self.record_undo(before);
        if !mapping.iter().any(|(_, instance)| *instance == selected) {
            self.selected = mapping.iter().find_map(|(_, instance)| {
                self.prefab_instance(*instance)
                    .is_some_and(|meta| meta.root)
                    .then_some(*instance)
            });
        }
        self.dirty = true;
        Ok(mapping.len())
    }

    pub(crate) fn can_apply_prefab(&self) -> bool {
        self.file.is_some() && self.can_revert_prefab()
    }

    /// 把已有实体与组件的值写回来源预制体，场景编辑历史仍由本会话持有。
    pub fn apply_selected_prefab(&self) -> Result<usize, String> {
        if self.is_playing() {
            return Err("Play 期间不能 Apply 预制体".into());
        }
        let file = self.file.as_ref().ok_or("请先保存场景再 Apply 预制体")?;
        let (selected, path) = self.selected_prefab_path()?;
        let (asset, expected) =
            load_prefab_with_bytes(&path).map_err(|err| format!("读取预制体失败：{err}"))?;
        let candidate = apply_prefab_values(&self.world, &asset, selected)?;
        let current =
            std::fs::read(&file.path).map_err(|err| format!("读取当前场景失败：{err}"))?;
        if current != file.bytes {
            return Err("当前场景已被外部修改；请重新打开后再 Apply".into());
        }
        save_prefab_if_unchanged(&path, &candidate, &expected)
            .map_err(|err| format!("写回预制体失败：{err}"))?;
        Ok(candidate.entities.len())
    }

    fn selected_prefab_path(&self) -> Result<(EntityId, PathBuf), String> {
        let selected = self.selected.ok_or_else(|| "没有选中实体".to_string())?;
        let source = &self
            .prefab_instance(selected)
            .ok_or_else(|| "选中的实体没有预制体来源".to_string())?
            .source;
        let path = match self.prefab_project_root()? {
            Some(root) => {
                let input = if source.starts_with("guid://") {
                    xgc_asset::scan(&root)
                        .map_err(|err| err.to_string())?
                        .resolve(source)
                        .ok_or_else(|| format!("找不到预制体来源 {source}"))?
                        .to_path_buf()
                } else {
                    root.join(project_relative(Path::new(source))?)
                };
                project_prefab_path(&root, &input)?
            }
            None => canonical_prefab_path(Path::new(source))?,
        };
        Ok((selected, path))
    }

    fn prefab_instance(&self, entity: EntityId) -> Option<&PrefabInstance> {
        self.world.get(self.world.key(entity)?)
    }

    fn prefab_project_root(&self) -> Result<Option<PathBuf>, String> {
        self.project_root()
            .map(|root| {
                root.join(".")
                    .canonicalize()
                    .map_err(|err| format!("读取项目根目录失败：{err}"))
            })
            .transpose()
    }
}

fn path_text(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("预制体路径无法编码为 UTF-8：{}", path.display()))
}

fn project_relative(path: &Path) -> Result<String, String> {
    let text = path_text(path)?;
    normalize_project_relative(text).map_err(|err| format!("预制体项目相对路径不合法：{err}"))
}

fn canonical_prefab_path(path: &Path) -> Result<PathBuf, String> {
    let path = path
        .canonicalize()
        .map_err(|err| format!("找不到预制体 {}：{err}", path.display()))?;
    if AssetKind::from_path(path_text(&path)?) != AssetKind::Prefab {
        return Err(format!(
            "预制体路径须以 .prefab.json 结尾：{}",
            path.display()
        ));
    }
    Ok(path)
}

fn project_prefab_path(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let path = canonical_prefab_path(path)?;
    if !path.starts_with(root) {
        return Err(format!("预制体必须位于项目内：{}", path.display()));
    }
    Ok(path)
}

fn project_source(root: &Path, path: &Path) -> Result<String, String> {
    let relative_path = path
        .strip_prefix(root)
        .map_err(|_| format!("预制体必须位于项目内：{}", path.display()))?;
    let relative = project_relative(relative_path)?;
    if root.join(&relative) != path {
        return Err(format!("预制体路径无法规范化：{relative_path:?}"));
    }
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(".meta");
    if Path::new(&sidecar)
        .try_exists()
        .map_err(|err| format!("读取预制体元数据失败：{err}"))?
    {
        let catalog = xgc_asset::scan(root).map_err(|err| err.to_string())?;
        let guid = catalog
            .guid_for_path(path)
            .ok_or_else(|| format!("预制体元数据没有可用的 GUID：{relative}"))?;
        Ok(format!("guid://{guid}"))
    } else {
        Ok(relative)
    }
}
