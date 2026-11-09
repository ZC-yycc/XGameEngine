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

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::asset::{
    decode_prefab_references, decode_scene_references, encode_prefab_references,
    encode_scene_references, AssetDatabase, AssetGuid, AssetKind, AssetMoveReport, ImportSettings,
};
use crate::config::{BuildConfig, ContentConfig, RenderConfig, RuntimeConfig, WindowConfig};
use crate::id::EntityId;
use crate::layers::Layers;
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

/// 两个场景路径是否指同一份文件（只忽略分隔符与首尾空白）。
///
/// **不分配**：`scene_index()` 可能被脚本每帧调用，稳态热路径上不该为了一次
/// 比较去 `replace` 出一个新 `String`。也不做大小写折叠 —— Windows 上确实
/// 大小写不敏感，但清单与运行时路径都由本引擎写出，真出现大小写差异属于
/// 配置错误，应该暴露而不是被悄悄当成同一关。
///
/// 收在 core：运行时（`World::scene_index`）与脚本（`ctx.scene_index`）
/// 必须用同一份规则，否则同一份清单在两边算出不同的关卡下标。
pub fn same_scene_path(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().as_bytes(), b.trim().as_bytes());
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x == y || (*x == b'\\' && *y == b'/') || (*x == b'/' && *y == b'\\'))
}

/// 把用户输入的场景路径规范化，并挡掉会跑出项目外的写法。
///
/// **收在 core 而不是编辑器里**：这条规则现在有三个消费方 —— 编辑器（改清单 /
/// 另存为新场景）、`--check`（校验发布清单）、构建器（打包前逐关装载）。
/// 三处各抄一份的结果必然漂移，而漂移的后果是"检查通过、构建成功、运行时打不开"。
///
/// 规则 = [`normalize_project_relative`]（越界 + Windows 不认的写法）再要求
/// `.ron` 结尾、且不是 `.prefab.ron`（预制体不是场景）。
///
/// **MCP 的 `scene` 参数也走这一条**（`xgame-mcp` 的 `scene_rel` 只在前面补
/// 一个 `.ron` 后缀）。此前那里自己写了一份更松的检查：接受 `a.RON`、
/// `./a.ron`、甚至 `a.prefab.ron`，于是"编辑器不认的路径，智能体能写"。
pub fn normalize_scene_relative(raw: &str) -> Result<String, String> {
    let normalized = normalize_project_relative(raw).map_err(|e| e.to_string())?;
    if !normalized.ends_with(".ron") || normalized.ends_with(".prefab.ron") {
        return Err(format!("场景文件必须以 .ron 结尾: {raw}"));
    }
    Ok(normalized)
}

/// 项目内相对路径的**唯一**校验入口（场景 / 预制体 / 资产 / 脚本共用）。
///
/// 返回规范化后的路径（正斜杠、吃掉开头的 `./`），**不检查**扩展名与目录白名单
/// —— 那些是各消费方自己的事（见 [`normalize_scene_relative`] 与
/// [`crate::asset::normalize_asset_relative`]）。
///
/// 规则分两类，理由都是"Windows 会**静默**改写，于是写下的字符串和盘上的文件
/// 不是同一个"：
/// - **越界**：绝对路径 / 盘符 / `..`。`ProjectHandle::path` 只是 `root.join(..)`，
///   而 `Path::join` 遇到绝对路径会**整个替换**掉根；
/// - **Windows 不认**：冒号（NTFS 备用数据流）、结尾空格与点（Win32 直接吃掉）、
///   控制字符与 `< > " | ? *`、以及保留设备名（`CON` / `NUL` / `COM1` …，
///   `File::create("CON.ron")` 打开的是控制台，写进去的字节直接消失）。
///
/// 这条纪律此前**只有资产侧**有（`normalize_asset_relative` 早就挡了字符），
/// 场景侧只挡越界 —— 于是 `Scenes/Level:1.ron` 这个写法能被写进发布清单，保存
/// 显示成功、场景从此只存在于这台机器的 NTFS 数据流里（git 存不下）。
pub fn normalize_project_relative(raw: &str) -> Result<String, PathDefect> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(PathDefect::Empty);
    }
    let mut rel = trimmed.replace('\\', "/");
    // 只吃**开头**的 `./`。`Path::components` 会把夹在中间的 `.` 自动消掉
    // （`a/./b` → `a/b`），但开头那个会变成 `CurDir` 组件、被下面的 match 拒掉 ——
    // 同一个文件两种写法得到两种结果，没必要。
    while let Some(rest) = rel.strip_prefix("./") {
        rel = rest.to_string();
    }
    if rel.is_empty() {
        return Err(PathDefect::Empty);
    }
    // Windows 盘符（`C:`）与 UNC 前缀在非 Windows 上不一定被 `Path` 认成绝对路径，
    // 这里显式再挡一次；编辑器的主要平台是 Windows，不能靠平台差异放行。
    if rel.starts_with('/') || rel.as_bytes().get(1) == Some(&b':') {
        return Err(PathDefect::Absolute);
    }

    let mut parts = Vec::new();
    for component in Path::new(&rel).components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_string_lossy();
                validate_path_component(&part)?;
                parts.push(part.into_owned());
            }
            _ => return Err(PathDefect::ParentEscape),
        }
    }
    if parts.is_empty() {
        return Err(PathDefect::Empty);
    }
    Ok(parts.join("/"))
}

/// 项目相对路径不合法时**具体哪里不合法**。
///
/// 分成枚举而不是直接给字符串，是为了让 `--check` 能输出纯 ASCII 的短标签
/// （控制台代码页不是 UTF-8，中文原因会在 CI 日志里变成乱码、也匹配不上），
/// 而编辑器 / 报错仍然拿到一句能读的中文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathDefect {
    /// 空路径（或只有 `./`）。
    Empty,
    /// 绝对路径 / 盘符 / UNC 前缀。
    Absolute,
    /// `..` 或根组件（会跳到项目外）。
    ParentEscape,
    /// 组件含冒号（Windows 会当成 NTFS 备用数据流）。
    Colon { part: String },
    /// 组件以空格或点结尾（Windows 会静默吃掉）。
    TrailingSpaceOrDot { part: String },
    /// 组件含控制字符或 `< > " | ? *`。
    IllegalChar { part: String },
    /// 组件是 Windows 保留设备名（`CON` / `NUL` / `COM1` …）。
    ReservedName { part: String },
}

impl PathDefect {
    /// 纯 ASCII 的短标签（`--check` 报告用）。
    pub fn ascii(&self) -> &'static str {
        match self {
            Self::Empty => "empty path",
            Self::Absolute => "absolute path or drive prefix",
            Self::ParentEscape => "`..` escapes the project",
            Self::Colon { .. } => "colon (NTFS alternate data stream)",
            Self::TrailingSpaceOrDot { .. } => "trailing space or dot",
            Self::IllegalChar { .. } => "illegal character (control or < > \" | ? *)",
            Self::ReservedName { .. } => "windows reserved device name",
        }
    }
}

impl std::fmt::Display for PathDefect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "路径不能为空"),
            Self::Absolute => write!(f, "必须是项目相对路径（不能以 `/` 或盘符开头）"),
            Self::ParentEscape => write!(f, "路径不能包含 `..` 或根组件"),
            Self::Colon { part } => write!(
                f,
                "路径组件 `{part}` 含冒号（Windows 会把 `名字:后缀` 当成备用数据流）"
            ),
            Self::TrailingSpaceOrDot { part } => write!(
                f,
                "路径组件 `{part}` 以空格或点结尾（Windows 会静默吃掉这两个字符）"
            ),
            Self::IllegalChar { part } => {
                write!(f, "路径组件 `{part}` 含非法字符（控制字符或 < > \" | ? *）")
            }
            Self::ReservedName { part } => write!(
                f,
                "路径组件 `{part}` 是 Windows 保留设备名（打开的是设备，不是文件）"
            ),
        }
    }
}

/// Windows 保留设备名：这些名字（**带不带扩展名都一样**）在 Win32 上打开的是
/// 设备而不是文件。
const RESERVED_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 这个名字是不是 Windows 保留设备名（只看第一个 `.` 之前的部分）。
///
/// `CON` / `con.ron` 都算；`CONSOLE` / `console.ron` / `COM10` 不算 —— 保留的是
/// **整名相等**（且大小写不敏感），别按前缀匹配，否则正常的词会被误杀。
fn is_reserved_device_name(part: &str) -> bool {
    let base = part.split('.').next().unwrap_or(part);
    RESERVED_DEVICE_NAMES
        .iter()
        .any(|name| base.eq_ignore_ascii_case(name))
}

/// 单个路径组件的字符级校验（[`normalize_project_relative`] 的一部分）。
fn validate_path_component(part: &str) -> Result<(), PathDefect> {
    if part.contains(':') {
        return Err(PathDefect::Colon {
            part: part.to_string(),
        });
    }
    if part.ends_with([' ', '.']) {
        return Err(PathDefect::TrailingSpaceOrDot {
            part: part.to_string(),
        });
    }
    if part
        .chars()
        .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Err(PathDefect::IllegalChar {
            part: part.to_string(),
        });
    }
    if is_reserved_device_name(part) {
        return Err(PathDefect::ReservedName {
            part: part.to_string(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 运行时读路径：模型 / 脚本 / 音频的解析入口
// ---------------------------------------------------------------------------

/// 运行时**读**路径解析失败的原因。
///
/// 分成两类而不是一句话，是因为两者的处置完全不同：
/// - [`Invalid`](Self::Invalid)：路径本身不合规（越界 / Windows 不认的写法）。
///   这是**永久性**失败 —— 同一份工程换台机器也读不到，必须报错；
/// - [`Missing`](Self::Missing)：路径合规但盘上没有。可能是打包漏了文件，
///   也可能是作者写错，报错里要能看见**找过哪些目录**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadPathError {
    /// 路径本身不合规。
    Invalid(PathDefect),
    /// 合规但没找到（含按 `search_paths` 逐个找过）。
    Missing {
        /// 规范化之后的项目相对路径。
        relative: String,
        /// 实际试过的候选（项目相对路径，按尝试顺序）。
        tried: Vec<String>,
    },
}

impl std::fmt::Display for ReadPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(defect) => write!(f, "{defect}"),
            Self::Missing { relative, tried } => {
                write!(f, "文件不存在: {relative}")?;
                if tried.len() > 1 {
                    write!(f, "（已按 search_paths 试过: {}）", tried.join(", "))?;
                }
                Ok(())
            }
        }
    }
}

/// 把一个**项目相对**的读路径解析成绝对路径（模型 / 脚本 / 音频的共用入口）。
///
/// # 为什么要这一层
///
/// 运行时此前是 `root.join(path)` 直接读：`Path::join` 遇到绝对路径会**整个
/// 替换**掉根，遇到 `..` 会逐级上跳 —— 于是一份手写 / 下载来的场景可以让运行时
/// 读项目外的任意文件（`--check` 的 `unsafe_paths` 会点名，但**不跑检查的宿主
/// 仍然会去读**）。检查是工具，门禁必须在读取的地方。
///
/// 规范化只走 [`normalize_project_relative`] 这一条规则（与场景 / 资产相同），
/// 不在读取路径上再养第三份判断。
///
/// # `search_paths` 的语义（此前是"未定义"）
///
/// 有序的**附加搜索根**（项目相对目录）。解析顺序：
/// 1. 直接 `root/<relative>`；
/// 2. 依次试 `root/<search_path>/<relative>`；
/// 3. 都没有 → [`ReadPathError::Missing`]，报错里列出所有试过的路径。
///
/// 三条约束：
/// - 每个条目本身也要过 `normalize_project_relative`（不许越界 / 不许 Windows
///   不认的写法）；不合规的条目**跳过**（`--check` 会把它们报成问题），
///   不会让一次读取变成"读到项目外"；
/// - 只影响**读**：写盘（保存场景 / 存档）永远落在清单声明的位置，不四处探路；
/// - 空列表 = 行为与引入本函数之前**逐位一致**（直接那条命中，或报缺失）。
///
/// # 已知边界
///
/// 只按"文件存在"判定，不区分大小写（Windows 文件系统本身不区分）；
/// 不做符号链接逃逸检测 —— 链接目标在项目外时仍会被读到，那是**工程作者**
/// 主动放进仓库的东西，与"脚本里写 `../..`"不是一类问题。
pub fn resolve_read_path(
    root: &Path,
    relative: &str,
    search_paths: &[String],
) -> Result<PathBuf, ReadPathError> {
    let rel = normalize_project_relative(relative).map_err(ReadPathError::Invalid)?;
    let direct = root.join(&rel);
    if direct.is_file() {
        return Ok(direct);
    }
    let mut tried = vec![rel.clone()];
    for entry in search_paths {
        let Ok(dir) = normalize_project_relative(entry) else {
            continue;
        };
        let candidate = format!("{dir}/{rel}");
        if root.join(&candidate).is_file() {
            return Ok(root.join(candidate));
        }
        tried.push(candidate);
    }
    Err(ReadPathError::Missing {
        relative: rel,
        tried,
    })
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

# 存档（玩家进度：本机玩出来的数据，不是关卡内容；要联机共享就手动提交）
Saves/
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
    /// 层与碰撞矩阵（32 层名字 + 谁和谁碰）。
    ///
    /// 放在清单而不是场景里：层是**项目级约定**（所有关卡共享一份排队规则），
    /// 每张场景各配一套会让"同一个敌人进第二关就穿墙"。同样是后加字段、
    /// 同样 `#[serde(default)]`：默认全通，老清单不写这一节，行为不变。
    #[serde(default, skip_serializing_if = "Layers::is_default")]
    pub layers: Layers,
    /// 画面渲染参数（色调映射 / 曝光）。
    ///
    /// 同样是后加字段、同样靠 `#[serde(default)]` 兼容旧清单。放在清单而不是
    /// 场景里：画面风格是项目的属性（编辑器视口 / 独立游戏 / 无头截图三端一致），
    /// 不是"这张地图"的属性。
    #[serde(default)]
    pub render: RenderConfig,
    /// 质量档位（低 / 中 / 高 + 显式覆盖）。**全空 = 不存在**（不落盘）——
    /// 老项目没有这一节时行为与引入档位之前逐位一致（MSAA 取适配器最大、
    /// 阴影默认 2048、LOD 缩放 1.0）。语义见 [`crate::quality`]。
    #[serde(
        default,
        skip_serializing_if = "crate::quality::QualityConfig::is_empty"
    )]
    pub quality: crate::quality::QualityConfig,
    /// 构建 / 发布设置（版本号 + 有序关卡清单）。
    ///
    /// 同样是后加字段、同样 `#[serde(default)]`：老清单没有这一节，
    /// 空清单的语义是"没配"，有效顺序退化成只有入口场景，行为与加这功能之前一致。
    #[serde(default)]
    pub build: BuildConfig,
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

    /// 配置过的**关卡顺序**（项目相对路径，已规范化）。未配置返回空。
    ///
    /// 规则只有三条，但每条都有来由：
    ///
    /// 1. 逐项 `trim` + 反斜杠转正斜杠 —— 清单是手改的文本，Windows 路径习惯会漏进来；
    ///    运行时与资产系统都只认正斜杠。
    /// 2. 空串丢弃、重复项只留第一次出现 —— 重复会让 `scene_index()` 出现两个答案，
    ///    而且 `load_scene_at` 指到哪一个都算"对"，这种歧义必须在入口处消掉。
    /// 3. 清完之后为空就是**未配置**，返回空 —— 不退回 `[entry_scene]`。
    ///
    /// 第 3 条是刻意的：如果未配置时返回 `[entry]`，脚本的 `scene_index()` 会对
    /// 单场景项目返回 `0`，作者会以为"这是第 0 关，可以 `load_scene_at(1)`"，
    /// 然后在运行时撞越界错误。返回空则 `scene_index()` 是 `nil`，
    /// 语义诚实：**没有关卡顺序，只能按路径切**。
    pub fn scene_order(&self) -> Vec<String> {
        let mut order: Vec<String> = Vec::new();
        for raw in &self.build.scenes {
            let path = raw.trim().replace('\\', "/");
            if path.is_empty() || order.iter().any(|p| p == &path) {
                continue;
            }
            order.push(path);
        }
        order
    }

    /// 清单里的**内容目录**有哪些写法不合法（越界 / Windows 不认）。
    ///
    /// 只查内容目录：入口场景与 `build.scenes` 各有自己的判定入口
    /// （前者装载时硬失败、后者进 `build_missing_scenes`），在这里再报一遍
    /// 只会让同一个错出现两次、问题数虚高。
    ///
    /// 为什么要单独查：这些目录全是**手改清单的重灾区**，而越界的写法不会在
    /// 读取时报错 —— `list_scenes` 扫到项目外、构建复制到项目外，症状是
    /// "编辑器里什么都没有，但检查全绿"。
    pub fn content_path_defects(&self) -> Vec<String> {
        let content = &self.content;
        let mut out = Vec::new();
        for (label, raw) in [
            ("content.scenes_dir", content.scenes_dir.as_str()),
            ("content.models_dir", content.models_dir.as_str()),
            ("content.textures_dir", content.textures_dir.as_str()),
            ("content.audio_dir", content.audio_dir.as_str()),
            ("content.shaders_dir", content.shaders_dir.as_str()),
            ("content.prefabs_dir", content.prefabs_dir.as_str()),
            ("content.scripts_dir", content.scripts_dir.as_str()),
        ] {
            // 空目录名是合法的：它表示"就在项目根下"（老清单里可能出现）。
            if raw.trim().is_empty() {
                continue;
            }
            if let Err(defect) = normalize_project_relative(raw) {
                // 标签用 ASCII：报告要能被 CI 的文本匹配与 GBK 控制台一起看。
                out.push(format!(
                    "{PROJECT_FILE_NAME} -> {label} {raw} ({})",
                    defect.ascii()
                ));
            }
        }
        // `search_paths` 的条目同样要体检：它们直接参与运行时读路径解析
        // （见 [`resolve_read_path`]），越界的条目会让读取探到项目外。
        // 运行时对不合规条目是**跳过**，所以这里必须报出来 —— 否则"配了但没生效"
        // 会变成无声的行为差。
        for (index, raw) in content.search_paths.iter().enumerate() {
            if let Err(defect) = normalize_project_relative(raw) {
                out.push(format!(
                    "{PROJECT_FILE_NAME} -> content.search_paths[{index}] {raw} ({})",
                    defect.ascii()
                ));
            }
        }
        out
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
            layers: Layers::default(),
            render: RenderConfig::default(),
            quality: crate::quality::QualityConfig::default(),
            build: BuildConfig::default(),
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

    /// 项目相对路径不合法（越界 / Windows 不认的写法）。
    ///
    /// 与 [`Self::Write`] 分开：那个是"盘上出了事"，这个是"根本不该去碰盘"。
    /// 混在一起会让调用方以为是磁盘问题，跑去查权限。
    #[error("路径不合法 ({relative}): {reason}")]
    InvalidPath { relative: String, reason: String },

    #[error("不支持文件格式版本 {version} ({path})；旧格式写入已拒绝")]
    UnsupportedFileVersion { path: PathBuf, version: u32 },
}

/// 旧编辑会话保存时若文件已由外部升级，禁止覆盖其未知组件数据。
fn guard_v1_write(path: &Path, requested: u32) -> Result<(), ProjectError> {
    if requested != crate::file_format::V1 {
        return Err(ProjectError::UnsupportedFileVersion {
            path: path.to_path_buf(),
            version: requested,
        });
    }
    let disk = crate::file_format::disk_version(path).map_err(|source| ProjectError::Write {
        path: path.to_path_buf(),
        source,
    })?;
    if let Some(version) = disk.filter(|version| *version != crate::file_format::V1) {
        return Err(ProjectError::UnsupportedFileVersion {
            path: path.to_path_buf(),
            version,
        });
    }
    Ok(())
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

        let manifest: Project =
            ron::from_str(&text).map_err(|source| ProjectError::ParseManifest {
                path: manifest_path,
                source,
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
            std::fs::create_dir_all(&dir)
                .map_err(|source| ProjectError::CreateDir { path: dir, source })?;
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
    pub fn set_asset_import(&self, relative: &str, import: ImportSettings) -> Result<(), String> {
        self.assets().set_import(relative, import)
    }

    /// 只给一个资产生成 `.meta`（写盘），返回它的 GUID。
    ///
    /// 与 [`ensure_asset_metas`](Self::ensure_asset_metas) 的区别是**范围**：
    /// 这个只碰点名的那个资产。MCP `asset_import` 用它，避免"导入一个资产"
    /// 顺手给全项目所有未导入文件写身份证。
    pub fn ensure_asset_meta(&self, relative: &str) -> Result<AssetGuid, String> {
        self.assets().ensure_meta(relative)
    }

    /// 移动资产并保留 GUID（磁盘场景 / 预制体里的 `guid://` 引用无需改写）。
    pub fn move_asset(&self, from: &str, to: &str) -> Result<AssetMoveReport, String> {
        self.assets().move_asset(from, to)
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
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("无法读取 '{relative}': {e}"))?;
        let mut material =
            MaterialAsset::from_ron(&text).map_err(|e| format!("无法解析 '{relative}': {e}"))?;
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
        write_atomic(&path, &text)
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
        // 与 `load_scene` 同一条边界：预制体路径来自场景里的 `source`，
        // 手写 RON 里一句 `../..` 就能读到项目外。
        let relative = normalize_project_relative(relative).map_err(|e| e.to_string())?;
        let path = self.path(&relative);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("无法读取 '{relative}': {e}"))?;
        let mut prefab =
            Prefab::from_ron(&text).map_err(|e| format!("无法解析 '{relative}': {e}"))?;
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

    /// 显式只读 v2 预制体入口；旧的 `load_prefab` 仍拒绝 v2，避免旧编辑器丢失身份。
    pub fn load_prefab_v2(&self, relative: &str) -> Result<crate::scene_v2::PrefabV2, String> {
        let relative = normalize_project_relative(relative).map_err(|e| e.to_string())?;
        let text = std::fs::read_to_string(self.path(&relative))
            .map_err(|e| format!("无法读取 '{relative}': {e}"))?;
        let mut prefab = crate::scene_v2::PrefabV2::from_ron(&text)
            .map_err(|e| format!("无法解析 '{relative}': {e}"))?;
        crate::asset::decode_prefab_v2_references(&mut prefab, &self.assets());
        Ok(prefab)
    }

    /// 把预制体写回项目内的相对路径。
    ///
    /// 写盘前把资产引用编码成 GUID（并补齐缺 `.meta` 的资产身份证）。
    /// 这样"改了资产名再保存"就把新身份固化进文件，而不是继续存路径。
    pub fn write_prefab(&self, relative: &str, prefab: &Prefab) -> Result<(), ProjectError> {
        // 路径校验放在最前面：不合法就**不该去扫资产库**（那是几百毫秒的活）。
        let relative =
            normalize_project_relative(relative).map_err(|defect| ProjectError::InvalidPath {
                relative: relative.to_owned(),
                reason: defect.to_string(),
            })?;
        guard_v1_write(&self.path(&relative), prefab.format_version)?;
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

        let path = self.path(&relative);
        let text = prefab.to_ron().map_err(|e| ProjectError::Write {
            path: path.clone(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
        })?;
        // 与 `write_scene` 同一条：内容没变就不动盘。
        if unchanged_on_disk(&path, &text) {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ProjectError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        write_atomic(&path, &text)
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
    /// 资产字段的 `"Resources/Box.glb"` → `"guid://…"`，注释与普通字段原样保留。
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
    /// 场景 / 预制体 / 材质三个阶段共用 —— 只替换资产字段里的字符串，
    /// 避免注释或恰好同名的普通字段被误改。
    fn rewrite_literals(
        &self,
        relative: &str,
        text: String,
        mut references: Vec<String>,
    ) -> Result<bool, String> {
        let path = self.path(relative);
        let replaced = {
            let assets = self.assets();
            references.sort();
            references.dedup();
            let replacements: std::collections::BTreeMap<_, _> = references
                .into_iter()
                .filter(|r| !r.is_empty() && !r.contains("://") && !Path::new(r).is_absolute())
                .filter_map(|r| assets.guid_of(&r).map(|guid| (r, guid.to_reference())))
                .collect();
            replace_asset_literals(&text, &replacements)
        };
        let Some(replaced) = replaced else {
            return Ok(false);
        };
        // 迁移仍保留原文注释和空白；写盘复用主保存的暂存与替换，
        // 目标被占用或写入中断时不能截断原场景、预制体或材质。
        write_atomic(&path, &replaced)
            .map(|_| true)
            .map_err(|e| e.to_string())
    }

    /// 从项目内的相对路径加载场景。
    ///
    /// 磁盘上的资产引用是 GUID，这里解码成项目相对路径。悬空 GUID（资产被删）
    /// 会原样保留 `guid://…` 字符串，好让 `--check` 与 Inspector 都能指出断链。
    ///
    /// 路径先过 [`normalize_scene_relative`]：`path()` 只是 `root.join(..)`，而
    /// `join` 遇到绝对路径会**整个替换**掉根 —— 手写 / 下载来的清单里一句
    /// `/etc/passwd` 就能让引擎去读项目外的文件。
    pub fn load_scene(&self, relative: &str) -> Result<Scene, String> {
        let relative = normalize_scene_relative(relative)?;
        let path = self.path(&relative);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("无法读取 '{relative}': {e}"))?;
        // 发布前后按诊断文本取差集；解析失败必须使用项目内路径，否则同一
        // 份坏场景复制到 staging 后会被误判成构建新引入的问题。
        self.parse_scene_text(&text, Path::new(&relative))
    }

    /// 显式只读 v2 场景入口；在项目路径边界读取每份来源资产并复核组件关联。
    /// v2 的正式保存 / 运行时结构操作仍未开放，不能用 `to_v1` 的结果写回此文件。
    pub fn load_scene_v2(&self, relative: &str) -> Result<crate::scene_v2::SceneV2, String> {
        let relative = normalize_scene_relative(relative)?;
        let text = std::fs::read_to_string(self.path(&relative))
            .map_err(|e| format!("无法读取 '{relative}': {e}"))?;
        let mut scene = crate::scene_v2::SceneV2::from_ron(&text)
            .map_err(|e| format!("无法解析 '{relative}': {e}"))?;
        crate::asset::decode_scene_v2_references(&mut scene, &self.assets());
        let mut prefabs = std::collections::BTreeMap::new();
        for entity in &scene.entities {
            if let Some(reference) = &entity.prefab {
                // 路径必须先经 normalize_project_relative，不能让来源引用越出项目。
                if !prefabs.contains_key(&reference.source) {
                    let prefab = self.load_prefab_v2(&reference.source)?;
                    prefabs.insert(reference.source.clone(), prefab);
                }
            }
        }
        scene.validate_prefab_sources(&prefabs)?;
        Ok(scene)
    }

    /// 从 v1 原始文件只读预览升级结果；已升级的来源预制体沿用其真实组件 ID。
    /// 不走 `load_scene` 的自动修复 / 数值夹取路径，迁移前必须看见坏数据。
    pub fn preview_scene_v2(&self, relative: &str) -> Result<crate::scene_v2::SceneV2, String> {
        let relative = normalize_scene_relative(relative)?;
        let text = std::fs::read_to_string(self.path(&relative))
            .map_err(|e| format!("无法读取 '{relative}': {e}"))?;
        let mut scene: Scene = parse_v1_upgrade_text(&text, &relative)?;
        if !scene.duplicate_ids().is_empty() {
            return Err(format!("场景 '{relative}' 存在重复实体 ID，拒绝自动迁移"));
        }
        decode_scene_references(&mut scene, &self.assets());
        let mut prefabs = std::collections::BTreeMap::new();
        for entity in &scene.entities {
            let Some(reference) = &entity.prefab else {
                continue;
            };
            if prefabs.contains_key(&reference.source) {
                continue;
            }
            let source_path = normalize_project_relative(&reference.source)
                .map_err(|e| format!("预制体来源路径不安全: {e}"))?;
            let file = self.path(&source_path);
            let source = match crate::file_format::disk_version(&file)
                .map_err(|e| format!("无法读取预制体 '{source_path}': {e}"))?
            {
                Some(2) => self.load_prefab_v2(&source_path)?,
                Some(1) | None => {
                    let text = std::fs::read_to_string(&file)
                        .map_err(|e| format!("无法读取预制体 '{source_path}': {e}"))?;
                    let mut source: Prefab = parse_v1_upgrade_text(&text, &source_path)?;
                    if !source.duplicate_ids().is_empty() {
                        return Err(format!("预制体 '{source_path}' 存在重复实体 ID"));
                    }
                    decode_prefab_references(&mut source, &self.assets());
                    crate::scene_v2::PrefabV2::from_v1(&source)?
                }
                Some(version) => {
                    return Err(format!("预制体 '{source_path}' 版本 {version} 不支持预览"))
                }
            };
            prefabs.insert(reference.source.clone(), source);
        }
        crate::scene_v2::SceneV2::from_v1_with_prefabs(&scene, &prefabs)
    }

    /// 加载清单里指定的入口场景。
    pub fn load_entry_scene(&self) -> Result<Scene, String> {
        self.load_scene(&self.manifest.entry_scene)
    }

    /// 只读地统计某个场景文件里的重复实体 ID（**不修复、不写盘**）。
    ///
    /// `--check` 专用。`load_scene` 会在加载边界调 `ensure_unique_ids` 把重复
    /// ID 修掉，于是一切"加载后再检查"的路径都看不见这类落盘伤 —— 而它正是
    /// "删一个物体连带删别的"的成因。这里回到**原始文本**再解析一遍：只报告，
    /// 不动数据（检查必须只读，见模块文档）。
    ///
    /// 注意这是**第二次解析**同一个文件。检查是低频 CLI 命令，多一次解析换
    /// "CI 能看见文件里的病"，划算。
    pub fn scene_duplicate_ids(&self, relative: &str) -> Result<Vec<(EntityId, usize)>, String> {
        let path = self.path(relative);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("无法读取 '{}': {}", path.display(), e))?;
        let scene: Scene =
            ron::from_str(&text).map_err(|e| format!("无法解析 '{}': {:#}", path.display(), e))?;
        Ok(scene.duplicate_ids())
    }

    /// 只读地统计某个预制体文件里的重复实体 ID（**不修复、不写盘**）。
    ///
    /// `--check` 专用，与 [`Self::scene_duplicate_ids`] 对称，但**不能**像场景那样
    /// 在加载边界修：预制体的 ID 被场景里的 `PrefabInstanceRef::source_id` 跨文件
    /// 引用，重分配会让所有既有实例的 `source_id` 失配（`Apply` 时整棵树被当成新增）。
    /// 详见 [`crate::prefab::Prefab::duplicate_ids`]。
    pub fn prefab_duplicate_ids(&self, relative: &str) -> Result<Vec<(EntityId, usize)>, String> {
        let path = self.path(relative);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("无法读取 '{relative}': {e}"))?;
        // 检查报告要与发布包的同一文件逐字节可比较，原因里不能带项目绝对路径。
        let prefab = Prefab::from_ron(&text).map_err(|e| format!("无法解析 '{relative}': {e}"))?;
        Ok(prefab.duplicate_ids())
    }

    /// 把场景写回项目内的相对路径。
    ///
    /// 写盘前把资产引用编码成 GUID（并补齐缺 `.meta` 的资产身份证）——
    /// 于是"打开旧项目 + 保存一次"就完成了从路径引用到 GUID 引用的迁移，
    /// 不需要额外的迁移命令。
    ///
    /// **内容一个字节都没变时不动盘**：一次 `Ctrl+S` 不该刷新 mtime —— 编辑器
    /// 的热重载扫描按 mtime 判"外部改动"，白刷一次等于凭空触发一次重载；
    /// 对只读 / 被占用的文件，也变成"没改动就存得下去"。
    pub fn write_scene(&self, relative: &str, scene: &Scene) -> Result<(), ProjectError> {
        let relative =
            normalize_scene_relative(relative).map_err(|reason| ProjectError::InvalidPath {
                relative: relative.to_owned(),
                reason,
            })?;
        let path = self.path(&relative);
        let text = self.scene_text(scene, &path)?;
        if unchanged_on_disk(&path, &text) {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ProjectError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        write_atomic(&path, &text)
    }

    /// 把场景序列化成落盘文本（资产引用已编码成 GUID）。
    ///
    /// `write_scene` 与 `write_autosave` 共用这一份实现 —— 两条路写出来的
    /// 必须是**同一种格式**，否则"从自动保存恢复"会把 GUID 变成路径，
    /// 下一次保存再把它们编回去（中间任何一步的 diff 都会显得莫名其妙）。
    fn scene_text(&self, scene: &Scene, path: &Path) -> Result<String, ProjectError> {
        guard_v1_write(path, scene.format_version)?;
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
        let mut scene: Scene =
            ron::from_str(text).map_err(|e| format!("无法解析 '{}': {:#}", path.display(), e))?;
        // 重复 ID 是旧版本计数器不对齐留下的**落盘伤**（见 `EntityId::reserve`）。
        // 在这里修掉，否则"删一个物体连带删别的"会从文件里继续复现。
        let repaired = scene.ensure_unique_ids();
        if repaired > 0 {
            eprintln!(
                "warning: '{}' 有 {} 个重复的实体 ID，已重新分配；\
                 请保存一次场景把修复结果写回（层级可能需手工校正）",
                path.display(),
                repaired
            );
        }
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
        let text = ron::ser::to_string_pretty(&self.manifest, pretty).map_err(|e| {
            ProjectError::Write {
                path: path.clone(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
            }
        })?;
        write_atomic(&path, &text)
    }
}

// ---------------------------------------------------------------------------
// 默认场景
// ---------------------------------------------------------------------------

/// v1 普通读取保持宽容；只有不可逆的格式升级必须点名被忽略的字段。
fn parse_v1_upgrade_text<T: serde::de::DeserializeOwned>(
    text: &str,
    relative: &str,
) -> Result<T, String> {
    let mut deserializer =
        ron::de::Deserializer::from_str(text).map_err(|e| format!("无法解析 '{relative}': {e}"))?;
    let mut ignored = Vec::new();
    let value =
        serde_ignored::deserialize(&mut deserializer, |path| ignored.push(path.to_string()))
            .map_err(|e| format!("无法解析 '{relative}': {e}"))?;
    deserializer
        .end()
        .map_err(|e| format!("无法解析 '{relative}': {e}"))?;
    if !ignored.is_empty() {
        return Err(format!(
            "'{relative}' 存在无法自动升级的未知字段: {}",
            ignored.join(", ")
        ));
    }
    Ok(value)
}

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
        // 控制字符也要滤掉：`"\u{1}"` 这种名字在资源管理器里看不见、在日志里
        // 还会把终端搞乱，而它是能通过 RON 字符串存下来的。
        .filter(|c| {
            !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.').to_string();
    // 保留设备名：`CON` 这类名字在 Windows 上打开的是**设备**而不是文件，
    // 写进去的字节直接消失。这里加下划线避让 —— 用它的地方（新建项目 / 场景 /
    // 预制体 / 材质）都是"从用户输入生成文件名"，拒绝会让用户莫名其妙，
    // 而改名后仍一眼认得出原来输入的是什么。
    if is_reserved_device_name(&cleaned) {
        format!("{cleaned}_")
    } else {
        cleaned
    }
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

/// 原子写：同目录独占暂存文件，再 rename 覆盖目标。
///
/// 实现已收敛到 [`crate::fsx`]（三处副本的差错点各不相同，见该模块文档）；
/// 这里只负责把 IO 错误映射成项目错误。
pub(crate) fn write_atomic(path: &Path, text: &str) -> Result<(), ProjectError> {
    crate::fsx::write_atomic(path, text).map_err(|source| ProjectError::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// 盘上内容与待写文本**逐字节相同**时返回 `true`（调用方据此跳过写入）。
///
/// 读失败（文件不存在 / 没权限）一律返回 `false`：那种情况该不该写由后续的
/// 真实写入去报错，别在这里静默吞掉 —— 吞掉之后"保存成功但文件不存在"就成了
/// 一个新的、更难查的坑。
fn unchanged_on_disk(path: &Path, text: &str) -> bool {
    match std::fs::read(path) {
        Ok(bytes) => bytes == text.as_bytes(),
        Err(_) => false,
    }
}

/// 首次登记资产时只发布到空位；并发生成者不能覆盖对方的 GUID。
pub(crate) fn write_atomic_new(path: &Path, text: &str) -> std::io::Result<()> {
    crate::fsx::write_atomic_new(path, text)
}

/// 只替换 RON 里的字符串 token；注释、字符字面量和普通文字原样复制。
///
/// 先按场景/预制体/材质解析过一次，故这里只需扫描词法边界；用 RON 解码
/// token 再查路径，避免转义引号和反斜杠让文本匹配误伤别的字段。
fn replace_asset_literals(
    text: &str,
    replacements: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"//") {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < bytes.len() && depth > 0 {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else {
            let start = i;
            let raw = bytes[i] == b'r'
                && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
                && (i + 1 == bytes.len() || bytes[i + 1] == b'"' || bytes[i + 1] == b'#');
            if raw {
                i += 1;
                while i < bytes.len() && bytes[i] == b'#' {
                    i += 1;
                }
            }
            if i < bytes.len() && bytes[i] == b'"' {
                let hashes = if raw { i - start - 1 } else { 0 };
                i += 1;
                while i < bytes.len() {
                    if !raw && bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                    } else if bytes[i] == b'"'
                        && bytes
                            .get(i + 1..i + 1 + hashes)
                            .is_some_and(|suffix| suffix.iter().all(|b| *b == b'#'))
                    {
                        i += 1 + hashes;
                        break;
                    } else {
                        i += 1;
                    }
                }
                if let Ok(value) = ron::from_str::<String>(&text[start..i]) {
                    let before = text[..start].trim_end();
                    let asset_field = [
                        "model:",
                        "material_asset: Some(",
                        "clip:",
                        "source:",
                        "albedo:",
                    ]
                    .iter()
                    .any(|field| before.ends_with(field))
                        || before.rsplit_once("scripts:").is_some_and(|(_, tail)| {
                            tail.trim_start().starts_with('[') && !tail.contains(']')
                        });
                    if asset_field {
                        if let Some(guid) = replacements.get(&value) {
                            out.push_str(&text[copied..start]);
                            out.push_str(&format!("\"{guid}\""));
                            copied = i;
                        }
                    }
                }
            } else if bytes[start] == b'\'' {
                // 字符字面量内的引号不是字符串起点。
                i = start + 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                    } else if bytes[i] == b'\'' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            } else {
                i = start + 1;
            }
        }
    }
    if copied == 0 {
        None
    } else {
        out.push_str(&text[copied..]);
        Some(out)
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
fn collect_by_extension(
    scan_dir: &Path,
    project_root: &Path,
    extension: &str,
    out: &mut Vec<String>,
) {
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
    #[cfg(windows)]
    #[test]
    fn atomic_replace_failure_preserves_old_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let (base, handle) = temp_project("atomic_replace");
        let path = handle.path("Scenes/locked.ron");
        std::fs::write(&path, "旧内容").unwrap();
        // 允许读写但不允许删除/替换：旧实现会偷偷退回覆盖写。
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&path)
            .unwrap();
        let result = super::write_atomic(&path, "新内容");
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "旧内容");
        assert!(!path.with_extension("ron.tmp").exists());
        drop(lock);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn config_and_material_saves_preserve_locked_targets() {
        use std::os::windows::fs::OpenOptionsExt;
        let (base, mut handle) = temp_project("atomic_config_material");
        let material_path = "Resources/locked.mat";
        handle
            .write_material(material_path, &MaterialAsset::default())
            .unwrap();
        let mut results = Vec::new();
        for relative in [PROJECT_FILE_NAME, material_path] {
            let path = handle.path(relative);
            let before = std::fs::read(&path).unwrap();
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(3)
                .open(&path)
                .unwrap();
            let result = if relative == PROJECT_FILE_NAME {
                handle.manifest.name = "更新后的项目".into();
                handle.save()
            } else {
                let material = MaterialAsset {
                    metallic: 0.73,
                    ..Default::default()
                };
                handle.write_material(relative, &material)
            };
            results.push((
                relative,
                result.is_err(),
                std::fs::read(&path).unwrap() == before,
            ));
            drop(lock);
        }
        std::fs::remove_dir_all(base).unwrap();
        for (relative, rejected, preserved) in results {
            assert!(
                rejected && preserved,
                "{relative}: 必须拒绝替换并保留原字节，拒绝={rejected}, 保留={preserved}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn migration_locked_target_preserves_original_and_reports_error() {
        use std::os::windows::fs::OpenOptionsExt;
        let (base, handle) = temp_project("migration_lock");
        let asset = "Resources/Box.glb";
        std::fs::write(handle.path(asset), b"glb").unwrap();
        handle.ensure_asset_metas().unwrap();
        let scene = "Scenes/locked.ron";
        let original =
            format!("(name: \"Legacy\", entities: [(name: \"Box\", model: \"{asset}\")])\n");
        let path = handle.path(scene);
        std::fs::write(&path, &original).unwrap();
        let legacy_tmp = path.with_extension("ron.tmp");
        std::fs::write(&legacy_tmp, b"earlier interrupted migration").unwrap();
        // 允许文件内容读写，却拒绝删除/替换；旧的直接 write 会悄悄成功。
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&path)
            .unwrap();
        let report = handle.migrate_asset_references();
        assert!(
            report.errors.iter().any(|e| e.contains(scene)),
            "{report:?}"
        );
        assert_eq!(report.rewritten_scenes, 0);
        assert_eq!(std::fs::read(&path).unwrap(), original.as_bytes());
        assert_eq!(
            std::fs::read(&legacy_tmp).unwrap(),
            b"earlier interrupted migration"
        );
        drop(lock);
        let report = handle.migrate_asset_references();
        assert!(!report.has_errors(), "{report:?}");
        assert_eq!(report.rewritten_scenes, 1);
        assert!(std::fs::read_to_string(&path).unwrap().contains("guid://"));
        std::fs::remove_dir_all(base).unwrap();
    }

    /// 把文件的修改时间挪到过去：模拟"上一次保存崩在中途留下的暂存"。
    fn age_file(path: &std::path::Path, ago: std::time::Duration) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        let when = std::time::SystemTime::now() - ago;
        file.set_times(
            std::fs::FileTimes::new()
                .set_modified(when)
                .set_accessed(when),
        )
        .unwrap();
    }

    /// 崩溃留下的暂存文件必须被后续保存回收，否则项目目录会一直攒垃圾
    /// （旧实现只保证"不阻断保存"，从不清理）。
    #[test]
    fn main_save_reclaims_abandoned_staging() {
        let (base, handle) = temp_project("staging_gc");
        let scene_path = "Scenes/gc.ron";
        handle
            .write_scene(scene_path, &crate::Scene::new("旧场景"))
            .unwrap();
        let path = handle.path(scene_path);

        let abandoned = path.with_extension(format!("ron.tmp.{}.7", std::process::id()));
        std::fs::write(&abandoned, b"abandoned").unwrap();
        age_file(&abandoned, std::time::Duration::from_secs(3600));
        // 另一份刚写下的暂存可能是别的进程正在用的，不能动。
        let fresh = path.with_extension(format!("ron.tmp.{}.8", std::process::id()));
        std::fs::write(&fresh, b"fresh").unwrap();
        // 用户自己的同名风格文件（缺少两段数字）也不能被当成垃圾。
        let user_file = path.with_extension("ron.tmp");
        std::fs::write(&user_file, b"user").unwrap();
        age_file(&user_file, std::time::Duration::from_secs(3600));

        handle
            .write_scene(scene_path, &crate::Scene::new("新场景"))
            .unwrap();

        assert!(!abandoned.exists(), "超龄暂存应被回收");
        assert!(fresh.exists(), "新鲜暂存可能属于并发写入者，不能回收");
        assert!(user_file.exists(), "不匹配暂存命名的文件不能动");
        assert_eq!(handle.load_scene(scene_path).unwrap().name, "新场景");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn stale_staging_file_does_not_block_main_saves() {
        let (base, mut handle) = temp_project("stale_staging");
        let scene_path = "Scenes/stale.ron";
        let prefab_path = "Prefabs/stale.prefab.ron";
        let material_path = "Resources/stale.mat";
        handle
            .write_scene(scene_path, &crate::Scene::new("旧场景"))
            .unwrap();
        handle
            .write_prefab(prefab_path, &crate::Prefab::new("旧模板"))
            .unwrap();
        handle
            .write_material(material_path, &MaterialAsset::default())
            .unwrap();
        for relative in [PROJECT_FILE_NAME, scene_path, prefab_path, material_path] {
            let path = handle.path(relative);
            let stale = path.with_extension(format!(
                "{}.tmp",
                path.extension().unwrap().to_string_lossy()
            ));
            std::fs::write(&stale, b"unfinished previous save").unwrap();
            let result = match relative {
                PROJECT_FILE_NAME => {
                    handle.manifest.name = "新项目".into();
                    handle.save()
                }
                p if p == scene_path => handle.write_scene(p, &crate::Scene::new("新场景")),
                p if p == prefab_path => handle.write_prefab(p, &crate::Prefab::new("新模板")),
                p => handle.write_material(
                    p,
                    &MaterialAsset {
                        metallic: 0.75,
                        ..Default::default()
                    },
                ),
            };
            result.unwrap();
            assert_eq!(std::fs::read(&stale).unwrap(), b"unfinished previous save");
        }
        assert_eq!(
            ProjectHandle::open(handle.root()).unwrap().manifest.name,
            "新项目"
        );
        assert_eq!(handle.load_scene(scene_path).unwrap().name, "新场景");
        assert_eq!(handle.load_prefab(prefab_path).unwrap().name, "新模板");
        assert_eq!(handle.load_material(material_path).unwrap().metallic, 0.75);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn legacy_staging_directory_does_not_block_main_saves() {
        let (base, handle) = temp_project("atomic_main");
        let scene_path = "Scenes/atomic.ron";
        let prefab_path = "Prefabs/atomic.prefab.ron";
        let scene = crate::Scene::new("旧场景");
        let prefab = crate::Prefab::new("旧模板");
        handle.write_scene(scene_path, &scene).unwrap();
        handle.write_prefab(prefab_path, &prefab).unwrap();
        for relative in [scene_path, prefab_path] {
            let path = handle.path(relative);
            let before = std::fs::read(&path).unwrap();
            let tmp = path.with_extension("ron.tmp");
            std::fs::create_dir(&tmp).unwrap();
            let result = if relative == scene_path {
                handle.write_scene(relative, &crate::Scene::new("新场景"))
            } else {
                handle.write_prefab(relative, &crate::Prefab::new("新模板"))
            };
            assert!(result.is_ok(), "旧版暂存目录不得阻断新保存: {result:?}");
            assert_ne!(std::fs::read(&path).unwrap(), before);
            std::fs::remove_dir(tmp).unwrap();
        }
        assert_eq!(handle.load_scene(scene_path).unwrap().name, "新场景");
        assert_eq!(handle.load_prefab(prefab_path).unwrap().name, "新模板");
        std::fs::remove_dir_all(base).unwrap();
    }

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
            vec!["Scripts/a.lua".to_owned(), "Scripts/sub/b.lua".to_owned(),],
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
            assert!(
                text.contains(needle),
                ".gitignore 应当忽略 {needle}，实际：\n{text}"
            );
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

        assert!(
            handle.remove_autosave(DEFAULT_ENTRY_SCENE),
            "第一次删除应成功"
        );
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
        let text = std::fs::read_to_string(handle.autosave_path(DEFAULT_ENTRY_SCENE)).unwrap();
        assert!(text.contains("guid://"), "副本里应当是 GUID 引用：\n{text}");
        assert!(
            !text.contains("Resources/Box.glb"),
            "副本里不该出现资产路径字面量：\n{text}"
        );

        // 读回来是**路径**（内存态）—— 与 `load_scene` 同一条边界。
        let back = handle.read_autosave(DEFAULT_ENTRY_SCENE).expect("读回副本");
        assert!(back.entities.iter().any(|e| e.model == "Resources/Box.glb"));

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

    /// 旧读写器遇到新版文件必须明确报错；否则未知 components 会被 serde 丢掉。
    #[test]
    fn future_scene_and_prefab_versions_cannot_be_loaded_or_overwritten() {
        let (base, handle) = temp_project("future_component_format");
        let scene_path = "Scenes/future.ron";
        let prefab_path = "Prefabs/future.prefab.ron";
        let scene =
            "(format_version: 2, name: \"Future\", entities: [(name: \"Actor\", components: [])])";
        let prefab =
            "(format_version: 2, name: \"Future\", entities: [(name: \"Actor\", components: [])])";
        std::fs::create_dir_all(handle.path("Prefabs")).unwrap();
        std::fs::write(handle.path(scene_path), scene).unwrap();
        std::fs::write(handle.path(prefab_path), prefab).unwrap();

        assert!(ron::from_str::<crate::Scene>(scene).is_err());
        assert!(crate::Prefab::from_ron(prefab).is_err());
        assert!(handle.load_scene(scene_path).is_err());
        assert!(handle.load_prefab(prefab_path).is_err());
        assert!(handle.scene_duplicate_ids(scene_path).is_err());
        assert!(handle.prefab_duplicate_ids(prefab_path).is_err());
        assert!(handle
            .write_scene(scene_path, &crate::Scene::new("Old"))
            .is_err());
        assert!(handle
            .write_prefab(prefab_path, &crate::Prefab::new("Old"))
            .is_err());
        assert_eq!(
            std::fs::read_to_string(handle.path(scene_path)).unwrap(),
            scene
        );
        assert_eq!(
            std::fs::read_to_string(handle.path(prefab_path)).unwrap(),
            prefab
        );

        let mut invalid = crate::Scene::new("Bad");
        invalid.format_version = 3;
        assert!(handle.write_scene("Scenes/new.ron", &invalid).is_err());
        assert!(!handle.path("Scenes/new.ron").exists());
        assert_eq!(
            ron::from_str::<crate::Scene>("(name: \"Old\", entities: [])")
                .unwrap()
                .format_version,
            1
        );

        let _ = std::fs::remove_dir_all(base);
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

        let material_text =
            std::fs::read_to_string(handle.path("Resources/Materials/Legacy.mat")).unwrap();
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
    fn migration_preserves_comments_and_unrelated_strings() {
        let (base, handle) = temp_project("migration_comments");
        let asset = "Resources/Box.glb";
        std::fs::write(handle.path(asset), b"glb").unwrap();
        let original = format!(
            "// model: \"{asset}\" (documentation)\n/* \"{asset}\" */\n(\n    name: \"{asset}\",\n    entities: [(name: \"Box\", model: \"{asset}\")],\n)\n"
        );
        let path = handle.path("Scenes/comments.ron");
        std::fs::write(&path, &original).unwrap();

        let report = handle.migrate_asset_references();
        assert!(!report.has_errors(), "{report:?}");
        assert_eq!(report.rewritten_scenes, 1);
        let changed = std::fs::read_to_string(&path).unwrap();
        let guid = handle.assets().guid_of(asset).unwrap().to_reference();
        assert_eq!(
            changed,
            format!(
                "// model: \"{asset}\" (documentation)\n/* \"{asset}\" */\n(\n    name: \"{asset}\",\n    entities: [(name: \"Box\", model: \"{guid}\")],\n)\n"
            )
        );
        assert_eq!(
            handle.load_scene("Scenes/comments.ron").unwrap().entities[0].model,
            asset
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn migration_scanner_respects_nested_comments_and_escaped_strings() {
        let asset = "Resources/Box.glb".to_owned();
        let guid = "guid://test".to_owned();
        let replacements = std::collections::BTreeMap::from([(asset.clone(), guid.clone())]);
        let text = r###"(/* outer /* model: "Resources/Box.glb" */ */
            name: "Resources/Box.glb", // model: "Resources/Box.glb"
            scripts: ["Resources/Box.glb"],
            model: "Resources/Box.glb",
            note: "escaped quote: \"Resources/Box.glb\"",
            other: r#"Resources/Box.glb"#,
        )"###;
        let changed = replace_asset_literals(text, &replacements).unwrap();
        assert!(changed.contains("/* outer /* model: \"Resources/Box.glb\" */ */"));
        assert!(changed.contains("name: \"Resources/Box.glb\""));
        assert!(changed.contains("// model: \"Resources/Box.glb\""));
        assert!(changed.contains("scripts: [\"guid://test\"]"));
        assert!(changed.contains("model: \"guid://test\""));
        assert!(changed.contains("note: \"escaped quote: \\\"Resources/Box.glb\\\"\""));
        assert!(changed.contains("other: r#\"Resources/Box.glb\"#"));
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
        assert_eq!(
            listed,
            vec!["Prefabs/Enemies/Goblin.prefab.ron".to_string()]
        );
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

    /// 老清单没有 `quality` 节 → "今天的行为"（适配器最大采样 / 2048 / 1.0）。
    #[test]
    fn old_manifest_without_quality_uses_pre_quality_behavior() {
        let ron = r#"(format_version: 1, name: "Old")"#;
        let project: Project = ron::from_str(ron).expect("老清单应当能反序列化");
        assert!(project.quality.is_empty(), "没有 quality 节 = 不存在");
        let settings = project.quality.resolve(4);
        assert_eq!(settings.msaa, 4);
        assert_eq!(settings.shadow_resolution, crate::DEFAULT_SHADOW_RESOLUTION);
        assert_eq!(settings.lod_scale, 1.0);
    }

    /// `quality` 节写盘 → 读回一致；全空时**不写这一节**（老项目逐字节不变）。
    #[test]
    fn quality_survives_manifest_roundtrip() {
        let mut project = Project::new("Quality");
        let text = ron::to_string(&project).expect("序列化");
        assert!(
            !text.contains("quality"),
            "全空的 quality 节不该落盘: {text}"
        );

        project.quality = crate::quality::QualityConfig {
            preset: Some(crate::quality::QualityPreset::Low),
            msaa: None,
            shadow_resolution: Some(2048),
            lod_scale: None,
        };
        let text = ron::to_string(&project).expect("序列化");
        assert!(text.contains("quality"), "配了就必须落盘");
        let back: Project = ron::from_str(&text).expect("反序列化");
        assert_eq!(back.quality, project.quality);
    }

    // -- 发布清单（build 节）------------------------------------------------

    /// 未配置清单 = 空顺序（**不**退回 `[entry_scene]`）。
    ///
    /// 这条是"单场景项目"的语义：老 `project.ron` 没有 `build` 节，反序列化后
    /// `scenes` 为空 ⇒ 脚本 `scene_index()` 是 `nil`。若退回 `[entry]`，
    /// 单场景项目会拿到 `0`，作者以为能 `load_scene_at(1)`，实际必然越界。
    #[test]
    fn scene_order_is_empty_when_unconfigured() {
        let mut project = Project::new("Demo");
        project.entry_scene = "Scenes/Main.ron".into();
        assert!(project.build.scenes.is_empty(), "默认不写清单");
        assert!(project.scene_order().is_empty());
        assert!(!project
            .scene_order()
            .contains(&"Scenes/Main.ron".to_string()));

        // 只有空白项也算"没配"。
        project.build.scenes = vec!["".into(), "   ".into()];
        assert!(project.scene_order().is_empty());
    }

    /// 顺序即索引：规范化不能改变相对次序，重复项只留第一次出现。
    #[test]
    fn scene_order_normalizes_and_keeps_first_occurrence() {
        let mut project = Project::new("Demo");
        project.build.scenes = vec![
            " Scenes\\Menu.ron ".into(),
            "Scenes/Level1.ron".into(),
            "Scenes/Menu.ron".into(), // 重复（规范化后与第一项相同）
            "".into(),                // 空串
            "Scenes/Level2.ron".into(),
        ];
        assert_eq!(
            project.scene_order(),
            vec![
                "Scenes/Menu.ron".to_string(),
                "Scenes/Level1.ron".to_string(),
                "Scenes/Level2.ron".to_string(),
            ]
        );
        assert!(project
            .scene_order()
            .contains(&"Scenes/Level1.ron".to_string()));
        assert!(!project
            .scene_order()
            .contains(&"Scenes/Level9.ron".to_string()));
    }

    /// 清单是后加字段：旧清单（没有 `build` 节）必须能反序列化，
    /// 且版本号有默认值（否则构建报告 / 启动日志里会出现空版本）。
    #[test]
    fn build_section_defaults_for_legacy_manifest() {
        let legacy = r#"(
            format_version: 1,
            name: "Old",
            entry_scene: "Scenes/scene.ron",
        )"#;
        let project: Project = ron::from_str(legacy).expect("旧清单必须能读");
        assert_eq!(project.build.version, "0.1.0");
        assert!(project.build.scenes.is_empty());
        assert!(project.scene_order().is_empty(), "旧清单 = 没有关卡顺序");

        // 序列化会写出 build 节（新项目 / 保存过的项目都带上默认值）。
        let text = ron::to_string(&project).expect("序列化");
        assert!(text.contains("build"), "build 节必须落盘: {text}");
    }

    /// 路径规则：越界 / 非场景文件一律拒绝（发布清单与编辑器共用同一份）。
    #[test]
    fn normalize_scene_relative_rejects_escapes() {
        assert_eq!(
            normalize_scene_relative("Scenes\\Level2.ron").unwrap(),
            "Scenes/Level2.ron"
        );
        for bad in [
            "",
            "  ",
            "/etc/passwd.ron",
            "C:////Scenes////A.ron",
            "../Outside.ron",
            "Scenes/../../Escape.ron",
            "Scenes/A.txt",
            "Prefabs/Thing.prefab.ron",
        ] {
            assert!(
                normalize_scene_relative(bad).is_err(),
                "{bad:?} 不该被当成场景路径"
            );
        }
    }

    /// 开头的 `./` 吃掉；夹在中间的 `.` 由 `Path::components` 自动消掉。
    ///
    /// 这条规则存在的理由：MCP 的 `scene` 参数此前自己写了一份更松的检查，
    /// `./Scenes/a.ron` 被原样传下去、`a.prefab.ron` 也被当成场景。两处规则
    /// 一松一紧，就是"编辑器不认的路径，智能体能写"。
    #[test]
    fn normalize_scene_relative_strips_leading_dot_slash() {
        assert_eq!(
            normalize_scene_relative("./Scenes/a.ron").unwrap(),
            "Scenes/a.ron"
        );
        assert_eq!(
            normalize_scene_relative("././Scenes/a.ron").unwrap(),
            "Scenes/a.ron"
        );
        assert_eq!(
            normalize_scene_relative("  .\\Scenes\\a.ron  ").unwrap(),
            "Scenes/a.ron"
        );
        // 中间的 `.` 不是非法，`Path::components` 已经规范化掉了。
        assert_eq!(
            normalize_scene_relative("Scenes/./a.ron").unwrap(),
            "Scenes/a.ron"
        );
        // 指向父目录的仍然拒绝（包括藏在 `./` 后面的）。
        assert!(normalize_scene_relative("./../Escape.ron").is_err());
        // 只有 `.` 也不是场景。
        assert!(normalize_scene_relative("./").is_err());
    }

    /// 扩展名大小写：`.RON` 不认（编辑器写出来的一律小写，大小写差异属于配置错误）。
    #[test]
    fn normalize_scene_relative_is_case_sensitive_on_extension() {
        assert!(normalize_scene_relative("Scenes/A.RON").is_err());
        assert!(normalize_scene_relative("Scenes/A.ron").is_ok());
    }

    /// Windows 会**静默改写**这些名字，于是"清单里写的字符串"和"盘上那个文件"
    /// 不是同一个 —— 保存看起来成功、下次打开却找不到，或者 git 根本存不下。
    ///
    /// 这一条此前只挡越界（`..` / 绝对路径），非法字符全靠下游各自手写；
    /// 资产侧早就挡了，场景侧没有 —— 于是 `Scenes/CON.ron` 能被写进发布清单。
    #[test]
    fn normalize_scene_relative_rejects_windows_hostile_names() {
        for bad in [
            // NTFS 备用数据流：写进去的内容跑到 `Level` 的 ADS 上，git 存不下来。
            "Scenes/Level:1.ron",
            // 组件结尾的空格 / 点：Win32 会静默吃掉，落盘路径与清单字符串不一致。
            "Scenes/Level /A.ron",
            "Scenes/Level./A.ron",
            // 控制字符与 Windows 非法字符（含 `?` / `*`，它们在 NTFS 上直接创建失败）。
            "Scenes/a\tb.ron",
            "Scenes/a\u{7}b.ron",
            "Scenes/a<b.ron",
            "Scenes/a>b.ron",
            "Scenes/a|b.ron",
            "Scenes/a?b.ron",
            "Scenes/a*b.ron",
            // 保留设备名：`File::create("CON.ron")` 打开的是控制台，写进去的数据消失。
            "Scenes/CON.ron",
            "Scenes/nul.ron",
            "Scenes/Aux.ron",
            "Scenes/com1.ron",
            "Scenes/LPT9.ron",
        ] {
            assert!(
                normalize_scene_relative(bad).is_err(),
                "{bad:?} 不该被当成合法场景路径"
            );
        }
        // 负对照：名字里带空格 / 中文 / 保留字的**前缀**都照常放行
        // （`console` 不是 `CON`，别把正常的词误杀）。
        for ok in [
            "Scenes/Level 2.ron",
            "Scenes/第二关.ron",
            "Scenes/a-b_c.ron",
            "Scenes/console.ron",
            "Scenes/connection.ron",
        ] {
            assert!(normalize_scene_relative(ok).is_ok(), "{ok:?} 应该被接受");
        }
    }

    /// 写盘边界同样要拦：路径不合法时**不产生任何文件**，而不是交给操作系统
    /// 去静默改写（写到 ADS / 另一个目录名下，用户完全看不见）。
    ///
    /// 两条都是**今天真的会静默成功**的写法 —— 不是"理论上可能"：
    /// - `Scenes/Level:1.ron` → NTFS 备用数据流，内容挂在 `Scenes/Level` 的 ADS 上。
    ///   目录扫描看不见它（枚举只给出基名 `Level`），git 也存不下流内容 ——
    ///   保存"成功"、场景从此只在这台机器上存在。
    /// - `Scenes/sub /x.ron` → Win32 静默吃掉目录名结尾的空格，文件落进 `Scenes/sub`。
    #[test]
    fn write_scene_rejects_hostile_paths() {
        let base = std::env::temp_dir().join(format!(
            "xgame-scene-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let project = ProjectHandle::create(&base, "PathGate").expect("建项目");

        let err = project
            .write_scene("Scenes/Level:1.ron", &default_scene())
            .expect_err("备用数据流写法必须被拒绝");
        assert!(
            err.to_string().contains("Level:1.ron"),
            "错误信息要点名是哪个路径: {err}"
        );
        assert!(
            !project.path("Scenes/Level").exists(),
            "拒绝之后不能留下 ADS 宿主文件"
        );

        let err = project
            .write_scene("Scenes/sub /x.ron", &default_scene())
            .expect_err("目录名结尾空格必须被拒绝");
        assert!(
            err.to_string().contains("sub "),
            "错误信息要点名是哪个路径: {err}"
        );
        assert!(
            !project.path("Scenes/sub").exists(),
            "拒绝之后不能悄悄把目录建在别的名字下"
        );

        // 负对照：同样的字只是不在结尾，就完全合法。
        project
            .write_scene("Scenes/Level 2.ron", &default_scene())
            .expect("正常名字必须能写");

        let _ = std::fs::remove_dir_all(&base);
    }
}
