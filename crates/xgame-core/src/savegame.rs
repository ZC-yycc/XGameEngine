//! 存档（`SaveGame`）：把**运行时状态**落成一份可读的文本文件。
//!
//! # 为什么场景文件不够用
//!
//! 场景是**编辑态**的持久化：它记录实体怎么摆、挂了什么脚本，是"关卡长什么样"。
//! 游戏进度是另一件事 —— 玩家走到哪、血还剩多少、门开没开。此前引擎里
//! **完全没有**这一形态（差距分析 P2 #32），于是每个项目都得自己想办法：
//! 导出场景（会把运行时生成物一起写进去、还会冲掉作者的场景文件）、
//! 或者用 `log()` 打出来（等于没有）。
//!
//! # 存什么 / 不存什么（v1 的边界，刻意写清）
//!
//! 存：
//! - 每个实体的**局部变换**（位置 / 旋转 / 缩放）—— 按 `EntityId` 索引；
//! - 刚体的**线速度 / 角速度**（只有声明了动态刚体的实体才有）；
//! - 全局变量与**每实体变量**（脚本 `save_set` / `entity_save_set` 写的，
//!   引擎不解释语义 —— 血量、金币、钥匙都在这里，由作者定义）。
//!
//! 不存（都是有意取舍，不是遗漏）：
//! - **运行时新生成 / 已销毁的实体**。存档是一份"状态覆盖层"，不是场景快照：
//!   读档时按 ID 找现有实体，找不到就跳过并计入
//!   [`SaveApplyReport::missing`](crate::savegame 的说明见 `xgame-runtime`)。
//!   要做"连生成物一起复原"得先有实体原型注册表，那是下一版的事；
//! - **脚本内部状态**（`state_*` 表）。它是 Lua 值，可能是表 / 函数 / 用户数据，
//!   序列化它等于给引擎引入一套 Lua 值的落盘格式。要跨会话保存的东西**写进
//!   变量**（`save_set`），这也是引擎能保证语义的那一半；
//! - **时钟**。存档里记 `game_time` 只作展示（"已游玩 3 小时"），读档不从它
//!   恢复时钟 —— 时钟是从进程启动开始累加的，倒着改它会打乱固定步与动画相位。
//!
//! # 落盘纪律
//!
//! 与场景 / 清单同一条：路径过 [`crate::normalize_project_relative`]（`slot` 里写
//! `../x` 会被拒），写盘走 [`crate::fsx::write_atomic`]（不必担心半截文件），
//! 实体按 `EntityId` 排序（同状态两次存档**逐字节相同**，可 diff、可断言）。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsx::write_atomic;
use crate::id::EntityId;
use crate::project::{normalize_project_relative, PathDefect};

/// 存档目录（项目相对）。
pub const SAVE_DIR: &str = "Saves";
/// 存档扩展名（**不带点**，与 `PREFAB_EXTENSION` 的风格一致）。
pub const SAVE_EXTENSION: &str = "sav";
/// 存档格式版本。读档时**必须**相等 —— 存档是玩家数据，宁可报错让人看见。
pub const SAVE_FORMAT_VERSION: u32 = 1;

/// 一个存档值。
///
/// 只有三种标量：数字 / 布尔 / 文本。表、函数、实体引用都不收 —— 存档要能被
/// 人类读懂、被 `--check` 类工具检查、被别的语言（Web 端、工具链）解析。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SaveValue {
    /// 数值（统一 `f64`：脚本里 `1` 与 `1.0` 是同一个 Lua number）。
    Number(f64),
    /// 布尔。
    Bool(bool),
    /// 文本。
    Text(String),
}

impl SaveValue {
    /// 当数值读（其它类型返回 `None`）。
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(v) => Some(*v),
            _ => None,
        }
    }
    /// 当布尔读。
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(*v),
            _ => None,
        }
    }
    /// 当文本读。
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(v) => Some(v),
            _ => None,
        }
    }
    /// 是不是有限数值（`NaN` / `inf` 不许进存档：读回来会让物理与渲染出怪）。
    pub fn is_finite(&self) -> bool {
        match self {
            Self::Number(v) => v.is_finite(),
            _ => true,
        }
    }
}

impl From<f64> for SaveValue {
    fn from(v: f64) -> Self {
        Self::Number(v)
    }
}
impl From<f32> for SaveValue {
    fn from(v: f32) -> Self {
        Self::Number(v as f64)
    }
}
impl From<bool> for SaveValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}
impl From<&str> for SaveValue {
    fn from(v: &str) -> Self {
        Self::Text(v.to_owned())
    }
}
impl From<String> for SaveValue {
    fn from(v: String) -> Self {
        Self::Text(v)
    }
}

/// 单个实体的存档状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityState {
    /// 实体 ID（`EntityId::raw()`）。跨文件不复用，但**只在同一份场景内**稳定。
    pub id: u64,
    /// 局部位置。
    pub position: [f32; 3],
    /// 局部旋转（四元数 `x, y, z, w`）。
    pub rotation: [f32; 4],
    /// 局部缩放。
    pub scale: [f32; 3],
    /// 线速度（只有动态刚体才有；静态 / 运动学实体是零向量，不写这一行）。
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub linear_velocity: [f32; 3],
    /// 角速度。
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub angular_velocity: [f32; 3],
    /// 这个实体上的用户变量（`entity_save_set`）。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, SaveValue>,
}

fn is_zero3(v: &[f32; 3]) -> bool {
    v.iter().all(|c| *c == 0.0)
}

/// 一份存档。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveGame {
    /// 格式版本（见 [`SAVE_FORMAT_VERSION`]）。
    pub format_version: u32,
    /// 存档时的场景（项目相对路径；空 = 宿主没告诉运行时）。
    ///
    /// 读档时**不强制**同一场景：不同就由运行时切过去（见 `World::load_game`）。
    #[serde(default)]
    pub scene: String,
    /// 存档时的游戏内累计时间（秒）。**只作展示**，读档不恢复时钟。
    #[serde(default)]
    pub game_time: f32,
    /// 全局变量（`save_set`）。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, SaveValue>,
    /// 实体状态，按 ID 升序（写盘前排序 ⇒ 同状态逐字节相同）。
    #[serde(default)]
    pub entities: Vec<EntityState>,
}

impl Default for SaveGame {
    fn default() -> Self {
        Self {
            format_version: SAVE_FORMAT_VERSION,
            scene: String::new(),
            game_time: 0.0,
            vars: BTreeMap::new(),
            entities: Vec::new(),
        }
    }
}

impl SaveGame {
    /// 空存档（新游戏的起点）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 按 ID 排序并在读档边界做体检（写作端也调，保证落盘的就是排好序的）。
    ///
    /// 体检项：`format_version` 必须相等；每个实体的变换 / 速度必须是**有限数**；
    /// 旋转必须接近单位四元数（容差 1e-3 —— 存档是浮点，不该要求逐位相等，
    /// 但明显坏掉的（`|q|² = 0.5`）必须拒绝：那会让实体缩成 0.7 倍）。
    pub fn validated(&mut self) -> Result<(), Vec<String>> {
        let mut defects = Vec::new();
        if self.format_version != SAVE_FORMAT_VERSION {
            defects.push(format!(
                "format_version {} != {SAVE_FORMAT_VERSION}",
                self.format_version
            ));
        }
        for e in &self.entities {
            if !e.position.iter().all(|c| c.is_finite())
                || !e.scale.iter().all(|c| c.is_finite())
                || !e.rotation.iter().all(|c| c.is_finite())
                || !e.linear_velocity.iter().all(|c| c.is_finite())
                || !e.angular_velocity.iter().all(|c| c.is_finite())
            {
                defects.push(format!("entity {} has non-finite transform", e.id));
            }
            let [x, y, z, w] = e.rotation;
            let norm2 = x * x + y * y + z * z + w * w;
            if (norm2 - 1.0).abs() > 1e-3 {
                defects.push(format!(
                    "entity {} rotation is not unit (|q|^2 = {norm2})",
                    e.id
                ));
            }
            for (k, v) in &e.vars {
                if !v.is_finite() {
                    defects.push(format!("entity {} var '{k}' is not finite", e.id));
                }
            }
        }
        for (k, v) in &self.vars {
            if !v.is_finite() {
                defects.push(format!("var '{k}' is not finite"));
            }
        }
        if !defects.is_empty() {
            return Err(defects);
        }
        self.entities.sort_by_key(|e| e.id);
        Ok(())
    }

    /// 找到某个实体的状态。
    pub fn entity(&self, id: EntityId) -> Option<&EntityState> {
        self.entities.iter().find(|e| e.id == id.raw())
    }
}

/// 运行时持有的变量表（脚本 `save_set` / `entity_save_set` 写进来的）。
///
/// 与 [`SaveGame`] 分开：档是**一次快照**，变量表是**活状态**。存档时把变量表
/// 摊进快照，读档时再摊回来。
///
/// 为什么变量不直接塞进 `Entity`：它们是**玩法层数据**，引擎不知道语义；
/// 塞进 `Entity` 会让场景文件长出一块作者从没写过的字段，还会把"关卡长什么样"
/// 与"这一局玩到哪"混在同一次保存里。
#[derive(Debug, Clone, Default)]
pub struct SaveVars {
    /// 全局变量（跨场景保留 —— 与脚本全局变量同款语义：切关不清）。
    pub global: BTreeMap<String, SaveValue>,
    /// 每实体变量（**实体绑定态**：切场景 / 实体销毁时清）。
    pub per_entity: HashMap<EntityId, BTreeMap<String, SaveValue>>,
}

impl SaveVars {
    /// 记一个全局变量。非有限数值拒绝写入（返回 `false`），理由同
    /// [`SaveValue::is_finite`]。
    pub fn set_global(&mut self, key: impl Into<String>, value: SaveValue) -> bool {
        if !value.is_finite() {
            return false;
        }
        self.global.insert(key.into(), value);
        true
    }

    /// 读一个全局变量。
    pub fn global(&self, key: &str) -> Option<&SaveValue> {
        self.global.get(key)
    }

    /// 删除一个全局变量（返回是否存在过）。
    pub fn remove_global(&mut self, key: &str) -> bool {
        self.global.remove(key).is_some()
    }

    /// 记一个实体变量。实体不在场景里也照记（读回时如果实体没了，值就悬着 ——
    /// 与存档里的悬空 ID 同类，读档会跳过并计数）。
    pub fn set_entity(&mut self, id: EntityId, key: impl Into<String>, value: SaveValue) -> bool {
        if !value.is_finite() {
            return false;
        }
        self.per_entity
            .entry(id)
            .or_default()
            .insert(key.into(), value);
        true
    }

    /// 读一个实体变量。
    pub fn entity(&self, id: EntityId, key: &str) -> Option<&SaveValue> {
        self.per_entity.get(&id).and_then(|m| m.get(key))
    }

    /// 清掉某个实体的全部变量（实体销毁 / 被删除时调）。
    pub fn clear_entity(&mut self, id: EntityId) {
        self.per_entity.remove(&id);
    }

    /// 清掉**所有实体**变量（切场景时调：ID 只在单个文件内唯一，留着必然串味）。
    pub fn clear_entities(&mut self) {
        self.per_entity.clear();
    }

    /// 把活状态摊进一份存档的变量部分。
    pub fn write_into(&self, save: &mut SaveGame) {
        save.vars = self.global.clone();
        for e in &mut save.entities {
            if let Some(vars) = self.per_entity.get(&EntityId::from_raw(e.id)) {
                e.vars = vars.clone();
            }
        }
    }

    /// 从存档恢复活状态（**替换**而不是合并：读档是"回到那一刻"，
    /// 合并会把存档之后新写的键留下来，症状是读档后血条显示上一次的数）。
    pub fn read_from(&mut self, save: &SaveGame) {
        self.global = save.vars.clone();
        self.per_entity.clear();
        for e in &save.entities {
            if e.vars.is_empty() {
                continue;
            }
            self.per_entity
                .insert(EntityId::from_raw(e.id), e.vars.clone());
        }
    }
}

/// 存档路径不合规 / 读写失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    /// `slot` 不能当文件名用（越界 / Windows 不认的写法）。
    InvalidSlot {
        slot: String,
        defect: PathDefect,
    },
    /// 读写失败（含目录建不出来）。
    Io { path: String, message: String },
    /// 文本解析不了（存档被手改坏 / 不是存档文件）。
    Parse { path: String, message: String },
    /// 内容不合法（版本不对 / 非有限数 / 非单位旋转）。**拒绝加载**而不是
    /// 静默修：坏存档被"修"过之后，玩家看到的是自己没玩过的状态。
    Invalid { path: String, defects: Vec<String> },
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSlot { slot, defect } => {
                write!(f, "存档名 '{slot}' 不能当文件名用：{defect}")
            }
            Self::Io { path, message } => write!(f, "存档读写失败 '{path}': {message}"),
            Self::Parse { path, message } => write!(f, "存档解析失败 '{path}': {message}"),
            Self::Invalid { path, defects } => {
                write!(f, "存档内容不合法 '{path}': {}", defects.join("; "))
            }
        }
    }
}

/// `slot` → 项目相对路径（`Saves/<slot>.sav`）。
///
/// `slot` 允许带子目录（`chapter1/slot2`），但整条路径必须过
/// [`normalize_project_relative`] —— `../x`、盘符、`a:b`、`CON` 全部拒绝。
///
/// 作者写 `slot1` 与 `slot1.sav` 落到**同一个**文件（多余的 `.sav` 后缀被吃掉）：
/// 两种写法产生两个文件的话，症状就是"存了却读不到"。
pub fn save_slot_relative(slot: &str) -> Result<String, SaveError> {
    let trimmed = slot.trim();
    if trimmed.is_empty() {
        return Err(SaveError::InvalidSlot {
            slot: slot.to_owned(),
            defect: PathDefect::Empty,
        });
    }
    // 作者写 `slot1.sav` 与 `slot1` 必须落到**同一个**文件，否则"存了却读不到"。
    // 结尾的斜杠也吃掉：`chapter/` 与 `chapter` 是同一个槽，而不是 `chapter/.sav`。
    let trimmed = trimmed.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return Err(SaveError::InvalidSlot {
            slot: slot.to_owned(),
            defect: PathDefect::Empty,
        });
    }
    let base = trimmed
        .strip_suffix(&format!(".{SAVE_EXTENSION}"))
        .unwrap_or(trimmed);
    let relative = format!("{SAVE_DIR}/{base}.{SAVE_EXTENSION}");
    normalize_project_relative(&relative).map_err(|defect| SaveError::InvalidSlot {
        slot: slot.to_owned(),
        defect,
    })
}

/// `slot` 对应的绝对路径（**不检查存在性，也不碰盘**）。
pub fn save_slot_path(root: &Path, slot: &str) -> Result<PathBuf, SaveError> {
    Ok(root.join(save_slot_relative(slot)?))
}

/// 写一份存档（原子写；父目录自动补建）。
///
/// 返回写入的项目相对路径（日志 / 无头报告用）。成功写盘后会顺手回收
/// `Saves/` 里超龄的暂存文件（崩溃残留），与主保存同一条纪律。
pub fn write_save(root: &Path, slot: &str, save: &SaveGame) -> Result<String, SaveError> {
    let relative = save_slot_relative(slot)?;
    let mut save = save.clone();
    save.validated().map_err(|defects| SaveError::Invalid {
        path: relative.clone(),
        defects,
    })?;
    let path = root.join(&relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| SaveError::Io {
            path: relative.clone(),
            message: e.to_string(),
        })?;
    }
    let text = ron::ser::to_string_pretty(&save, ron::ser::PrettyConfig::default()).map_err(|e| {
        SaveError::Io {
            path: relative.clone(),
            message: e.to_string(),
        }
    })?;
    write_atomic(&path, &text).map_err(|e| SaveError::Io {
        path: relative.clone(),
        message: e.to_string(),
    })?;
    if let Some(dir) = path.parent() {
        crate::fsx::sweep_staging(dir);
    }
    Ok(relative)
}

/// 读一份存档（检查版本与数值合法性）。
///
/// **只读**：不会建目录、不会补 `.meta`、不会顺手修复 —— 与 `--check` 的
/// 只读纪律一致（存档读取会出现在游戏启动路径上）。
pub fn read_save(root: &Path, slot: &str) -> Result<SaveGame, SaveError> {
    let relative = save_slot_relative(slot)?;
    let path = root.join(&relative);
    let text = std::fs::read_to_string(&path).map_err(|e| SaveError::Io {
        path: relative.clone(),
        message: e.to_string(),
    })?;
    let save: SaveGame = ron::from_str(&text).map_err(|e| SaveError::Parse {
        path: relative.clone(),
        message: format!("{e:#}"),
    })?;
    let mut save = save;
    save.validated().map_err(|defects| SaveError::Invalid {
        path: relative,
        defects,
    })?;
    Ok(save)
}

/// 这个存档是否存在（**只读**，不解析内容）。
pub fn save_exists(root: &Path, slot: &str) -> bool {
    save_slot_path(root, slot).is_ok_and(|path| path.is_file())
}

/// 删除一个存档（返回"是否真的删掉了"）。不存在不算错。
pub fn delete_save(root: &Path, slot: &str) -> Result<bool, SaveError> {
    let path = save_slot_path(root, slot)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(SaveError::Io {
            path: slot.to_owned(),
            message: e.to_string(),
        }),
    }
}

/// 列出所有存档槽（槽名，不含目录与扩展名，**升序**）。
///
/// 只扫 `Saves/` 的直接子项（子目录里的档不进列表：列表要给"读档"菜单用，
/// 层级展开是 UI 的事）。暂存文件（`*.sav.tmp.*`）跳过。
pub fn list_saves(root: &Path) -> Vec<String> {
    let dir = root.join(SAVE_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let suffix = format!(".{SAVE_EXTENSION}");
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name();
            let name = name.to_str()?;
            let stem = name.strip_suffix(&suffix)?;
            // 暂存文件 `x.sav.tmp.<pid>.<n>` 不匹配上面的后缀（它以 `.tmp.` 结尾
            // 之前没有 `.sav`），但 `x.sav.sav` 这种双扩展名仍然合法，不用特判。
            (!stem.is_empty()).then(|| stem.to_owned())
        })
        .collect();
    out.sort();
    out
}
