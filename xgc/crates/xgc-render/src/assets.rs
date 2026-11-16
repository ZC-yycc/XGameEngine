//! 网格缓存 + 模型路径解析：`render.mesh.model` 可以是 `builtin://…` 或
//! 相对 asset root 的 `.glb`。GUID 解析属于资产目录，留给后续 `xgc-asset`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use xgc_asset::AssetCatalog;

use super::glb::{load_glb, MeshError};
use super::mesh::{builtin_mesh, Mesh};

pub struct MeshCache {
    root: PathBuf,
    catalog: Option<AssetCatalog>,
    meshes: HashMap<String, Mesh>,
}

impl MeshCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            catalog: None,
            meshes: HashMap::new(),
        }
    }

    pub fn with_catalog(root: impl Into<PathBuf>, catalog: AssetCatalog) -> Self {
        Self {
            root: root.into(),
            catalog: Some(catalog),
            meshes: HashMap::new(),
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
        if model.starts_with("guid://") {
            let Some(catalog) = &self.catalog else {
                return Err(MeshError::Unsupported("guid:// 需要资产目录".to_string()));
            };
            let path = catalog
                .resolve(model)
                .ok_or_else(|| MeshError::Missing(format!("资产目录没有 {model}")))?;
            let mesh = load_glb(path)?;
            self.meshes.insert(model.to_string(), mesh);
            return Ok(self.meshes.get(model));
        }
        if model.split(['/', '\\']).any(|part| part == "..") {
            return Err(MeshError::Unsupported(
                "模型路径不能越出 asset root".to_string(),
            ));
        }
        let path = self.root.join(model);
        if !path.exists() {
            return Ok(None);
        }
        let mesh = load_glb(&path)?;
        self.meshes.insert(model.to_string(), mesh);
        Ok(self.meshes.get(model))
    }
}
