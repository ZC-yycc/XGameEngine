//! 项目（Project）—— XGameEngine 的工作单元。
//!
//! 从这一版开始，「引擎」与「游戏项目」是两个独立的东西：
//!
//! - **引擎**是源码仓库本身（`crates/` 下的各个 crate）；
//! - **游戏项目**是磁盘上一个自包含的目录，包含项目清单、场景和资源。
//!
//! 编辑器与独立游戏二进制都不再依赖「进程当前工作目录」，而是先打开一个
//! [`ProjectHandle`]，再把项目内的**相对路径**（`Scenes/scene.ron`、
//! `Resources/Box.glb` 这类）解析为绝对路径。
//!
//! # 项目目录布局
//!
//! ```text
//! <项目名>/
//! ├── project.ron          # 项目清单（本模块的 Project）
//! ├── Scenes/              # 场景文件（.ron）
//! ├── Prefabs/             # 预制体资产（*.prefab.ron）
//! └── Resources/           # 模型 / 纹理 / 音频等资产
//! ```
//!
//! # 为什么清单里存的是相对路径
//!
//! 场景文件里的 `model: "Resources/Box.glb"` 是**项目相对路径**。这样整个项目
//! 目录可以随便移动、拷贝、进版本库，不会因为绝对路径而失效。解析工作统一由
//! [`ProjectHandle`] 承担。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{ContentConfig, WindowConfig};
use crate::prefab::{Prefab, PREFAB_EXTENSION};
use crate::scene::Scene;
use crate::transform::Transform;

/// 项目清单文件的固定文件名（位于项目根目录下）。
pub const PROJECT_FILE_NAME: &str = "project.ron";

/// 当前项目格式版本。
///
/// 清单结构发生不兼容变化时递增该值，并在 [`ProjectHandle::open`] 中补迁移逻辑。
pub const PROJECT_FORMAT_VERSION: u32 = 1;

/// 默认入口场景（相对项目根目录）。
pub const DEFAULT_ENTRY_SCENE: &str = "Scenes/scene.ron";

// ---------------------------------------------------------------------------
// 项目清单
// ---------------------------------------------------------------------------

/// 项目清单——游戏项目的元数据，序列化为项目根目录下的 `project.ron`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    /// 清单格式版本，见 [`PROJECT_FORMAT_VERSION`]。
    #[serde(default = "default_format_version")]
    pub format_version: u32,
    /// 项目名（显示用，通常与项目目录名一致）。
    pub name: String,
    /// 启动场景路径，**相对项目根目录**。
    #[serde(default = "default_entry_scene")]
    pub entry_scene: String,
    /// 运行时窗口设置（独立游戏二进制使用）。
    #[serde(default)]
    pub window: WindowConfig,
    /// 内容目录布局。
    #[serde(default)]
    pub content: ContentConfig,
}

fn default_format_version() -> u32 {
    PROJECT_FORMAT_VERSION
}

fn default_entry_scene() -> String {
    DEFAULT_ENTRY_SCENE.to_string()
}

impl Project {
    /// 以给定名称创建一个使用默认布局的清单。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
}

impl Default for Project {
    fn default() -> Self {
        Self {
            format_version: PROJECT_FORMAT_VERSION,
            name: "Untitled".into(),
            entry_scene: DEFAULT_ENTRY_SCENE.into(),
            window: WindowConfig::default(),
            content: ContentConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

/// 项目操作的错误类型。
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("项目目录不存在: {0}")]
    DirectoryNotFound(PathBuf),

    #[error("'{0}' 不是有效的 XGameEngine 项目：未找到 {PROJECT_FILE_NAME}")]
    ManifestMissing(PathBuf),

    #[error("读取项目清单失败 ({path}): {source}")]
    ReadManifest {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("解析项目清单失败 ({path}): {source}")]
    ParseManifest {
        path: PathBuf,
        #[source]
        source: ron::error::SpannedError,
    },

    #[error("写入文件失败 ({path}): {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("创建目录失败 ({path}): {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("项目目录已存在: {0}")]
    AlreadyExists(PathBuf),

    #[error("项目名不能为空，且不能只由非法字符组成")]
    InvalidName,
}

// ---------------------------------------------------------------------------
// 项目句柄
// ---------------------------------------------------------------------------

/// 已打开的项目：项目清单 + 根目录。
///
/// 这是编辑器与游戏二进制访问项目资产的**唯一入口**——所有相对路径都经过
/// [`ProjectHandle::path`] 解析，避免散落各处的 `Path::join`。
#[derive(Debug, Clone)]
pub struct ProjectHandle {
    /// 项目根目录（绝对路径）。
    root: PathBuf,
    /// 项目清单。
    manifest: Project,
}

impl ProjectHandle {
    /// 打开磁盘上已有的项目目录。
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ProjectError> {
        let root = absolute(root.as_ref());

        if !root.is_dir() {
            return Err(ProjectError::DirectoryNotFound(root));
        }

        let manifest_path = root.join(PROJECT_FILE_NAME);
        if !manifest_path.is_file() {
            return Err(ProjectError::ManifestMissing(root));
        }

        let text = std::fs::read_to_string(&manifest_path).map_err(|source| {
            ProjectError::ReadManifest {
                path: manifest_path.clone(),
                source,
            }
        })?;

        let manifest: Project = ron::from_str(&text).map_err(|source| {
            ProjectError::ParseManifest {
                path: manifest_path,
                source,
            }
        })?;

        Ok(Self { root, manifest })
    }

    /// 在 `parent/<name>` 下创建一个新项目：建目录、写清单、写默认场景。
    ///
    /// 目录已存在时返回 [`ProjectError::AlreadyExists`]，**不会**覆盖已有的东西。
    pub fn create(parent: impl AsRef<Path>, name: &str) -> Result<Self, ProjectError> {
        let name = name.trim();
        let dir_name = sanitize_dir_name(name);
        if name.is_empty() || dir_name.is_empty() {
            return Err(ProjectError::InvalidName);
        }

        let root = absolute(parent.as_ref()).join(&dir_name);
        if root.exists() {
            return Err(ProjectError::AlreadyExists(root));
        }

        // 先建句柄，再按清单里声明的目录建结构——目录名以清单为准，
        // 免得以后改了默认布局要在这里跟着改一份。
        let handle = Self {
            root,
            manifest: Project::new(name),
        };

        // 基础目录结构：场景 + 预制体 + 资产
        for sub in [
            handle.manifest.content.scenes_dir.as_str(),
            handle.manifest.content.prefabs_dir.as_str(),
            handle.manifest.content.models_dir.as_str(),
        ] {
            let dir = handle.path(sub);
            std::fs::create_dir_all(&dir).map_err(|source| ProjectError::CreateDir {
                path: dir,
                source,
            })?;
        }

        // 清单 + 默认场景
        handle.save()?;
        handle.write_scene(DEFAULT_ENTRY_SCENE, &default_scene())?;

        Ok(handle)
    }

    /// 项目根目录（绝对路径）。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 项目清单。
    pub fn manifest(&self) -> &Project {
        &self.manifest
    }

    /// 项目清单（可变）。
    pub fn manifest_mut(&mut self) -> &mut Project {
        &mut self.manifest
    }

    /// 项目名。
    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    /// 把项目内的相对路径解析为绝对路径。
    ///
    /// 传入绝对路径时原样返回，方便调用方混用。
    pub fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        let relative = relative.as_ref();
        if relative.is_absolute() {
            relative.to_path_buf()
        } else {
            self.root.join(relative)
        }
    }

    /// 入口场景的绝对路径。
    pub fn entry_scene_path(&self) -> PathBuf {
        self.path(&self.manifest.entry_scene)
    }

    /// 场景目录的绝对路径。
    pub fn scenes_dir(&self) -> PathBuf {
        self.path(&self.manifest.content.scenes_dir)
    }

    /// 资产目录的绝对路径。
    pub fn assets_dir(&self) -> PathBuf {
        self.path(&self.manifest.content.models_dir)
    }

    /// 预制体目录的绝对路径。
    pub fn prefabs_dir(&self) -> PathBuf {
        self.path(&self.manifest.content.prefabs_dir)
    }

    /// 脚本目录的绝对路径。
    pub fn scripts_dir(&self) -> PathBuf {
        self.path(&self.manifest.content.scripts_dir)
    }

    /// 把一个绝对路径转成**项目相对路径**（统一用正斜杠）。
    ///
    /// 不在项目内（或不是 `root` 的后代）时返回 `None`。
    /// 与 [`path`](Self::path) 互为逆操作：资产清单要能反着写回项目里，
    /// 就不能只存"绝对路径"。
    pub fn relative_path(&self, absolute: impl AsRef<Path>) -> Option<String> {
        let relative = absolute.as_ref().strip_prefix(&self.root).ok()?;
        Some(relative.to_string_lossy().replace('\\', "/"))
    }

    /// 列出项目里所有脚本（**项目相对路径**，按字典序）。
    ///
    /// 与 [`list_prefabs`](Self::list_prefabs) 一样允许子目录归类，
    /// 目录不存在时返回空列表 —— 一个还没写脚本的项目完全合法。
    pub fn list_scripts(&self) -> Vec<String> {
        let scan = self.scripts_dir();
        let mut out = Vec::new();
        collect_by_extension(&scan, &self.root, "rhai", &mut out);
        out.sort();
        out
    }

    /// 由预制体名推出它在项目里的**相对路径**（`Prefabs/<安全名>.prefab.ron`）。
    ///
    /// 名字里的路径分隔符与 Windows 非法字符会被剔除；全被过滤光时退回 `Prefab`。
    pub fn prefab_path_for(&self, name: &str) -> String {
        let dir = self
            .manifest
            .content
            .prefabs_dir
            .trim()
            .trim_matches(['/', '\\']);
        let file = format!("{}.{}", sanitize_asset_name(name), PREFAB_EXTENSION);
        if dir.is_empty() {
            file
        } else {
            format!("{dir}/{file}")
        }
    }

    /// 从项目内的相对路径加载预制体。
    pub fn load_prefab(&self, relative: &str) -> Result<Prefab, String> {
        let path = self.path(relative);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("无法读取 '{}': {}", path.display(), e))?;
        Prefab::from_ron(&text).map_err(|e| format!("无法解析 '{}': {e}", path.display()))
    }

    /// 把预制体写回项目内的相对路径。
    pub fn write_prefab(&self, relative: &str, prefab: &Prefab) -> Result<(), ProjectError> {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ProjectError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let text = prefab.to_ron().map_err(|e| ProjectError::Write {
            path: path.clone(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
        })?;
        std::fs::write(&path, text).map_err(|source| ProjectError::Write { path, source })
    }

    /// 列出项目里所有预制体资产（**项目相对路径**，按字典序）。
    ///
    /// 扫描 `content.prefabs_dir` 整棵子树，允许用户按子目录归类
    /// （`Prefabs/Enemies/Goblin.prefab.ron`）。目录不存在时返回空列表而不是报错——
    /// 还没建过预制体的项目是常态。
    pub fn list_prefabs(&self) -> Vec<String> {
        let scan = self.prefabs_dir();
        let mut out = Vec::new();
        collect_prefabs(&scan, &self.root, &mut out);
        out.sort();
        out
    }

    /// 从项目内的相对路径加载场景。
    pub fn load_scene(&self, relative: &str) -> Result<Scene, String> {
        let path = self.path(relative);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("无法读取 '{}': {}", path.display(), e))?;
        ron::from_str(&text).map_err(|e| format!("无法解析 '{}': {:#}", path.display(), e))
    }

    /// 加载清单里指定的入口场景。
    pub fn load_entry_scene(&self) -> Result<Scene, String> {
        self.load_scene(&self.manifest.entry_scene)
    }

    /// 把场景写回项目内的相对路径。
    pub fn write_scene(&self, relative: &str, scene: &Scene) -> Result<(), ProjectError> {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ProjectError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let pretty = ron::ser::PrettyConfig::default();
        let text = ron::ser::to_string_pretty(scene, pretty).map_err(|e| ProjectError::Write {
            path: path.clone(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
        })?;
        std::fs::write(&path, text).map_err(|source| ProjectError::Write { path, source })
    }

    /// 把项目清单写回磁盘。
    pub fn save(&self) -> Result<(), ProjectError> {
        let path = self.root.join(PROJECT_FILE_NAME);
        let pretty = ron::ser::PrettyConfig::default();
        let text =
            ron::ser::to_string_pretty(&self.manifest, pretty).map_err(|e| ProjectError::Write {
                path: path.clone(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
            })?;
        std::fs::write(&path, text).map_err(|source| ProjectError::Write { path, source })
    }
}

// ---------------------------------------------------------------------------
// 默认场景
// ---------------------------------------------------------------------------

/// 新建项目时写入的默认场景：一台主相机 + 一盏方向光。
///
/// 放在 core 而不是编辑器里，是为了让「新建项目」这件事不依赖 UI 层。
pub fn default_scene() -> Scene {
    let mut scene = Scene::new("New Scene");
    scene.entities = vec![crate::scene::Entity::new("Main Camera")
        .with_transform(Transform::from_translation(glam::Vec3::new(0.0, 1.0, -5.0)))];
    // 光源不再是一个名叫 "Directional Light" 的空实体——空实体既没有模型也不
    // 参与光照，摆在那里只会让人以为改它就能改光。现在光走 `Scene::lights`，
    // 由 `Scene::new` 自动带上一盏默认方向光。
    scene
}

// ---------------------------------------------------------------------------
// 路径工具
// ---------------------------------------------------------------------------

/// 把路径变成绝对路径（相对路径按当前工作目录补齐）。
fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        strip_verbatim(path.to_path_buf())
    } else {
        match std::env::current_dir() {
            Ok(cwd) => strip_verbatim(cwd.join(path)),
            Err(_) => path.to_path_buf(),
        }
    }
}

/// 去掉 Windows `\\?\` 扩展长度前缀——它会让路径在 UI 里很难看，
/// 而且与普通路径字符串比较时不相等。非 Windows 上原样返回。
fn strip_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
        path
    }
    #[cfg(not(windows))]
    {
        path
    }
}

/// 把项目名转成安全的目录名。
///
/// 只做最小处理：剔除路径分隔符和 Windows 非法字符，保留中文等 Unicode 字符。
fn sanitize_dir_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect();
    cleaned.trim().trim_end_matches('.').to_string()
}

/// 把资产名收拾成安全的文件名（不含扩展名）。
///
/// 与 [`sanitize_dir_name`] 的唯一区别：这里**不允许返回空串**——
/// `format!("{}.prefab.ron", "")` 会得到 `.prefab.ron`（一个隐藏文件），
/// 既看不见也选不中，所以全被过滤光时退回 `Prefab`。
pub fn sanitize_asset_name(name: &str) -> String {
    let cleaned = sanitize_dir_name(name);
    if cleaned.is_empty() {
        "Prefab".to_string()
    } else {
        cleaned
    }
}

/// 递归收集预制体资产，把相对**项目根**的路径（正斜杠形式）写进 `out`。
///
/// 相对项目根而不是相对扫描目录：这样 `ProjectHandle::path()` 能直接解析回来，
/// 与 `Entity::prefab.source` 里存的形式完全一致。
fn collect_prefabs(scan_dir: &Path, project_root: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(scan_dir) else {
        return; // 目录还不存在 = 一个预制体都没有，不是错误
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_prefabs(&path, project_root, out);
        } else if path
            .file_name()
            .map(|n| n.to_string_lossy().ends_with(PREFAB_EXTENSION))
            .unwrap_or(false)
        {
            if let Ok(relative) = path.strip_prefix(project_root) {
                // 统一成正斜杠：Windows 上 `strip_prefix` 会给反斜杠，混着存会让
                // 同一份资产出现两种写法，UI 里看着像两个不同的文件。
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

/// 递归收集某个扩展名的文件（项目相对路径，正斜杠）。
///
/// 与 `collect_prefabs` 同构，只是把"后缀"参数化了 —— 预制体、脚本、
/// 以后还有音频，各自的目录不同但收集逻辑一模一样。
fn collect_by_extension(scan_dir: &Path, project_root: &Path, extension: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(scan_dir) else {
        return; // 目录还不存在 = 一个都没有，不是错误
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_by_extension(&path, project_root, extension, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some(extension) {
            if let Ok(relative) = path.strip_prefix(project_root) {
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrip() {
        let project = Project::new("Demo");
        let text = ron::ser::to_string_pretty(&project, ron::ser::PrettyConfig::default())
            .expect("serialize");
        let restored: Project = ron::from_str(&text).expect("deserialize");
        assert_eq!(restored.name, "Demo");
        assert_eq!(restored.entry_scene, DEFAULT_ENTRY_SCENE);
        assert_eq!(restored.format_version, PROJECT_FORMAT_VERSION);
    }

    #[test]
    fn create_and_open_roundtrip() {
        let base = std::env::temp_dir().join(format!("xgame_project_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let handle = ProjectHandle::create(&base, "My Game").expect("create");
        assert_eq!(handle.name(), "My Game");
        assert!(handle.root().join(PROJECT_FILE_NAME).is_file());
        assert!(handle.entry_scene_path().is_file());

        let reopened = ProjectHandle::open(handle.root()).expect("open");
        assert_eq!(reopened.manifest().name, "My Game");

        // 重复创建必须失败，而不是覆盖
        assert!(matches!(
            ProjectHandle::create(&base, "My Game"),
            Err(ProjectError::AlreadyExists(_))
        ));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sanitize_strips_path_separators() {
        assert_eq!(sanitize_dir_name("a/b\\c"), "abc");
        assert_eq!(sanitize_dir_name("  我的项目  "), "我的项目");
        assert_eq!(sanitize_dir_name("..."), "");
    }

    #[test]
    fn path_resolves_relative_to_root() {
        let handle = ProjectHandle {
            root: PathBuf::from("/tmp/demo"),
            manifest: Project::new("demo"),
        };
        assert_eq!(
            handle.path("Resources/Box.glb"),
            PathBuf::from("/tmp/demo/Resources/Box.glb")
        );
    }

    /// 仓库里自带的示例项目必须始终可加载。
    ///
    /// 这条测试专门用来抓「文件挪了地方但场景里的路径没跟着改」这类问题——
    /// 它会把每个 `entity.model` 都解析一遍并确认文件真的在。
    /// 脱离仓库单独分发 xgame-core 时找不到示例项目，直接跳过。
    #[test]
    fn bundled_sample_project_loads() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/SampleScene");
        if !root.join(PROJECT_FILE_NAME).is_file() {
            return;
        }

        let handle = ProjectHandle::open(&root).expect("示例项目应当能打开");
        let scene = handle.load_entry_scene().expect("入口场景应当能加载");
        assert!(!scene.entities.is_empty(), "示例场景不应为空");

        for entity in &scene.entities {
            if entity.model.is_empty() {
                continue;
            }
            // 内置图元（`builtin://cube` 之类）由渲染器现场生成，本来就没有文件 ——
            // 只有真的写成磁盘路径的模型才该在这里被查存在性。
            if entity.model.starts_with("builtin://") {
                continue;
            }
            let path = handle.path(&entity.model);
            assert!(
                path.is_file(),
                "实体 '{}' 引用的模型不存在: {}",
                entity.name,
                path.display()
            );
        }

        // 场景里的预制体实例必须真的指向存在的资产——否则打开编辑器一 Apply
        // 就会报"找不到文件"，但那时已经很难回溯是哪一步写错了。
        for entity in &scene.entities {
            let Some(link) = &entity.prefab else {
                continue;
            };
            assert!(
                handle.path(&link.source).is_file(),
                "实体 '{}' 引用的预制体不存在: {}",
                entity.name,
                link.source
            );
        }
    }

    /// 示例项目里的预制体资产必须能加载，且内部层级与引用的模型都成立。
    #[test]
    fn bundled_sample_prefabs_load() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../projects/SampleScene");
        if !root.join(PROJECT_FILE_NAME).is_file() {
            return;
        }
        let handle = ProjectHandle::open(&root).expect("示例项目应当能打开");

        for relative in handle.list_prefabs() {
            let prefab = handle
                .load_prefab(&relative)
                .unwrap_or_else(|e| panic!("预制体 {relative} 加载失败: {e}"));
            assert!(!prefab.is_empty(), "{relative} 不应为空");
            assert_eq!(prefab.roots().len(), 1, "{relative} 应当恰好有一个根");
            for entity in &prefab.entities {
                if entity.model.is_empty() {
                    continue;
                }
                assert!(
                    handle.path(&entity.model).is_file(),
                    "{relative}: 实体 '{}' 引用的模型不存在: {}",
                    entity.name,
                    entity.model
                );
            }
        }
    }

    #[test]
    fn prefab_path_for_sanitizes_names() {
        let handle = ProjectHandle {
            root: PathBuf::from("/tmp/demo"),
            manifest: Project::new("demo"),
        };
        assert_eq!(handle.prefab_path_for("Enemy"), "Prefabs/Enemy.prefab.ron");
        assert_eq!(handle.prefab_path_for("a/b:c"), "Prefabs/abc.prefab.ron");
        // 全被过滤光时不能生成 ".prefab.ron"（隐藏文件）
        assert_eq!(handle.prefab_path_for("   "), "Prefabs/Prefab.prefab.ron");
    }

    #[test]
    fn list_prefabs_finds_nested_assets() {
        let base = std::env::temp_dir().join(format!("xgame_prefab_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let handle = ProjectHandle::create(&base, "prefabs").expect("create");
        // 默认应当建出 Prefabs 目录，且里面还没有资产
        assert!(handle.prefabs_dir().is_dir(), "新建项目应当带 Prefabs 目录");
        assert!(handle.list_prefabs().is_empty());

        // 放一个（含子目录的）预制体进去
        let mut prefab = Prefab::new("Goblin");
        prefab.entities.push(crate::scene::Entity::new("Goblin"));
        handle
            .write_prefab("Prefabs/Enemies/Goblin.prefab.ron", &prefab)
            .expect("write");

        let listed = handle.list_prefabs();
        assert_eq!(listed, vec!["Prefabs/Enemies/Goblin.prefab.ron".to_string()]);
        let loaded = handle.load_prefab(&listed[0]).expect("load");
        assert_eq!(loaded.name, "Goblin");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 老项目的 `project.ron` 里没有 `prefabs_dir`，反序列化时必须补默认值，
    /// 而不是直接报错——否则升级引擎后老项目一个都打不开。
    #[test]
    fn old_manifest_without_prefabs_dir_defaults() {
        let ron = r#"(
            format_version: 1,
            name: "Old",
            entry_scene: "Scenes/scene.ron",
            window: (title: "Old", width: 800, height: 600, resizable: true, vsync: true),
            content: (
                scenes_dir: "Scenes",
                models_dir: "Resources",
                textures_dir: "Resources",
                audio_dir: "Resources",
                shaders_dir: "Shaders",
                search_paths: [],
            ),
        )"#;
        let project: Project = ron::from_str(ron).expect("老清单应当能反序列化");
        assert_eq!(project.content.prefabs_dir, "Prefabs");
    }
}
