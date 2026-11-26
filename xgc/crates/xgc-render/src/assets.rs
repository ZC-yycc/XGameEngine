//! 网格和材质缓存。项目文件统一经资产目录解析，只有模型读取使用搜索根兜底。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use xgc_asset::{AssetCatalog, AssetPaths, ReadPathError, SearchPaths};
use xgc_world::{Animator, RootMotion, RootMotionLibrary, World};

use super::glb::{load_model, MeshError};
use super::material::MaterialAsset;
use super::mesh::{builtin_mesh, Mesh};

pub struct MeshCache {
    assets: AssetPaths,
    meshes: HashMap<String, Mesh>,
    materials: HashMap<String, MaterialAsset>,
}

impl MeshCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_assets(AssetPaths::new(root, SearchPaths::default(), None))
    }

    pub fn with_catalog(root: impl Into<PathBuf>, catalog: AssetCatalog) -> Self {
        Self::with_assets(AssetPaths::new(root, SearchPaths::default(), Some(catalog)))
    }

    pub fn with_assets(assets: AssetPaths) -> Self {
        Self {
            assets,
            meshes: HashMap::new(),
            materials: HashMap::new(),
        }
    }

    pub fn root(&self) -> &Path {
        self.assets.root()
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
        let path = match self.assets.resolve(model) {
            Ok(path) => path,
            Err(ReadPathError::Missing { .. }) => return Ok(None),
            Err(err) => return Err(mesh_path_error(err)),
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
            let path = match self.assets.resolve_direct(reference) {
                Ok(path) => path,
                Err(err) => match mesh_path_error(err) {
                    // 悬空引用由体检点名，渲染仍保留模型材质。
                    MeshError::Missing(_) => return Ok(None),
                    err => return Err(err),
                },
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

    /// 按世界里的 `Animator.root_motion` 配置收集各模型的基础剪辑平移轨道。
    ///
    /// 只扫描显式写了根运动的实体：根运动是配置项，不缺省对所有模型扫描
    /// （那会把每次启动都拖成整包 GLTF 解析）。三个宿主（CLI / editor / player）
    /// 共用这一份，避免某个宿主漏接而让同一份世界在不同宿主里跑出两种位置。
    pub fn root_motion_library(&mut self, world: &World) -> RootMotionLibrary {
        let mut models = std::collections::BTreeSet::new();
        world.for_each::<Animator>(|key, animator| {
            if RootMotion::parse(&animator.root_motion).is_some() {
                if let Some(model) = world.primary_model(key) {
                    models.insert(model);
                }
            }
        });
        let mut library = RootMotionLibrary::new();
        for model in models {
            let Ok(Some(mesh)) = self.get(model) else {
                continue;
            };
            let Some(animations) = mesh.animations.as_ref() else {
                continue;
            };
            let set = animations.root_motion_set();
            if !set.is_empty() {
                library.insert(model.to_owned(), set);
            }
        }
        library
    }
}

fn mesh_path_error(err: ReadPathError) -> MeshError {
    match err {
        ReadPathError::Invalid(_) | ReadPathError::GuidRequiresCatalog => {
            MeshError::Unsupported(err.to_string())
        }
        err => MeshError::Missing(err.to_string()),
    }
}
