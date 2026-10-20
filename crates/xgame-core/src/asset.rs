//! 资产身份（GUID）与资产数据库。
//!
//! # 为什么必须从"路径引用"升级到"GUID 引用"
//!
//! 资产管线是唯一的 P0 数据债（见 `Docs/差距分析.md` 第四节）：此前场景里
//! `model: "Resources/Box.glb"` 存的是路径，**资产一改名 / 一挪目录就断链**，
//! 只能靠 `--check` 事后发现，再一个个手改场景。等场景与预制体多起来，
//! 改引用格式等于重写全部资产文件与所有外部工具（MCP / 脚本 / 编辑器面板）。
//! 所以引用格式必须现在就收敛到稳定身份。
//!
//! # 核心设计：**磁盘存 GUID，内存存路径**
//!
//! 这是本模块最重要的一条取舍，理由有三：
//!
//! 1. **消费点零改动**。渲染、物理、音频、脚本、UI 全都按路径打开文件；
//!    如果内存里也换成 GUID，等于把"GUID → 路径"的解析插进每一处热路径，
//!    还要把 243 个调用点全部重写。这里让 [`ProjectHandle`] 在**加载/保存
//!    的边界**做一次转换，内存态永远是可直接使用的项目相对路径。
//! 2. **人类与智能体都还能读**。场景文件是纯文本、可 diff 的 RON（这是本仓库
//!    刻意保留的优势），而 `.meta` 里 `guid + 路径` 的对应关系随时可查。
//! 3. **迁移是隐式的**。读到路径引用 → 查库补 GUID → 存盘自动写 GUID。
//!    不需要单独的迁移命令，老项目打开 + 保存一次就完成了迁移。
//!
//! 代价是"资产被删掉后内存里会残留一个 `guid://…` 字符串"——这恰好是想要的：
//! `--check` 能把它点名（悬空 GUID），而不是静默当成空引用。
//!
//! # `.meta` 文件
//!
//! 每个资产旁边一份 `<文件名>.meta`（RON 纯文本）：
//!
//! ```ron
//! (guid: "8f3a...", kind: Model, import: (model_scale: 2.0))
//! ```
//!
//! 它跟着资产文件一起改名 / 移动，所以场景里的 GUID 永远能找到新家 ——
//! 这正是 Unity `.meta` 的做法。GUID 只在首次导入时生成一次，之后永不变。
//!
//! # 与 `AssetId` 的关系
//!
//! [`AssetId`] 是早期的路径句柄（保留是为了不破坏已有 API），新代码请用
//! [`AssetGuid`] + [`AssetDatabase`]。

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::prefab::PREFAB_EXTENSION;
use crate::scene::{Entity, Scene};

/// 磁盘上 GUID 引用的前缀。
///
/// 选带 scheme 的形式而不是裸十六进制：解析时一眼能分辨"这是 GUID 引用"还是
/// "这是路径"，而且永远不会与项目里的相对路径撞车（项目相对路径不以 `xxx://` 开头）。
/// 同时它和已有的 `builtin://cube` 是同一套写法，用户不会觉得多了一套规则。
pub const GUID_SCHEME: &str = "guid://";

/// `.meta` 文件的后缀（挂在资产文件后面：`Box.glb.meta`）。
pub const META_EXTENSION: &str = "meta";

/// 扫描项目时跳过的目录名（构建产物 / 版本库元数据 / 编辑器缓存）。
///
/// 这些目录里的文件不是资产：`target/` 是编译缓存，`dist/` 是发布产物，
/// `Plugins/` 是运行时加载的临时副本。扫进去会让资产库又慢又吵
/// （每个 `.rlib` 都想成为资产）。
const SKIPPED_DIRS: &[&str] = &[
    "target",
    "dist",
    ".git",
    ".svn",
    ".hg",
    ".workbuddy",
    ".vs",
    ".idea",
    "node_modules",
    "Plugins",
    "bin",
    "obj",
];

// ---------------------------------------------------------------------------
// GUID
// ---------------------------------------------------------------------------

/// 资产稳定标识：32 位小写十六进制（128 位）。
///
/// 用字符串而不是 `u128` 存：`.meta` 与场景文件里的写法必须一模一样，
/// 格式化/解析只有一处实现，不会出现"存的和查的不一致"。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetGuid(String);

impl AssetGuid {
    /// 解析一个 GUID 字面量（32 位十六进制，大小写不敏感）。
    ///
    /// 只接受 **32 位**：短 GUID 会让"前缀匹配"这种错误写法有可能通过，
    /// 而 GUID 的全部价值就在于精确。
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() != 32 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some(Self(text.to_ascii_lowercase()))
    }

    /// 生成一个新的 GUID。
    ///
    /// 不做密码学强度的随机：资产 GUID 只需要"在一台机器的一个项目里不撞"。
    /// 熵源是 `RandomState`（进程启动时由 OS 随机初始化）+ 纳秒时间 + 原子序号 +
    /// 进程号，任何一项变化都会改变结果；就算同一纳秒里连续生成两个，序号也不同。
    ///
    /// 为什么不用 `uuid` crate：为一个 20 行的生成器引一个新依赖不划算，
    /// 而且这层是 core 的最底层，依赖越少越好。
    pub fn generate() -> Self {
        use std::collections::hash_map::RandomState;
        use std::hash::BuildHasher;
        use std::sync::atomic::{AtomicU64, Ordering};

        static SEQ: AtomicU64 = AtomicU64::new(0);

        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();

        // `hash_one` = 建 hasher + 喂一个值 + finish（clippy 也要求这么写）。
        let high = RandomState::new().hash_one((seq, now, pid));
        let low = RandomState::new().hash_one((high, now, pid, seq));

        Self(format!("{high:016x}{low:016x}"))
    }

    /// 字面量。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 场景 / 预制体里写的那种引用形式（`guid://…`）。
    pub fn to_reference(&self) -> String {
        format!("{GUID_SCHEME}{}", self.0)
    }
}

impl std::fmt::Debug for AssetGuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AssetGuid({})", self.0)
    }
}

impl std::fmt::Display for AssetGuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for AssetGuid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for AssetGuid {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct GuidVisitor;

        impl Visitor<'_> for GuidVisitor {
            type Value = AssetGuid;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("32 位十六进制资产 GUID")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                AssetGuid::parse(value).ok_or_else(|| E::custom(format!("非法资产 GUID: {value}")))
            }
        }

        deserializer.deserialize_str(GuidVisitor)
    }
}

// ---------------------------------------------------------------------------
// 资产类型与导入设置
// ---------------------------------------------------------------------------

/// 资产大类。
///
/// 变体名即 `.meta` 里的磁盘字面量（`kind: Model`）——改名会让所有旧 `.meta`
/// 解析失败，所以变体名是契约（有测试钉住）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AssetKind {
    /// glTF 模型（`.glb` / `.gltf`）。
    Model,
    /// 纹理（`.png` / `.jpg` / `.ktx2` / `.hdr` …）。
    Texture,
    /// 音频（`.wav` / `.ogg` / `.mp3` / `.flac`）。
    Audio,
    /// Lua 脚本（`.lua`）。
    Script,
    /// 预制体（`.prefab.ron`）。
    Prefab,
    /// 场景（`.ron`，但排除 `project.ron`）。
    Scene,
    /// WGSL 着色器（`.wgsl`）。
    Shader,
    /// 其它被登记但引擎暂不特殊处理的文件。
    #[default]
    Other,
}

impl AssetKind {
    /// 由项目相对路径推断类型。
    ///
    /// 判定顺序有讲究：`.prefab.ron` 必须先于 `.ron` 判断，否则预制体会被
    /// 当成场景；`project.ron` 是清单不是资产，返回 `None`。
    pub fn from_path(relative: &str) -> Option<Self> {
        let file = relative.rsplit('/').next().unwrap_or(relative);
        let lower = file.to_ascii_lowercase();

        if lower == "project.ron" {
            return None;
        }
        if lower.ends_with(&format!(".{PREFAB_EXTENSION}")) {
            return Some(Self::Prefab);
        }

        let ext = lower.rsplit_once('.').map(|(_, e)| e)?;
        Some(match ext {
            "glb" | "gltf" => Self::Model,
            "png" | "jpg" | "jpeg" | "ktx2" | "hdr" | "exr" | "tga" | "dds" | "bmp" => {
                Self::Texture
            }
            "wav" | "ogg" | "mp3" | "flac" => Self::Audio,
            "lua" => Self::Script,
            "ron" => Self::Scene,
            "wgsl" => Self::Shader,
            _ => return None,
        })
    }

    /// 给 UI 用的短标签。
    pub fn label(self) -> &'static str {
        match self {
            Self::Model => "模型",
            Self::Texture => "纹理",
            Self::Audio => "音频",
            Self::Script => "脚本",
            Self::Prefab => "预制体",
            Self::Scene => "场景",
            Self::Shader => "着色器",
            Self::Other => "其它",
        }
    }

    /// 是否属于"可拖进场景生成实体"的资产。
    pub fn is_draggable_into_scene(self) -> bool {
        matches!(self, Self::Model | Self::Prefab)
    }
}

/// 每个资产的导入参数。
///
/// 这是"导入管线"的落点：同一条 `.meta` 记着"这个资产怎么进项目"。
/// 字段一律后加友好（`serde(default)`），旧的 `.meta` 缺哪个补哪个。
///
/// 现阶段真正被消费的是 `model_scale`（加载模型时乘在根变换上）与
/// `texture_srgb` / `texture_nearest`（纹理采样）。`audio_streaming` 先落盘，
/// 等流式长音频落地后接上 —— 提前存字段是为了避免以后再加一次格式迁移。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ImportSettings {
    /// 模型缩放（仅 `AssetKind::Model` 有效）。1.0 = 原样。
    #[serde(default = "one")]
    pub model_scale: f32,
    /// 纹理是否按 sRGB 采样（颜色贴图 = true，法线 / 粗糙度贴图 = false）。
    #[serde(default = "yes")]
    pub texture_srgb: bool,
    /// 纹理是否用最近邻过滤（像素风资源要开）。
    #[serde(default)]
    pub texture_nearest: bool,
    /// 音频是否流式播放（长 BGM 用，避免整段解码进内存）。
    #[serde(default)]
    pub audio_streaming: bool,
}

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

impl Default for ImportSettings {
    fn default() -> Self {
        Self {
            model_scale: 1.0,
            texture_srgb: true,
            texture_nearest: false,
            audio_streaming: false,
        }
    }
}

impl ImportSettings {
    /// 全默认时不落盘（`.meta` 保持干净，diff 里只出现真正调过的参数）。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

// ---------------------------------------------------------------------------
// .meta
// ---------------------------------------------------------------------------

/// 一个资产的身份证 + 导入设置（磁盘文件 `<资产>.meta`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetMeta {
    /// 稳定标识。
    pub guid: AssetGuid,
    /// 资产大类（冗余信息，便于人读与排查；判定以真实扩展名为准）。
    #[serde(default)]
    pub kind: AssetKind,
    /// 导入设置。
    #[serde(default, skip_serializing_if = "ImportSettings::is_default")]
    pub import: ImportSettings,
}

impl AssetMeta {
    /// 为某类资产生成一份**默认**身份证（GUID 现场生成）。
    pub fn fresh(kind: AssetKind) -> Self {
        Self {
            guid: AssetGuid::generate(),
            kind,
            import: ImportSettings::default(),
        }
    }

    /// 解析 `.meta` 文本。
    pub fn from_ron(text: &str) -> Result<Self, String> {
        ron::from_str(text).map_err(|e| format!("{e}"))
    }

    /// 序列化为 `.meta` 文本（带换行，方便 diff 与手改）。
    pub fn to_ron(&self) -> Result<String, String> {
        let mut text =
            ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).map_err(|e| {
                format!("{e}")
            })?;
        text.push('\n');
        Ok(text)
    }
}

// ---------------------------------------------------------------------------
// 诊断
// ---------------------------------------------------------------------------

/// 两个资产用了同一个 GUID。
///
/// 这不是"警告"级别的问题：GUID 撞车后，`guid://x` 解析到哪一个取决于扫描
/// 顺序，等于引用变成了随机。必须报出来让人手工改一个。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuidConflict {
    /// 撞车的 GUID。
    pub guid: AssetGuid,
    /// 声称拥有它的所有资产（项目相对路径，字典序）。
    pub paths: Vec<String>,
}

/// 坏 `.meta`（解析失败）。保留原文位置以便让人去修。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidMeta {
    /// `.meta` 的项目相对路径。
    pub path: String,
    /// 失败原因。
    pub reason: String,
}

/// 资产库在扫描期发现的问题（只报告，不改盘）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssetDiagnostics {
    /// 有资产文件但没有 `.meta` 的（项目相对路径，字典序）。
    pub missing_meta: Vec<String>,
    /// `.meta` 存在但对应资产已不在的（孤儿 meta）。
    pub orphan_meta: Vec<String>,
    /// 解析失败的 `.meta`。
    pub invalid_meta: Vec<InvalidMeta>,
    /// GUID 冲突。
    pub conflicts: Vec<GuidConflict>,
}

impl AssetDiagnostics {
    /// 有没有需要人处理的问题。
    pub fn has_issues(&self) -> bool {
        !self.missing_meta.is_empty()
            || !self.orphan_meta.is_empty()
            || !self.invalid_meta.is_empty()
            || !self.conflicts.is_empty()
    }

    /// 问题总数。
    pub fn issue_count(&self) -> usize {
        self.missing_meta.len()
            + self.orphan_meta.len()
            + self.invalid_meta.len()
            + self.conflicts.len()
    }
}

// ---------------------------------------------------------------------------
// 资产库
// ---------------------------------------------------------------------------

/// 一条资产记录。
#[derive(Debug, Clone, PartialEq)]
pub struct AssetRecord {
    /// 项目相对路径（正斜杠）。
    pub path: String,
    /// 资产类型。
    pub kind: AssetKind,
    /// 导入设置（来自 `.meta`）。
    pub import: ImportSettings,
}

/// 资产数据库：GUID ↔ 路径 的双向索引 + 扫描期诊断。
///
/// **只读契约**：`scan` 绝不写盘（`--check` 必须保持只读幂等），
/// 生成 `.meta` 是显式的 [`ensure_metas`](Self::ensure_metas)。
#[derive(Debug, Clone, Default)]
pub struct AssetDatabase {
    root: PathBuf,
    by_guid: BTreeMap<AssetGuid, AssetRecord>,
    by_path: BTreeMap<String, AssetGuid>,
    diagnostics: AssetDiagnostics,
}

impl AssetDatabase {
    /// 扫描一个项目目录，建立索引。
    ///
    /// 复杂度 O(项目文件数)：项目级资产通常几百到几千，一次扫描毫秒级；
    /// 编辑器在打开项目 / 显式刷新时扫，不在每帧扫。
    pub fn scan(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        let mut files: Vec<(String, AssetKind)> = Vec::new();
        let mut metas: BTreeMap<String, AssetMeta> = BTreeMap::new();
        let mut db = Self {
            root: root.clone(),
            ..Self::default()
        };

        collect(&root, &root, &mut files, &mut metas, &mut db.diagnostics);

        // 同一个 GUID 可能被多份 `.meta` 声称 —— 按资产路径排序后取第一个，
        // 其余记冲突。为什么要确定"第一个"：解析必须是确定性的，否则同一个
        // 项目两次运行可能拿到不同结果（无头截图逐字节一致的前提就没了）。
        let mut claimed_by: BTreeMap<AssetGuid, Vec<String>> = BTreeMap::new();
        for (path, kind) in &files {
            let Some(meta) = metas.get(path) else {
                db.diagnostics.missing_meta.push(path.clone());
                continue;
            };
            claimed_by
                .entry(meta.guid.clone())
                .or_default()
                .push(path.clone());
            let record = AssetRecord {
                path: path.clone(),
                kind: *kind,
                import: meta.import,
            };
            // `or_insert` 保留先到者（files 已排序），冲突在下面统一报。
            db.by_path.insert(path.clone(), meta.guid.clone());
            db.by_guid.entry(meta.guid.clone()).or_insert(record);
        }

        for (guid, mut paths) in claimed_by {
            if paths.len() > 1 {
                paths.sort();
                db.diagnostics
                    .conflicts
                    .push(GuidConflict { guid, paths });
            }
        }

        // 孤儿 meta：有 `.meta` 但资产文件没了。常见于用户直接删资产文件
        // 却留下伴生文件 —— 不报出来的话，GUID 会"看起来存在"但打不开。
        for path in metas.keys() {
            if !files.iter().any(|(p, _)| p == path) {
                db.diagnostics
                    .orphan_meta
                    .push(format!("{path}.{META_EXTENSION}"));
            }
        }

        db.diagnostics.missing_meta.sort();
        db.diagnostics.orphan_meta.sort();
        db
    }

    /// 项目根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 扫描期发现的问题。
    pub fn diagnostics(&self) -> &AssetDiagnostics {
        &self.diagnostics
    }

    /// 资产总数。
    pub fn len(&self) -> usize {
        self.by_guid.len()
    }

    /// 资产是否为空。
    pub fn is_empty(&self) -> bool {
        self.by_guid.is_empty()
    }

    /// 按 GUID 查路径。
    pub fn path_of(&self, guid: &AssetGuid) -> Option<&str> {
        self.by_guid.get(guid).map(|r| r.path.as_str())
    }

    /// 按路径查 GUID。
    pub fn guid_of(&self, relative: &str) -> Option<&AssetGuid> {
        self.by_path.get(relative)
    }

    /// 按 GUID 查完整记录。
    pub fn record(&self, guid: &AssetGuid) -> Option<&AssetRecord> {
        self.by_guid.get(guid)
    }

    /// 按路径查完整记录。
    pub fn record_of_path(&self, relative: &str) -> Option<&AssetRecord> {
        self.guid_of(relative).and_then(|g| self.record(g))
    }

    /// 全部资产，按**路径**字典序（UI 列表与 MCP 输出都用它，保证顺序稳定）。
    pub fn sorted_entries(&self) -> Vec<(&AssetGuid, &AssetRecord)> {
        let mut entries: Vec<_> = self.by_guid.iter().collect();
        entries.sort_by(|a, b| a.1.path.cmp(&b.1.path));
        entries
    }

    /// 某个目录前缀下的资产（资源浏览器按树展示用）。
    pub fn entries_under(&self, dir_prefix: &str) -> Vec<(&AssetGuid, &AssetRecord)> {
        let prefix = if dir_prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", dir_prefix.trim_end_matches('/'))
        };
        self.sorted_entries()
            .into_iter()
            .filter(|(_, r)| r.path.starts_with(&prefix))
            .collect()
    }

    /// 把一个引用（可能是 `guid://…`，也可能是旧式路径）解析成**项目相对路径**。
    ///
    /// - `guid://…` → 查库；查不到返回 `None`（悬空 GUID，调用方要当作断链）
    /// - 普通路径 → 原样返回 `Some`
    /// - 空串 → `None`（空引用不是错误，但也没有路径）
    pub fn resolve_reference(&self, reference: &str) -> Option<String> {
        if reference.is_empty() {
            return None;
        }
        if let Some(raw) = reference.strip_prefix(GUID_SCHEME) {
            let guid = AssetGuid::parse(raw)?;
            return self.path_of(&guid).map(|p| p.to_string());
        }
        Some(reference.to_string())
    }

    /// 把一个项目相对路径编码成**磁盘引用形式**。
    ///
    /// 不是资产的（`builtin://cube` 这类程序化几何、空串、绝对路径、
    /// 以及库里没有的悬空路径）一律原样返回 —— 悬空引用保持可读，
    /// 让 `--check` 能报出人类看得懂的路径，而不是一串十六进制。
    pub fn encode_reference(&self, path: &str) -> String {
        if path.is_empty() || path.contains("://") || Path::new(path).is_absolute() {
            return path.to_string();
        }
        match self.guid_of(path) {
            Some(guid) => guid.to_reference(),
            None => path.to_string(),
        }
    }

    /// 为所有"有资产但没 `.meta`"的文件生成 `.meta`（写盘）并补进索引。
    ///
    /// 返回生成的数量。写失败（只读目录 / 权限）时返回 `Err`，但**已成功的
    /// 部分保留在索引里** —— 调用方通常只关心"能生成多少是多少"。
    pub fn ensure_metas(&mut self) -> Result<usize, String> {
        // 先把要做的活拷出来：循环内部要改 self，借用冲突。
        let pending: Vec<String> = self.diagnostics.missing_meta.clone();
        if pending.is_empty() {
            return Ok(0);
        }

        let mut created = 0usize;
        let mut first_error: Option<String> = None;

        for path in pending {
            let Some(kind) = AssetKind::from_path(&path) else {
                continue;
            };
            let meta = AssetMeta::fresh(kind);
            let meta_path = self.root.join(format!("{path}.{META_EXTENSION}"));
            let text = meta
                .to_ron()
                .map_err(|e| format!("序列化 {} 失败: {e}", meta_path.display()))?;
            if let Err(e) = std::fs::write(&meta_path, text) {
                first_error.get_or_insert_with(|| format!("写入 {} 失败: {e}", meta_path.display()));
                continue;
            }

            self.by_path.insert(path.clone(), meta.guid.clone());
            self.by_guid.insert(
                meta.guid,
                AssetRecord {
                    path,
                    kind,
                    import: meta.import,
                },
            );
            created += 1;
        }

        // 已生成的从"缺 meta"清单里摘掉（失败的留着，下次还会再试）。
        self.diagnostics
            .missing_meta
            .retain(|p| !self.by_path.contains_key(p));

        match first_error {
            Some(e) => Err(e),
            None => Ok(created),
        }
    }

    /// 重新扫描（编辑器"刷新资产" / 文件变更后用）。
    pub fn refresh(&mut self) {
        // 没有根目录的空索引（单测里手工构造的句柄）不去扫盘：
        // `scan("")` 的语义是"扫当前工作目录"，那是几百毫秒的意外。
        if self.root.as_os_str().is_empty() {
            return;
        }
        *self = Self::scan(&self.root);
    }

    /// 改写某个资产的导入设置（写回它的 `.meta`）。
    ///
    /// 资产还没有身份证时先补一张（`ensure_metas`），而不是报错 ——
    /// 用户点开一个刚拷进来的资产就要调缩放，是再正常不过的路径。
    pub fn set_import(&mut self, relative: &str, import: ImportSettings) -> Result<(), String> {
        if self.guid_of(relative).is_none() {
            self.ensure_metas()?;
        }
        let Some(guid) = self.guid_of(relative).cloned() else {
            return Err(format!("资产没有身份证: {relative}"));
        };
        let kind = self
            .record(&guid)
            .map(|r| r.kind)
            .unwrap_or(AssetKind::Other);

        let meta = AssetMeta {
            guid: guid.clone(),
            kind,
            import,
        };
        let meta_path = self.root.join(format!("{relative}.{META_EXTENSION}"));
        std::fs::write(&meta_path, meta.to_ron()?)
            .map_err(|e| format!("写入 {} 失败: {e}", meta_path.display()))?;

        if let Some(record) = self.by_guid.get_mut(&guid) {
            record.import = import;
        }
        Ok(())
    }

    /// 某个资产当前的导入设置（没有记录时给默认值）。
    pub fn import_of(&self, relative: &str) -> ImportSettings {
        self.record_of_path(relative)
            .map(|r| r.import)
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// 场景 / 预制体的引用编解码
// ---------------------------------------------------------------------------

/// 把实体上所有资产引用从**内存路径**编码成**磁盘 GUID**。
pub fn encode_entity_references(entity: &mut Entity, db: &AssetDatabase) {
    encode_slot(&mut entity.model, db);
    for script in &mut entity.scripts {
        encode_slot(script, db);
    }
    if let Some(audio) = &mut entity.audio {
        encode_slot(&mut audio.clip, db);
    }
    if let Some(link) = &mut entity.prefab {
        encode_slot(&mut link.source, db);
    }
    // UI 图片与模型 / 音频同款：磁盘上存 GUID，改名不断链。
    // （P0 资产管线的规则见 `Docs/决策记录.md` §十四.1：转换只发生在
    //  ProjectHandle 的读写边界，这里正是那个边界。）
    if let Some(image) = entity.ui.as_mut().and_then(|ui| ui.image.as_mut()) {
        encode_slot(&mut image.path, db);
    }
}

/// 把实体上所有资产引用从**磁盘 GUID** 解码成**内存路径**。
///
/// 解码失败（GUID 悬空）时**保留原始 `guid://…` 字符串**：这样 `--check`
/// 能把它当成"悬空 GUID"点名，UI 也能显示出来让人知道这里断链了。
pub fn decode_entity_references(entity: &mut Entity, db: &AssetDatabase) {
    decode_slot(&mut entity.model, db);
    for script in &mut entity.scripts {
        decode_slot(script, db);
    }
    if let Some(audio) = &mut entity.audio {
        decode_slot(&mut audio.clip, db);
    }
    if let Some(link) = &mut entity.prefab {
        decode_slot(&mut link.source, db);
    }
    if let Some(image) = entity.ui.as_mut().and_then(|ui| ui.image.as_mut()) {
        decode_slot(&mut image.path, db);
    }
}

/// 整张场景的引用编码（保存前）。
pub fn encode_scene_references(scene: &mut Scene, db: &AssetDatabase) {
    for entity in &mut scene.entities {
        encode_entity_references(entity, db);
    }
}

/// 整张场景的引用解码（加载后）。
pub fn decode_scene_references(scene: &mut Scene, db: &AssetDatabase) {
    for entity in &mut scene.entities {
        decode_entity_references(entity, db);
    }
}

/// 预制体资产的引用编码（保存前）。
///
/// 预制体是"一个更小的场景"，引用字段完全同构 —— 复用一个 walk 而不是抄一份，
/// 否则以后加了新的资产引用字段，两处必然漂移。
pub fn encode_prefab_references(prefab: &mut crate::prefab::Prefab, db: &AssetDatabase) {
    for entity in &mut prefab.entities {
        encode_entity_references(entity, db);
    }
}

/// 预制体资产的引用解码（加载后）。
pub fn decode_prefab_references(prefab: &mut crate::prefab::Prefab, db: &AssetDatabase) {
    for entity in &mut prefab.entities {
        decode_entity_references(entity, db);
    }
}

fn encode_slot(slot: &mut String, db: &AssetDatabase) {
    if slot.is_empty() {
        return;
    }
    *slot = db.encode_reference(slot);
}

fn decode_slot(slot: &mut String, db: &AssetDatabase) {
    if slot.is_empty() {
        return;
    }
    if let Some(resolved) = db.resolve_reference(slot) {
        *slot = resolved;
    }
}

/// 判断一个引用字符串是不是 GUID 形式。
pub fn is_guid_reference(reference: &str) -> bool {
    reference.starts_with(GUID_SCHEME)
}

/// 是否为悬空引用（`guid://` 存在但库里找不到）。
pub fn is_dangling_reference(reference: &str, db: &AssetDatabase) -> bool {
    is_guid_reference(reference) && db.resolve_reference(reference).is_none()
}

// ---------------------------------------------------------------------------
// 内部：目录遍历
// ---------------------------------------------------------------------------

fn collect(
    root: &Path,
    dir: &Path,
    files: &mut Vec<(String, AssetKind)>,
    metas: &mut BTreeMap<String, AssetMeta>,
    diagnostics: &mut AssetDiagnostics,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // 目录不存在 = 没资产，不是错误
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if SKIPPED_DIRS.iter().any(|skip| name == *skip) {
                continue;
            }
            collect(root, &path, files, metas, diagnostics);
            continue;
        }
        let Some(relative) = relative_posix(root, &path) else {
            continue;
        };

        if let Some(asset_path) = relative.strip_suffix(&format!(".{META_EXTENSION}")) {
            match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| AssetMeta::from_ron(&text))
            {
                Ok(meta) => {
                    metas.insert(asset_path.to_string(), meta);
                }
                Err(reason) => diagnostics.invalid_meta.push(InvalidMeta {
                    path: relative,
                    reason,
                }),
            }
            continue;
        }

        if let Some(kind) = AssetKind::from_path(&relative) {
            // 场景文件不进资产库：它由 `project.ron` 的 `entry_scene` 按路径引用，
            // 没有任何资产会反过来引用一个场景，因此没有"改名不断链"的需求。
            // 强行要求 `.meta` 只会让"新建项目"立刻带出一条待处理的诊断。
            if kind != AssetKind::Scene {
                files.push((relative, kind));
            }
        }
    }
}

/// 项目内的相对路径（正斜杠）。不在项目内返回 `None`。
fn relative_posix(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    Some(relative.to_string_lossy().replace('\\', "/"))
}

/// 人类可读的资产清单（编辑器面板 / MCP / 排查脚本共用一份格式）。
pub fn render_asset_list(db: &AssetDatabase) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "assets: {}", db.len());
    for (guid, record) in db.sorted_entries() {
        let _ = writeln!(
            out,
            "  {} {:<8} {} -> {}",
            &guid.as_str()[..8],
            record.kind.label(),
            record.path,
            guid
        );
    }
    let diag = db.diagnostics();
    if diag.has_issues() {
        let _ = writeln!(out, "diagnostics: {}", diag.issue_count());
        for p in &diag.missing_meta {
            let _ = writeln!(out, "  missing .meta: {p}");
        }
        for p in &diag.orphan_meta {
            let _ = writeln!(out, "  orphan .meta:  {p}");
        }
        for bad in &diag.invalid_meta {
            let _ = writeln!(out, "  invalid .meta: {} ({})", bad.path, bad.reason);
        }
        for c in &diag.conflicts {
            let _ = writeln!(out, "  guid conflict: {} -> {:?}", c.guid, c.paths);
        }
    }
    out
}

/// 早期的路径式资产句柄（保留：已有 API 与文档在用；新代码请用 [`AssetGuid`]）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetId(String);

impl AssetId {
    /// 从路径字符串创建资产标识。
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    /// 获取资产路径。
    pub fn path(&self) -> &str {
        &self.0
    }

    /// 获取文件扩展名（小写）。
    pub fn extension(&self) -> Option<&str> {
        Path::new(&self.0).extension().and_then(|e| e.to_str())
    }

    /// 获取文件名（包含扩展名）。
    pub fn file_name(&self) -> Option<&str> {
        Path::new(&self.0).file_name().and_then(|n| n.to_str())
    }

    /// 获取去掉扩展名的文件名。
    pub fn stem(&self) -> Option<&str> {
        Path::new(&self.0).file_stem().and_then(|s| s.to_str())
    }

    /// 是否为有效路径（非空）。
    pub fn is_valid(&self) -> bool {
        !self.0.is_empty()
    }
}

impl From<String> for AssetId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for AssetId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl std::fmt::Display for AssetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for AssetId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一个临时项目目录（用完删）。
    fn temp_root(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "xgame_asset_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    /// 在 root 下写一个文件（顺带建目录）。
    fn put(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn guid_parse_requires_exactly_32_hex() {
        assert!(AssetGuid::parse("0123456789abcdef0123456789abcdef").is_some());
        assert!(AssetGuid::parse("0123456789ABCDEF0123456789ABCDEF").is_some());
        assert!(AssetGuid::parse("0123456789abcdef").is_none(), "短 GUID 必须拒绝");
        assert!(
            AssetGuid::parse("0123456789abcdef0123456789abcdeg").is_none(),
            "非十六进制必须拒绝"
        );
        assert!(AssetGuid::parse("").is_none());
    }

    /// 生成器必须"每次调用都不同"——撞 GUID 会让两个资产变成同一个。
    #[test]
    fn generated_guids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..2000 {
            let guid = AssetGuid::generate();
            assert_eq!(guid.as_str().len(), 32);
            assert!(seen.insert(guid), "生成了重复 GUID");
        }
    }

    #[test]
    fn guid_ron_roundtrip() {
        let guid = AssetGuid::generate();
        let text = ron::to_string(&guid).unwrap();
        assert!(text.starts_with('"'), "GUID 必须以字符串落盘: {text}");
        let back: AssetGuid = ron::from_str(&text).unwrap();
        assert_eq!(back, guid);
        // 非法字面量在反序列化期就被拒绝，而不是变成"查不到的 GUID"。
        assert!(ron::from_str::<AssetGuid>("\"nope\"").is_err());
    }

    /// 变体名是磁盘契约：改名字等于让所有旧 `.meta` 解析失败。
    #[test]
    fn asset_kind_labels_are_stable() {
        assert_eq!(ron::to_string(&AssetKind::Model).unwrap(), "Model");
        assert_eq!(ron::to_string(&AssetKind::Prefab).unwrap(), "Prefab");
        assert_eq!(ron::to_string(&AssetKind::Scene).unwrap(), "Scene");
        let kind: AssetKind = ron::from_str("Texture").unwrap();
        assert_eq!(kind, AssetKind::Texture);
    }

    /// `.prefab.ron` 必须先于 `.ron` 判定；`project.ron` 不是资产。
    #[test]
    fn kind_inference_order() {
        assert_eq!(AssetKind::from_path("Prefabs/A.prefab.ron"), Some(AssetKind::Prefab));
        assert_eq!(AssetKind::from_path("Scenes/A.ron"), Some(AssetKind::Scene));
        assert_eq!(AssetKind::from_path("project.ron"), None);
        assert_eq!(AssetKind::from_path("Resources/Box.glb"), Some(AssetKind::Model));
        assert_eq!(AssetKind::from_path("Scripts/a.lua"), Some(AssetKind::Script));
        assert_eq!(AssetKind::from_path("src/main.rs"), None);
        assert_eq!(AssetKind::from_path("Cargo.toml"), None);
    }

    /// 默认导入设置不落盘：`.meta` 里只出现真正调过的参数。
    #[test]
    fn default_import_settings_are_not_serialized() {
        let meta = AssetMeta {
            guid: AssetGuid::generate(),
            kind: AssetKind::Model,
            import: ImportSettings::default(),
        };
        let text = meta.to_ron().unwrap();
        assert!(!text.contains("import"), "默认设置不该出现: {text}");

        let mut meta = meta;
        meta.import.model_scale = 2.5;
        let text = meta.to_ron().unwrap();
        assert!(text.contains("model_scale"), "调过的参数必须落盘: {text}");
        // 部分字段写盘后仍能读回（其余靠 serde default）。
        let back = AssetMeta::from_ron(&text).unwrap();
        assert_eq!(back.import.model_scale, 2.5);
        assert!(back.import.texture_srgb, "缺省字段走默认值");
    }

    /// 扫描：有资产没 `.meta` → 记 missing_meta；生成后索引可用且幂等。
    #[test]
    fn scan_then_ensure_metas_is_idempotent() {
        let root = temp_root("ensure");
        put(&root, "Resources/Box.glb", b"glb");
        put(&root, "Resources/Audio/Beep.wav", b"wav");
        put(&root, "Scripts/Player.lua", b"-- lua");
        put(&root, "Prefabs/Enemy.prefab.ron", b"(name:\"E\",entities:[])");
        put(&root, "project.ron", b"(name:\"x\")");
        put(&root, "src/main.rs", b"fn main() {}");

        let mut db = AssetDatabase::scan(&root);
        assert_eq!(db.len(), 0, "还没有任何 meta，索引应为空");
        assert_eq!(db.diagnostics().missing_meta.len(), 4, "{:?}", db.diagnostics());
        assert!(
            !db.diagnostics()
                .missing_meta
                .iter()
                .any(|p| p == "project.ron" || p == "src/main.rs"),
            "清单与非资产不该要求 meta"
        );

        let created = db.ensure_metas().expect("生成 meta");
        assert_eq!(created, 4);
        assert_eq!(db.len(), 4);
        assert!(!db.diagnostics().has_issues());

        // 幂等：再扫一次不应再生成。
        let mut db2 = AssetDatabase::scan(&root);
        assert!(!db2.diagnostics().has_issues(), "生成的 meta 应当被扫到");
        assert_eq!(db2.ensure_metas().unwrap(), 0);

        // `.meta` 真的落盘了，且能解析。
        assert!(root.join("Resources/Box.glb.meta").is_file());
        let text = std::fs::read_to_string(root.join("Resources/Box.glb.meta")).unwrap();
        assert!(AssetMeta::from_ron(&text).is_ok());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 核心价值：**资产改名 / 挪目录后，GUID 引用依然解析得到**。
    ///
    /// 这条测试就是 P0 的验收判据。旧实现（路径引用）下它必然红：
    /// 路径变了就找不到文件。
    #[test]
    fn moving_asset_keeps_guid_reference_resolvable() {
        let root = temp_root("move");
        put(&root, "Resources/Box.glb", b"glb");

        let mut db = AssetDatabase::scan(&root);
        db.ensure_metas().unwrap();
        let guid = db.guid_of("Resources/Box.glb").expect("有 guid").clone();
        let reference = guid.to_reference();
        assert_eq!(
            db.resolve_reference(&reference).as_deref(),
            Some("Resources/Box.glb")
        );

        // 用户用文件管理器把资产（连同 meta）挪到子目录。
        std::fs::create_dir_all(root.join("Resources/Models")).unwrap();
        std::fs::rename(
            root.join("Resources/Box.glb"),
            root.join("Resources/Models/Box.glb"),
        )
        .unwrap();
        std::fs::rename(
            root.join("Resources/Box.glb.meta"),
            root.join("Resources/Models/Box.glb.meta"),
        )
        .unwrap();

        let db = AssetDatabase::scan(&root);
        assert_eq!(
            db.resolve_reference(&reference).as_deref(),
            Some("Resources/Models/Box.glb"),
            "GUID 必须跟到新路径（这正是路径引用做不到的）"
        );
        assert_eq!(db.guid_of("Resources/Models/Box.glb"), Some(&guid));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 悬空 GUID 必须解析成 `None`（而不是悄悄返回原始字符串），
    /// 否则 `--check` 与 UI 都无法区分"断了"和"好的"。
    #[test]
    fn dangling_guid_resolves_to_none_and_fails_check() {
        let root = temp_root("dangling");
        let db = AssetDatabase::scan(&root);
        let missing = AssetGuid::generate().to_reference();
        assert_eq!(db.resolve_reference(&missing), None);
        assert!(is_dangling_reference(&missing, &db));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 编码只碰"库里的资产"，其余原样：
    /// `builtin://` 是程序化几何，绝对路径与空串不该被改写。
    #[test]
    fn encode_leaves_non_assets_alone() {
        let root = temp_root("encode");
        put(&root, "Resources/Box.glb", b"glb");
        let mut db = AssetDatabase::scan(&root);
        db.ensure_metas().unwrap();

        let guid = db.guid_of("Resources/Box.glb").unwrap().clone();
        assert_eq!(
            db.encode_reference("Resources/Box.glb"),
            format!("guid://{guid}")
        );
        assert_eq!(db.encode_reference("builtin://cube"), "builtin://cube");
        assert_eq!(db.encode_reference(""), "");
        assert_eq!(
            db.encode_reference("Resources/NotThere.glb"),
            "Resources/NotThere.glb",
            "悬空路径保持可读，方便 check 报出人类懂的名字"
        );
        let abs = if cfg!(windows) { "C:/x/Box.glb" } else { "/x/Box.glb" };
        assert_eq!(db.encode_reference(abs), abs);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 场景级往返：内存路径 → 磁盘 GUID → 内存路径，逐字段一致。
    #[test]
    fn scene_reference_roundtrip_through_guid() {
        let root = temp_root("scene");
        put(&root, "Resources/Box.glb", b"glb");
        put(&root, "Scripts/Player.lua", b"-- lua");
        put(&root, "Resources/Audio/Beep.wav", b"wav");
        put(&root, "Prefabs/Enemy.prefab.ron", b"(name:\"E\",entities:[])");
        let mut db = AssetDatabase::scan(&root);
        db.ensure_metas().unwrap();

        let mut entity = Entity::new("Box")
            .with_model("Resources/Box.glb")
            .with_script("Scripts/Player.lua")
            .with_audio(crate::AudioSource::new("Resources/Audio/Beep.wav"));
        entity.prefab = Some(crate::prefab::PrefabInstanceRef {
            source: "Prefabs/Enemy.prefab.ron".into(),
            source_id: entity.id,
            is_root: true,
        });
        entity.scripts.push("builtin://no".into()); // 非资产混入也要原样

        let mut scene = Scene::new("S");
        scene.entities.push(entity);

        let original = scene.entities[0].clone();
        encode_scene_references(&mut scene, &db);
        assert!(scene.entities[0].model.starts_with(GUID_SCHEME));
        assert!(scene.entities[0].scripts[0].starts_with(GUID_SCHEME));
        assert!(scene.entities[0].audio.as_ref().unwrap().clip.starts_with(GUID_SCHEME));
        assert!(scene.entities[0].prefab.as_ref().unwrap().source.starts_with(GUID_SCHEME));

        decode_scene_references(&mut scene, &db);
        let back = &scene.entities[0];
        assert_eq!(back.model, original.model);
        assert_eq!(back.scripts, original.scripts);
        assert_eq!(back.audio.as_ref().unwrap().clip, original.audio.as_ref().unwrap().clip);
        assert_eq!(
            back.prefab.as_ref().unwrap().source,
            original.prefab.as_ref().unwrap().source
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 两个资产共用一个 GUID → 报冲突，且解析结果**确定性**（字典序第一份）。
    #[test]
    fn duplicate_guid_is_reported_and_first_wins() {
        let root = temp_root("conflict");
        put(&root, "Resources/A.glb", b"a");
        put(&root, "Resources/B.glb", b"b");
        let shared = AssetGuid::generate();
        for name in ["A", "B"] {
            let meta = AssetMeta {
                guid: shared.clone(),
                kind: AssetKind::Model,
                import: ImportSettings::default(),
            };
            put(
                &root,
                &format!("Resources/{name}.glb.meta"),
                meta.to_ron().unwrap().as_bytes(),
            );
        }

        let db = AssetDatabase::scan(&root);
        assert_eq!(db.diagnostics().conflicts.len(), 1);
        assert_eq!(db.diagnostics().conflicts[0].paths, vec!["Resources/A.glb", "Resources/B.glb"]);
        assert_eq!(db.resolve_reference(&shared.to_reference()).as_deref(), Some("Resources/A.glb"));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 删了资产却留下 `.meta` → 报孤儿；坏 `.meta` → 报无效。
    #[test]
    fn orphan_and_invalid_meta_are_reported() {
        let root = temp_root("orphan");
        put(&root, "Resources/Gone.glb.meta", b"(guid:\"0123456789abcdef0123456789abcdef\")");
        put(&root, "Resources/Bad.glb", b"x");
        put(&root, "Resources/Bad.glb.meta", b"not ron at all (((");

        let db = AssetDatabase::scan(&root);
        assert_eq!(db.diagnostics().orphan_meta.len(), 1, "{:?}", db.diagnostics());
        assert_eq!(db.diagnostics().invalid_meta.len(), 1, "{:?}", db.diagnostics());
        assert!(db.diagnostics().has_issues());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 构建产物目录不该被当成资产（否则每个 `.rlib` 都想登记）。
    #[test]
    fn build_dirs_are_skipped() {
        let root = temp_root("skip");
        put(&root, "target/debug/x.glb", b"x");
        put(&root, "dist/bundle/y.wav", b"y");
        put(&root, "Resources/Real.glb", b"r");

        let db = AssetDatabase::scan(&root);
        assert_eq!(db.diagnostics().missing_meta, vec!["Resources/Real.glb"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 同一份项目两次扫描必须给出**完全一致**的索引（无头截图逐字节一致的地基）。
    #[test]
    fn scan_is_deterministic() {
        let root = temp_root("determinism");
        put(&root, "Resources/b.glb", b"b");
        put(&root, "Resources/a.glb", b"a");
        put(&root, "Resources/c.wav", b"c");
        let mut db = AssetDatabase::scan(&root);
        db.ensure_metas().unwrap();

        let first: Vec<String> = AssetDatabase::scan(&root)
            .sorted_entries()
            .iter()
            .map(|(g, r)| format!("{g}|{}", r.path))
            .collect();
        let second: Vec<String> = AssetDatabase::scan(&root)
            .sorted_entries()
            .iter()
            .map(|(g, r)| format!("{g}|{}", r.path))
            .collect();
        assert_eq!(first, second);

        let _ = std::fs::remove_dir_all(&root);
    }
}
