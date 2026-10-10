//! 中央编辑器状态——拥有场景、选择、相机和工具设置。
//!
//! 所有编辑器面板从该状态读取并通过它写入，保持引擎数据模型和编辑器 UI 同步。
//!
//! 状态**绑定在一个已打开的项目上**（[`ProjectHandle`]）：场景与资产一律按项目
//! 根目录解析，编辑器不再依赖进程当前工作目录。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use xgame_math::glam::{Quat, Vec3};
use xgame_core::prefab::{prefab_display_name, Prefab, PrefabInstanceRef};
use xgame_core::project::ProjectHandle;
use xgame_core::{Entity, EntityId, Scene, Transform};
use xgame_physics::DebugLine;
use strum::Display;
use crate::editor_input::EditorInputManager;


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
    pub fn log_level_color(&self) -> egui::Color32 {
        match self {
            LogLevel::Info => egui::Color32::WHITE,
            LogLevel::Warning => egui::Color32::YELLOW,
            LogLevel::Error => egui::Color32::RED,
            LogLevel::Success => egui::Color32::GREEN,
        }
    }

    pub fn log_level_icon(&self) -> &'static str {
        match self {
            LogLevel::Info => "ℹ",
            LogLevel::Warning => "⚠",
            LogLevel::Error => "✖",
            LogLevel::Success => "✔",
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

impl Default for EditorCamera {
    fn default() -> Self {
        Self {
            yaw: -2.356,                // ≈ −135° — 从 (+, +, +) 方向看向原点
            pitch: -0.524,              // ≈ −30°  — 略微向下
            position: Vec3::new(5.0, 5.0, 5.0),
            sensitivity: 0.0025,
            pan_speed: 5.0,
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
        self.pan_speed = (self.pan_speed + delta * 0.05).clamp(0.5, 50.0);
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
    
    /// 输入管理器：预留的「绑定表 → 动作」输入层，**目前尚未接入**（没有任何地方
    /// 调用它的 `update()`，实际输入仍由 `scene_view` / `main` 直接处理）。
    /// 详见 `editor_input.rs` 顶部的模块说明。
    #[allow(dead_code)]
    pub input_manager: EditorInputManager,
}

impl EditorState {
    /// 打开一个项目：加载其入口场景并初始化编辑器状态。
    pub fn new(project: ProjectHandle) -> Self {
        let scene = match project.load_entry_scene() {
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
            selected_entity: None,

            editor_camera: EditorCamera::default(),
            tool_mode: ToolMode::Translate,
            play_mode: PlayMode::Edit,
            show_grid: true,
            show_physics_debug: false,

            console_messages: Vec::new(),
            next_entity_id: 1,

            viewport_logical_size: egui::Vec2::ZERO,
            viewport_pixel_size: (0, 0),

            fly_state: false,
            zoom_indicator: 0.0,

            scene_renderer: None,
            #[cfg(test)]
            last_render_view_proj: None,
            game_runtime: None,
            play_snapshot: None,

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

            undo_stack: UndoStack::new(),
            undo_pending: None,

            gizmo_hovered: None,
            gizmo_active: None,
            gizmo_snap: 0.0,

            input_manager: EditorInputManager::new(),
        };

        // 打开项目时先把资产清单扫出来，面板第一帧就有内容。
        state.refresh_prefabs();
        state.refresh_scripts();
        state
    }

    /// 重新扫描项目里的 `.rhai` 脚本（项目相对路径，已排序）。
    ///
    /// 与预制体清单同样**不每帧扫盘**：目录扫描是一串系统调用，
    /// 逐帧做就是"界面没动却在耗 CPU"。
    pub fn refresh_scripts(&mut self) {
        self.script_list = self.project.list_scripts();
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

    /// 进入播放模式：拍下场景快照，启动游戏运行时。
    pub fn enter_play_mode(&mut self) {
        // 快照 + 启动都要在改 `play_mode` 之前完成：运行时的 `start` 可能失败
        // （脚本编译错误），那时状态应该还是"干净的编辑态"。
        self.play_snapshot = Some(self.scene.clone());
        let runtime = crate::game_runtime::GameRuntime::start(
            self.scene.clone(),
            self.project.root(),
        );
        self.game_runtime = Some(runtime);
        self.refresh_scripts();
        self.log_info("▶ Play — 已启动运行时（物理 / 输入 / 脚本）");
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

    /// 把当前场景保存回项目的入口场景文件。
    pub fn save_scene(&mut self) -> Result<(), String> {
        let entry = self.project.manifest().entry_scene.clone();
        let relative = self.project.path(&entry);
        self.project
            .write_scene(&entry, &self.scene)
            .map(|()| self.log_success(format!("已保存场景 → {}", relative.display())))
            .map_err(|e| {
                let message = e.to_string();
                self.log_error(format!("保存失败: {message}"));
                message
            })
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
}
