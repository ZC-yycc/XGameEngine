//! 中央编辑器状态——拥有场景、选择、相机和工具设置。
//!
//! 所有编辑器面板从该状态读取并通过它写入，保持引擎数据模型和编辑器 UI 同步。
//!
//! 状态**绑定在一个已打开的项目上**（[`ProjectHandle`]）：场景与资产一律按项目
//! 根目录解析，编辑器不再依赖进程当前工作目录。

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use xgame_math::glam::{Quat, Vec3};
use xgame_core::prefab::{prefab_display_name, Prefab, PrefabInstanceRef};
use xgame_core::project::ProjectHandle;
use xgame_core::{
    AssetKind, Entity, EntityId, FieldDesc, ImportSettings, Inspect, MaterialAsset, Scene, Transform,
};
use xgame_physics::DebugLine;
use strum::Display;


/// 资产文件名去掉扩展名（`Resources/Box.glb` → `Box`；`A.prefab.ron` → `A`）。
///
/// 只取第一个 `.` 之前的部分：预制体是双后缀，用 `file_stem` 会得到 `A.prefab`。
fn asset_stem(relative: &str) -> String {
    let file = relative.rsplit('/').next().unwrap_or(relative);
    let stem = file.split('.').next().unwrap_or(file);
    if stem.is_empty() {
        "Asset".to_string()
    } else {
        stem.to_string()
    }
}

/// 场景文件在界面上的显示名（`Scenes/Level2.ron` → `Level2`）。
///
/// 菜单里不该出现整条项目相对路径：一排 `Scenes/...` 前缀只会把真正不同的
/// 部分挤出可视区。完整路径放在悬停提示里。
pub(crate) fn scene_label(relative: &str) -> String {
    let file = relative.rsplit('/').next().unwrap_or(relative);
    file.strip_suffix(".ron").unwrap_or(file).to_string()
}

/// 校验并规范化一个**项目相对**场景路径。
///
/// 场景列表来自磁盘，但设置窗口、对话框和以后的脚本也可能把路径递进来。
/// 统一在这里挡掉绝对路径、`..`、非 `.ron` 与预制体（`.prefab.ron`）：
/// 保存路径越界会把文件写到项目外，读路径越界则等于允许编辑器读任意文件。
fn normalize_scene_relative(raw: &str) -> Result<String, String> {
    let rel = raw.trim().replace('\\', "/");
    if rel.is_empty() {
        return Err("场景路径不能为空".into());
    }
    // Windows 盘符（`C:`）与 UNC 前缀在非 Windows 上不一定被 `Path` 认成绝对路径，
    // 这里显式再挡一次；编辑器的主要平台是 Windows，不能靠平台差异放行。
    if rel.starts_with('/') || rel.as_bytes().get(1) == Some(&b':') {
        return Err(format!("场景路径必须是项目相对路径: {raw}"));
    }

    let path = Path::new(&rel);
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => return Err(format!("场景路径不能包含 `..` 或绝对路径: {raw}")),
        }
    }
    let normalized = parts.join("/");
    if !normalized.ends_with(".ron") || normalized.ends_with(".prefab.ron") {
        return Err(format!("场景文件必须以 .ron 结尾: {raw}"));
    }
    Ok(normalized)
}

/// 给新材质找一个不撞车的路径（`Wood.mat` → `Wood 2.mat` → …）。
///
/// 只查磁盘存在性，不依赖资产库索引：刚打开项目 / 索引还没刷新时也要正确。
fn unique_material_path(project: &ProjectHandle, base: &str) -> String {
    if !project.path(base).exists() {
        return base.to_string();
    }
    let stem = base
        .strip_suffix(&format!(".{}", xgame_core::MATERIAL_EXTENSION))
        .unwrap_or(base);
    for n in 2..10_000 {
        let candidate = format!("{stem} {n}.{}", xgame_core::MATERIAL_EXTENSION);
        if !project.path(&candidate).exists() {
            return candidate;
        }
    }
    base.to_string()
}

/// 场景视图中当前激活的变换工具。
#[derive(Debug, Clone, Copy, Display, PartialEq, Eq)]
pub enum ToolMode {
    Translate,
    Rotate,
    Scale,
    Rect,
}

/// 编辑器播放状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayMode {
    Edit,
    Playing,
    Paused,
}

/// 「未保存更改」确认框要执行的退出动作。
///
/// 退出有两条路——关窗口和返回项目枢纽——但**要确认的逻辑只有一份**，
/// 所以把"去哪"带在动作里，弹窗与放行流程对两条路一视同仁。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitTarget {
    /// 返回项目枢纽（同一窗口内换页，会丢弃当前会话）。
    Hub,
    /// 关闭编辑器窗口。
    Quit,
}

/// 有未保存改动时，用户请求的**场景切换**动作。
///
/// 和 [`ExitTarget`] 分开：退出是 Copy 的小枚举，切场景要带一条项目相对路径。
/// 两者共用「保存 / 不保存 / 取消」三选一的交互，但落点不同。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneChange {
    /// 打开另一个场景文件。
    Open(String),
    /// 新建场景（先走命名对话框）。
    New,
}

/// Edit 模式下一份「可调参数的组件副本」（纯数据，不挂 World、不跑钩子）。
///
/// 为什么需要它：组件实例只在 Play 时由注册表构造，Edit 模式下没有实例可改；
/// 而"先调好参数再 Play"恰恰是调参的常态。存两样东西是为了**对齐校验** ——
/// 用户把 `behaviors[0]` 从 A 换成 B 之后，旧副本的参数必须作废重建，
/// 否则会把 A 的参数写进 B 的字段里（两边都是 `f32` 的话，编译器管不着）。
struct EditComponent {
    /// 构造时的注册名，与场景里 `behaviors[下标]` 比对。
    name: String,
    component: Box<dyn xgame_runtime::Behavior>,
}

/// Gizmo 交互操作柄标识（与 ToolMode 组合使用）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GizmoPart {
    AxisX,
    AxisY,
    AxisZ,
    PlaneXY,
    PlaneXZ,
    PlaneYZ,
    Uniform,
    RingX,
    RingY,
    RingZ,
}

/// 活动拖拽状态。
#[derive(Debug, Clone)]
pub struct GizmoDrag {
    /// 被拖拽的操作柄。
    pub part: GizmoPart,
    /// 拖拽起始时 entity transform 的对应值（translation / scale 的 Vec3，或 rotation Euler）。
    ///
    /// Translate 时为**世界**位置；Scale 时为**局部**缩放（缩放天然定义在物体自身轴上）。
    pub start_value: Vec3,
    /// 拖拽平面上首次命中点的世界坐标。
    ///
    /// 只在「平面拖拽」与「旋转环」两种情形有意义：前者靠它与当前命中点求差得到位移，
    /// 后者靠它与实体位置求出起始角度方向。
    ///
    /// **不要**拿它算轴坐标——沿轴拖拽请用 [`start_axis`](Self::start_axis)。
    pub start_hit: Vec3,
    /// 沿轴拖拽的起始**轴坐标**：鼠标射线在「过实体、方向为轴」的直线上投影，
    /// 从 `entity_pos` 起算的有符号距离（世界单位）。
    ///
    /// 与 `start_hit` 的分工见 [`crate::gizmo::axis_param_under_mouse`] 的说明：
    /// 轴上的位移量必须用直线投影，用平面交点会恒得零。
    pub start_axis: f32,
    /// 轴拖拽参考线的**固定锚点**（世界坐标）——拖拽开始时实体的世界位置。
    ///
    /// # 为什么必须单独存一个锚点，不能直接用 `ctx.entity_pos`
    ///
    /// [`axis_param_under_mouse`](crate::gizmo::axis_param_under_mouse) 求的是
    /// 「过 `ctx.entity_pos`、方向为轴」的直线与鼠标射线的最近点参数 `s`。
    /// 而 `ctx` 是**每帧按当前位置重建**的，拖拽每帧又在改位置 —— 于是参考线
    /// 跟着物体一起跑，`s` 里混进了"物体自己刚挪的那一段"：
    ///
    /// ```text
    /// Δ_k = A_k − Δ_{k-1}        // A_k = 鼠标本帧"想"走的位移
    /// ```
    ///
    /// 这是一个增益为 −1 的正反馈：极点落在 `z = −1`（奈奎斯特），物体在相邻两帧
    /// 之间**来回跳**（拖拽抽搐），稳态只走到鼠标位移的一半，且把鼠标的像素级噪声
    /// 放大成可见的抖动。
    ///
    /// 把参考线锚死在拖拽起点，`s_now` 就只是鼠标射线的函数，环断开。
    pub axis_anchor: Vec3,
    pub start_screen_pos: egui::Pos2,
    /// 拖拽起始时实体的**世界**旋转。
    ///
    /// 旋转必须按"起始值 × 增量"重算，而不是每帧往当前值上乘增量——后者会在
    /// 一次拖拽里把同一个增量累积几十遍，表现为"轻轻一动就转过头"。
    pub start_rotation: Quat,
}

/// 控制台面板的日志级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
    Success,
}

impl LogLevel {
    /// 日志等级对应的文字颜色（走主题常量，别就地写 `Color32::YELLOW`——
    /// 纯黄纯红在暗色面板上过于刺眼，UE 的日志用的是压暗一档的暖黄/砖红）。
    pub fn color(&self) -> egui::Color32 {
        use crate::panels::theme::colors;
        match self {
            LogLevel::Info => colors::TEXT_PRIMARY,
            LogLevel::Warning => colors::WARN,
            LogLevel::Error => colors::ERROR,
            LogLevel::Success => colors::PLAY_GREEN,
        }
    }
}


/// 控制台保留的最大消息条数。
///
/// 面板只渲染最近 500 条（见 `panels/console.rs`），所以这个上限纯粹是**内存**
/// 护栏：一个每帧打日志的脚本不该把编辑器的内存拖垮。1000 条足够保留"刚才发生了什么"。
const MAX_CONSOLE_MESSAGES: usize = 1000;

/// 触顶时一次性丢弃的条数。
///
/// 不逐条丢：`Vec::remove(0)` 是 O(n) 的整表搬运，而 `log` 在脚本刷屏时是每帧
/// 都会被调用的热点。成批丢把这次搬运摊到几百条日志上，代价可以忽略。
const CONSOLE_TRIM_BATCH: usize = 200;

/// 单条控制台消息。
#[derive(Debug, Clone)]
pub struct LogMessage {
    pub level: LogLevel,
    pub text: String,
    pub count: u32,
}

impl LogMessage {
    pub fn new(level: LogLevel, text: impl Into<String>) -> Self {
        Self {
            level,
            text: text.into(),
            count: 1,
        }
    }
}

/// 自由飞行相机——yaw/pitch 控制朝向，`pan_*` 移动位置。
#[derive(Debug, Clone)]
pub struct EditorCamera {
    pub yaw: f32,
    pub pitch: f32,
    /// 世界空间中的相机位置。
    pub position: Vec3,
    pub sensitivity: f32,
    /// 平移速度（世界单位/秒）。
    pub pan_speed: f32,
}

impl EditorCamera {
    /// 右键转视角的默认灵敏度。
    pub const DEFAULT_SENSITIVITY: f32 = 0.0025;
    /// 默认平移速度（世界单位/秒）。
    pub const DEFAULT_PAN_SPEED: f32 = 5.0;
    /// 平移速度下限 / 上限（滚轮与偏好窗口共用同一对边界）。
    pub const MIN_PAN_SPEED: f32 = 0.5;
    pub const MAX_PAN_SPEED: f32 = 50.0;
    /// 转视角灵敏度下限 / 上限。
    pub const MIN_SENSITIVITY: f32 = 0.0005;
    pub const MAX_SENSITIVITY: f32 = 0.02;
}

impl Default for EditorCamera {
    fn default() -> Self {
        Self {
            yaw: -2.356,                // ≈ −135° — 从 (+, +, +) 方向看向原点
            pitch: -0.524,              // ≈ −30°  — 略微向下
            position: Vec3::new(5.0, 5.0, 5.0),
            sensitivity: Self::DEFAULT_SENSITIVITY,
            pan_speed: Self::DEFAULT_PAN_SPEED,
        }
    }
}

impl EditorCamera {
    
    /// 由 yaw/pitch 计算的前方向单位向量。
    pub fn forward(&self) -> Vec3 {
        Vec3::new(
            self.pitch.cos() * self.yaw.cos(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.sin(),
        )
        .normalize()
    }

    /// 观察点位置（自由飞行模式下等于 position）。
    pub fn eye(&self) -> Vec3 {
        self.position
    }

    pub fn rotate(&mut self, dx: f32, dy: f32) {
        self.yaw += dx * self.sensitivity;
        self.pitch -= dy * self.sensitivity;   // egui 的 dy 向下为正，所以取反
        self.pitch = self.pitch.clamp(
            -std::f32::consts::FRAC_PI_2 + 0.01,
            std::f32::consts::FRAC_PI_2 - 0.01,
        );
    }

    /// 用滚轮调整移动速度。
    pub fn zoom(&mut self, delta: f32) {
        self.pan_speed =
            (self.pan_speed + delta * 0.05).clamp(Self::MIN_PAN_SPEED, Self::MAX_PAN_SPEED);
    }

    /// 沿相机右方向平移（A / D）— 移动 position。
    pub fn pan_right(&mut self, amount: f32, dt: f32) {
        let fwd = self.forward();
        let right = fwd.cross(Vec3::Y).normalize();
        self.position += right * amount * self.pan_speed * dt;
    }

    /// 沿相机前方向的水平投影平移（W / S）— 移动 position。
    pub fn pan_forward(&mut self, amount: f32, dt: f32) {
        let fwd = self.forward();
        let fwd_h = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero();
        self.position += fwd_h * amount * self.pan_speed * dt;
    }

    /// 沿世界空间上/下方向升降（Q / E）。
    pub fn move_up(&mut self, amount: f32, dt: f32) {
        self.position.y += amount * self.pan_speed * dt;
    }
}

// ---------------------------------------------------------------------------
// Undo / Redo
// ---------------------------------------------------------------------------

/// 快照式撤销栈。
///
/// 每次可撤销的修改**之前**，把整个 `Scene` 压进 `undo`；撤销时把当前场景
/// 推进 `redo` 再回填旧场景。
///
/// # 为什么用整场景快照，而不是命令模式
///
/// 命令模式要求每个控件、每个操作都实现正向和反向两个动作——漏掉任何一个，
/// 都会变成"撤销之后状态错乱"的隐蔽 bug，而且很难测。快照只写一次、处处生效，
/// 代价是内存。本引擎的场景规模（几百个实体）下这个交换非常划算。
/// 真正的商业引擎会走增量命令，那是场景大到几十万对象、快照成本顶不住之后
/// 才必须做的优化——现在提前上只会把自己绕进去。
pub struct UndoStack {
    undo: Vec<Scene>,
    redo: Vec<Scene>,
    /// 栈深上限。超了就丢最旧的一帧——撤销历史不该无限涨内存。
    limit: usize,
}

impl UndoStack {
    /// 默认保留的步数。
    pub const DEFAULT_LIMIT: usize = 100;

    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: Self::DEFAULT_LIMIT,
        }
    }

    /// 压入一个"修改前"的快照，并清空重做栈。
    ///
    /// 新操作会作废原有的重做链——这是所有编辑器的一致语义：一旦从历史中间
    /// 分叉出去，原来的"前进方向"就不存在了。
    pub fn push(&mut self, snapshot: Scene) {
        self.undo.push(snapshot);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// 撤销：弹出上一个快照，把当前场景推进重做栈。
    pub fn undo(&mut self, current: &Scene) -> Option<Scene> {
        let previous = self.undo.pop()?;
        self.redo.push(current.clone());
        Some(previous)
    }

    /// 重做：与 [`undo`](Self::undo) 对称。
    pub fn redo(&mut self, current: &Scene) -> Option<Scene> {
        let next = self.redo.pop()?;
        self.undo.push(current.clone());
        Some(next)
    }

    /// 是否有可撤销的步骤。
    #[allow(dead_code)]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// 是否有可重做的步骤。
    #[allow(dead_code)]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Editor state
// ---------------------------------------------------------------------------

/// 材质编辑器里正在编辑的一份材质资产。
///
/// # 为什么需要草稿而不是直接改文件
///
/// 材质有 4 个字段 + 一个贴图引用，每拖一下数值框就写盘的话：
/// 1. 撤销 / 重做没有粒度（拖一次产生几十个中间状态）；
/// 2. 渲染侧要每帧失效重载（磁盘 + GPU 纹理）；
/// 3. 用户拖到一半反悔没有"取消"。
///
/// 所以控件只改草稿，显式点 Apply 才落盘；[`baseline`](Self::baseline) 保留
/// 打开时 / 上次保存后的内容，用来显示"未保存"并支持 Revert。
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialDraft {
    /// 目标文件（项目相对路径）。
    pub relative: String,
    /// 正在编辑的内容。
    pub asset: MaterialAsset,
    /// 上次加载 / 保存时的内容（脏检查与 Revert 的基准）。
    baseline: MaterialAsset,
}

impl MaterialDraft {
    fn new(relative: String, asset: MaterialAsset) -> Self {
        Self {
            relative,
            baseline: asset.clone(),
            asset,
        }
    }

    /// 是否有未保存的改动。
    pub fn dirty(&self) -> bool {
        self.asset != self.baseline
    }

    /// 丢弃未保存改动，回到上次加载 / 保存的内容。
    pub fn revert(&mut self) {
        self.asset = self.baseline.clone();
    }

    fn mark_saved(&mut self) {
        self.baseline = self.asset.clone();
    }
}

/// 所有面板共享的顶层编辑器状态。
pub struct EditorState {
    // -- Project --
    /// 当前打开的项目（含项目根目录与清单）。
    ///
    /// 场景文件的读写、模型路径的解析都以它为准。
    pub project: ProjectHandle,
    /// 请求「关闭项目」返回项目枢纽；由工具栏按钮置位，主循环消费。
    pub return_to_hub: bool,
    /// 最近一次「构建」的产物目录；`None` = 本次会话还没构建过。
    ///
    /// 工具栏据此决定要不要显示「打开输出目录」按钮。
    pub last_build_dir: Option<PathBuf>,

    // -- Scene --
    /// 当前加载的场景（实体 + 元数据）。
    pub scene: Scene,
    /// 当前编辑场景的**项目相对路径**（保存目标）。
    ///
    /// 它和 `scene.name` 是两件事：`scene.name` 是文件里的展示名，可以随便起；
    /// 本字段决定 `save_scene` 写哪个文件。切场景时必须一起换，否则在 Level2
    /// 里按 Ctrl+S 会把入口场景覆盖掉。
    pub current_scene: String,
    /// 当前选中实体在 `scene.entities` 中的索引；`None` = 未选中。
    pub selected_entity: Option<usize>,

    // -- Editor camera --
    pub editor_camera: EditorCamera,

    // -- Tool state --
    pub tool_mode: ToolMode,
    pub play_mode: PlayMode,

    // -- Toggles --
    pub show_grid: bool,
    pub show_physics_debug: bool,

    // -- Console --
    pub console_messages: Vec<LogMessage>,

    // -- New entity tracking --
    pub next_entity_id: u32,

    // -- Scene view (viewport dimensions, set by scene_view.rs each frame) --
    /// 视口大小（egui 逻辑像素）。
    pub viewport_logical_size: egui::Vec2,
    /// 视口大小（物理像素 = 逻辑像素 x pixels_per_point）。
    pub viewport_pixel_size: (u32, u32),
    /// 视口在窗口里的矩形（egui 逻辑点）。UI 指针映射用：
    /// 主循环在面板绘制**之前**推进 Play，只能用上一帧的矩形 —— egui 自己
    /// 处理输入也是这个模式（布局与输入天然差一帧）。
    pub viewport_rect: egui::Rect,

    // -- Fly mode state --
    /// 用户在视口上按住右键时为 `true`。
    /// 即使鼠标离开视口，在右键释放前保持 `true`。
    /// 控制 scene_view 中 WASD/QE 飞行移动的开关。
    pub fly_state: bool,

    /// 缩放速度叠加层的倒计时（秒）。
    /// 大于 0 时，scene_view 在视口上显示 "Speed: X.X"。
    pub zoom_indicator: f32,

    // -- Scene renderer (initialised by main.rs after state creation) --
    /// 可选的 3D 场景渲染器——wgpu 后端不可用时为 `None`。
    ///
    /// 渲染在 **UI 线程上同步进行**（见 `scene_renderer` 模块与 `scene_view::render_scene`）：
    /// 输入处理完 → 重画离屏 3D → 同帧上屏。编辑器视口必须让"输入到像素"零管线延迟
    /// （拖 gizmo / 转视角要钉在光标下），而指纹门控让静态帧整帧跳过、稳态渲染实测
    /// 单帧 CPU 仅 ~0.4ms —— 同步渲染又快又没有异步线程与窗口模态循环抢资源。
    pub scene_renderer: Option<crate::scene_renderer::SceneRenderer>,
    /// 本帧离屏渲染**实际用到**的视图投影矩阵（仅供测试）。
    ///
    /// 帧序是这个视口手感的核心不变量：**渲染必须发生在本帧输入之后**。反过来的话
    /// 画面永远是"上一帧的相机 / 场景"，拖物体、转视角都会白白慢一帧。这条不变量在
    /// 无头环境里没法从渲染结果上观察，所以把渲染那一刻的矩阵记下来，让测试能断言
    /// "渲染看到的就是本帧处理完的相机"。
    #[cfg(test)]
    pub(crate) last_render_view_proj: Option<glam::Mat4>,


    // -- Game runtime (play mode) --
    /// 游戏运行时——仅在 Play 模式下为 `Some`。
    /// 拥有物理世界、输入与脚本宿主。
    pub game_runtime: Option<crate::game_runtime::GameRuntime>,
    /// 进入 Play 模式时的场景快照。
    ///
    /// 必须存**整份场景**而不是只存变换：脚本可以 `spawn_entity` / `destroy`，
    /// 退出 Play 时要把这些实体一起收回去，只恢复变换会留下一堆"凭空多出来的
    /// 实体"。快照在进入 Play 时拍一次，代价可以接受。
    pub play_snapshot: Option<Scene>,

    /// Edit 模式下用户调好参数的组件副本（key = `(实体 id, behaviors 下标)`）。
    ///
    /// # 为什么 Edit 模式也要有"实例"
    ///
    /// 组件实例只在 Play 时由注册表构造（`Default` 起步），Edit 模式下根本
    /// 没有东西可改 —— 但"先拖滑块再按 Play 看效果"是 Unity 工作流的常态。
    /// 所以编辑态也持有一份**纯数据**的组件（没有 World、不跑任何钩子），
    /// Inspector 直接改它，按下 Play 时 [`Self::seed_play_from_edit_components`]
    /// 把值灌进真正跑起来的那份。
    ///
    /// 惰性构造：只有 Inspector 真要画某个组件时才造一次。组件的 `Default`
    /// 可能不便宜（比如要解析配置），不该"打开项目就构造全场景所有组件"。
    /// 构造时立刻套上场景里存的字段值（P1.5 的 `behavior_params`，见
    /// [`Self::inspect_component`]）—— Edit 看到的、Play 跑的、打包玩家拿到的
    /// 必须是同一份数。
    ///
    /// **声明位置是硬约束**：值里装着指向插件 DLL 的函数指针，必须**先于
    /// `plugins`（DLL 句柄）销毁**，否则 drop 胶水会跳进已 FreeLibrary 的代码区。
    /// 同理它得排在 `game_runtime` 之后 —— 那份 `World` 里的组件实例同理。
    edit_components: HashMap<(EntityId, usize), EditComponent>,

    // -- Rust 插件（P0.5 / UE 模式）--
    /// 插件会话：已加载 DLL 句柄 + 当前代组件名。
    ///
    /// **声明位置是硬约束**：struct 字段按声明顺序 drop，`game_runtime`
    /// （其 `World` 的组件实例与注册表里全是**指向插件 DLL 的函数指针**）必须
    /// 先于本字段销毁，否则 drop 胶水会跳进已 FreeLibrary 的代码区。详见
    /// [`crate::plugin_session`] 模块文档。
    pub plugins: crate::plugin_session::PluginSession,
    /// 正在飞行的后台 `cargo build`；`None` = 空闲。
    pub plugin_build: Option<crate::plugin_build::BuildTask>,
    /// 用户按了 ▶ Play 但插件还没编译完：构建成功后自动进 Play。
    ///
    /// 按下 Play 的那次调用会先退回到 Edit（此时没有可跑的运行时），编译
    /// 完成再由 [`poll_plugin_build`](Self::poll_plugin_build) 真正进入 ——
    /// 用户视角是"按了 Play，转了两秒，自己跑起来了"，UE 同款。
    pub pending_play: bool,

    // -- Save / exit --
    /// 上次**落盘（或打开）时**的场景快照 —— 与它逐字段比较即知有没有未保存改动。
    ///
    /// 用快照比较而不是给每处改场景的代码塞 `mark_dirty()`：改场景的入口太多
    /// （Inspector 就有几十处 `.changed()`、gizmo 逐帧写变换），漏一处就是
    /// "改了却不提示保存"。比较是 O(实体数)，只在真正询问时做（关闭 / 回枢纽 /
    /// 工具栏按钮），并按帧 memo —— 见 [`scene_dirty_memo`](Self::scene_dirty_memo)。
    saved_snapshot: Scene,
    /// [`scene_dirty_memo`](Self::scene_dirty_memo) 的按帧缓存 `(pass_nr, 结果)`。
    dirty_memo: Option<(u64, bool)>,
    /// 「场景另存为」对话框是否显示。
    pub save_as_open: bool,
    /// 「场景另存为」的名称输入。
    pub save_as_name: String,
    /// 「新建场景」对话框是否显示。
    pub new_scene_open: bool,
    /// 「新建场景」的名称输入。
    pub new_scene_name: String,
    /// 有未保存改动时暂存的场景切换请求；`Some` 时弹确认框。
    pub pending_scene_change: Option<SceneChange>,
    /// 「项目设置」窗口是否显示。
    pub settings_open: bool,
    /// 请求打开「编辑器偏好」窗口。
    ///
    /// 工具栏只知道 `EditorState`，而偏好窗口还要操作停靠布局树（在
    /// `EditorSession` 里），所以这里只是一个一次性请求位，由主循环消费。
    pub open_preferences: bool,
    /// 设置窗口的编辑草稿：打开时克隆清单，按 Apply 才写回。
    ///
    /// 半成品设置不该边敲键盘边生效 —— 与 Undo 同理，确认才落地。
    pub settings_draft: Option<xgame_core::project::Project>,
    /// 待确认的退出动作；`Some` 时弹「未保存更改」确认框。
    pub exit_dialog: Option<ExitTarget>,
    /// 用户已在确认框里放行（保存成功 / 选了不保存），主循环消费后直接执行。
    ///
    /// 存在的意义：「不保存」之后场景依旧是脏的（内存 ≠ 磁盘），只看脏标记会
    /// 把确认框再弹一遍死循环 —— 放行是一次性凭证。
    pub exit_confirmed: Option<ExitTarget>,

    // -- Clipboard --
    /// 复制/粘贴用的实体剪贴板（整棵子树，根在前）。
    ///
    /// 存的是实体快照而不是"指向某个索引的引用"——原实体在粘贴前可能已经
    /// 被删掉，存引用会拿到悬空索引。带子树的副本在粘贴时会重新分配所有 ID，
    /// 并按映射还原内部父子关系。
    clipboard: Option<Vec<Entity>>,

    // -- Hierarchy panel --
    /// 层级面板里被折叠（不展开显示子节点）的实体 ID 集合。
    ///
    /// 按 **ID** 而不是索引记录：删除/撤销让索引整体位移，按索引记会让"折叠"
    /// 莫名其妙地跳到别的实体上。ID 认人的实体一旦被删，这条记录自然失效，
    /// 无需维护。
    pub collapsed_entities: HashSet<EntityId>,
    /// 层级面板中正在被拖拽的实体（用于拖拽重挂父节点）。
    pub hierarchy_drag: Option<EntityId>,
    /// 层级面板的搜索过滤词。
    ///
    /// 必须存在状态里：它是跨帧保持的用户输入，之前在面板里写成局部变量、
    /// 每帧被重置成空串，等于搜索功能根本没生效。
    pub hierarchy_filter: String,

    // -- Prefabs --
    /// 已发现的预制体资产（**项目相对路径**，已排序）。
    ///
    /// 缓存在状态里，而不是每次画面板都去扫盘：目录扫描是一串系统调用，
    /// 逐帧做就是典型的"界面明明没动却在耗 CPU"。改动资产后显式调
    /// [`refresh_prefabs`](Self::refresh_prefabs) 刷新。
    pub prefab_list: Vec<String>,
    /// 预制体面板的搜索过滤词（跨帧保持）。
    pub prefab_filter: String,
    /// 「从选中创建预制体」的命名对话框是否显示。
    pub prefab_save_dialog: bool,
    /// 命名对话框中的名称输入。
    pub prefab_save_name: String,
    /// 待在确认框里确认删除的预制体路径；`Some` 时弹出删除确认。
    pub prefab_delete_pending: Option<String>,

    // -- Scripts --
    /// 已发现的脚本资产（**项目相对路径**，已排序）。
    ///
    /// 与 [`prefab_list`](Self::prefab_list) 同理：缓存在状态里，
    /// 只在这几处刷新 —— 打开项目、Play 前重新扫描、点 ⟳。
    pub script_list: Vec<String>,

    // -- Assets（资源浏览器）--
    /// 已登记的资产（项目相对路径 + 类型，按路径排序）。
    ///
    /// 来源是 `ProjectHandle` 的资产库（有 `.meta` 的才在里面），不是逐帧扫盘。
    /// 与 `prefab_list` 的关系：预制体既在这里（浏览器要显示），也在
    /// `prefab_list`（预制体面板/Create from Selection 工作流用）。
    pub asset_browser: Vec<(String, xgame_core::AssetKind)>,
    /// 资源浏览器的搜索过滤词（跨帧保持）。
    pub asset_filter: String,
    /// 资源浏览器里选中的资产（项目相对路径）。
    pub asset_selected: Option<String>,
    /// 正在从资源浏览器拖进视口的资产（`Some` 时鼠标处于"拖资产"状态）。
    ///
    /// 刻意不用 egui 的 `DragAndDrop` payload：视口是一块自绘的离屏纹理，
    /// 拖拽的命中与落点都由我们自己算（见 `panels::scene_view`），
    /// 一条 `Option<String>` 比跨控件传 payload 更直白，也更好测。
    pub asset_drag: Option<String>,

    // -- Materials --
    /// 当前在资源面板里编辑的材质草稿（`None` = 没选中材质）。
    pub material_draft: Option<MaterialDraft>,
    /// 材质草稿加载失败的 `(路径, 原因)` —— 负缓存，避免每帧重复读坏文件 + 刷日志。
    pub material_draft_error: Option<(String, String)>,
    /// 「新建材质」命名对话框是否显示。
    pub material_create_dialog: bool,
    /// 对话框里的材质名。
    pub material_create_name: String,
    /// 当前打开的浮动材质编辑器（项目相对路径，`None` = 窗口关闭）。
    ///
    /// 材质编辑器不内嵌在 Assets 面板里：底部资源面板在 1280×800 下只有
    /// ~150px 高，四个字段 + 三个按钮必然被裁掉。改成浮动窗口后，面板里只留
    /// 路径 / GUID / 「编辑材质… / 指派」两行，窗口按内容自动定高。
    pub material_editor_open: Option<String>,

    // -- Undo / redo --
    /// 快照式撤销栈（场景级）。
    undo_stack: UndoStack,
    /// 拖拽交互中暂存的"拖拽前"快照。拖拽期间逐帧入栈会毁掉撤销粒度，
    /// 所以先挂在这里，松手时才真正压进 `undo_stack`。
    pub undo_pending: Option<Scene>,

    // -- Gizmo state --
    /// 当前鼠标悬停的 gizmo 操作柄（用于高亮绘制）。
    pub gizmo_hovered: Option<GizmoPart>,
    /// 当前正在拖拽的操作柄状态（`Some` 时处于拖拽中）。
    pub gizmo_active: Option<GizmoDrag>,
    /// 拖拽捕捉步长（0 表示不捕捉）。
    #[allow(dead_code)]
    pub gizmo_snap: f32,

    // -- Hierarchy panel cache --
    /// 折叠集合的版本号：每次折叠/展开切换 +1。
    ///
    /// 层级面板的行缓存以它为 key 的一部分 —— 直接哈希 `HashSet` 不可靠
    /// （迭代序随内部布局变），显式版本号既稳定又是 O(1)。
    pub collapsed_version: u64,
    /// 层级面板的行模型缓存（指纹没变就整段复用，见 `panels::hierarchy::RowCache`）。
    pub hierarchy_cache: crate::panels::hierarchy::RowCache,

    // -- Profiler panel --
    /// 帧采样环形缓冲（数据面见 [`crate::profiler`]）。
    ///
    /// 主循环在帧末往这里写一条，面板随时读 —— 采集与绘制完全解耦：面板没被打开
    /// 时照样在采（切过去就能看到刚才那段），关掉面板也不影响采样成本（一次写入）。
    pub profiler: crate::profiler::Profiler,
}

impl EditorState {
    /// 打开一个项目：加载其入口场景并初始化编辑器状态。
    pub fn new(project: ProjectHandle) -> Self {
        // 入口场景 = 本次会话的起始「当前场景」。之后切换场景只改
        // `current_scene`，不会再碰清单里的启动场景（那是显式操作）。
        let current_scene = project.manifest().entry_scene.clone();
        let scene = match project.load_scene(&current_scene) {
            Ok(scene) => scene,
            Err(e) => {
                eprintln!("[EditorState] {e}");
                xgame_core::project::default_scene()
            }
        };
        let mut state = Self {
            project,
            return_to_hub: false,
            last_build_dir: None,

            scene,
            current_scene,
            selected_entity: None,

            editor_camera: EditorCamera::default(),
            tool_mode: ToolMode::Translate,
            play_mode: PlayMode::Edit,
            show_grid: true,
            show_physics_debug: false,

            console_messages: Vec::new(),
            next_entity_id: 1,

            viewport_logical_size: egui::Vec2::ZERO,
            viewport_rect: egui::Rect::NOTHING,
            viewport_pixel_size: (0, 0),

            fly_state: false,
            zoom_indicator: 0.0,

            scene_renderer: None,
            #[cfg(test)]
            last_render_view_proj: None,
            game_runtime: None,
            play_snapshot: None,
            edit_components: HashMap::new(),

            plugins: crate::plugin_session::PluginSession::new(),
            plugin_build: None,
            pending_play: false,

            saved_snapshot: Scene::new(""),
            dirty_memo: None,
            save_as_open: false,
            save_as_name: String::new(),
            new_scene_open: false,
            new_scene_name: String::new(),
            pending_scene_change: None,
            settings_open: false,
            open_preferences: false,
            settings_draft: None,
            exit_dialog: None,
            exit_confirmed: None,

            clipboard: None,
            collapsed_entities: HashSet::new(),
            hierarchy_drag: None,
            hierarchy_filter: String::new(),

            prefab_list: Vec::new(),
            prefab_filter: String::new(),
            prefab_save_dialog: false,
            prefab_save_name: String::new(),
            prefab_delete_pending: None,

            script_list: Vec::new(),

            asset_browser: Vec::new(),
            asset_filter: String::new(),
            asset_selected: None,
            asset_drag: None,

            material_draft: None,
            material_draft_error: None,
            material_create_dialog: false,
            material_create_name: String::new(),
            material_editor_open: None,

            undo_stack: UndoStack::new(),
            undo_pending: None,

            gizmo_hovered: None,
            gizmo_active: None,
            gizmo_snap: 0.0,

            collapsed_version: 0,
            hierarchy_cache: crate::panels::hierarchy::RowCache::default(),

            profiler: crate::profiler::Profiler::new(),
        };

        // 打开项目时先把资产清单扫出来，面板第一帧就有内容。
        state.refresh_prefabs();
        state.refresh_scripts();
        state.refresh_assets();
        // 脏检查的基准：刚打开的场景 = 刚落盘的内容 = 干净。
        state.saved_snapshot = state.scene.clone();
        state
    }

    /// 重新扫描项目里的脚本（`.lua`，项目相对路径，已排序）。
    ///
    /// 与预制体清单同样**不每帧扫盘**：目录扫描是一串系统调用，
    /// 逐帧做就是"界面没动却在耗 CPU"。
    pub fn refresh_scripts(&mut self) {
        self.script_list = self.project.list_scripts();
    }

    // -----------------------------------------------------------------------
    // 资源浏览器（Assets）
    // -----------------------------------------------------------------------

    /// 重新扫描资产库，并刷新三个依赖它的列表（资产 / 预制体 / 脚本）。
    ///
    /// 会顺手给缺 `.meta` 的资产生成身份证 —— 这是"打开项目 / 导入资产"该做的
    /// 事。`--check` 那条路不写盘（检查必须只读），两者刻意分开。
    pub fn refresh_assets(&mut self) {
        // `ensure_asset_metas` 内部会先重新扫盘，所以这里不必再单独 refresh。
        if let Err(e) = self.project.ensure_asset_metas() {
            // 只读目录 / 权限问题：仍然把已扫到的列出来，别让面板空着。
            self.log_warning(format!("生成资产 .meta 失败: {e}"));
        }
        self.asset_browser = self.project.list_assets();
        // 资产变了，预制体清单与脚本清单可能跟着变（同一份磁盘状态）。
        self.prefab_list = self.project.list_prefabs();
        self.script_list = self.project.list_scripts();
    }

    /// 某个资产的导入设置（UI 显示用）。
    pub fn asset_import(&self, relative: &str) -> ImportSettings {
        self.project.asset_import(relative)
    }

    /// 改写某个资产的导入设置（写回 `.meta`）。
    pub fn set_asset_import(&mut self, relative: &str, import: ImportSettings) {
        match self.project.set_asset_import(relative, import) {
            Ok(()) => {
                // 导入参数变了，渲染侧的模型缩放会在下一次加载时生效；
                // 索引也要跟着刷新（`record.import` 是缓存）。
                self.asset_browser = self.project.list_assets();
                // `model_scale` / sRGB / 最近邻都烘焙在已加载的网格与 GPU 纹理里：
                // 不丢缓存的话"改了导入参数但视口纹丝不动"。
                if let Some(renderer) = self.scene_renderer.as_mut() {
                    renderer.invalidate_assets();
                }
            }
            Err(e) => self.log_error(format!("保存导入设置失败: {e}")),
        }
    }

    // -----------------------------------------------------------------------
    // 材质资产
    // -----------------------------------------------------------------------

    /// 项目里所有材质（项目相对路径，按字典序）。
    ///
    /// 从已缓存的资产清单过滤，不扫盘 —— Inspector 的下拉框每帧都可能重建。
    pub fn material_list(&self) -> Vec<String> {
        self.asset_browser
            .iter()
            .filter(|(_, kind)| *kind == AssetKind::Material)
            .map(|(path, _)| path.clone())
            .collect()
    }

    /// 取得（必要时加载）某个材质的编辑草稿。
    ///
    /// 坏文件只读一次：失败原因存进 [`material_draft_error`](Self::material_draft_error)，
    /// 同一路径再次请求直接返回 `None`，不会每帧刷日志。修好文件后点 ⟳ 或重新选中
    /// 另一个资产再回来即可重试。
    pub fn material_draft_mut(&mut self, relative: &str) -> Option<&mut MaterialDraft> {
        if let Some((failed, _)) = &self.material_draft_error {
            if failed == relative {
                return None;
            }
        }
        let current = self
            .material_draft
            .as_ref()
            .is_some_and(|d| d.relative == relative);
        if !current {
            match self.project.load_material(relative) {
                Ok(asset) => {
                    self.material_draft = Some(MaterialDraft::new(relative.to_string(), asset));
                    self.material_draft_error = None;
                }
                Err(e) => {
                    self.material_draft = None;
                    self.material_draft_error = Some((relative.to_string(), e.clone()));
                    self.log_error(format!("材质 '{relative}' 加载失败: {e}"));
                    return None;
                }
            }
        }
        self.material_draft.as_mut()
    }

    /// 材质草稿当前的加载错误（资源面板显示用）。
    pub fn material_draft_error(&self, relative: &str) -> Option<&str> {
        self.material_draft_error
            .as_ref()
            .filter(|(path, _)| path == relative)
            .map(|(_, message)| message.as_str())
    }

    /// 把草稿写回 `.mat`，并让渲染侧下一帧重新加载。
    ///
    /// 成功时刷新资产清单（新文件的 `.meta` 在这一步补上）并把草稿基准推进
    /// 到刚保存的内容。播放态硬拒绝：场景里跑的是运行时副本，改资产不该在
    /// 这一帧偷偷影响正在跑的游戏。
    pub fn save_material_draft(&mut self) {
        if !self.is_editing() {
            self.log_warning("Cannot edit materials while playing");
            return;
        }
        let Some(draft) = self.material_draft.as_ref() else {
            return;
        };
        let relative = draft.relative.clone();
        let asset = draft.asset.clone();
        match self.project.write_material(&relative, &asset) {
            Ok(()) => {
                self.refresh_assets();
                if let Some(draft) = self.material_draft.as_mut() {
                    draft.mark_saved();
                }
                self.asset_selected = Some(relative.clone());
                self.material_draft_error = None;
                if let Some(renderer) = self.scene_renderer.as_mut() {
                    renderer.invalidate_materials();
                }
                self.log_success(format!("Saved material '{relative}'"));
            }
            Err(e) => self.log_error(format!("保存材质 '{relative}' 失败: {e}")),
        }
    }

    /// 新建一份材质（默认参数 + 白底），返回项目相对路径。
    ///
    /// 重名自动加 ` 2` / ` 3` 后缀，不覆盖已有文件 —— 资产文件覆盖是
    /// 不可撤销的破坏性操作，不该由一次"新建"顺手完成。
    pub fn create_material(&mut self, name: &str) -> Option<String> {
        if !self.is_editing() {
            self.log_warning("Cannot create materials while playing");
            return None;
        }
        let relative = unique_material_path(&self.project, &self.project.material_path_for(name));
        let asset = MaterialAsset::default();
        if let Err(e) = self.project.write_material(&relative, &asset) {
            self.log_error(format!("创建材质 '{relative}' 失败: {e}"));
            return None;
        }
        self.refresh_assets();
        self.asset_selected = Some(relative.clone());
        self.material_draft = Some(MaterialDraft::new(relative.clone(), asset));
        self.material_draft_error = None;
        if let Some(renderer) = self.scene_renderer.as_mut() {
            renderer.invalidate_materials();
        }
        self.log_success(format!("Created material '{relative}'"));
        Some(relative)
    }

    /// 把一份材质资产指派给实体（拖放 / 双击 / 资源面板按钮共用）。
    ///
    /// 只改 `entity.material_asset`，不动逐实体覆盖 —— 两层正交，用户可以
    /// "资产定基调 + 覆盖微调这个实例"。
    pub fn assign_material(&mut self, entity_index: usize, relative: &str) -> bool {
        if !self.is_editing() {
            self.log_warning("Cannot assign materials while playing");
            return false;
        }
        if AssetKind::from_path(relative) != Some(AssetKind::Material) {
            self.log_warning(format!("'{relative}' 不是材质资产"));
            return false;
        }
        if entity_index >= self.scene.entities.len() {
            return false;
        }
        self.push_undo();
        self.scene.entities[entity_index].material_asset = Some(relative.to_string());
        self.selected_entity = Some(entity_index);
        self.log_success(format!(
            "Assigned material '{relative}' to '{}'",
            self.scene.entities[entity_index].name
        ));
        true
    }

    /// 把资产放进场景，返回新实体索引。
    ///
    /// 模型 = 新建实体；预制体 = 实例化；材质 = 指派给当前选中实体（没有选中
    /// 时给一句提示 —— 材质是"贴在物体上"的，没有目标就没有意义）。
    /// `position` 是**世界坐标**（视口里拖放时的落点由调用方算好；双击则是
    /// 相机前方）。整次放置只压一次撤销栈。
    pub fn spawn_asset_at(&mut self, relative: &str, position: Vec3) -> Option<usize> {
        if !self.is_editing() {
            self.log_warning("Cannot place assets while playing");
            return None;
        }
        let kind = AssetKind::from_path(relative)?;
        match kind {
            AssetKind::Material => {
                let Some(index) = self.selected_entity else {
                    self.log_warning("先选中一个实体，再把材质拖到它身上");
                    return None;
                };
                self.assign_material(index, relative);
                Some(index)
            }
            AssetKind::Model => {
                self.push_undo();
                let id = self.next_entity_id;
                self.next_entity_id += 1;
                let idx = self.scene.entities.len();
                let mut entity = Entity::new(format!("{} ({id})", asset_stem(relative)));
                entity.model = relative.to_string();
                entity.transform.position = position;
                self.scene.entities.push(entity);
                self.selected_entity = Some(idx);
                self.log_success(format!("Placed model '{relative}'"));
                Some(idx)
            }
            AssetKind::Prefab => {
                // `instantiate_prefab_by_path` 自己压撤销栈并落在根层 ——
                // 根层意味着局部坐标就是世界坐标，直接改 position 即可。
                let idx = self.instantiate_prefab_by_path(relative)?;
                if let Some(entity) = self.scene.entities.get_mut(idx) {
                    entity.transform.position = position;
                }
                Some(idx)
            }
            other => {
                self.log_warning(format!(
                    "{} 资产不能直接放进场景（只有模型与预制体可以）",
                    other.label()
                ));
                None
            }
        }
    }

    /// 双击放置的默认落点：相机正前方 5 米，且不低于地面。
    ///
    /// 不放在相机脚下（会挡住视线），也不放在原点（相机可能正看着别处）。
    /// 夹到 `y >= 0` 是因为项目约定地面在 y=0 附近：放到地下会让人以为
    /// "拖进来但没渲染"。
    pub fn placement_point_in_front(&self) -> Vec3 {
        let mut point = self.editor_camera.eye() + self.editor_camera.forward() * 5.0;
        point.y = point.y.max(0.0);
        point
    }


    /// 是否处于编辑模式（非 Play 且非 Pause）。
    pub fn is_editing(&self) -> bool {
        self.play_mode == PlayMode::Edit
    }

    /// 选中实体（只读快捷方式）。
    ///
    /// 目前各处都是直接用 `state.scene.entities[idx]`，这条没人调用；保留是因为
    /// 它把「当前选中」这层语义包了一层，配合 [`entity_mut`](Self::entity_mut) 成对。
    #[allow(dead_code)]
    pub fn entity(&self) -> Option<&Entity> {
        self.selected_entity.map(|i| &self.scene.entities[i])
    }

    /// 选中实体的可变引用。
    ///
    /// 检视面板改成了"在实体副本上渲染控件、确认有改动才写回"（见
    /// `panels/inspector.rs` 里的撤销集成说明），所以这条直接改场景的快捷方式
    /// 暂时没人用了。留着是因为按索引直接改单个实体仍然是最自然的写法，
    /// 后续要加"不做撤销的临时编辑"时会用到。
    #[allow(dead_code)]
    pub fn entity_mut(&mut self) -> Option<&mut Entity> {
        let i = self.selected_entity?;
        self.scene.entities.get_mut(i)
    }

    // -- 剪贴板 --

    /// 把选中实体**及其整棵子树**复制进剪贴板。不改动场景，也不进撤销栈。
    pub fn copy_selected(&mut self) {
        let Some(idx) = self.selected_entity else {
            return;
        };
        let subtree = self.scene.subtree(idx);
        let items: Vec<Entity> = subtree
            .iter()
            .map(|&i| self.scene.entities[i].clone())
            .collect();
        let Some(label) = items.first().map(|e| e.name.clone()) else {
            return;
        };
        let extra = items.len().saturating_sub(1);
        self.clipboard = Some(items);
        if extra > 0 {
            self.log_info(format!("Copied '{label}' (+{extra} children)"));
        } else {
            self.log_info(format!("Copied '{label}'"));
        }
    }

    /// 把剪贴板里的子树粘贴进场景。
    ///
    /// 粘贴出的副本重新分配全部 ID，并按映射还原内部父子关系；子树根落到
    /// **场景根层**（避免引用一个可能已经被删掉的外部父节点）。
    pub fn paste_entity(&mut self) {
        if !self.is_editing() {
            self.log_warning("Cannot paste while playing");
            return;
        }
        let Some(items) = self.clipboard.clone() else {
            self.log_warning("Clipboard is empty");
            return;
        };
        let Some(root_name) = items.first().map(|e| e.name.clone()) else {
            return;
        };

        self.push_undo();

        // 先给每个待粘贴实体分配新 ID，建立 旧→新 映射
        let mut id_map: HashMap<EntityId, EntityId> = HashMap::new();
        for entity in &items {
            id_map.insert(entity.id, EntityId::new());
        }

        let mut first_idx = None;
        for (k, original) in items.iter().enumerate() {
            let mut entity = original.clone();
            entity.id = id_map[&original.id];
            entity.parent = match original.parent {
                Some(pid) if id_map.contains_key(&pid) => Some(id_map[&pid]),
                _ => None, // 粘贴到根层
            };
            entity.name = format!("{} (Copy)", original.name);
            if k == 0 {
                // 错开一点位置。完全重叠的话用户看不出粘贴成功了，会以为快捷键没生效。
                entity.transform.position.x += 0.5;
                entity.transform.position.z += 0.5;
            }
            let new_idx = self.scene.entities.len();
            self.scene.entities.push(entity);
            if k == 0 {
                first_idx = Some(new_idx);
            }
        }
        self.selected_entity = first_idx;
        self.log_info(format!("Pasted '{root_name}'"));
    }

    // -- 撤销 / 重做 --

    /// 记录当前场景到撤销栈。必须在修改**之前**调用。
    ///
    /// 时机很关键：如果先改了再调，记下来的就是改后的状态，Ctrl+Z 相当于
    /// "撤销到当前"，看起来像按钮没反应——这类 bug 排查起来很费劲。
    pub fn push_undo(&mut self) {
        self.undo_stack.push(self.scene.clone());
    }

    /// 压入一个**已经准备好**的快照。
    ///
    /// 拖拽交互需要的是"拖拽开始前"的状态，不能在这里现克隆，所以由调用方
    /// 在合适的时机把快照备好再交进来。
    pub fn push_undo_snapshot(&mut self, snapshot: Scene) {
        self.undo_stack.push(snapshot);
    }

    /// 撤销一步。Play 模式下场景被锁定，不参与撤销。
    pub fn undo(&mut self) {
        if !self.is_editing() {
            self.log_warning("Cannot undo while playing");
            return;
        }
        match self.undo_stack.undo(&self.scene) {
            Some(previous) => {
                self.scene = previous;
                self.clamp_selection();
                self.undo_pending = None;
                self.log_info("Undo");
            }
            None => self.log_warning("Nothing to undo"),
        }
    }

    /// 重做一步。
    pub fn redo(&mut self) {
        if !self.is_editing() {
            self.log_warning("Cannot redo while playing");
            return;
        }
        match self.undo_stack.redo(&self.scene) {
            Some(next) => {
                self.scene = next;
                self.clamp_selection();
                self.undo_pending = None;
                self.log_info("Redo");
            }
            None => self.log_warning("Nothing to redo"),
        }
    }

    /// 是否有可撤销的步骤。
    ///
    /// 目前没有 UI 消费它（撤销只有 Ctrl+Z 快捷键，不依赖可撤销状态），
    /// 保留给「工具栏按钮按状态置灰」用。
    #[allow(dead_code)]
    pub fn can_undo(&self) -> bool {
        self.undo_stack.can_undo()
    }

    /// 是否有可重做的步骤。见 [`can_undo`](Self::can_undo) 的说明。
    #[allow(dead_code)]
    pub fn can_redo(&self) -> bool {
        self.undo_stack.can_redo()
    }

    /// 实体数量变了之后，把选中索引收敛回合法范围。
    ///
    /// # 为什么必须是"每帧开头"这一道
    ///
    /// `selected_entity` 是**按位置**记的索引，而改变实体数量或顺序的操作有十几处
    /// （撤销 / 重做 / 删除 / 复制 / 粘贴 / 预制体实例化与还原 / 从资产实例化……）。
    /// 只要有一处忘了同步，下一帧 `entities[idx]` 就会**直接 panic，整个编辑器挂掉** ——
    /// 这不是"显示不对"，是"进程没了"，与"像商业引擎一样稳定"直接冲突。
    ///
    /// 与其在十几处逐个补 `get()`，不如在每帧入口做一次 O(1) 收敛：越界就夹回
    /// 合法的邻居（空场景则清空选择）。这层"总闸"让所有路径天然安全，新增操作
    /// 也不必再记得维护它。
    pub fn clamp_selection(&mut self) {
        if let Some(i) = self.selected_entity {
            if i >= self.scene.entities.len() {
                self.selected_entity = if self.scene.entities.is_empty() {
                    None
                } else {
                    Some(self.scene.entities.len() - 1)
                };
            }
        }
    }

    // -- Console helpers --

    pub fn log_info(&mut self, text: impl Into<String>) {
        self.log(LogLevel::Info, text);
    }

    pub fn log_warning(&mut self, text: impl Into<String>) {
        self.log(LogLevel::Warning, text);
    }

    pub fn log_error(&mut self, text: impl Into<String>) {
        self.log(LogLevel::Error, text);
    }

    pub fn log_success(&mut self, text: impl Into<String>) {
        self.log(LogLevel::Success, text);
    }

    pub fn log(&mut self, level: LogLevel, text: impl Into<String>) {
        let text: String = text.into();
        // Deduplicate consecutive identical messages
        if let Some(last) = self.console_messages.last_mut() {
            if last.level == level && last.text == text {
                last.count += 1;
                return;
            }
        }
        // 超上限就**成批**丢掉最旧的一批。
        //
        // 去重只挡得住"连续完全相同的消息"；一个每帧打一条**内容不同**日志的脚本
        // 能在几分钟里把这张表撑到几十万条 —— 面板只渲染最近 500 条不会卡，但内存
        // 是真的一直在涨。成批丢而不是逐条 `remove(0)`：后者每帧都要搬一次整表。
        if self.console_messages.len() >= MAX_CONSOLE_MESSAGES {
            self.console_messages.drain(..CONSOLE_TRIM_BATCH);
        }
        self.console_messages
            .push(LogMessage::new(level, text));
    }

    // -- 实体操作 --

    /// 向场景添加一个新的空实体并返回其索引。
    ///
    /// 在 Play 模式下不可用。
    pub fn create_entity(&mut self) -> usize {
        if !self.is_editing() {
            self.log_warning("Cannot create entities while playing");
            return usize::MAX;
        }
        self.push_undo();
        let id = self.next_entity_id;
        self.next_entity_id += 1;
        let idx = self.scene.entities.len();
        self.scene.entities.push(
            Entity::new(format!("GameObject ({})", id)),
        );
        self.selected_entity = Some(idx);
        self.log_info(format!("Created new GameObject ({})", id));
        idx
    }

    /// 创建一个屏幕空间 UI 元素（对应商业引擎的 `Create > UI > ...`）。
    ///
    /// 默认摆在左上角内缩 40px 的位置：全部叠在 (0,0) 的话，新建第二个元素
    /// 就完全看不见（正好盖住第一个），作者会以为"创建没生效"。
    pub fn create_ui(&mut self, kind: xgame_core::UiKind) -> usize {
        if !self.is_editing() {
            self.log_warning("Cannot create UI while playing");
            return usize::MAX;
        }
        self.push_undo();
        let id = self.next_entity_id;
        self.next_entity_id += 1;
        let idx = self.scene.entities.len();

        let (label, element) = match kind {
            xgame_core::UiKind::Panel => ("UI Panel", xgame_core::UiElement::panel(320.0, 180.0)),
            xgame_core::UiKind::Text => ("UI Label", xgame_core::UiElement::text("Text")),
            xgame_core::UiKind::Image => ("UI Image", xgame_core::UiElement::image("")),
            xgame_core::UiKind::Button => ("UI Button", xgame_core::UiElement::button("Button")),
        };
        let size = element.size();
        let element = element.at([0.0, 0.0], 40.0, 40.0, size[0], size[1]);

        let mut entity = Entity::new(format!("{label} ({id})"));
        entity.ui = Some(element);
        self.scene.entities.push(entity);
        self.selected_entity = Some(idx);
        self.log_info(format!("Created {label} ({id})"));
        idx
    }

    /// 用内置基础图元创建实体（对应商业引擎 `Create > 3D Object`）。
    ///
    /// 和 [`create_entity`](Self::create_entity) 的唯一区别是给实体填上
    /// `builtin://...` 形式的模型标识——渲染器认出它之后直接生成几何，
    /// 不需要磁盘上真的存在模型文件。
    pub fn create_primitive(&mut self, kind: xgame_render::mesh::BuiltinMesh) -> usize {
        if !self.is_editing() {
            self.log_warning("Cannot create entities while playing");
            return usize::MAX;
        }
        self.push_undo();
        let id = self.next_entity_id;
        self.next_entity_id += 1;
        let idx = self.scene.entities.len();
        let mut entity = Entity::new(format!("{} ({})", kind.label(), id));
        entity.model = kind.uri();
        self.scene.entities.push(entity);
        self.selected_entity = Some(idx);
        self.log_info(format!("Created {} ({})", kind.label(), id));
        idx
    }

    /// 复制当前选中的实体**及其整棵子树**（对应 Unity 的 Ctrl+D）。
    ///
    /// 在 Play 模式下不可用。副本作为原节点的兄弟出现在同一父节点下。
    pub fn duplicate_entity(&mut self) {
        if !self.is_editing() {
            self.log_warning("Cannot duplicate entities while playing");
            return;
        }
        let Some(idx) = self.selected_entity else {
            return;
        };
        self.push_undo();

        let subtree = self.scene.subtree(idx);
        let originals: Vec<Entity> = subtree
            .iter()
            .map(|&i| self.scene.entities[i].clone())
            .collect();

        // 旧 ID → 新 ID。子树内部的父子引用按这张映射重定向；根的外部父节点
        // 保持不变（副本与原来并排）。
        let mut id_map: HashMap<EntityId, EntityId> = HashMap::new();
        for entity in &originals {
            id_map.insert(entity.id, EntityId::new());
        }

        let mut first_new = None;
        for (k, original) in originals.iter().enumerate() {
            let mut clone = original.clone();
            clone.id = id_map[&original.id];
            // 父引用指向的实体也在这批副本里就映射到新 ID，否则（指向子树外的
            // 父节点）保留原 ID 不变。
            clone.parent = original.parent.map(|pid| *id_map.get(&pid).unwrap_or(&pid));
            clone.name = format!("{} (Copy)", original.name);
            if k == 0 {
                // 只挪子树根；子节点跟着父走，额外偏移会让整棵树散开。
                clone.transform.position.x += 0.5;
                clone.transform.position.z += 0.5;
            }
            let new_idx = self.scene.entities.len();
            self.scene.entities.push(clone);
            if k == 0 {
                first_new = Some(new_idx);
            }
        }

        self.selected_entity = first_new;
        self.log_info("Duplicated entity");
    }

    /// 删除当前选中的实体**及其整棵子树**（对应 Unity 的 Delete）。
    ///
    /// 在 Play 模式下不可用。
    pub fn delete_selected(&mut self) {
        if !self.is_editing() {
            self.log_warning("Cannot delete entities while playing");
            return;
        }
        let Some(idx) = self.selected_entity else {
            return;
        };
        self.push_undo();

        let removed = self.scene.remove_subtree(idx);
        if removed.is_empty() {
            return;
        }
        let name = removed[0].name.clone();
        let children = removed.len() - 1;
        // 索引在整棵子树被移除后整体位移，收敛到合法范围
        self.selected_entity = if self.scene.entities.is_empty() {
            None
        } else {
            Some(idx.min(self.scene.entities.len() - 1))
        };
        if children > 0 {
            self.log_info(format!("Deleted '{name}' (+{children} children)"));
        } else {
            self.log_info(format!("Deleted '{name}'"));
        }
    }

    // -----------------------------------------------------------------------
    // 层级（父子关系）
    // -----------------------------------------------------------------------

    /// 某个实体的**世界变换**（层级解算后的结果）。
    ///
    /// gizmo 定位、拾取、状态栏坐标都要用到世界变换——实体的 `transform` 是局部值，
    /// 有父节点时和世界里看到的位置并不相同。
    ///
    /// 注意：调用方现在都是直接走 `state.scene.world_transform(idx)`，这条转发没人用。
    #[allow(dead_code)]
    pub fn world_transform_of(&self, index: usize) -> Transform {
        self.scene.world_transform(index)
    }

    /// 把实体 `child` 重新挂到 `parent` 下（`None` = 移到场景根层）。
    ///
    /// 保世界变换、防成环，且进撤销栈。层级面板拖拽、右键菜单都走这条。
    pub fn reparent_entity(&mut self, child: usize, parent: Option<usize>) {
        if !self.is_editing() {
            self.log_warning("Cannot reparent while playing");
            return;
        }
        if child >= self.scene.entities.len() {
            return;
        }
        // 已经在目标父节点下就不要产生一个"什么都没变"的撤销点
        if self.scene.parent_index(child) == parent {
            return;
        }

        let child_name = self.scene.entities[child].name.clone();
        let snapshot = self.scene.clone();
        if self.scene.set_parent(child, parent) {
            self.push_undo_snapshot(snapshot);
            match parent {
                Some(p) => {
                    let parent_name = self.scene.entities[p].name.clone();
                    self.log_info(format!("Parented '{child_name}' under '{parent_name}'"));
                }
                None => self.log_info(format!("Unparented '{child_name}'")),
            }
        } else {
            self.log_warning(format!(
                "Cannot parent '{child_name}' there — that would create a cycle"
            ));
        }
    }

    /// 把当前选中实体挂到指定父节点下（`None` = 移到根层）。
    ///
    /// 层级面板的拖拽与右键菜单现在都直接调 [`reparent_entity`](Self::reparent_entity)，
    /// 这条包装暂时没人用。
    #[allow(dead_code)]
    pub fn reparent_selected(&mut self, parent: Option<usize>) {
        if let Some(child) = self.selected_entity {
            self.reparent_entity(child, parent);
        }
    }

    // -----------------------------------------------------------------------
    // 预制体（Prefab）
    // -----------------------------------------------------------------------

    /// 重新扫描项目里的预制体资产，刷新列表缓存。
    pub fn refresh_prefabs(&mut self) {
        self.prefab_list = self.project.list_prefabs();
    }

    /// 当前选中实体的名字，用作「创建预制体」对话框的默认名。
    pub fn prefab_default_name(&self) -> String {
        self.selected_entity
            .and_then(|i| self.scene.entities.get(i))
            .map(|e| e.name.clone())
            .unwrap_or_default()
    }

    /// 打开「从选中创建预制体」对话框（默认名取选中实体名）。
    pub fn begin_create_prefab(&mut self) {
        if !self.is_editing() {
            self.log_warning("Cannot create a prefab while playing");
            return;
        }
        if self.selected_entity.is_none() {
            self.log_warning("Select an entity first");
            return;
        }
        self.prefab_save_name = self.prefab_default_name();
        self.prefab_save_dialog = true;
    }

    /// 把当前选中实体（含整棵子树）存成一个预制体资产。
    ///
    /// 存完之后，场景里这批实体**原地变成该预制体的实例**——这样"建预制体"
    /// 之后立刻就能 Apply / Revert，不用先删掉再拖一个回来（Unity 也是这个语义）。
    /// 返回写入的项目相对路径。
    pub fn create_prefab_from_selection(&mut self, name: &str) -> Option<String> {
        if !self.is_editing() {
            self.log_warning("Cannot create a prefab while playing");
            return None;
        }
        let Some(root) = self.selected_entity else {
            self.log_warning("Select an entity first");
            return None;
        };
        if root >= self.scene.entities.len() {
            return None;
        }

        let trimmed = name.trim();
        let display = if trimmed.is_empty() {
            self.scene.entities[root].name.clone()
        } else {
            trimmed.to_string()
        };
        let relative = self.project.prefab_path_for(&display);

        let mut prefab = Prefab::from_subtree(&self.scene, root);
        prefab.name = display.clone();

        if let Err(e) = self.project.write_prefab(&relative, &prefab) {
            self.log_error(format!("创建预制体失败: {e}"));
            return None;
        }

        // 场景里这批实体现在就是实例了。这一步会改实体数据（贴来源标记），进撤销栈。
        self.push_undo();
        self.mark_subtree_as_instance(root, &relative);

        self.refresh_prefabs();
        let extra = prefab.len().saturating_sub(1);
        self.log_success(format!(
            "已创建预制体 '{display}' ({extra} 个子节点) → {relative}"
        ));
        Some(relative)
    }

    /// 把一棵子树标记为某个预制体的实例。
    ///
    /// `source_id` 直接取实体**当前的 id**：预制体刚由这批实体生成，资产里
    /// 记的就是这些 id，天然一一对应（见 [`Prefab::from_subtree`] 的说明）。
    fn mark_subtree_as_instance(&mut self, root: usize, source: &str) {
        for idx in self.scene.subtree(root) {
            let is_root = idx == root;
            let entity = &mut self.scene.entities[idx];
            let source_id = entity.id;
            entity.prefab = Some(PrefabInstanceRef {
                source: source.to_string(),
                source_id,
                is_root,
            });
        }
    }

    /// 从预制体资产实例化一份到场景里（落在根层，选中新实例根）。
    pub fn instantiate_prefab_by_path(&mut self, relative: &str) -> Option<usize> {
        if !self.is_editing() {
            self.log_warning("Cannot instantiate a prefab while playing");
            return None;
        }
        let prefab = match self.project.load_prefab(relative) {
            Ok(p) => p,
            Err(e) => {
                self.log_error(format!("加载预制体失败: {e}"));
                return None;
            }
        };
        if prefab.is_empty() {
            self.log_warning(format!(
                "预制体 '{}' 是空的，没有可实例化的内容",
                prefab_display_name(relative)
            ));
            return None;
        }

        self.push_undo();
        match self.scene.instantiate_prefab(&prefab, relative) {
            Some(root) => {
                self.selected_entity = Some(root);
                self.log_info(format!(
                    "已实例化 '{}' ({} 个实体)",
                    prefab_display_name(relative),
                    prefab.len()
                ));
                Some(root)
            }
            None => None,
        }
    }

    /// 实体所属预制体实例的根索引（不属于任何实例时为 `None`）。
    pub fn prefab_instance_root(&self, index: usize) -> Option<usize> {
        self.scene.prefab_instance_root(index)
    }

    /// 是否为预制体实例的**根**（决定要不要显示 Apply / Revert）。
    pub fn is_prefab_root(&self, index: usize) -> bool {
        self.scene
            .entities
            .get(index)
            .and_then(|e| e.prefab.as_ref())
            .is_some_and(|p| p.is_root)
    }

    /// 把实例当前的改动**写回**预制体资产（Unity 的 Apply）。
    ///
    /// 只写资产文件，不改场景——所以不进撤销栈（撤销栈管的是场景）。
    pub fn apply_prefab(&mut self, root_idx: usize) {
        if !self.is_editing() {
            self.log_warning("Cannot apply a prefab while playing");
            return;
        }
        let Some(source) = self
            .scene
            .entities
            .get(root_idx)
            .and_then(|e| e.prefab.as_ref())
            .filter(|p| p.is_root)
            .map(|p| p.source.clone())
        else {
            self.log_warning("Select the root of a prefab instance to apply");
            return;
        };

        let mut prefab = Prefab::from_subtree(&self.scene, root_idx);
        prefab.name = prefab_display_name(&source);
        if let Err(e) = self.project.write_prefab(&source, &prefab) {
            self.log_error(format!("应用预制体失败: {e}"));
            return;
        }
        self.log_success(format!(
            "已把改动应用到预制体 '{}' ({source})",
            prefab.name
        ));
    }

    /// 用资产覆盖回实例（Unity 的 Revert）。
    ///
    /// 实例整棵重建，但**根的世界变换保留**——否则"还原"会把用户摆好的位置
    /// 也一起抹掉，那不是还原，是回到出厂设置。
    pub fn revert_prefab(&mut self, root_idx: usize) {
        if !self.is_editing() {
            self.log_warning("Cannot revert a prefab while playing");
            return;
        }
        let Some(source) = self
            .scene
            .entities
            .get(root_idx)
            .and_then(|e| e.prefab.as_ref())
            .filter(|p| p.is_root)
            .map(|p| p.source.clone())
        else {
            self.log_warning("Select the root of a prefab instance to revert");
            return;
        };

        let prefab = match self.project.load_prefab(&source) {
            Ok(p) => p,
            Err(e) => {
                self.log_error(format!("还原预制体失败: {e}"));
                return;
            }
        };

        // 先记下"要保住什么"：根的父节点（按 ID 记——删除会让索引位移）与世界矩阵。
        let parent_id = self
            .scene
            .parent_index(root_idx)
            .map(|p| self.scene.entities[p].id);
        let old_world = self.scene.world_matrix(root_idx);

        self.push_undo();
        self.scene.remove_subtree(root_idx);

        let Some(new_root) = self.scene.instantiate_prefab(&prefab, &source) else {
            self.log_warning("预制体是空的，实例已移除");
            self.selected_entity = None;
            return;
        };

        // 让新实例根回到原来的世界位置，再挂回原来的父节点下。
        // 此刻它还没有父，把局部设成世界即可；`set_parent` 保留世界变换。
        self.scene.entities[new_root].transform = Transform::from_matrix(old_world);
        let parent = parent_id.and_then(|id| self.scene.index_of(id));
        self.scene.set_parent(new_root, parent);

        self.selected_entity = Some(new_root);
        self.log_info(format!(
            "已从资产还原预制体 '{}' ({source})",
            prefab_display_name(&source)
        ));
    }

    /// 删除一个预制体资产文件（面板上确认后调用）。
    pub fn delete_prefab_asset(&mut self, relative: &str) {
        let path = self.project.path(relative);
        match std::fs::remove_file(&path) {
            Ok(()) => {
                self.log_info(format!("已删除预制体资产: {relative}"));
                self.refresh_prefabs();
            }
            Err(e) => self.log_error(format!("删除失败 ({}): {e}", path.display())),
        }
    }

    // -----------------------------------------------------------------------
    // Game runtime (play mode)
    // -----------------------------------------------------------------------

    /// 进入播放模式：必要时先编译 Rust 插件，再拍快照、启动游戏运行时。
    ///
    /// # 插件项目会走两条路
    ///
    /// 项目根有 `Cargo.toml` 且产物过期时，这里**不进 Play**，而是起一个后台
    /// `cargo build`、置 [`pending_play`](Self::pending_play)、把 `play_mode`
    /// 退回 Edit —— UI 上看不出停顿（工具栏显示编译中），编译完成后由
    /// [`poll_plugin_build`](Self::poll_plugin_build) 自动进 Play。
    /// 旧实现那种"按 Play 就卡在主线程等 cargo"的做法会让编辑器假死几十秒，
    /// 与 UE 的后台编译体验相去甚远。
    ///
    /// 没有插件项目、或产物已新鲜时走老路，`needs_build` 只做几次 `stat`。
    pub fn enter_play_mode(&mut self) {
        // cargo 起不来（没装 / 环境变量坏掉）时 `start_plugin_build` 返回 false：
        // 那就不阻塞 Play —— 项目里可能还有上次编译的旧产物，能玩就先玩，报错
        // 已经在控制台里了。一条本可以阻止游戏启动的工具链问题，不该反过来让
        // 游戏启动不了。
        let deferred = crate::plugin_build::is_plugin_project(self.project.root())
            && crate::plugin_build::needs_build(self.project.root())
            && self.start_plugin_build();
        if deferred {
            return;
        }

        // 加载插件（幂等：内容没变不重载）。非插件项目这一步是空扫描。
        let (names, reloaded) = self.plugins.ensure_loaded(self.project.root());
        if reloaded {
            if names.is_empty() {
                self.log_warning("Plugins/ 里没有任何可用的插件 DLL（编译失败？）");
            } else {
                self.log_info(format!("Rust 插件已加载，注册组件：{}", names.join(", ")));
            }
        }

        // 快照 + 启动都要在改 `play_mode` 之前完成：运行时的 `start` 可能失败
        // （脚本编译错误），那时状态应该还是"干净的编辑态"。
        self.play_snapshot = Some(self.scene.clone());
        let registry = self.plugins.build_registry();
        let runtime = crate::game_runtime::GameRuntime::start(
            self.scene.clone(),
            self.project.root(),
            &self.project.manifest().runtime,
            registry,
        );
        self.game_runtime = Some(runtime);
        // 把 Edit 模式调好的组件参数灌进去 —— 必须在第一帧 update 之前，
        // 否则组件的 `start` 会先按 `Default` 跑掉一遍。
        self.seed_play_from_edit_components();
        self.refresh_scripts();
        self.pending_play = false;
        let plugins_note = if self.plugins.is_empty() {
            String::new()
        } else {
            format!("，Rust 插件 {} 个", self.plugins.names().len())
        };
        self.log_info(format!(
            "▶ Play — 已启动运行时（物理 / 输入 / 脚本{plugins_note}）"
        ));
    }

    /// 起一次后台插件编译。返回 `true` = 本次 Play 被推迟（编译完成后再进）。
    fn start_plugin_build(&mut self) -> bool {
        if self.plugin_build.is_some() {
            // 已有构建在飞：排队等它，完成时一起进 Play。
            self.defer_play_for_build("插件正在后台编译，完成后自动进入 Play…");
            return true;
        }
        match crate::plugin_build::spawn_build(self.project.root()) {
            Ok(task) => {
                self.plugin_build = Some(task);
                self.defer_play_for_build("插件源码有改动，正在后台编译，完成后自动进入 Play…");
                true
            }
            Err(e) => {
                self.log_error(format!("无法启动插件编译：{e}"));
                false // 起不来就别挡住 Play（降级成用现有产物）
            }
        }
    }

    /// 本次 Play 被推迟：退回 Edit 并记下"编译完成后要进 Play"。
    fn defer_play_for_build(&mut self, msg: &str) {
        self.pending_play = true;
        self.play_mode = PlayMode::Edit;
        self.log_info(msg);
    }

    /// 轮询后台插件编译；完成后同步产物，若用户按过 Play 则自动进入播放模式。
    ///
    /// 放在每帧入口调用（`editor_ui` 的第 0 步之后）：编译可能在任意一帧完成，
    /// 而"进入 Play"的状态跃迁检测在同一帧的工具栏之后 —— 先把状态摆好，
    /// 同帧就能完成"编译完成 → 进 Play"，用户感觉不到中间那一帧。
    pub fn poll_plugin_build(&mut self) {
        let outcome = self.plugin_build.as_ref().and_then(|t| t.poll());
        let Some(outcome) = outcome else {
            return;
        };
        self.plugin_build = None;

        match outcome {
            crate::plugin_build::BuildOutcome::Success(dlls) => {
                let synced = crate::plugin_build::sync_plugins_dir(self.project.root(), &dlls);
                if synced.is_empty() {
                    self.log_error(
                        "插件编译成功，但产物一个都没能拷进 Plugins/（见上方错误）",
                    );
                    self.pending_play = false;
                    return;
                }
                let (names, _) = self.plugins.ensure_loaded(self.project.root());
                if names.is_empty() {
                    self.log_error(
                        "插件产物已就位，但没有任何组件注册成功 —— 版本握手失败？（见 stderr）",
                    );
                    self.pending_play = false;
                    return;
                }
                self.log_info(format!("✅ 插件编译完成，注册组件：{}", names.join(", ")));
                if std::mem::take(&mut self.pending_play) {
                    self.play_mode = PlayMode::Playing;
                    self.enter_play_mode();
                }
            }
            crate::plugin_build::BuildOutcome::Failed(msg) => {
                // 编译失败留在编辑态（可改代码再按 Play），错误原样进控制台。
                self.log_error(format!("插件编译失败：\n{msg}"));
                self.pending_play = false;
            }
        }
    }

    /// 后台是否正在编译插件（工具栏指示器读）。
    pub fn plugin_building(&self) -> bool {
        self.plugin_build.is_some()
    }

    /// 后台编译已进行的秒数；没有编译在跑时 `None`（工具栏"已编译 Xs"用）。
    pub fn plugin_build_elapsed(&self) -> Option<u64> {
        self.plugin_build
            .as_ref()
            .map(|t| t.started.elapsed().as_secs())
    }

    /// 项目根是否是 Rust 插件项目（工具栏「Build」按钮据此显隐）。
    pub fn is_plugin_project(&self) -> bool {
        crate::plugin_build::is_plugin_project(self.project.root())
    }

    /// 手动编译一次插件（**不**进 Play）——只想让产物跟上、不想跑起来时用。
    pub fn build_plugins(&mut self) {
        if !self.is_plugin_project() || self.plugin_build.is_some() {
            return;
        }
        match crate::plugin_build::spawn_build(self.project.root()) {
            Ok(task) => {
                self.plugin_build = Some(task);
                self.log_info("🔨 正在编译 Rust 插件…");
            }
            Err(e) => self.log_error(format!("无法启动插件编译：{e}")),
        }
    }

    /// 当前已注册的 Rust 组件名（Inspector 的 Behaviors 下拉菜单读）。
    pub fn behavior_names(&self) -> &[String] {
        self.plugins.names()
    }

    /// 取一个「可检视的组件」，供 Inspector 读写。
    ///
    /// - **Play / Paused**：返回 World 里**真正跑着的那份**实例 —— 拖一下滑块
    ///   下一次 `update` 就生效，这是 Unity 式的热调参。
    /// - **Edit**：返回本状态里惰性构造的编辑副本，值由
    ///   [`Self::seed_play_from_edit_components`] 带进下一次 Play。
    ///
    /// `name` 是场景里写的注册名（编辑态要靠它构造实例；Play 侧只按下标定位，
    /// 但会用它核对下标处是不是同一个组件 —— 返回 `None` 等于说"对不上，别画"）。
    ///
    /// 返回的名字生命周期绑 `&mut self`（不是 `'static`）：Edit 侧的名字来自
    /// 存的 `String`，而 `World` 那侧给的是 `'static` —— 短的那个说了算。
    pub fn inspect_component(
        &mut self,
        entity_id: EntityId,
        slot: usize,
        name: &str,
    ) -> Option<(&str, &mut dyn Inspect)> {
        if self.game_runtime.is_some() {
            let (live_name, comp) = self.game_runtime.as_mut()?.behavior_at_mut(entity_id, slot)?;
            // 下标对上了但类型换了（Play 期间改场景改不到这里，但 Edit 侧
            // 快照恢复后索引会平移）—— 让调用方知道这份数据不适用。
            return (live_name == name).then_some((live_name, comp));
        }

        // 缺失或名字对不上都重建：把参数写到错误的组件类型上是静默的错误，
        // 编译器管不着（两边都有 `f32` 字段），只能在这里挡住。
        let stale = self
            .edit_components
            .get(&(entity_id, slot))
            .is_none_or(|e| e.name != name);
        if stale {
            let mut component = self.plugins.construct(name)?;
            // P1.5：新构造的副本从 `Default` 起步，立刻套上场景里存的字段值 ——
            // 与 `World` 装配走同一对 capture/apply 函数，Edit 看到的、Play 跑的、
            // 打包玩家拿到的必须是同一份数。
            if let Some(index) = self.scene.find_entity(entity_id) {
                if let Some(values) = self.scene.entities[index].behavior_params.get(name) {
                    xgame_core::inspect::apply_field_values(&mut *component, values);
                }
            }
            self.edit_components.insert(
                (entity_id, slot),
                EditComponent {
                    name: name.to_owned(),
                    component,
                },
            );
        }
        self.edit_components
            .get_mut(&(entity_id, slot))
            .map(|e| (e.name.as_str(), &mut *e.component as &mut dyn Inspect))
    }

    /// 只读地取一个组件的字段表（**不构造实例**）。
    ///
    /// 注册表里缓存了每种组件的静态表，所以"还没调过参的组件"也能画出字段
    /// 预览 —— 零分配，也不必先把组件造出来。走常驻缓存，可每帧调。
    pub fn behavior_field_table(&self, name: &str) -> Option<&'static [FieldDesc]> {
        self.plugins.field_table(name)
    }

    /// 把 Edit 模式调好的参数灌进刚启动的运行时。
    ///
    /// 时机：`GameRuntime::start` 之后、第一帧 `update` 之前 —— 组件的 `start`
    /// 还没跑，参数能被 `start` 读到（比如"起始高度 = 某个可调值"）。
    ///
    /// **逐字段复制而不是换实例**：World 那份已经装配完毕，换掉等于作废装配
    /// 状态；而且只有可检视字段该被外部影响 —— 状态字段（`#[inspect(skip)]`）
    /// 本来就属于实例自己。
    ///
    /// Play 期间的改动**不回写**这里：停止 Play 时丢弃，与 Unity 默认一致
    /// （Play 中改的是"试试看"，不是"就这么定了"）。
    fn seed_play_from_edit_components(&mut self) {
        let Some(runtime) = self.game_runtime.as_mut() else {
            return;
        };
        for ((entity_id, slot), edit) in &self.edit_components {
            let Some((live_name, comp)) = runtime.behavior_at_mut(*entity_id, *slot) else {
                continue;
            };
            // 场景里这个下标已经换成别的组件了 —— 这份副本作废。
            if live_name != edit.name {
                continue;
            }
            for i in 0..edit.component.inspect_fields().len() {
                comp.inspect_set(i, edit.component.inspect_get(i));
            }
        }
    }

    /// 退出播放模式：丢弃运行时，把场景恢复成进入 Play 之前的样子。
    ///
    /// 恢复的是**整份场景快照**：脚本可能 `spawn_entity` / `destroy` 过实体，
    /// 只恢复变换会留下"凭空多出来的实体"。
    pub fn exit_play_mode(&mut self) {
        if let Some(snapshot) = self.play_snapshot.take() {
            self.scene = snapshot;
        }
        self.game_runtime = None;
        self.selected_entity = None;
        self.log_info("⏹ Stop — 场景已还原到进入 Play 之前的状态");
    }

    /// 将运行时推进 `dt` 秒。
    ///
    /// 仅在 `play_mode != PlayMode::Edit` 时调用（Pause 时不调）。
    pub fn physics_step(&mut self, dt: f32) {
        // 先把日志收出来、结束对运行时的借用，再写 Console —— 否则
        // "运行时可变借用"和"log_* 需要 &mut self"会撞在一起。
        let logs = {
            let Some(runtime) = self.game_runtime.as_mut() else {
                return;
            };
            // 编辑器相机就是"玩家的耳朵"：Play 期间监听者每帧跟随它
            //（在 update 之前写入，本帧的音频结算用的就是最新姿态）。
            let eye = self.editor_camera.eye();
            let fwd = self.editor_camera.forward();
            runtime.set_listener(eye.to_array(), fwd.to_array());
            runtime.update_with_scene(dt, &mut self.scene);
            runtime.take_logs()
        };

        for log in logs {
            let who = log
                .entity
                .and_then(|id| {
                    self.scene
                        .find_entity(id)
                        .map(|i| self.scene.entities[i].name.clone())
                })
                .unwrap_or_else(|| log.script.clone());
            let text = format!("[{who}] {}", log.message);
            match log.level {
                xgame_runtime::LogLevel::Info => self.log_info(text),
                xgame_runtime::LogLevel::Warn => self.log_warning(text),
                xgame_runtime::LogLevel::Error => self.log_error(text),
            }
        }
    }

    /// 重新编译所有脚本（脚本热重载）。
    ///
    /// 只在 Play 模式下有意义：编辑态没有运行时，脚本本来就没在跑。
    pub fn reload_scripts(&mut self) {
        if let Some(runtime) = self.game_runtime.as_mut() {
            runtime.reload_scripts();
            self.log_success("已重新编译全部脚本");
        } else {
            self.refresh_scripts();
            self.log_info("已重新扫描脚本目录");
        }
    }

    /// 获取物理调试线条（用于 scene_view 叠加绘制）。
    ///
    /// 仅在 `show_physics_debug` 为 true 时调用。
    pub fn debug_lines(&self) -> Vec<DebugLine> {
        if self.show_physics_debug {
            if let Some(runtime) = self.game_runtime.as_ref() {
                return runtime.debug_lines();
            }
        }
        Vec::new()
    }

    /// 获取物理统计（刚体数, 碰撞体数）。
    pub fn physics_stats(&self) -> (usize, usize) {
        self.game_runtime.as_ref().map_or((0, 0), |rt| rt.stats())
    }

    /// 获取音频统计（无运行时返回默认值 —— available=false，状态栏显示
    /// 不出来声卡状态也合理）。
    pub fn audio_stats(&self) -> xgame_core::AudioStats {
        self.game_runtime
            .as_ref()
            .map_or(xgame_core::AudioStats::default(), |rt| rt.audio_stats())
    }

    /// 粒子是否还在动（帧调度用；编辑态恒 false —— 粒子只在 Play 里模拟）。
    pub fn particles_animating(&self) -> bool {
        self.game_runtime
            .as_ref()
            .is_some_and(|rt| rt.particles_animating())
    }

    /// 粒子统计（状态栏 / 诊断；编辑态为全 0）。
    pub fn particle_stats(&self) -> xgame_particle::ParticleStats {
        self.game_runtime
            .as_ref()
            .map_or(xgame_particle::ParticleStats::default(), |rt| rt.particle_stats())
    }

    /// 最近一帧的绘制读数（合批 / 剔除 / 半透明）。
    ///
    /// 没有渲染器（无头 / 测试 / wgpu 不可用）时返回全零 —— 调用方据此画
    /// "只有帧时间"，比显示一排 0 更不容易误读。
    pub fn render_stats(&self) -> xgame_render::scene_pipeline::FrameStats {
        self.scene_renderer
            .as_ref()
            .map_or_else(Default::default, |r| r.frame_stats())
    }

    // -----------------------------------------------------------------------
    // 项目
    // -----------------------------------------------------------------------

    /// 项目名（用于窗口标题、状态栏）。
    pub fn project_name(&self) -> &str {
        self.project.name()
    }

    /// 把项目内的相对路径解析为绝对路径。
    ///
    /// 场景里的 `entity.model` 存的是项目相对路径，落盘加载前都要经过这里。
    /// 目前场景渲染器内部自行解析，此方法留给后续的资源浏览器 / 拖拽入场景使用。
    #[allow(dead_code)]
    pub fn asset_path(&self, relative: &str) -> PathBuf {
        self.project.path(relative)
    }

    /// 把当前场景保存回它自己的文件（[`current_scene`](Self::current_scene)）。
    ///
    /// 早先这里写死 `entry_scene`：只有一张地图时等价，一旦切到别的关卡，
    /// 在 Level2 里按 Ctrl+S 会把入口场景覆盖成 Level2 的内容。现在保存目标
    /// 与启动场景彻底分开，入口场景只能由 [`set_startup_scene`](Self::set_startup_scene)
    /// 或项目设置显式修改。
    ///
    /// 成功后刷新脏检查基准（[`saved_snapshot`](Self::saved_snapshot)），
    /// 否则刚保存完工具栏还挂着 "未保存" 的星号。
    pub fn save_scene(&mut self) -> Result<(), String> {
        // 播放中**不许存盘**：`self.scene` 此刻是运行时的副本，物理和脚本每帧都在
        // 改它 —— 存下去等于把"跑了三秒之后的现场"写进场景文件（树倒了、箱子飞了、
        // Fox 站在半空），而用户以为自己只是按了一下 Ctrl+S。
        //
        // 退出 Play 会拿 `play_snapshot` 覆盖回来，所以这里挡住不会丢任何编辑。
        if self.play_mode != PlayMode::Edit {
            let message = "播放中不能保存场景 —— 先按 ⏹ Stop 回到编辑态".to_owned();
            self.log_warning(message.clone());
            return Err(message);
        }
        let relative = if self.current_scene.trim().is_empty() {
            // 理论不可达（`new` 一定填入口场景）；留一条兜底，免得空路径把
            // `project.path("")` 变成项目根目录然后写盘失败得莫名其妙。
            self.project.manifest().entry_scene.clone()
        } else {
            self.current_scene.clone()
        };
        let path = self.project.path(&relative);
        match self.project.write_scene(&relative, &self.scene) {
            Ok(()) => {
                self.mark_saved();
                self.log_success(format!("已保存场景 → {}", path.display()));
                Ok(())
            }
            Err(e) => {
                let message = e.to_string();
                self.log_error(format!("保存失败: {message}"));
                Err(message)
            }
        }
    }

    /// 落盘成功后的收尾：脏检查基准对齐 + 按帧 memo 失效。
    fn mark_saved(&mut self) {
        self.saved_snapshot = self.scene.clone();
        self.dirty_memo = None;
    }

    /// 场景有没有未保存改动（与上次落盘的快照逐字段比较）。
    ///
    /// O(实体数)。每帧都要问的调用方（工具栏 / 标题栏 / 状态栏）请走
    /// [`scene_dirty_memo`](Self::scene_dirty_memo)。
    pub fn scene_dirty(&self) -> bool {
        self.scene != self.saved_snapshot
    }

    /// 按帧 memo 版的脏检查：同一 egui pass 内多次询问只比较一次。
    ///
    /// 比较在交互期可能每帧发生（编辑器 idle 时不出帧，交互期出帧率可到
    /// 上千），不 memo 的话一个大场景能把好几毫秒花在"只是想知道一下"上。
    /// `pass_nr` 来自 `ctx.cumulative_pass_nr()`。
    pub fn scene_dirty_memo(&mut self, pass_nr: u64) -> bool {
        // Play / Pause 态下场景是**运行时的副本**：物理每帧都在改它，退出 Play 时
        // 会被 `play_snapshot` 整个覆盖 —— 那不是"用户的改动"，标题里打星号、
        // 关窗时问"要不要保存"都是误导（用户一个字都没改，物理却已经跑了几百帧）。
        // 热调参同理，按丢弃处理。
        if self.play_mode != PlayMode::Edit {
            return false;
        }
        if let Some((frame, dirty)) = self.dirty_memo {
            if frame == pass_nr {
                return dirty;
            }
        }
        let dirty = self.scene_dirty();
        self.dirty_memo = Some((pass_nr, dirty));
        dirty
    }

    /// 场景文件列表（**项目相对路径**，已排序）。
    ///
    /// 磁盘扫描是真相源：用户在资源管理器里新建 / 删除场景后，下次打开菜单
    /// 就能看到，不需要维护第二份清单。入口场景与当前场景即使不在 `Scenes/`
    /// 里也会补进来（老项目可能把场景放在自定义目录）。
    pub fn scene_list(&self) -> Vec<String> {
        let mut scenes: BTreeSet<String> = self.project.list_scenes().into_iter().collect();
        for raw in [
            self.current_scene.as_str(),
            self.project.manifest().entry_scene.as_str(),
        ] {
            if let Ok(path) = normalize_scene_relative(raw) {
                scenes.insert(path);
            }
        }
        scenes.into_iter().collect()
    }

    /// 把一个用户输入的**场景名**变成项目相对路径（不加扩展名时补 `.ron`）。
    ///
    /// 名字只作文件名：路径分隔符、`..` 与 Windows 非法字符会被拒绝，
    /// 防手滑把场景写到项目外面去。
    pub fn scene_path_for_name(&self, raw_name: &str) -> Result<String, String> {
        let name = raw_name.trim();
        let name = name.strip_suffix(".ron").unwrap_or(name).trim_end();
        if name.is_empty() {
            return Err("场景名不能为空".into());
        }
        if name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) || name.contains("..") {
            return Err(format!("场景名含非法字符: {name}"));
        }
        let scenes_dir = self
            .project
            .manifest()
            .content
            .scenes_dir
            .trim()
            .trim_matches(['/', '\\'])
            .to_string();
        let relative = if scenes_dir.is_empty() {
            format!("{name}.ron")
        } else {
            format!("{scenes_dir}/{name}.ron")
        };
        normalize_scene_relative(&relative)
    }

    /// 打开另一个场景并把它设为当前编辑场景。
    ///
    /// **不在这里问"要不要保存"**：确认框需要 egui，而状态层要能在测试里
    /// 直接调用。交互入口走 [`request_scene_open`](Self::request_scene_open)。
    pub fn open_scene(&mut self, relative: &str) -> Result<(), String> {
        if self.play_mode != PlayMode::Edit {
            let message = "播放中不能切换场景 —— 先按 ⏹ Stop 回到编辑态".to_owned();
            self.log_warning(message.clone());
            return Err(message);
        }
        let relative = normalize_scene_relative(relative)?;
        let scene = match self.project.load_scene(&relative) {
            Ok(scene) => scene,
            Err(e) => {
                let message = format!("打开场景失败: {e}");
                self.log_error(message.clone());
                return Err(message);
            }
        };
        let path = self.project.path(&relative);
        self.adopt_scene(scene, relative);
        self.log_success(format!("已打开场景 → {}", path.display()));
        Ok(())
    }

    /// 交互入口：有未保存改动就先挂起，交给确认框；干净时直接切换。
    pub fn request_scene_open(&mut self, relative: String) {
        if self.play_mode != PlayMode::Edit {
            self.log_warning("播放中不能切换场景 —— 先按 ⏹ Stop 回到编辑态");
            return;
        }
        let Ok(relative) = normalize_scene_relative(&relative) else {
            self.log_error(format!("场景路径无效: {relative}"));
            return;
        };
        if relative == self.current_scene {
            return;
        }
        if self.scene_dirty() {
            self.pending_scene_change = Some(SceneChange::Open(relative));
        } else {
            // `open_scene` 自己会把失败写进 Console，这里不再重复记一条。
            let _ = self.open_scene(&relative);
        }
    }

    /// 交互入口：新建场景；有未保存改动时先挂起确认。
    pub fn request_new_scene(&mut self) {
        if self.play_mode != PlayMode::Edit {
            self.log_warning("播放中不能新建场景 —— 先按 ⏹ Stop 回到编辑态");
            return;
        }
        if self.scene_dirty() {
            self.pending_scene_change = Some(SceneChange::New);
        } else {
            self.begin_new_scene();
        }
    }

    /// 打开「新建场景」命名对话框。
    pub fn begin_new_scene(&mut self) {
        self.new_scene_open = true;
        self.new_scene_name = "NewScene".to_string();
    }

    /// 确认框放行后执行暂存的场景动作。
    pub fn finish_scene_change(&mut self, change: SceneChange) {
        match change {
            SceneChange::Open(path) => {
                let _ = self.open_scene(&path);
            }
            SceneChange::New => self.begin_new_scene(),
        }
    }

    /// 在场景目录里创建一个空场景（主相机 + 方向光）并切过去。
    ///
    /// **不改启动场景**：新建关卡不等于"游戏从这里开始"，要改启动场景请用
    /// [`set_startup_scene`](Self::set_startup_scene)。同名文件已存在时拒绝，
    /// 避免一次手滑覆盖掉已有内容。
    pub fn new_scene(&mut self, raw_name: &str) -> Result<(), String> {
        if self.play_mode != PlayMode::Edit {
            let message = "播放中不能新建场景 —— 先按 ⏹ Stop 回到编辑态".to_owned();
            self.log_warning(message.clone());
            return Err(message);
        }
        let relative = match self.scene_path_for_name(raw_name) {
            Ok(path) => path,
            Err(e) => {
                self.log_error(format!("新建场景失败: {e}"));
                return Err(e);
            }
        };
        if self.project.path(&relative).exists() {
            let message = format!("场景已存在: {relative}");
            self.log_error(format!("新建场景失败: {message}"));
            return Err(message);
        }

        let mut scene = xgame_core::project::default_scene();
        scene.name = scene_label(&relative);
        if let Err(e) = self.project.write_scene(&relative, &scene) {
            let message = e.to_string();
            self.log_error(format!("新建场景失败: {message}"));
            return Err(message);
        }
        let path = self.project.path(&relative);
        self.adopt_scene(scene, relative);
        self.log_success(format!(
            "已新建场景 → {}（启动场景仍是 {}）",
            path.display(),
            self.project.manifest().entry_scene
        ));
        Ok(())
    }

    /// 场景切换 / 新建后的统一收尾。
    ///
    /// 撤销栈、选中、折叠状态、Edit 组件副本全部属于**上一张场景**：不清掉的话
    /// 撤销会把旧场景整个贴回来，组件副本还会按 (EntityId, slot) 撞到新场景的
    /// 实体上。这些状态没有跨场景意义，重置是唯一正确的语义。
    fn adopt_scene(&mut self, scene: Scene, relative: String) {
        self.scene = scene;
        self.current_scene = relative;
        self.selected_entity = None;
        self.undo_stack = UndoStack::new();
        self.undo_pending = None;
        self.edit_components.clear();
        self.collapsed_entities.clear();
        self.collapsed_version = self.collapsed_version.wrapping_add(1);
        self.hierarchy_cache = crate::panels::hierarchy::RowCache::default();
        self.gizmo_hovered = None;
        self.gizmo_active = None;
        self.asset_drag = None;
        self.play_snapshot = None;
        self.saved_snapshot = self.scene.clone();
        self.dirty_memo = None;
    }

    /// 把指定场景设为项目启动场景（写 `project.ron` 的 `entry_scene`）。
    ///
    /// 下一次打开项目 / 构建 / 独立游戏启动生效；当前编辑场景不变。
    pub fn set_startup_scene(&mut self, relative: &str) -> Result<(), String> {
        let relative = normalize_scene_relative(relative)?;
        if !self.project.path(&relative).is_file() {
            let message = format!("启动场景不存在: {relative}");
            self.log_error(message.clone());
            return Err(message);
        }
        let previous = self.project.manifest().entry_scene.clone();
        if previous == relative {
            return Ok(());
        }
        self.project.manifest_mut().entry_scene = relative.clone();
        if let Err(e) = self.project.save() {
            // 清单写失败就回滚内存值，别让界面显示一个磁盘上不存在的启动场景。
            self.project.manifest_mut().entry_scene = previous;
            let message = e.to_string();
            self.log_error(format!("设置启动场景失败: {message}"));
            return Err(message);
        }
        self.log_success(format!(
            "启动场景已设为 {relative}（下次打开 / 构建生效）"
        ));
        Ok(())
    }

    /// 场景**另存为**一个新文件，并把编辑器切过去。
    ///
    /// - 旧场景文件保持原样 —— 改坏了随时能从旧文件重来；
    /// - **启动场景不变**：另存为是"给当前关卡做一个副本"，不是"让游戏从这里开始"；
    ///   要改启动场景用 [`set_startup_scene`](Self::set_startup_scene)；
    /// - 场景名同步成新文件名（场景名就是给它当展示名用的）。
    ///
    /// 名字只作**文件名**：路径分隔符、`..` 与 Windows 非法字符会被拒绝，
    /// 防手滑把场景写到项目外面去（清单是手改/机器写的文本，同样不能信）。
    pub fn save_scene_as(&mut self, raw_name: &str) -> Result<(), String> {
        // 同 `save_scene`：播放中不许写盘（`self.scene` 是运行时副本）。
        if self.play_mode != PlayMode::Edit {
            let message = "播放中不能另存场景 —— 先按 ⏹ Stop 回到编辑态".to_owned();
            self.log_warning(message.clone());
            return Err(message);
        }
        let relative = match self.scene_path_for_name(raw_name) {
            Ok(path) => path,
            Err(e) => {
                self.log_error(format!("另存为失败: {e}"));
                return Err(e);
            }
        };
        if relative == self.current_scene {
            let message = "目标就是当前场景；用 Save 直接保存".to_string();
            self.log_warning(message.clone());
            return Err(message);
        }
        if self.project.path(&relative).exists() {
            let message = format!("场景已存在: {relative}");
            self.log_error(format!("另存为失败: {message}"));
            return Err(message);
        }

        // 写盘成功后才改内存里的场景名：写失败时界面仍应显示旧场景，
        // 不能让一次失败的「另存为」顺手把当前场景弄脏。
        let mut scene = self.scene.clone();
        scene.name = scene_label(&relative);
        if let Err(e) = self.project.write_scene(&relative, &scene) {
            let message = e.to_string();
            self.log_error(format!("另存为失败: {message}"));
            return Err(message);
        }

        self.scene = scene;
        self.current_scene = relative.clone();
        self.mark_saved();
        self.log_success(format!(
            "已另存为 → {}（启动场景仍是 {}）",
            self.project.path(&relative).display(),
            self.project.manifest().entry_scene
        ));
        Ok(())
    }

    /// 把设置草稿写回内存清单并落盘（校验集中在这里，面板只管画）。
    ///
    /// 校验不过时草稿**原样留着**——用户敲错一个数字不该被弹窗清空重来。
    pub fn apply_settings_draft(&mut self) -> Result<(), String> {
        let Some(mut draft) = self.settings_draft.take() else {
            return Ok(());
        };

        let mut invalid = |msg: String, draft: xgame_core::project::Project| -> Result<(), String> {
            self.log_error(format!("设置未应用: {msg}"));
            self.settings_draft = Some(draft);
            Err(msg)
        };

        draft.name = draft.name.trim().to_string();
        if draft.name.is_empty() {
            return invalid("项目名不能为空".into(), draft);
        }
        draft.entry_scene = draft.entry_scene.trim().replace('\\', "/");
        if draft.entry_scene.is_empty() {
            return invalid("入口场景路径不能为空".into(), draft);
        }
        // 入口场景也会被构建器 / 独立游戏直接拼接成磁盘路径：与场景菜单用同一套
        // 校验，挡掉绝对路径、`..` 和预制体文件，别让手改的 project.ron 把启动
        // 路径指到项目外。
        draft.entry_scene = match normalize_scene_relative(&draft.entry_scene) {
            Ok(path) => path,
            Err(e) => return invalid(e, draft),
        };

        // 清单是手改的文本，垃圾值统一在这里夹回安全范围（同 sanitized 语义）。
        draft.window.width = draft.window.width.clamp(320, 16_384);
        draft.window.height = draft.window.height.clamp(240, 16_384);
        let sane_runtime = draft.runtime.sanitized();
        if sane_runtime != draft.runtime {
            self.log_warning(format!(
                "运行时参数超出安全范围已自动纠正（重力 {:?} → {:?}，步长 {}s → {}s）",
                draft.runtime.gravity,
                sane_runtime.gravity,
                draft.runtime.fixed_dt,
                sane_runtime.fixed_dt
            ));
            draft.runtime = sane_runtime;
        }

        // 画面参数同理（曝光 0 / 负数 / NaN 会让视口全黑或出 NaN 像素）。
        let sane_render = draft.render.sanitized();
        if sane_render != draft.render {
            self.log_warning(format!(
                "渲染参数超出安全范围已自动纠正（曝光 {} → {}）",
                draft.render.exposure, sane_render.exposure
            ));
            draft.render = sane_render;
        }

        *self.project.manifest_mut() = draft;
        match self.project.save() {
            Ok(()) => {
                self.log_success("项目设置已写入 project.ron");
                Ok(())
            }
            Err(e) => {
                let message = e.to_string();
                self.log_error(format!("项目设置写盘失败: {message}"));
                Err(message)
            }
        }
    }

    // -----------------------------------------------------------------------
    // 构建
    // -----------------------------------------------------------------------

    /// 把当前项目打包成可独立运行的游戏（实现见 [`crate::builder`]）。
    ///
    /// 构建前先把场景存盘——否则打出去的游戏用的是上次保存的内容，而不是
    /// 编辑器里现在看到的这一份。存盘失败不阻断构建，但会明确警告。
    pub fn build_project(&mut self) {
        if let Err(e) = self.save_scene() {
            self.log_warning(format!("构建前保存场景失败，产物可能不是最新: {e}"));
        }

        match crate::builder::build(&self.project) {
            Ok(report) => {
                for w in &report.warnings {
                    self.log_warning(w.clone());
                }
                self.log_success(report.summary());
                self.log_info(format!("双击即可运行: {}", report.executable.display()));
                self.last_build_dir = Some(report.output_dir);
            }
            Err(e) => self.log_error(format!("构建失败: {e}")),
        }
    }
}

// ---------------------------------------------------------------------------
// 从 RON 文件加载场景
// ---------------------------------------------------------------------------

// 注：场景加载 / 默认场景已下沉到 `xgame_core::ProjectHandle`：
//   - 加载：`ProjectHandle::load_entry_scene()`
//   - 默认场景：`xgame_core::project::default_scene()`
// 编辑器不再自己拼路径，也不再维护第二份「默认场景」定义。

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个挂在临时目录上的编辑器状态（用完记得删目录）。
    fn temp_state(tag: &str) -> (EditorState, std::path::PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "xgame_state_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let handle = xgame_core::ProjectHandle::create(&base, "StateTest").expect("创建临时项目");
        (EditorState::new(handle), base)
    }

    // -- 资源浏览器（P0 资产管线）------------------------------------------

    /// 把模型资产放进场景：新实体带路径、落在给定世界坐标、选中、可撤销。
    #[test]
    fn spawn_model_asset_creates_entity_at_position() {
        let (mut state, base) = temp_state("spawn_model");
        std::fs::create_dir_all(state.project.path("Resources")).unwrap();
        std::fs::write(state.project.path("Resources/Box.glb"), b"glb").unwrap();
        state.refresh_assets();
        assert_eq!(
            state.asset_browser.len(),
            1,
            "刷新后应看见刚放进来的资产: {:?}",
            state.asset_browser
        );

        let before = state.scene.entities.len();
        let idx = state
            .spawn_asset_at("Resources/Box.glb", Vec3::new(1.0, 0.0, -2.0))
            .expect("模型应能放进场景");
        assert_eq!(state.scene.entities.len(), before + 1);
        let entity = &state.scene.entities[idx];
        assert_eq!(entity.model, "Resources/Box.glb", "实体引用必须是路径（内存态）");
        assert_eq!(entity.transform.position, Vec3::new(1.0, 0.0, -2.0));
        assert!(
            entity.name.starts_with("Box"),
            "名字应取自文件名，实际 {}",
            entity.name
        );
        assert_eq!(state.selected_entity, Some(idx), "放置后应选中新实体");
        assert!(state.can_undo(), "放置必须进撤销栈");

        state.undo();
        assert_eq!(
            state.scene.entities.len(),
            before,
            "撤销应撤掉这次放置"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 预制体资产走实例化路径，并且整体搬到落点（根层 = 局部坐标即世界坐标）。
    #[test]
    fn spawn_prefab_asset_instantiates_at_position() {
        let (mut state, base) = temp_state("spawn_prefab");
        std::fs::create_dir_all(state.project.path("Resources")).unwrap();
        std::fs::write(state.project.path("Resources/Box.glb"), b"glb").unwrap();
        let mut prefab = Prefab::new("Tower");
        prefab.entities.push(Entity::new("Body").with_model("Resources/Box.glb"));
        let relative = state.project.prefab_path_for("Tower");
        state.project.write_prefab(&relative, &prefab).unwrap();
        state.refresh_assets();

        let before = state.scene.entities.len();
        let idx = state
            .spawn_asset_at(&relative, Vec3::new(3.0, 0.0, 4.0))
            .expect("预制体应能实例化");
        assert_eq!(state.scene.entities.len(), before + 1);
        assert_eq!(state.scene.entities[idx].transform.position, Vec3::new(3.0, 0.0, 4.0));
        assert!(
            state.scene.entities[idx].prefab.is_some(),
            "实例化出来的实体必须带来源标记"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 脚本 / 音频 / 纹理不能"放进场景"——必须明确拒绝并给提示，
    /// 而不是静默生成一个空实体（那会让用户以为资产导入失败）。
    #[test]
    fn spawn_rejects_non_scene_assets() {
        let (mut state, base) = temp_state("spawn_reject");
        std::fs::create_dir_all(state.project.path("Scripts")).unwrap();
        std::fs::write(state.project.path("Scripts/A.lua"), b"-- lua").unwrap();
        state.refresh_assets();

        let before = state.scene.entities.len();
        assert!(state.spawn_asset_at("Scripts/A.lua", Vec3::ZERO).is_none());
        assert_eq!(state.scene.entities.len(), before, "不该生成任何实体");
        assert!(
            state
                .console_messages
                .last()
                .map(|m| m.text.contains("不能直接放进场景"))
                .unwrap_or(false),
            "应给出可读提示"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 导入设置写进 `.meta`，重开项目后仍在（落盘契约）。
    #[test]
    fn import_settings_survive_reopen() {
        let (mut state, base) = temp_state("import_roundtrip");
        std::fs::create_dir_all(state.project.path("Resources")).unwrap();
        std::fs::write(state.project.path("Resources/Box.glb"), b"glb").unwrap();
        state.refresh_assets();

        let mut import = state.asset_import("Resources/Box.glb");
        import.model_scale = 2.5;
        state.set_asset_import("Resources/Box.glb", import);

        // 重开项目（模拟编辑器重启）后仍读得到。
        // 注意项目根是 `base/StateTest`（`create` 会在父目录下建同名子目录）。
        let root = state.project.root().to_path_buf();
        let reopened = xgame_core::ProjectHandle::open(&root).expect("重新打开项目");
        assert_eq!(reopened.asset_import("Resources/Box.glb").model_scale, 2.5);
        assert!(reopened.path("Resources/Box.glb.meta").is_file());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 新建材质：落盘 + 补 `.meta` + 选中 + 草稿就绪；重名自动加后缀不覆盖。
    #[test]
    fn create_material_writes_unique_asset_and_loads_draft() {
        let (mut state, base) = temp_state("create_material");
        let first = state.create_material("Wood").expect("应能创建材质");
        assert_eq!(first, "Resources/Materials/Wood.mat");
        assert!(state.project.path(&first).is_file(), "材质文件应落盘");
        assert!(
            state.project.path(format!("{first}.meta")).is_file(),
            "刷新资产时应补上 .meta"
        );
        assert_eq!(state.asset_selected.as_deref(), Some(first.as_str()));
        assert_eq!(
            state.material_draft.as_ref().map(|d| d.relative.as_str()),
            Some(first.as_str()),
            "新建后应直接进入编辑草稿"
        );

        let second = state.create_material("Wood").expect("重名也应能创建");
        assert_eq!(second, "Resources/Materials/Wood 2.mat");
        assert!(
            state.project.path(&first).is_file(),
            "新建不得覆盖已有材质"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 材质草稿：Apply 才写盘，Revert 恢复上次保存的内容。
    #[test]
    fn material_draft_apply_and_revert_control_disk() {
        let (mut state, base) = temp_state("material_draft");
        let relative = state.create_material("Metal").expect("创建材质");
        {
            let draft = state.material_draft_mut(&relative).expect("草稿应加载");
            draft.asset.base_color = [0.1, 0.2, 0.3, 1.0];
            draft.asset.metallic = 1.0;
            assert!(draft.dirty(), "改过字段就应标脏");
        }
        // 未 Apply 前，磁盘还是默认值。
        let on_disk = state.project.load_material(&relative).unwrap();
        assert_eq!(on_disk.base_color, [1.0; 4], "未 Apply 不该偷偷写盘");

        state.save_material_draft();
        let saved = state.project.load_material(&relative).unwrap();
        assert_eq!(saved.base_color, [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(saved.metallic, 1.0);
        assert!(
            !state.material_draft.as_ref().unwrap().dirty(),
            "Apply 后应回到干净状态"
        );

        {
            let draft = state.material_draft.as_mut().unwrap();
            draft.asset.roughness = 0.01;
            draft.revert();
            assert!(!draft.dirty(), "Revert 应丢弃未保存改动");
            assert_eq!(draft.asset.roughness, saved.roughness);
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 指派材质是场景改动：写实体字段、选中目标、可撤销；
    /// 双击材质资产（走 `spawn_asset_at`）在没有选中实体时拒绝。
    #[test]
    fn assign_material_targets_entity_and_is_undoable() {
        let (mut state, base) = temp_state("assign_material");
        let relative = state.create_material("Wood").expect("创建材质");
        let idx = state.scene.entities.len();
        state.scene.add_entity(Entity::new("Crate"));
        state.selected_entity = None;
        assert!(state.spawn_asset_at(&relative, Vec3::ZERO).is_none());
        assert_eq!(
            state.scene.entities[idx].material_asset, None,
            "没有目标实体时不能凭空指派"
        );

        assert!(state.assign_material(idx, &relative));
        assert_eq!(
            state.scene.entities[idx].material_asset.as_deref(),
            Some(relative.as_str())
        );
        assert_eq!(state.selected_entity, Some(idx));
        assert!(state.can_undo());

        state.undo();
        assert_eq!(
            state.scene.entities[idx].material_asset, None,
            "撤销应撤掉这次指派"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// `asset_stem` 必须是"第一个点之前"：预制体是双后缀，
    /// 用 `file_stem` 会得到 `Pillar.prefab` 这种半截名字。
    #[test]
    fn asset_stem_takes_first_segment() {
        assert_eq!(asset_stem("Resources/Box.glb"), "Box");
        assert_eq!(asset_stem("Prefabs/Pillar.prefab.ron"), "Pillar");
        assert_eq!(asset_stem("Box.glb"), "Box");
        assert_eq!(asset_stem(""), "Asset");
    }

    /// 控制台消息表必须有上限。
    ///
    /// 每条内容都不一样，去重拦不住 —— 这正是"每帧打一条不同日志的脚本"的情形。
    /// 没有上限的话，一张只增不减的表会把编辑器的内存一路推上去。
    #[test]
    fn console_history_is_capped() {
        let (mut state, base) = temp_state("console_cap");

        let total = MAX_CONSOLE_MESSAGES + 500;
        for i in 0..total {
            state.log_info(format!("msg {i}"));
        }

        assert!(
            state.console_messages.len() <= MAX_CONSOLE_MESSAGES,
            "控制台不该无限增长，实际 {} 条",
            state.console_messages.len()
        );
        // 丢的是**最旧**的，最新那条必须在。
        assert_eq!(
            state.console_messages.last().unwrap().text,
            format!("msg {}", total - 1),
            "裁剪必须丢最旧的，而不是最新的"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 选中索引越界时 `clamp_selection` 必须把它收回合法范围 ——
    /// 这是"改实体数量之后下一帧 `entities[idx]` 不 panic"的兜底。
    #[test]
    fn clamp_selection_rejects_out_of_range_index() {
        let (mut state, base) = temp_state("clamp");

        state.scene = xgame_core::Scene::new("clamp");
        for i in 0..3 {
            state.scene.add_entity(xgame_core::Entity::new(format!("e{i}")));
        }

        // 远越界 → 收到最后一个。
        state.selected_entity = Some(99);
        state.clamp_selection();
        assert_eq!(state.selected_entity, Some(2));

        // 场景清空 → 取消选中，而不是留下一个必然越界的索引。
        state.scene.entities.clear();
        state.clamp_selection();
        assert_eq!(state.selected_entity, None);

        // 合法值不该被动。
        state.scene.add_entity(xgame_core::Entity::new("only"));
        state.selected_entity = Some(0);
        state.clamp_selection();
        assert_eq!(state.selected_entity, Some(0));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 脏标记的完整生命周期：打开时干净 → 改动变脏 → 保存后干净。
    ///
    /// 旧实现没有脏标记这回事（`scene_dirty` 不存在，编译都过不去 ——
    /// 这就是"红"）。按帧 memo 还要验证：同一 pass 只比较一次、
    /// 改动后的下一 pass 能看到新结果（不然标题栏星号会卡死）。
    #[test]
    fn dirty_tracks_edits_and_clears_on_save() {
        let (mut state, base) = temp_state("dirty");

        assert!(!state.scene_dirty(), "刚打开的场景应当是干净的");
        assert!(!state.scene_dirty_memo(1), "按帧 memo 的结果应一致");

        state.scene.add_entity(xgame_core::Entity::new("edited"));
        assert!(state.scene_dirty(), "改了场景应当变脏");
        // memo 按 pass 缓存：pass 1 的旧值照常命中（这正是它省开销的方式，
        // 同一 pass 内改了场景的边角情况下一 pass 自然修正），新 pass 才重算。
        assert!(!state.scene_dirty_memo(1), "同一 pass 的缓存命中旧值");
        assert!(state.scene_dirty_memo(2), "新 pass 应当重算并看到脏");

        state.save_scene().expect("保存应成功");
        assert!(!state.scene_dirty(), "保存后应当干净");
        assert!(!state.scene_dirty_memo(3), "memo 失效后重算应为干净");

        // Play 往返不应弄脏：进入前是干净的，退出时快照原样恢复。
        state.enter_play_mode();
        state.exit_play_mode();
        assert!(!state.scene_dirty(), "Play 往返不改变落盘内容");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 场景路径是**保存目标**：切到 Level2 后按 Save 只写 Level2，
    /// 入口场景文件与 `project.ron` 都不该被碰。
    ///
    /// 旧实现里 `save_scene` 写死 `entry_scene`，这条断言必红（会把入口场景
    /// 覆盖成 Level2 的内容）。
    #[test]
    fn open_scene_repoints_save_without_touching_entry() {
        let (mut state, base) = temp_state("open_scene");
        let entry = state.project.manifest().entry_scene.clone();

        let mut level2 = xgame_core::project::default_scene();
        level2.name = "Level2".to_owned();
        state
            .project
            .write_scene("Scenes/Level2.ron", &level2)
            .expect("先放一个第二场景");

        state.scene.add_entity(xgame_core::Entity::new("entry stuff"));
        state.save_scene().expect("保存入口场景");
        let entry_before = state.project.load_scene(&entry).expect("读回入口场景");

        state.open_scene("Scenes/Level2.ron").expect("切换应成功");
        assert_eq!(state.current_scene, "Scenes/Level2.ron");
        assert_eq!(state.scene.name, "Level2");
        assert_eq!(
            state.project.manifest().entry_scene,
            entry,
            "切场景不该改启动场景"
        );

        state.scene.add_entity(xgame_core::Entity::new("level2 stuff"));
        state.save_scene().expect("保存当前场景");

        let level2_after = state
            .project
            .load_scene("Scenes/Level2.ron")
            .expect("读回 Level2");
        assert!(
            level2_after.entities.iter().any(|e| e.name == "level2 stuff"),
            "Save 必须写当前场景"
        );
        let entry_after = state.project.load_scene(&entry).expect("再读入口场景");
        assert_eq!(
            entry_before.entities.len(),
            entry_after.entities.len(),
            "入口场景文件必须一个字节都没变"
        );
        assert!(
            !entry_after.entities.iter().any(|e| e.name == "level2 stuff"),
            "当前场景的内容不许漏进入口场景"
        );
        assert!(!state.scene_dirty(), "保存后应当干净");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 「另存为」：写新文件、切当前场景、**启动场景不变**、旧文件原样保留、
    /// 目标已存在时拒绝（不许一次手滑覆盖别的关卡）。
    #[test]
    fn save_as_keeps_startup_and_old_file() {
        let (mut state, base) = temp_state("save_as");
        let old_entry = state.project.manifest().entry_scene.clone();
        let old_path = state.project.path(&old_entry);

        state.scene.add_entity(xgame_core::Entity::new("new stuff"));
        state.save_scene_as("Level2").expect("另存为应成功");

        assert_eq!(
            state.project.manifest().entry_scene, old_entry,
            "另存为只是做副本，不能偷偷改启动场景"
        );
        assert_eq!(state.current_scene, "Scenes/Level2.ron");
        assert!(state.project.path("Scenes/Level2.ron").is_file(), "新文件应落盘");
        assert!(old_path.is_file(), "旧场景文件必须原样保留");
        assert!(!state.scene_dirty(), "另存为后应当干净");
        assert_eq!(state.scene.name, "Level2", "场景名应同步成新名");

        let old_scene = state.project.load_scene(&old_entry).expect("旧场景可读回");
        assert!(
            !old_scene.entities.iter().any(|e| e.name == "new stuff"),
            "另存为不该动旧文件"
        );

        // 目标已存在：拒绝，且当前场景不切走、文件内容不变。
        state
            .project
            .write_scene("Scenes/Taken.ron", &xgame_core::project::default_scene())
            .expect("先占一个文件名");
        assert!(
            state.save_scene_as("Taken").is_err(),
            "同名文件已存在时必须拒绝，而不是覆盖"
        );
        assert_eq!(state.current_scene, "Scenes/Level2.ron");

        // 非法名一律拒绝：空名、路径穿越、分隔符。
        assert!(state.save_scene_as("").is_err());
        assert!(state.save_scene_as("   ").is_err());
        assert!(state.save_scene_as("../evil").is_err());
        assert!(state.save_scene_as("a/b").is_err());
        assert!(state.save_scene_as("a\\b").is_err());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 新建场景：落盘、切过去、**不自动设为启动场景**、同名拒绝。
    #[test]
    fn new_scene_switches_without_setting_startup() {
        let (mut state, base) = temp_state("new_scene");
        let entry = state.project.manifest().entry_scene.clone();

        state.new_scene("Level2").expect("新建应成功");
        assert_eq!(state.current_scene, "Scenes/Level2.ron");
        assert_eq!(state.scene.name, "Level2");
        assert!(
            state.project.path("Scenes/Level2.ron").is_file(),
            "新场景必须立刻落盘"
        );
        assert!(!state.scene.entities.is_empty(), "空场景也要有相机 + 光");
        assert_eq!(
            state.project.manifest().entry_scene,
            entry,
            "新建关卡不等于让游戏从这里开始"
        );
        assert!(!state.scene_dirty(), "刚创建的场景应当是干净的");

        // 同名再建：拒绝，且不把当前场景切成一个坏状态。
        assert!(state.new_scene("Level2").is_err());
        assert_eq!(state.current_scene, "Scenes/Level2.ron");

        // 名字只作文件名：路径穿越 / 分隔符 / 预制体后缀都不行。
        assert!(state.new_scene("../evil").is_err());
        assert!(state.new_scene("a/b").is_err());
        assert!(state.new_scene("x.prefab").is_err());
        assert!(!state.project.path("Scenes/evil.ron").exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 启动场景是显式动作：写 `project.ron`，重启后仍然生效。
    #[test]
    fn set_startup_scene_persists() {
        let (mut state, base) = temp_state("startup");
        state
            .project
            .write_scene("Scenes/Level2.ron", &xgame_core::project::default_scene())
            .expect("先放一个第二场景");

        state
            .set_startup_scene("Scenes/Level2.ron")
            .expect("设置启动场景应成功");
        assert_eq!(state.project.manifest().entry_scene, "Scenes/Level2.ron");

        let reopened = xgame_core::ProjectHandle::open(state.project.root()).expect("重开项目");
        assert_eq!(
            reopened.manifest().entry_scene,
            "Scenes/Level2.ron",
            "启动场景必须落盘，不能只活在内存里"
        );

        // 不存在的文件不许设成启动场景。
        assert!(state.set_startup_scene("Scenes/Missing.ron").is_err());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 名字 → 路径的约束：补 `.ron`、拒绝绝对路径 / `..` / 分隔符 / 预制体。
    #[test]
    fn scene_path_and_name_validation() {
        let (state, base) = temp_state("scene_paths");

        assert_eq!(
            state.scene_path_for_name("Level 2").unwrap(),
            "Scenes/Level 2.ron"
        );
        assert_eq!(
            state.scene_path_for_name("Level2.ron").unwrap(),
            "Scenes/Level2.ron"
        );
        for bad in ["", "   ", "../evil", "a/b", "a\\b", "x.prefab", "C:evil"] {
            assert!(
                state.scene_path_for_name(bad).is_err(),
                "`{bad}` 不该被当成合法场景名"
            );
        }

        assert_eq!(
            normalize_scene_relative("Scenes\\Win.ron").unwrap(),
            "Scenes/Win.ron"
        );
        for bad in [
            "",
            "/abs.ron",
            "C:/abs.ron",
            "Scenes/../evil.ron",
            "x.txt",
            "Scenes/X.prefab.ron",
        ] {
            assert!(
                normalize_scene_relative(bad).is_err(),
                "`{bad}` 不该通过路径校验"
            );
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 有未保存改动时，切换请求只挂起、不动场景；保存后才真正切。
    #[test]
    fn dirty_scene_change_is_deferred() {
        let (mut state, base) = temp_state("deferred");
        state
            .project
            .write_scene("Scenes/Level2.ron", &xgame_core::project::default_scene())
            .expect("先放一个第二场景");
        let entry = state.current_scene.clone();

        state.scene.add_entity(xgame_core::Entity::new("dirty"));
        state.request_scene_open("Scenes/Level2.ron".to_owned());
        assert_eq!(state.current_scene, entry, "脏的时候不能直接切");
        assert_eq!(
            state.pending_scene_change,
            Some(SceneChange::Open("Scenes/Level2.ron".to_owned()))
        );

        // 取消 / 保存后重新请求：干净时直接切。
        state.pending_scene_change = None;
        state.save_scene().expect("保存");
        state.request_scene_open("Scenes/Level2.ron".to_owned());
        assert_eq!(state.current_scene, "Scenes/Level2.ron");
        assert!(state.pending_scene_change.is_none());

        // 新建同理：脏时只挂起。
        state.scene.add_entity(xgame_core::Entity::new("dirty again"));
        state.request_new_scene();
        assert!(state.pending_scene_change == Some(SceneChange::New));
        assert!(!state.new_scene_open, "确认前不该弹命名框");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// P1.5：Edit 模式的组件副本必须**从场景里存的字段值起步**。
    ///
    /// 参数此前只活在编辑器内存里（存盘不带走）；这条断言守的是"打开场景 →
    /// 选中实体 → Inspector 里看到的必须是上次存盘的值"这条最基本的等式。
    /// 旧实现（构造只给 `Default`）下读到 0.0，断言必红。
    #[test]
    fn edit_component_is_seeded_from_scene_params() {
        use xgame::Inspect; // trait 与 derive 同名不同命名空间（serde 同款玩法）
        use xgame_core::inspect::{FieldValue, FieldValues};

        #[derive(Default, Inspect)]
        struct Tuned {
            #[inspect(min = 0.0, max = 100.0)]
            speed: f32,
            label: String,
        }
        impl xgame::prelude::Lifecycle for Tuned {}

        let (mut state, base) = temp_state("param_seed");
        state.plugins.register_for_test::<Tuned>("Tuned");

        // 实体 A：存了参数；实体 B：同组件、没存参数（应当拿到构造默认值）。
        let mut a = xgame_core::Entity::new("a");
        a.behaviors.push("Tuned".to_owned());
        let mut fields = FieldValues::new();
        fields.insert("speed".to_owned(), FieldValue::F32(66.0));
        fields.insert("label".to_owned(), FieldValue::String("存盘值".into()));
        a.behavior_params.insert("Tuned".to_owned(), fields);
        state.scene.add_entity(a);
        state.scene.add_entity(xgame_core::Entity::new("b"));
        state.scene.entities.last_mut().expect("b 在末尾").behaviors.push("Tuned".to_owned());
        // 按**名**取 id：新建项目自带一个默认实体，`entities[0]` 不一定是 "a"。
        let id_a = state.scene.find_by_name("a").expect("a 应当在").id;
        let id_b = state.scene.find_by_name("b").expect("b 应当在").id;

        let (_, comp) = state.inspect_component(id_a, 0, "Tuned").expect("应当能构造");
        assert_eq!(
            comp.inspect_get(0),
            FieldValue::F32(66.0),
            "编辑副本必须吃到存盘的 speed"
        );
        assert_eq!(comp.inspect_get(1), FieldValue::String("存盘值".into()));

        // 没存参数 ⇒ 保留构造默认值（缺字段不动）。
        let (_, comp) = state.inspect_component(id_b, 0, "Tuned").expect("应当能构造");
        assert_eq!(comp.inspect_get(0), FieldValue::F32(0.0));
        assert_eq!(comp.inspect_get(1), FieldValue::String(String::new()));

        // 存盘的等式另一头：从实例 capture 出来的值 = 场景里那份。
        let (_, comp) = state.inspect_component(id_a, 0, "Tuned").expect("再取一次");
        let captured = xgame_core::inspect::capture_field_values(comp);
        assert_eq!(captured.get("speed"), Some(&FieldValue::F32(66.0)));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 项目设置的落地链路：草稿 → 校验 → 清单 → 落盘 → 读回一致。
    #[test]
    fn settings_draft_applies_and_persists() {
        let (mut state, base) = temp_state("settings");

        // 没有草稿时 apply 是 no-op（防呆：面板没开就点 Apply 不该 panic）。
        assert!(state.apply_settings_draft().is_ok());

        let mut draft = state.project.manifest().clone();
        draft.runtime.gravity = [0.0, -30.0, 0.0];
        draft.runtime.fixed_dt = 1.0 / 90.0;
        draft.window.width = 1920;
        state.settings_draft = Some(draft);
        state.apply_settings_draft().expect("合法设置应能应用");

        assert_eq!(state.project.manifest().runtime.gravity, [0.0, -30.0, 0.0]);
        assert_eq!(state.project.manifest().runtime.fixed_dt, 1.0 / 90.0);

        // 读回：从盘上重新打开项目，设置还在（写盘是真的发生了）。
        let reopened = xgame_core::ProjectHandle::open(state.project.root()).expect("重开项目");
        assert_eq!(reopened.manifest().runtime.fixed_dt, 1.0 / 90.0);
        assert_eq!(reopened.manifest().window.width, 1920);

        // 非法草稿：留着草稿等用户改，不写清单。
        let mut bad = state.project.manifest().clone();
        bad.name = "   ".into();
        state.settings_draft = Some(bad);
        assert!(state.apply_settings_draft().is_err());
        assert!(state.settings_draft.is_some(), "校验失败必须保留草稿");

        // 垃圾运行时参数进清单前要被夹安全（fixed_dt=0 = 时间静止）。
        let mut toxic = state.project.manifest().clone();
        toxic.runtime.fixed_dt = 0.0;
        toxic.runtime.gravity = [f32::NAN, -9.81, 0.0];
        state.settings_draft = Some(toxic);
        state.apply_settings_draft().expect("夹取后应当合法");
        assert_eq!(
            state.project.manifest().runtime.fixed_dt,
            1.0 / 60.0,
            "fixed_dt=0 必须退回默认"
        );
        assert!(state.project.manifest().runtime.gravity.iter().all(|x| x.is_finite()));

        let _ = std::fs::remove_dir_all(&base);
    }
}
