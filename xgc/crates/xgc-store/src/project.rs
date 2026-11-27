//! 项目清单 `project.xgc`：迁移产物与三个宿主的共同入口。
//!
//! 这里只放**项目级**信息：入口场景、关卡顺序、内容目录、输入映射、画面设置和碰撞矩阵。
//! 单个场景仍是 `scenes/*.json` 的 `WorldData`，两者各自版本化；清单不重复场景内容。
//!
//! 路径一律是项目相对路径（正斜杠），读写边界都走
//! `xgc_base::path::normalize_project_relative`，不在这里另写一套。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use xgc_base::layer::CollisionLayers;
use xgc_base::path::normalize_project_relative;
use xgc_base::render_settings::{RenderQuality, RenderSettings};

use crate::{load_world, write_atomic, Result, StoreError};

/// 清单格式版本；新增必填字段必须同时上调。
pub const PROJECT_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectManifest {
    pub format_version: u32,
    pub name: String,
    /// 启动场景；必须出现在 `scenes` 里。
    pub entry_scene: String,
    /// 关卡顺序（下标 0 起），也是构建清单。
    pub scenes: Vec<String>,
    pub content: ProjectContent,
    #[serde(default)]
    pub search_paths: Vec<String>,
    /// 输入映射的项目相对路径；没有输入映射时为 None。
    #[serde(default)]
    pub input: Option<String>,
    #[serde(default, skip_serializing_if = "RenderSettings::is_default")]
    pub render: RenderSettings,
    #[serde(default, skip_serializing_if = "RenderQuality::is_default")]
    pub quality: RenderQuality,
    #[serde(default, skip_serializing_if = "CollisionLayers::is_default")]
    pub layers: CollisionLayers,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectContent {
    pub models: String,
    pub textures: String,
    pub audio: String,
    /// 没有着色器目录时为空。
    #[serde(default)]
    pub shaders: Option<String>,
    pub prefabs: String,
    pub scripts: String,
}

impl ProjectManifest {
    /// 读盘后 / 写盘前的唯一校验点。清单不自洽时宁可拒载，也不让宿主
    /// 带着半份路径去猜。
    pub fn validate(&self) -> Result<()> {
        if self.format_version != PROJECT_FORMAT_VERSION {
            return Err(StoreError::Format(format!(
                "project.xgc 版本 {} 不受支持（当前 {PROJECT_FORMAT_VERSION}）",
                self.format_version
            )));
        }
        if self.name.trim().is_empty() {
            return Err(StoreError::Format("project.xgc 的 name 不能为空".into()));
        }
        let entry = self.normalized("entry_scene", &self.entry_scene)?;
        if self.scenes.is_empty() {
            return Err(StoreError::Format("project.xgc 的 scenes 不能为空".into()));
        }
        let mut seen = std::collections::BTreeSet::new();
        for scene in &self.scenes {
            let scene = self.normalized("scenes", scene)?;
            if !seen.insert(scene.clone()) {
                return Err(StoreError::Format(format!(
                    "project.xgc 重复登记场景：{scene}"
                )));
            }
        }
        if !seen.contains(&entry) {
            return Err(StoreError::Format(format!(
                "project.xgc 的 entry_scene 不在 scenes 里：{entry}"
            )));
        }
        self.normalized("content.models", &self.content.models)?;
        self.normalized("content.textures", &self.content.textures)?;
        self.normalized("content.audio", &self.content.audio)?;
        self.normalized("content.prefabs", &self.content.prefabs)?;
        self.normalized("content.scripts", &self.content.scripts)?;
        if let Some(shaders) = &self.content.shaders {
            if !shaders.trim().is_empty() {
                self.normalized("content.shaders", shaders)?;
            }
        }
        for search in &self.search_paths {
            self.normalized("search_paths", search)?;
        }
        if let Some(input) = &self.input {
            self.normalized("input", input)?;
        }
        self.layers
            .validate()
            .map_err(|err| StoreError::Format(format!("project.xgc 的 layers 不合法：{err}")))?;
        Ok(())
    }

    fn normalized(&self, field: &str, raw: &str) -> Result<String> {
        normalize_project_relative(raw).map_err(|defect| {
            StoreError::Format(format!("project.xgc 的 {field} 不合法（{raw}）：{defect}"))
        })
    }
}

pub fn save_project(path: &Path, manifest: &ProjectManifest) -> Result<()> {
    manifest.validate()?;
    let text = ron::ser::to_string_pretty(manifest, ron::ser::PrettyConfig::default())
        .map_err(|err| StoreError::Format(format!("project.xgc 序列化失败：{err}")))?;
    write_atomic(path, text.as_bytes())
}

pub fn load_project(path: &Path) -> Result<ProjectManifest> {
    let text = std::fs::read_to_string(path)?;
    let manifest: ProjectManifest = ron::from_str(&text)
        .map_err(|err| StoreError::Format(format!("project.xgc 解析失败：{err}")))?;
    manifest.validate()?;
    Ok(manifest)
}

/// 已打开的项目：清单 + 根目录。三个宿主都从这里取入口场景与内容根，
/// 不再各自拼 `entry_scene` 的路径。
#[derive(Clone, Debug)]
pub struct ProjectSession {
    root: PathBuf,
    manifest: ProjectManifest,
}

impl ProjectSession {
    /// `path` 可以是项目目录，也可以直接是 `project.xgc`。
    pub fn open(path: &Path) -> Result<Self> {
        let manifest_path = if path.is_dir() {
            path.join("project.xgc")
        } else {
            path.to_path_buf()
        };
        let manifest = load_project(&manifest_path)?;
        let root = manifest_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(Self { root, manifest })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> &ProjectManifest {
        &self.manifest
    }

    pub fn entry_scene(&self) -> &str {
        &self.manifest.entry_scene
    }

    /// 文件经规范化和实际路径校验后才交给宿主，确保目录链接不会把输入读到项目之外。
    pub fn input_path(&self) -> Result<Option<PathBuf>> {
        let Some(input) = &self.manifest.input else {
            return Ok(None);
        };
        let relative = self.manifest.normalized("input", input)?;
        let root = self.root.canonicalize()?;
        let path = root.join(relative).canonicalize()?;
        if !path.starts_with(&root) || !path.is_file() {
            return Err(StoreError::Format(format!(
                "输入映射 {input} 必须是项目内的文件"
            )));
        }
        Ok(Some(path))
    }

    /// 场景路径一律由清单的相对路径拼根目录；未登记的场景直接拒绝，
    /// 避免拼出项目外路径。
    pub fn scene_path(&self, scene: &str) -> Result<PathBuf> {
        let scene = normalize_project_relative(scene)
            .map_err(|defect| StoreError::Format(format!("场景路径不合法（{scene}）：{defect}")))?;
        let registered = self.manifest.scenes.iter().any(|known| {
            normalize_project_relative(known)
                .map(|known| known == scene)
                .unwrap_or(false)
        });
        if !registered {
            return Err(StoreError::Format(format!(
                "场景没有登记在 project.xgc 里：{scene}"
            )));
        }
        Ok(self.root.join(scene))
    }

    pub fn load_scene(&self, scene: &str) -> Result<xgc_world::WorldData> {
        load_world(&self.scene_path(scene)?)
    }

    pub fn load_entry(&self) -> Result<xgc_world::WorldData> {
        self.load_scene(&self.manifest.entry_scene)
    }
}
