//! 网格缓存 + 模型路径解析：`render.mesh.model` 可以是 `builtin://…` 或
//! 相对 asset root 的 `.glb`。GUID 解析属于资产目录，留给后续 `xgc-asset`。
//!
//! 材质资产（`.mat`）也在这里按引用缓存：模型与材质的解析规则（`guid://` /
//! 相对路径 / 拒绝越界）完全相同，分两套只会漂移。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use xgc_asset::AssetCatalog;

use super::glb::{load_model, MeshError};
use super::material::MaterialAsset;
use super::mesh::{builtin_mesh, Mesh};

pub struct MeshCache {
    root: PathBuf,
    catalog: Option<AssetCatalog>,
    meshes: HashMap<String, Mesh>,
    materials: HashMap<String, MaterialAsset>,
}

impl MeshCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            catalog: None,
            meshes: HashMap::new(),
            materials: HashMap::new(),
        }
    }

    pub fn with_catalog(root: impl Into<PathBuf>, catalog: AssetCatalog) -> Self {
        Self {
            root: root.into(),
            catalog: Some(catalog),
            meshes: HashMap::new(),
            materials: HashMap::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn len(&self) -> usize {
        self.meshes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty()
    }

    /// 已缓存则直接返回；builtin 走占位；相对路径按 `.glb` 导入；不存在返回 `None`。
    pub fn get(&mut self, model: &str) -> Result<Option<&Mesh>, MeshError> {
        if self.meshes.contains_key(model) {
            return Ok(self.meshes.get(model));
        }
        if model.starts_with("builtin://") {
            let Some(mesh) = builtin_mesh(model) else {
                return Ok(None);
            };
            self.meshes.insert(model.to_string(), mesh);
            return Ok(self.meshes.get(model));
        }
        let Some(path) = self.resolve_reference(model, "模型路径")? else {
            return Ok(None);
        };
        let mesh = load_model(&path)?;
        self.meshes.insert(model.to_string(), mesh);
        Ok(self.meshes.get(model))
    }

    /// 按引用加载 `.mat`；解析不了（文件不存在 / 不是合法 RON）返回 `None`。
    ///
    /// 与旧引擎一致：材质资产只是**加一层乘子**，读不到就退回模型材质，
    /// 不让整个实体从画面消失（`--check` 才负责把悬空引用点名）。
    pub fn material(&mut self, reference: &str) -> Result<Option<&MaterialAsset>, MeshError> {
        if !self.materials.contains_key(reference) {
            let path = match self.resolve_reference(reference, "材质路径") {
                Ok(path) => path,
                // 悬空引用：渲染退回模型材质（`--check` 负责把坏引用点名），不让实体消失。
                Err(MeshError::Missing(_)) => None,
                Err(err) => return Err(err),
            };
            let Some(path) = path else {
                return Ok(None);
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                return Ok(None);
            };
            let Ok(asset) = MaterialAsset::from_ron(&text) else {
                return Ok(None);
            };
            self.materials
                .insert(reference.to_string(), asset.sanitized());
        }
        Ok(self.materials.get(reference))
    }

    /// `guid://` 或相对 asset root 的路径；`subject` 只用于错误文案。
    fn resolve_reference(
        &self,
        reference: &str,
        subject: &str,
    ) -> Result<Option<PathBuf>, MeshError> {
        if reference.starts_with("guid://") {
            let Some(catalog) = &self.catalog else {
                return Err(MeshError::Unsupported("guid:// 需要资产目录".to_string()));
            };
            let path = catalog
                .resolve(reference)
                .ok_or_else(|| MeshError::Missing(format!("资产目录没有 {reference}")))?;
            return Ok(Some(path.to_path_buf()));
        }
        if reference.split(['/', '\\']).any(|part| part == "..") {
            return Err(MeshError::Unsupported(format!(
                "{subject}不能越出 asset root"
            )));
        }
        let path = self.root.join(reference);
        Ok(path.exists().then_some(path))
    }
}
