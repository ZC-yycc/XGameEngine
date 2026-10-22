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
//! ├── Resources/           # 模型 / 纹理 / 音频等资产
//! └── .autosave/           # 自动保存副本（崩溃恢复用，见 AUTOSAVE_DIR）
//! ```
//!
//! `.autosave/` 刻意**不在** `Scenes/` 下：`list_scenes` 递归扫 `Scenes/` 下
//! 所有 `.ron`，自动保存副本落在那里就会被当成真场景（Scenes 菜单列出它、
//! `--check` 校验它、`--migrate-assets` 改写它）。放在项目根的点目录里，
//! 上面三条扫描（场景 / 预制体 / 资产）一条都碰不到。
//!
//! # 为什么清单里存的是相对路径，而场景里存的是 GUID
//!
//! 清单里的 `entry_scene` / `content.*` 是**项目相对路径** —— 它们描述的是
//! "项目自己的布局"，跟着项目走，没有重命名的需求。
//!
//! 场景与预制体里对**资产**的引用（模型 / 音频 / 脚本 / 预制体）则存 **GUID**
//! （`guid://…`，见 [`crate::asset`]）：资产会被改名、会被挪目录，只有稳定身份
//! 才能不断链。两个格式之间的转换收在本模块的加载 / 保存边界上 ——
//! **内存里永远是路径**（于是渲染 / 物理 / 音频等所有消费点都不用知道 GUID），
//! 磁盘上永远是 GUID。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::asset::{
    decode_prefab_references, decode_scene_references, encode_prefab_references,
    encode_scene_references, AssetDatabase, AssetGuid, AssetKind, ImportSettings,
};
use crate::config::{ContentConfig, RenderConfig, RuntimeConfig, WindowConfig};
use crate::material::{MaterialAsset, MATERIAL_EXTENSION};
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

/// 自动保存目录（项目根下）。
///
/// **不能改成 `Scenes/` 下的子目录或文件后缀**：`Scenes/` 是递归扫描的，
/// 任何 `.ron` 都会进 `list_scenes`（→ `--check` / Scenes 菜单 / 资产迁移）。
/// 详见模块文档。
pub const AUTOSAVE_DIR: &str = ".autosave";

/// 自动保存副本的项目相对路径：`.autosave/<场景相对路径>`。
///
/// 保留子目录结构（`Scenes/Level2.ron` → `.autosave/Scenes/Level2.ron`），
/// 出问题时能一眼对上是哪个场景，不需要额外的映射表。
pub fn autosave_relative(scene_relative: &str) -> String {
    format!("{AUTOSAVE_DIR}/{scene_relative}")
}

/// 新建项目时写下的 `.gitignore`。
///
/// 三条都是**本引擎产生的东西**：`Build/`（编辑器 Build 按钮）、
/// `Plugins/`（Rust 插件编译产物）、`.autosave/`（崩溃恢复副本）。
/// 让用户第一次 `git add .` 就把它们提交进去，之后每次 Play / 自动保存都是
/// 一次无意义的二进制或整文件 diff —— 这个坑越早堵越好。
pub const DEFAULT_GITIGNORE: &str = "\
# 构建产物（编辑器 Build 按钮）
Build/

# Rust 插件编译产物（编辑器在 Play 时同步到这里）
Plugins/

# 自动保存副本（编辑器崩溃恢复用，属于本机现场，不入库）
.autosave/
";

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
    /// 玩法运行时参数（重力 / 固定步长）。
    ///
    /// 后加的字段：旧清单没有它，`#[serde(default)]` 补默认值 —— 与当年加
    /// `prefabs_dir` / `scripts_dir` 同一套路，不强制升级项目文件。
    #[serde(default)]
    pub runtime: RuntimeConfig,
    /// 画面渲染参数（色调映射 / 曝光）。
    ///
    /// 同样是后加字段、同样靠 `#[serde(default)]` 兼容旧清单。放在清单而不是
    /// 场景里：画面风格是项目的属性（编辑器视口 / 独立游戏 / 无头截图三端一致），
    /// 不是"这张地图"的属性。
    #[serde(default)]
    pub render: RenderConfig,
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
            runtime: RuntimeConfig::default(),
            render: RenderConfig::default(),
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
// 资产引用迁移
// ---------------------------------------------------------------------------

/// 资产引用迁移的结果（编辑器 `--migrate-assets` 与 MCP 共用）。
///
/// 迁移是"老项目一次性升级"：给资产发身份证、把场景 / 预制体里的路径引用
/// 改写成 GUID。它不改变任何运行语义 —— 迁完之后内存里读出来的路径一模一样。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssetMigrationReport {
    /// 本次新生成的 `.meta` 数量。
    pub created_metas: usize,
    /// 重写过的场景文件数。
    pub rewritten_scenes: usize,
    /// 重写过的预制体文件数。
    pub rewritten_prefabs: usize,
    /// 重写过的材质文件数（只改了 `albedo` 贴图引用）。
    pub rewritten_materials: usize,
    /// 单个文件失败的原因（不阻断其余文件）。
    pub errors: Vec<String>,
}

impl AssetMigrationReport {
    /// 有没有失败项。
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// 人和机器都能读的摘要（纯 ASCII）。
    pub fn render(&self) -> String {
        let mut out = String::new();
        use std::fmt::Write as _;
        let _ = writeln!(
            out,
            "migrate: metas={} scenes={} prefabs={} materials={} errors={}",
            self.created_metas,
            self.rewritten_scenes,
            self.rewritten_prefabs,
            self.rewritten_materials,
            self.errors.len()
        );
        for e in &self.errors {
            let _ = writeln!(out, "  error: {e}");
        }
        out
    }
}

// ---------------------------------------------------------------------------
// 项目句柄
// ---------------------------------------------------------------------------

/// 已打开的项目：项目清单 + 根目录 + 资产库。
///
/// 这是编辑器与游戏二进制访问项目资产的**唯一入口**——所有相对路径都经过
/// [`ProjectHandle::path`] 解析，避免散落各处的 `Path::join`。
///
/// 资产库放在 `Arc<Mutex<…>>` 里：`ProjectHandle` 到处被 clone（编辑器把句柄
/// 放进各个状态结构），复制一个句柄不该重新扫一遍盘；同时"刷新资产"必须对
/// 所有持有者同时生效 —— 共享一份索引是唯一不会漂移的做法。
#[derive(Debug, Clone)]
pub struct ProjectHandle {
    /// 项目根目录（绝对路径）。
    root: PathBuf,
    /// 项目清单。
    manifest: Project,
    /// 资产索引 + `.meta` 导入设置（惰性刷新，见 [`ProjectHandle::assets`]）。
    assets: Arc<Mutex<AssetDatabase>>,
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

        // 扫描一次资产库。项目级文件数通常是几百，毫秒级；放在打开项目时做，
        // 换来"每次加载场景都不必扫盘"。
        let assets = Arc::new(Mutex::new(AssetDatabase::scan(&root)));
        Ok(Self {
            root,
            manifest,
            assets,
        })
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
        // 目录此时还不存在（`root.exists()` 已被排除），扫出来就是一个空库；
        // 但仍然要给句柄一个真实 root，否则后续 `refresh` 无从下手。
        let assets = Arc::new(Mutex::new(AssetDatabase::scan(&root)));
        let handle = Self {
            root,
            manifest: Project::new(name),
            assets,
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
        // 新建项目自带 `.gitignore`：`Build/` 与 `Plugins/` 是构建产物，
        // `.autosave/` 是"这台机器这一刻的现场"（见 AUTOSAVE_DIR）——
        // 三者都不该进版本库。写失败不影响项目可用，所以只当尽力而为。
        let _ = std::fs::write(handle.root.join(".gitignore"), DEFAULT_GITIGNORE);
        if let Ok(mut db) = handle.assets.lock() {
            db.refresh();
        }

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

    // -- 资产库 ------------------------------------------------------------

    /// 借用资产库（GUID ↔ 路径 / 导入设置 / 扫描诊断）。
    ///
    /// 返回的 guard 持有锁：**不要**在持有它的同时调用本句柄的
    /// `load_scene` / `write_scene` / `load_prefab` / `write_prefab`
    /// （它们内部也要拿这把锁，会自锁）。需要跨调用时先取出值再放锁。
    pub fn assets(&self) -> MutexGuard<'_, AssetDatabase> {
        // 中毒的锁也照样用：一次 panic 不该让整个项目的资产索引永久不可用。
        self.assets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 重新扫描资产目录（编辑器"刷新资产"、脚本热重载后、外部改动文件后）。
    pub fn refresh_assets(&self) {
        self.assets().refresh();
    }

    /// 为缺 `.meta` 的资产生成身份证（写盘）。
    ///
    /// 编辑器打开项目、保存场景前、资源浏览器导入资产时调用。
    /// `--check` **不**调用它 —— 检查必须只读，否则"检查一遍"会改项目。
    ///
    /// 内部先重新扫盘：调用方通常是"刚把资产拷进项目"，此时索引还是打开项目
    /// 那会儿的，不刷新就会漏掉新文件（那样"补 meta"会静默什么都不做）。
    pub fn ensure_asset_metas(&self) -> Result<usize, String> {
        let mut assets = self.assets();
        assets.refresh();
        assets.ensure_metas()
    }

    /// 取某个资产的导入设置（没有记录时给默认值）。
    pub fn asset_import(&self, relative: &str) -> ImportSettings {
        let assets = self.assets();
        assets
            .record_of_path(relative)
            .map(|r| r.import)
            .unwrap_or_default()
    }

    /// 取某个资产的 GUID（供 UI 显示"身份"用）。
    pub fn asset_guid(&self, relative: &str) -> Option<AssetGuid> {
        self.assets().guid_of(relative).cloned()
    }

    /// 改写某个资产的导入设置（写回 `.meta` 并刷新索引）。
    pub fn set_asset_import(
        &self,
        relative: &str,
        import: ImportSettings,
    ) -> Result<(), String> {
        self.assets().set_import(relative, import)
    }

    /// 列出项目里所有已登记的资产（项目相对路径 + 类型，按路径字典序）。
    ///
    /// 资源浏览器 / MCP `asset_list` 共用 —— 不扫盘、直接读索引。
    pub fn list_assets(&self) -> Vec<(String, AssetKind)> {
        self.assets()
            .sorted_entries()
            .into_iter()
            .map(|(_, record)| (record.path.clone(), record.kind))
            .collect()
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

    /// 列出项目里所有脚本（**项目相对路径**，按字典序）—— 只认 `.lua`
    /// （唯一脚本轨；`.rhai` 已随双轨 P3 删除，见计划文档 §六）。
    ///
    /// 与 [`list_prefabs`](Self::list_prefabs) 一样允许子目录归类，
    /// 目录不存在时返回空列表 —— 一个还没写脚本的项目完全合法。
    pub fn list_scripts(&self) -> Vec<String> {
        let scan = self.scripts_dir();
        let mut out = Vec::new();
        collect_by_extension(&scan, &self.root, "lua", &mut out);
        out.sort();
        out
    }

    /// 列出项目里所有音频文件（**项目相对路径**，按字典序）——
    /// 认 `.wav` / `.ogg` / `.mp3`（kira + symphonia 的主力解码格式）。
    ///
    /// 扫的是 `content.audio_dir` 子树（默认 `Resources`，与模型 / 贴图
    /// 同一套目录约定）。目录不存在时返回空列表 —— 还没做声音的项目完全合法。
    pub fn list_audio(&self) -> Vec<String> {
        let scan = self.path(&self.manifest.content.audio_dir);
        let mut out = Vec::new();
        for ext in ["wav", "ogg", "mp3"] {
            collect_by_extension(&scan, &self.root, ext, &mut out);
        }
        out.sort();
        out
    }

    /// 列出项目里的共享材质（`.mat`，项目相对路径，按字典序）。
    ///
    /// 扫 `content.models_dir` 子树（默认 `Resources/Materials`），允许用户
    /// 按子目录归类。目录不存在时返回空列表 —— 还没做材质的项目完全合法。
    pub fn list_materials(&self) -> Vec<String> {
        let scan = self.assets_dir();
        let mut out = Vec::new();
        collect_by_extension(&scan, &self.root, MATERIAL_EXTENSION, &mut out);
        out.sort();
        out
    }

    /// 由材质名推出它在项目里的**相对路径**（`Resources/Materials/<安全名>.mat`）。
    ///
    /// 材质与模型 / 纹理同住 `Resources`（`models_dir` 是资源根，不是"只放
    /// 模型"的意思 —— 名字是历史遗留）。全被过滤光时退回 `Material`，
    /// 而不是 `Prefab`：文件名要能一眼看出这是什么。
    pub fn material_path_for(&self, name: &str) -> String {
        let dir = self
            .manifest
            .content
            .models_dir
            .trim()
            .trim_matches(['/', '\\']);
        let file = format!(
            "{}.{}",
            sanitize_asset_name_or(name, "Material"),
            MATERIAL_EXTENSION
        );
        if dir.is_empty() {
            format!("Materials/{file}")
        } else {
            format!("{dir}/Materials/{file}")
        }
    }

    /// 从项目内的相对路径（或 `guid://…` 引用）加载一份材质资产。
    ///
    /// 这是材质引用从磁盘 GUID 回到内存路径的**唯一转换点**（与
    /// `load_scene` / `load_prefab` 同一条规则）：`albedo` 也在这里从
    /// GUID 解码成可直接打开的路径。坏数据在返回前 [`MaterialAsset::sanitized`]。
    pub fn load_material(&self, reference: &str) -> Result<MaterialAsset, String> {
        let relative = {
            let assets = self.assets();
            assets
                .resolve_reference(reference)
                .unwrap_or_else(|| reference.to_string())
        };
        let path = self.path(&relative);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("无法读取 '{}': {}", path.display(), e))?;
        let mut material = MaterialAsset::from_ron(&text)
            .map_err(|e| format!("无法解析 '{}': {e}", path.display()))?;
        if !material.albedo.is_empty() {
            let assets = self.assets();
            if let Some(resolved) = assets.resolve_reference(&material.albedo) {
                material.albedo = resolved;
            }
            // 解不开（悬空 GUID）就保留原字面量：渲染会报"贴图打不开"，
            // `--check` 也会把这条引用点名，而不是静默变成白色。
        }
        Ok(material.sanitized())
    }

    /// 把材质资产写回项目内的相对路径（`albedo` 编码成 GUID）。
    ///
    /// 与场景 / 预制体同款的写前刷新：用户可能刚把贴图拷进项目，
    /// 不刷新就会把路径写成字面量而不是稳定身份。
    pub fn write_material(
        &self,
        relative: &str,
        material: &MaterialAsset,
    ) -> Result<(), ProjectError> {
        let encoded = {
            let mut assets = self.assets();
            assets.refresh();
            let _ = assets.ensure_metas();
            if assets.is_empty() {
                None
            } else {
                let mut copy = material.clone();
                if !copy.albedo.is_empty() {
                    copy.albedo = assets.encode_reference(&copy.albedo);
                }
                Some(copy)
            }
        };
        let material = encoded.as_ref().unwrap_or(material);

        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ProjectError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let text = material.to_ron().map_err(|e| ProjectError::Write {
            path: path.clone(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
        })?;
        std::fs::write(&path, text).map_err(|source| ProjectError::Write { path, source })
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
    ///
    /// 磁盘上的资产引用是 GUID，这里解码成项目相对路径再交给上层 ——
    /// 调用方永远只看见能直接打开的路径。
    pub fn load_prefab(&self, relative: &str) -> Result<Prefab, String> {
        let path = self.path(relative);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("无法读取 '{}': {}", path.display(), e))?;
        let mut prefab = Prefab::from_ron(&text)
            .map_err(|e| format!("无法解析 '{}': {e}", path.display()))?;
        {
            let assets = self.assets();
            decode_prefab_references(&mut prefab, &assets);
        }
        // 坏数据（NaN / 越界）在**加载边界**夹一次：手写 RON 里的一个 `nan`
        // 不该让整棵 UI 的矩形变成 NaN 再传进 GPU。运行时 / 渲染侧还有一道
        // 非有限几何的兜底（见 `xgame_ui::layout::visit`），两处职责不同：
        // 这里"修正"，那里"拦截"。
        sanitize_ui(&mut prefab.entities);
        sanitize_particles(&mut prefab.entities);
        Ok(prefab)
    }

    /// 把预制体写回项目内的相对路径。
    ///
    /// 写盘前把资产引用编码成 GUID（并补齐缺 `.meta` 的资产身份证）。
    /// 这样"改了资产名再保存"就把新身份固化进文件，而不是继续存路径。
    pub fn write_prefab(&self, relative: &str, prefab: &Prefab) -> Result<(), ProjectError> {
        let encoded = {
            let mut assets = self.assets();
            // 与 `write_scene` 同理：编码前保证索引是最新的。
            assets.refresh();
            let _ = assets.ensure_metas();
            if assets.is_empty() {
                None
            } else {
                let mut copy = prefab.clone();
                encode_prefab_references(&mut copy, &assets);
                Some(copy)
            }
        };
        let prefab = encoded.as_ref().unwrap_or(prefab);

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

    /// 列出项目里所有场景文件（**项目相对路径**，按字典序）。
    ///
    /// 只认 `.ron` 且排除 `.prefab.ron` —— 与 [`AssetKind::from_path`] 的
    /// 判定顺序保持一致，避免预制体被当成场景重写一遍。
    pub fn list_scenes(&self) -> Vec<String> {
        let scan = self.scenes_dir();
        let mut out = Vec::new();
        collect_by_extension(&scan, &self.root, "ron", &mut out);
        out.retain(|p| !p.ends_with(&format!(".{PREFAB_EXTENSION}")));
        out.sort();
        out
    }

    /// 把整个项目的资产引用迁移成 GUID（老项目一次性升级）。
    ///
    /// 做三件事：
    /// 1. 补齐所有资产的 `.meta`（GUID 在这里生成，之后永不变）；
    /// 2. 把场景 / 预制体里的路径字面量改写成 `guid://…`；
    /// 3. 汇总错误（单个文件坏掉不阻断其余文件 —— 迁移是批处理，能救多少救多少）。
    ///
    /// # 为什么是"字符串字面量替换"而不是"读出来再整体序列化"
    ///
    /// 场景文件在本仓库里不只是数据，还是**文档**：示例场景大量用注释解释
    /// "这块地面为什么是这个颜色"。整份重新序列化会把注释全部吃掉，
    /// 一次迁移就把教程变成裸数据。所以这里只在原文里替换
    /// `"Resources/Box.glb"` → `"guid://…"`，其余一个字节都不动。
    ///
    /// 幂等：已经迁移过的引用是 `guid://…`，不再匹配路径，也不会重复改写。
    pub fn migrate_asset_references(&self) -> AssetMigrationReport {
        let mut report = AssetMigrationReport::default();

        // 先扫盘 + 生成身份证，后面的引用编码才查得到 GUID。
        {
            let mut assets = self.assets();
            assets.refresh();
            match assets.ensure_metas() {
                Ok(n) => report.created_metas = n,
                Err(e) => report.errors.push(e),
            }
        }

        for relative in self.list_scenes() {
            match self.rewrite_references_in_place(&relative, false) {
                Ok(true) => report.rewritten_scenes += 1,
                Ok(false) => {}
                Err(e) => report.errors.push(format!("{relative}: {e}")),
            }
        }

        for relative in self.list_prefabs() {
            match self.rewrite_references_in_place(&relative, true) {
                Ok(true) => report.rewritten_prefabs += 1,
                Ok(false) => {}
                Err(e) => report.errors.push(format!("{relative}: {e}")),
            }
        }

        // 材质里的 albedo 也是资产引用：不迁的话，以后贴图改名材质就断链。
        for relative in self.list_materials() {
            match self.rewrite_material_references_in_place(&relative) {
                Ok(true) => report.rewritten_materials += 1,
                Ok(false) => {}
                Err(e) => report.errors.push(format!("{relative}: {e}")),
            }
        }

        report
    }

    /// 就地把一个场景 / 预制体文件里的资产引用路径改写成 GUID。
    ///
    /// 返回"是否真的改了"。只替换**完整的带引号字面量** —— `"Resources/Box.glb"`
    /// 不会误伤 `"Resources/Box.glb.bak"`（后者不包含前者的收尾引号）。
    fn rewrite_references_in_place(&self, relative: &str, is_prefab: bool) -> Result<bool, String> {
        let path = self.path(relative);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("无法读取 '{}': {e}", path.display()))?;

        // 先按正常路径解析一遍：既拿到"内存里的路径清单"，也顺手验证文件本身
        // 是可解析的（坏文件直接进 errors，而不是被文本替换掩盖过去）。
        let mut references: Vec<String> = Vec::new();
        if is_prefab {
            let prefab = Prefab::from_ron(&text)
                .map_err(|e| format!("无法解析 '{}': {e}", path.display()))?;
            collect_entity_references(&prefab.entities, &mut references);
        } else {
            let scene: Scene = ron::from_str(&text)
                .map_err(|e| format!("无法解析 '{}': {e:#}", path.display()))?;
            collect_entity_references(&scene.entities, &mut references);
        }

        self.rewrite_literals(relative, text, references)
    }

    /// 就地改写一份材质文件里的 `albedo` 路径（同样只替换完整字面量）。
    fn rewrite_material_references_in_place(&self, relative: &str) -> Result<bool, String> {
        let path = self.path(relative);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("无法读取 '{}': {e}", path.display()))?;
        // 先解析一遍：坏材质进 errors，而不是被文本替换掩盖过去。
        let material = MaterialAsset::from_ron(&text)
            .map_err(|e| format!("无法解析 '{}': {e}", path.display()))?;
        let references = if material.albedo.is_empty() {
            Vec::new()
        } else {
            vec![material.albedo]
        };
        self.rewrite_literals(relative, text, references)
    }

    /// 把一组"内存路径引用"在原文里改写成 GUID 字面量；返回是否真的改了。
    ///
    /// 场景 / 预制体 / 材质三个阶段共用 —— 三个文件的引用形态不同
    /// （实体字段 vs `albedo`），但"替换完整引号字面量"这条规则只有一份。
    fn rewrite_literals(
        &self,
        relative: &str,
        text: String,
        mut references: Vec<String>,
    ) -> Result<bool, String> {
        let path = self.path(relative);
        let mut replaced = text;
        let mut changed = false;
        {
            let assets = self.assets();
            references.sort();
            references.dedup();
            for reference in references {
                if reference.is_empty()
                    || reference.contains("://")
                    || Path::new(&reference).is_absolute()
                {
                    continue;
                }
                let Some(guid) = assets.guid_of(&reference) else {
                    continue; // 悬空路径：没有身份证，保持原样让 check 报出来
                };
                let literal = format!("\"{reference}\"");
                if replaced.contains(&literal) {
                    let with_guid = format!("\"{}\"", guid.to_reference());
                    replaced = replaced.replace(&literal, &with_guid);
                    changed = true;
                }
            }
        }

        if !changed {
            return Ok(false);
        }
        std::fs::write(&path, replaced)
            .map_err(|source| ProjectError::Write { path, source })
            .map(|_| true)
            .map_err(|e| e.to_string())
    }

    /// 从项目内的相对路径加载场景。
    ///
    /// 磁盘上的资产引用是 GUID，这里解码成项目相对路径。悬空 GUID（资产被删）
    /// 会原样保留 `guid://…` 字符串，好让 `--check` 与 Inspector 都能指出断链。
    pub fn load_scene(&self, relative: &str) -> Result<Scene, String> {
        let path = self.path(relative);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("无法读取 '{}': {}", path.display(), e))?;
        self.parse_scene_text(&text, &path)
    }

    /// 加载清单里指定的入口场景。
    pub fn load_entry_scene(&self) -> Result<Scene, String> {
        self.load_scene(&self.manifest.entry_scene)
    }

    /// 把场景写回项目内的相对路径。
    ///
    /// 写盘前把资产引用编码成 GUID（并补齐缺 `.meta` 的资产身份证）——
    /// 于是"打开旧项目 + 保存一次"就完成了从路径引用到 GUID 引用的迁移，
    /// 不需要额外的迁移命令。
    pub fn write_scene(&self, relative: &str, scene: &Scene) -> Result<(), ProjectError> {
        let path = self.path(relative);
        let text = self.scene_text(scene, &path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ProjectError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(&path, text).map_err(|source| ProjectError::Write { path, source })
    }

    /// 把场景序列化成落盘文本（资产引用已编码成 GUID）。
    ///
    /// `write_scene` 与 `write_autosave` 共用这一份实现 —— 两条路写出来的
    /// 必须是**同一种格式**，否则"从自动保存恢复"会把 GUID 变成路径，
    /// 下一次保存再把它们编回去（中间任何一步的 diff 都会显得莫名其妙）。
    fn scene_text(&self, scene: &Scene, path: &Path) -> Result<String, ProjectError> {
        let encoded = {
            let mut assets = self.assets();
            // 保存前重新扫一遍：用户可能在编辑器打开后往 `Resources/` 里拷了
            // 新资产（或从外部改了文件名），索引过期就会漏掉新资产、把引用
            // 继续写成路径。保存是低频操作，一次全扫（几百文件）也就毫秒级。
            assets.refresh();
            let _ = assets.ensure_metas();
            if assets.is_empty() {
                None
            } else {
                let mut copy = scene.clone();
                encode_scene_references(&mut copy, &assets);
                Some(copy)
            }
        };
        let scene = encoded.as_ref().unwrap_or(scene);
        let pretty = ron::ser::PrettyConfig::default();
        ron::ser::to_string_pretty(scene, pretty).map_err(|e| ProjectError::Write {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
        })
    }

    /// 解析场景文本（`guid://` 解码成路径 + UI / 粒子数值夹取）。
    fn parse_scene_text(&self, text: &str, path: &Path) -> Result<Scene, String> {
        let mut scene = ron::from_str(text)
            .map_err(|e| format!("无法解析 '{}': {:#}", path.display(), e))?;
        {
            let assets = self.assets();
            decode_scene_references(&mut scene, &assets);
        }
        // 同 `load_prefab`：UI 数值在加载时夹取一次。
        sanitize_ui(&mut scene.entities);
        sanitize_particles(&mut scene.entities);
        Ok(scene)
    }

    // -- 自动保存 ----------------------------------------------------------

    /// 某个场景的自动保存文件绝对路径。
    pub fn autosave_path(&self, scene_relative: &str) -> PathBuf {
        self.path(autosave_relative(scene_relative))
    }

    /// 写一份自动保存副本，返回落盘路径。
    ///
    /// **原子写**（`.tmp` + rename）：自动保存是"用户没在看着"的时候写的，
    /// 恰好在这一刻断电 / 被杀进程不能留下半截 RON —— 那比没有自动保存更糟，
    /// 因为恢复流程会拿它去覆盖内存里的场景。
    pub fn write_autosave(
        &self,
        scene_relative: &str,
        scene: &Scene,
    ) -> Result<PathBuf, ProjectError> {
        let path = self.autosave_path(scene_relative);
        let text = self.scene_text(scene, &path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ProjectError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        write_atomic(&path, &text)?;
        Ok(path)
    }

    /// 读回某个场景的自动保存副本。
    pub fn read_autosave(&self, scene_relative: &str) -> Result<Scene, String> {
        let path = self.autosave_path(scene_relative);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("无法读取自动保存 '{}': {}", path.display(), e))?;
        self.parse_scene_text(&text, &path)
    }

    /// 某场景是否有自动保存，返回最后一次写入时间。
    pub fn autosave_modified(&self, scene_relative: &str) -> Option<std::time::SystemTime> {
        std::fs::metadata(self.autosave_path(scene_relative))
            .ok()?
            .modified()
            .ok()
    }

    /// 删除某场景的自动保存；返回是否真的删掉了文件。
    ///
    /// 文件不存在不算错误（正常路径就是"从没自动保存过"）—— 调用方
    /// （手动保存成功 / 用户选择丢弃）不该为此收到一条警告。
    pub fn remove_autosave(&self, scene_relative: &str) -> bool {
        let path = self.autosave_path(scene_relative);
        match std::fs::remove_file(&path) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => {
                log::warn!("删除自动保存失败 ({}): {e}", path.display());
                false
            }
        }
    }

    /// 列出所有留有自动保存的场景（项目相对路径，已排序）。
    ///
    /// 只用于"打开项目时提示还有哪些场景留有副本"，不是场景清单的一环
    /// （`list_scenes` 与它彻底无关）。
    pub fn list_autosaves(&self) -> Vec<String> {
        let dir = self.root.join(AUTOSAVE_DIR);
        let mut out = Vec::new();
        collect_by_extension(&dir, &dir, "ron", &mut out);
        out.sort();
        out
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
///
/// 相机实体**带 `camera` 组件**（不只是名字叫 Main Camera）：引擎的单机位解析
/// 只认组件（`Scene::active_camera_index`），没组件的话新项目一按 Play 就会
/// 退回宿主的兜底机位 —— 用户会以为"相机没生效"。
pub fn default_scene() -> Scene {
    let mut scene = Scene::new("New Scene");
    scene.entities = vec![crate::scene::Entity::new("Main Camera")
        .with_transform(Transform::from_translation(glam::Vec3::new(0.0, 1.0, -5.0)))
        .with_camera(crate::camera::Camera::default())];
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
    sanitize_asset_name_or(name, "Prefab")
}

/// 同 [`sanitize_asset_name`]，但空名退回调用方指定的名字。
///
/// 不同资产类型要有各自的兜底：`...` 当材质名应该叫 `Material.mat`，
/// 而不是莫名其妙变成 `Prefab.mat`。
fn sanitize_asset_name_or(name: &str, fallback: &str) -> String {
    let cleaned = sanitize_dir_name(name);
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}

/// 原子写：先写 `<文件名>.tmp` 再 rename 覆盖目标。
///
/// Windows 的 `rename` 会覆盖已存在的目标（MoveFileEx + REPLACE_EXISTING），
/// 与 Unix 语义一致。rename 失败（文件被占用、跨卷）时退回直接写 ——
/// "写成功但不原子"好过"什么都没写"，而且这条路径只在异常情况下走。
fn write_atomic(path: &Path, text: &str) -> Result<(), ProjectError> {
    let tmp = path.with_extension(match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{ext}.tmp"),
        None => "tmp".to_owned(),
    });
    std::fs::write(&tmp, text).map_err(|source| ProjectError::Write {
        path: tmp.clone(),
        source,
    })?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        log::warn!("临时文件 rename 失败，改为直接写入: {e}");
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(path, text).map_err(|source| ProjectError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
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
fn collect_by_extension(scan_dir: &Path, project_root: &Path, extension: &str, out: &mut Vec<String>) {    let Ok(entries) = std::fs::read_dir(scan_dir) else {
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

/// 收集一组实体上出现的所有资产引用（内存态：项目相对路径或悬空 `guid://`）。
///
/// 与 `xgame_core::asset` 里的编码 / 解码 walk 是同一组字段 —— 加新的资产
/// 引用字段时**两处都要改**（这里漏一个，迁移就会漏掉它，断链留在原地）。
fn collect_entity_references(entities: &[crate::Entity], out: &mut Vec<String>) {
    for entity in entities {
        if !entity.model.is_empty() {
            out.push(entity.model.clone());
        }
        if let Some(material) = &entity.material_asset {
            if !material.is_empty() {
                out.push(material.clone());
            }
        }
        out.extend(entity.scripts.iter().cloned());
        if let Some(audio) = &entity.audio {
            if !audio.clip.is_empty() {
                out.push(audio.clip.clone());
            }
        }
        if let Some(link) = &entity.prefab {
            out.push(link.source.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

/// 把场景里所有 UI 元素的数值夹到合法范围（NaN / inf 换成默认值）。
///
/// 只在**加载边界**调用：写盘时不动（用户没改的东西不该被悄悄改写），
/// 运行时也不每帧夹（那是热路径）。
fn sanitize_ui(entities: &mut [crate::Entity]) {
    for entity in entities {
        if let Some(ui) = entity.ui.as_mut() {
            *ui = ui.sanitized();
        }
    }
}

/// 同上，但给粒子发射器：NaN 的半径 / 颜色会让 GPU 拿到垃圾实例，
/// 表现是"粒子整块消失或糊满屏"，所以加载边界必须挡一道。
fn sanitize_particles(entities: &mut [crate::Entity]) {
    for entity in entities {
        if let Some(p) = entity.particle.as_mut() {
            p.sanitized();
        }
    }
}

#[cfg(test)]
mod tests {
    /// 加载边界会把 NaN 换成默认值：一个手写的 `nan` 不该让 UI 整棵树消失。
    #[test]
    fn ui_values_are_clamped_at_load_boundary() {
        use crate::{Entity, UiElement};
        let mut entities = vec![Entity::new("hud")];
        let mut ui = UiElement::panel(10.0, 10.0);
        ui.anchor_min = [f32::NAN, f32::INFINITY];
        ui.style.corner_radius = f32::NAN;
        ui.text.size = -5.0;
        entities[0].ui = Some(ui);
        super::sanitize_ui(&mut entities);
        let ui = entities[0].ui.as_ref().unwrap();
        assert_eq!(ui.anchor_min, [0.0, 0.0]);
        assert_eq!(ui.style.corner_radius, 0.0);
        assert!(ui.text.size >= 1.0);
    }

    use super::*;
    use crate::scene::Entity;

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

    /// `list_scripts` 列出全部 `.lua`（子目录递归、按字典序、非脚本不混入），
    /// 且**不**列 `.rhai`（已删的语言，残留文件不进编辑器下拉）。
    #[test]
    fn list_scripts_scans_lua_only() {
        let base =
            std::env::temp_dir().join(format!("xgame_scripts_list_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let handle = ProjectHandle::create(&base, "LuaOnly").expect("create");

        let scripts = handle.scripts_dir();
        std::fs::create_dir_all(scripts.join("sub")).unwrap();
        std::fs::write(scripts.join("a.lua"), "function update(ctx) end").unwrap();
        std::fs::write(scripts.join("old.rhai"), "fn update(ctx) {}").unwrap();
        std::fs::write(scripts.join("sub/b.lua"), "function update(ctx) end").unwrap();
        std::fs::write(scripts.join("notes.txt"), "不是脚本").unwrap();

        assert_eq!(
            handle.list_scripts(),
            vec![
                "Scripts/a.lua".to_owned(),
                "Scripts/sub/b.lua".to_owned(),
            ],
            "只列 .lua、按字典序、子目录递归、非脚本与已删语言的残留不混入"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn sanitize_strips_path_separators() {
        assert_eq!(sanitize_dir_name("a/b\\c"), "abc");
        assert_eq!(sanitize_dir_name("  我的项目  "), "我的项目");
        assert_eq!(sanitize_dir_name("..."), "");
    }

    // -- 资产 GUID 的端到端（P0 验收）--------------------------------------

    /// 一个临时的、带内容的项目。
    fn temp_project(tag: &str) -> (PathBuf, ProjectHandle) {
        let base = std::env::temp_dir().join(format!(
            "xgame_guid_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let handle = ProjectHandle::create(&base, "GuidDemo").expect("创建项目");
        (base, handle)
    }

    /// 保存场景必须把资产引用写成 GUID —— 这是"磁盘存 GUID"的第一条判据。
    ///
    /// 旧实现（路径引用）下这条必然红：文件里会是 `Resources/Box.glb`。
    #[test]
    fn saving_scene_writes_guid_references() {
        let (base, handle) = temp_project("write");
        std::fs::create_dir_all(handle.path("Resources")).unwrap();
        std::fs::write(handle.path("Resources/Box.glb"), b"glb").unwrap();

        let mut scene = handle.load_entry_scene().expect("默认场景");
        scene
            .entities
            .push(Entity::new("Box").with_model("Resources/Box.glb"));
        let entry = handle.manifest().entry_scene.clone();
        handle.write_scene(&entry, &scene).expect("写场景");

        let text = std::fs::read_to_string(handle.path(&entry)).unwrap();
        assert!(
            text.contains("guid://"),
            "资产引用必须以 GUID 落盘，实际：\n{text}"
        );
        assert!(
            !text.contains("Resources/Box.glb"),
            "路径不该再出现在场景文件里（.meta 才有路径）:\n{text}"
        );
        // `.meta` 要同时被生成 —— 没有身份证就没法编码。
        assert!(handle.path("Resources/Box.glb.meta").is_file());

        // 读回来必须是**路径**（内存态），不是 guid:// 字符串。
        let reloaded = handle.load_entry_scene().unwrap();
        assert_eq!(reloaded.entities.last().unwrap().model, "Resources/Box.glb");

        let _ = std::fs::remove_dir_all(&base);
    }

    // -- 自动保存（崩溃恢复的数据面）--------------------------------------

    /// 新建项目自带 `.gitignore`，且覆盖引擎自己的三个产物目录。
    ///
    /// 旧实现（不写 `.gitignore`）下这条必红。漏掉的后果是用户第一次
    /// `git add .` 就把 `Plugins/` 的 DLL 和 `.autosave/` 的副本一起提交进去。
    #[test]
    fn create_writes_a_gitignore_for_generated_dirs() {
        let base =
            std::env::temp_dir().join(format!("xgame_gitignore_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let handle = ProjectHandle::create(&base, "IgnoreMe").expect("create");
        let text = std::fs::read_to_string(handle.root().join(".gitignore"))
            .expect("新建项目应当带 .gitignore");
        for needle in ["Build/", "Plugins/", AUTOSAVE_DIR] {
            assert!(text.contains(needle), ".gitignore 应当忽略 {needle}，实际：\n{text}");
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 副本的路径保留子目录、能读回，且**不进** `list_scenes`。
    ///
    /// `list_scenes` 是递归扫描的：副本一旦落在 `Scenes/` 下，`--check` 会拿它
    /// 当场景校验、Scenes 菜单会列出它、`--migrate-assets` 会改写它。
    #[test]
    fn autosave_roundtrips_and_stays_out_of_the_scene_list() {
        let (base, handle) = temp_project("autosave");
        let mut scene = handle.load_entry_scene().expect("默认场景");
        scene.name = "Autosaved".to_owned();
        scene.entities.push(Entity::new("extra"));

        let path = handle
            .write_autosave(DEFAULT_ENTRY_SCENE, &scene)
            .expect("写自动保存");
        assert_eq!(
            path,
            handle.root().join(AUTOSAVE_DIR).join(DEFAULT_ENTRY_SCENE),
            "副本应落在 .autosave/ 下的同名子路径"
        );
        assert!(path.is_file());

        let back = handle.read_autosave(DEFAULT_ENTRY_SCENE).expect("读回副本");
        assert_eq!(back.name, "Autosaved");
        assert!(back.entities.iter().any(|e| e.name == "extra"));

        assert_eq!(
            handle.list_scenes(),
            vec![DEFAULT_ENTRY_SCENE.to_owned()],
            "副本不该进场景清单"
        );
        assert_eq!(
            handle.list_autosaves(),
            vec![DEFAULT_ENTRY_SCENE.to_owned()],
            "但 list_autosaves 要能看见它"
        );
        assert!(handle.autosave_modified(DEFAULT_ENTRY_SCENE).is_some());

        assert!(handle.remove_autosave(DEFAULT_ENTRY_SCENE), "第一次删除应成功");
        assert!(
            !handle.remove_autosave(DEFAULT_ENTRY_SCENE),
            "文件已经不在了，重复删除不该报成功"
        );
        assert!(handle.list_autosaves().is_empty());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 副本与真保存必须是**同一种格式**（GUID 编码）。
    ///
    /// 不一致的后果很隐蔽：从副本恢复会把 GUID 退回路径，下一次保存再编回去，
    /// 中间每一次 diff 都像"引用被改过"，排查时完全找不到原因。
    #[test]
    fn autosave_uses_the_same_encoding_as_a_real_save() {
        let (base, handle) = temp_project("autosave_guid");
        std::fs::create_dir_all(handle.path("Resources")).unwrap();
        std::fs::write(handle.path("Resources/Box.glb"), b"glb").unwrap();

        let mut scene = handle.load_entry_scene().expect("默认场景");
        scene
            .entities
            .push(Entity::new("Box").with_model("Resources/Box.glb"));

        handle
            .write_autosave(DEFAULT_ENTRY_SCENE, &scene)
            .expect("写自动保存");
        let text =
            std::fs::read_to_string(handle.autosave_path(DEFAULT_ENTRY_SCENE)).unwrap();
        assert!(text.contains("guid://"), "副本里应当是 GUID 引用：\n{text}");
        assert!(
            !text.contains("Resources/Box.glb"),
            "副本里不该出现资产路径字面量：\n{text}"
        );

        // 读回来是**路径**（内存态）—— 与 `load_scene` 同一条边界。
        let back = handle.read_autosave(DEFAULT_ENTRY_SCENE).expect("读回副本");
        assert!(back
            .entities
            .iter()
            .any(|e| e.model == "Resources/Box.glb"));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// **P0 核心验收**：资产改名 / 挪目录后，场景依然指向它。
    ///
    /// 旧实现下这条红得最直接：场景里存的是 `Resources/Box.glb`，
    /// 文件挪走后模型路径就断了。
    #[test]
    fn renaming_asset_keeps_scene_reference_alive() {
        let (base, handle) = temp_project("rename");
        std::fs::create_dir_all(handle.path("Resources")).unwrap();
        std::fs::write(handle.path("Resources/Box.glb"), b"glb").unwrap();

        let mut scene = handle.load_entry_scene().unwrap();
        scene
            .entities
            .push(Entity::new("Box").with_model("Resources/Box.glb"));
        let entry = handle.manifest().entry_scene.clone();
        handle.write_scene(&entry, &scene).unwrap();

        // 用户用文件管理器把资产（连同 .meta）挪进子目录。
        std::fs::create_dir_all(handle.path("Resources/Models")).unwrap();
        std::fs::rename(
            handle.path("Resources/Box.glb"),
            handle.path("Resources/Models/Box.glb"),
        )
        .unwrap();
        std::fs::rename(
            handle.path("Resources/Box.glb.meta"),
            handle.path("Resources/Models/Box.glb.meta"),
        )
        .unwrap();

        handle.refresh_assets();
        let reloaded = handle.load_entry_scene().unwrap();
        let model = &reloaded.entities.last().unwrap().model;
        assert_eq!(model, "Resources/Models/Box.glb", "GUID 应解析到新路径");
        assert!(handle.path(model).is_file(), "解析出的路径必须真的存在");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 旧场景（存路径）打开后不改任何东西，保存一次就迁移成 GUID；
    /// 再读回来路径一模一样 —— 迁移不能改变任何语义。
    #[test]
    fn legacy_path_references_migrate_on_save() {
        let (base, handle) = temp_project("migrate");
        std::fs::create_dir_all(handle.path("Resources")).unwrap();
        std::fs::write(handle.path("Resources/Box.glb"), b"glb").unwrap();

        // 手写一份"老格式"场景（引用是路径）。
        let legacy = r#"(
            name: "Legacy",
            entities: [
                (
                    name: "Box",
                    transform: (position: (0.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0), scale: (1.0, 1.0, 1.0)),
                    model: "Resources/Box.glb",
                ),
            ],
        )"#;
        std::fs::write(handle.path("Scenes/legacy.ron"), legacy).unwrap();

        let before = handle.load_scene("Scenes/legacy.ron").expect("老场景可读");
        assert_eq!(before.entities[0].model, "Resources/Box.glb");

        handle.write_scene("Scenes/legacy.ron", &before).unwrap();
        let text = std::fs::read_to_string(handle.path("Scenes/legacy.ron")).unwrap();
        assert!(text.contains("guid://"), "保存后必须迁移成 GUID:\n{text}");

        let after = handle.load_scene("Scenes/legacy.ron").unwrap();
        assert_eq!(after.entities[0].model, "Resources/Box.glb");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 预制体资产的引用同样走 GUID：改名后预制体内部的模型引用不断。
    #[test]
    fn prefab_references_use_guid() {
        let (base, handle) = temp_project("prefab_guid");
        std::fs::create_dir_all(handle.path("Resources")).unwrap();
        std::fs::write(handle.path("Resources/Box.glb"), b"glb").unwrap();

        let mut prefab = Prefab::new("Enemy");
        prefab
            .entities
            .push(Entity::new("Body").with_model("Resources/Box.glb"));
        let relative = handle.prefab_path_for("Enemy");
        handle.write_prefab(&relative, &prefab).unwrap();

        let text = std::fs::read_to_string(handle.path(&relative)).unwrap();
        assert!(text.contains("guid://"), "预制体引用也要 GUID:\n{text}");

        // 挪走模型 + meta，预制体仍能解析。
        std::fs::create_dir_all(handle.path("Resources/Models")).unwrap();
        std::fs::rename(handle.path("Resources/Box.glb"), handle.path("Resources/Models/Box.glb")).unwrap();
        std::fs::rename(
            handle.path("Resources/Box.glb.meta"),
            handle.path("Resources/Models/Box.glb.meta"),
        )
        .unwrap();
        handle.refresh_assets();

        let loaded = handle.load_prefab(&relative).unwrap();
        assert_eq!(loaded.entities[0].model, "Resources/Models/Box.glb");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 材质资产走同一套 GUID 边界：`albedo` 落盘是 GUID，读回是路径。
    #[test]
    fn material_albedo_uses_guid_and_roundtrips() {
        let (base, handle) = temp_project("material_guid");
        std::fs::write(handle.path("Resources/Texture.png"), b"png").unwrap();
        let material = MaterialAsset {
            base_color: [0.8, 0.2, 0.1, 1.0],
            metallic: 0.0,
            roughness: 0.6,
            albedo: "Resources/Texture.png".into(),
        };
        let relative = handle.material_path_for("Wood");
        assert_eq!(relative, "Resources/Materials/Wood.mat");
        handle.write_material(&relative, &material).unwrap();

        let text = std::fs::read_to_string(handle.path(&relative)).unwrap();
        assert!(text.contains("guid://"), "albedo 必须以 GUID 落盘:\n{text}");
        assert!(
            !text.contains("Resources/Texture.png"),
            "磁盘上不该留贴图路径:\n{text}"
        );

        let loaded = handle.load_material(&relative).unwrap();
        assert_eq!(loaded, material, "读回的内存态必须是原路径");
        assert_eq!(
            handle.material_path_for("..."),
            "Resources/Materials/Material.mat",
            "空名兜底不能变成 Prefab"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// `--migrate-assets` 必须把材质里的 albedo 与实体上的 material_asset
    /// 一起迁成 GUID，并且迁移后语义不变（读回来还是路径）。
    #[test]
    fn migration_rewrites_material_and_entity_references() {
        let (base, handle) = temp_project("material_migrate");
        std::fs::write(handle.path("Resources/Texture.png"), b"png").unwrap();
        std::fs::create_dir_all(handle.path("Resources/Materials")).unwrap();
        std::fs::write(
            handle.path("Resources/Materials/Legacy.mat"),
            "(base_color: (1.0, 1.0, 1.0, 1.0), albedo: \"Resources/Texture.png\")\n",
        )
        .unwrap();
        let legacy_scene = r#"(
            name: "Legacy",
            entities: [
                (
                    name: "Box",
                    transform: (position: (0.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0), scale: (1.0, 1.0, 1.0)),
                    material_asset: Some("Resources/Materials/Legacy.mat"),
                ),
            ],
        )"#;
        std::fs::write(handle.path("Scenes/legacy.ron"), legacy_scene).unwrap();

        let report = handle.migrate_asset_references();
        assert!(!report.has_errors(), "{:?}", report.errors);
        assert_eq!(report.rewritten_materials, 1);
        assert_eq!(report.rewritten_scenes, 1);

        let material_text = std::fs::read_to_string(handle.path("Resources/Materials/Legacy.mat")).unwrap();
        assert!(material_text.contains("guid://"), "{material_text}");
        let scene_text = std::fs::read_to_string(handle.path("Scenes/legacy.ron")).unwrap();
        assert!(scene_text.contains("guid://"), "{scene_text}");

        let loaded = handle.load_scene("Scenes/legacy.ron").unwrap();
        assert_eq!(
            loaded.entities[0].material_asset.as_deref(),
            Some("Resources/Materials/Legacy.mat")
        );
        let material = handle
            .load_material(loaded.entities[0].material_asset.as_deref().unwrap())
            .unwrap();
        assert_eq!(material.albedo, "Resources/Texture.png");

        // 幂等：第二次迁移不该再改写。
        let again = handle.migrate_asset_references();
        assert_eq!(again.rewritten_materials, 0);
        assert_eq!(again.rewritten_scenes, 0);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn path_resolves_relative_to_root() {
        let handle = ProjectHandle {
            root: PathBuf::from("/tmp/demo"),
            manifest: Project::new("demo"),
            assets: Arc::new(Mutex::new(AssetDatabase::default())),
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

        // 音频引用同理：clip 断链不会在编辑器里显形（引擎解码失败只记日志
        // 然后无声），只有加载前就断言才能在 CI 里抓住"文件改名忘改场景"。
        for entity in &scene.entities {
            let Some(audio) = &entity.audio else {
                continue;
            };
            if audio.clip.is_empty() {
                continue;
            }
            assert!(
                handle.path(&audio.clip).is_file(),
                "实体 '{}' 引用的音频不存在: {}",
                entity.name,
                audio.clip
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
            assets: Arc::new(Mutex::new(AssetDatabase::default())),
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

    /// 老项目的 `project.ron` 里没有 `runtime` 节（重力 / 固定步长是后加的），
    /// 反序列化时必须补上与当年硬编码值一致的默认值，物理行为不能因为升级
    /// 引擎而悄悄变样。
    #[test]
    fn old_manifest_without_runtime_defaults() {
        let ron = r#"(format_version: 1, name: "Old")"#;
        let project: Project = ron::from_str(ron).expect("老清单应当能反序列化");
        assert_eq!(project.runtime, crate::config::RuntimeConfig::default());
        // 默认值 = 旧 WorldConfig::default() 里硬编码的那一对。
        assert_eq!(project.runtime.gravity, [0.0, -9.81, 0.0]);
        assert_eq!(project.runtime.fixed_dt, 1.0 / 60.0);
    }

    /// 新字段写盘 → 读回一致，且老代码能忽略未知字段（前向兼容反向验证：
    /// 清单里多了 `runtime` 节时 `manifest_roundtrip` 也覆盖到了）。
    #[test]
    fn runtime_survives_manifest_roundtrip() {
        let mut project = Project::new("RT");
        project.runtime.gravity = [0.0, -1.0, 0.0];
        project.runtime.fixed_dt = 1.0 / 120.0;
        let text = ron::to_string(&project).expect("序列化");
        assert!(text.contains("runtime"), "runtime 节必须落盘");
        let back: Project = ron::from_str(&text).expect("反序列化");
        assert_eq!(back.runtime, project.runtime);
    }

    /// 老清单没有 `render` 节（色调映射 / 曝光是后加的）→ 补默认值。
    /// 与 `old_manifest_without_runtime_defaults` 同一纪律：升级引擎不能
    /// 让旧项目直接打不开。
    #[test]
    fn old_manifest_without_render_defaults() {
        let ron = r#"(format_version: 1, name: "Old")"#;
        let project: Project = ron::from_str(ron).expect("老清单应当能反序列化");
        assert_eq!(project.render, RenderConfig::default());
    }

    /// 渲染参数写盘 → 读回一致（磁盘格式是消费契约，编辑器 / 游戏 / 截图
    /// 三端都要读它）。Bloom 一起过：它是后加字段，走 `serde(default)` 且
    /// 未调过时不落盘（见 `bloom_defaults_to_off`）。
    #[test]
    fn render_survives_manifest_roundtrip() {
        let mut project = Project::new("Render");
        project.render.exposure = 2.25;
        project.render.tone_map = crate::config::ToneMapMode::Off;
        project.render.bloom = crate::config::BloomConfig {
            intensity: 0.75,
            threshold: 1.5,
            knee: 0.4,
        };
        let text = ron::to_string(&project).expect("序列化");
        assert!(text.contains("render"), "render 节必须落盘");
        assert!(text.contains("bloom"), "调过的 Bloom 必须落盘");
        let back: Project = ron::from_str(&text).expect("反序列化");
        assert_eq!(back.render, project.render);
    }

    /// `tone_map` 的**变体标签即磁盘格式**：RON 里是 `Aces` / `Off` 两个字面量。
    /// 改标签名等于让所有写了 `tone_map` 的旧清单解析失败 —— 与
    /// `field_value_ron_variant_labels_are_stable`（P1.5）同一纪律。
    #[test]
    fn tone_map_ron_labels_are_stable() {
        let aces: crate::config::ToneMapMode = ron::from_str("Aces").expect("Aces");
        assert_eq!(aces, crate::config::ToneMapMode::Aces);
        let off: crate::config::ToneMapMode = ron::from_str("Off").expect("Off");
        assert_eq!(off, crate::config::ToneMapMode::Off);
        assert_eq!(ron::to_string(&aces).unwrap(), "Aces");
        assert_eq!(ron::to_string(&off).unwrap(), "Off");
    }
}
