//! 运行时世界：把场景、物理、输入、脚本串成一条确定的帧管线。
//!
//! # 为什么要有这一层
//!
//! 在这之前，编辑器内嵌的 `GameRuntime` 和独立游戏 `xgame-game` 各自实现了一遍
//! "物理步进 → 变换回写"，两份代码注定会漂移（事实上已经漂过一次：一处漏了
//! 父世界矩阵的逆变换，另一处记得写）。脚本、输入、音频再各自接一遍，
//! 很快就会变成 N 份不一致的实现。
//!
//! [`World`] 是这四件事的**唯一**编排者：编辑器按「▶ Play」和双击出的游戏
//! 跑的是同一段 `update`。想改帧序、加系统，只需要改这一个文件。
//!
//! # 帧顺序
//!
//! ```text
//! clock.tick(dt)
//!   ├─ dispatch_start            新实体（含上一轮脚本生成的）跑一次 start
//!   └─ 固定步长循环（每秒 fixed_dt 次，与渲染帧率解耦）
//!        ├─ dispatch_fixed_update   脚本的 fixed_update
//!        ├─ 消费脚本命令            补建 / 清理刚体
//!        ├─ sync_bodies             新实体建体、消失实体清体
//!        ├─ push_kinematic_bodies   运动学体：把脚本摆的位置推给物理
//!        ├─ physics.step(fixed_dt)
//!        └─ pull_dynamic_bodies     动态体：把物理算出的位置写回实体
//!   ├─ dispatch_update           脚本的 update（每渲染帧一次）
//!   ├─ 消费脚本命令
//!   ├─ 应用场景切换请求           `load_scene()` 在这里才真的换场景（帧末、借用已还）
//!   ├─ sync_bodies
//!   ├─ sync_audio                音频：autoplay 首播、空间声源推位置、失联停音
//!   ├─ flush_audio               音频：drain 命令束交给引擎（帧末一次结算）
//!   └─ input.end_frame()         清掉"仅本帧有效"的输入边沿
//! ```
//!
//! **动态体由物理驱动，运动学体由脚本驱动** —— 这条规则决定了上面两个方向的
//! 数据流。二者不会互相打架，因为一个刚体只属于其中一类。
//!
//! # 场景切换
//!
//! `load_scene` 只在**一处**被应用（`apply_pending_scene_load`，见帧顺序图），
//! 且只换**一份**场景对象。编辑器 Play 每帧把自己的场景与运行时那份
//! `mem::swap`，所以换场景之后两边会在下一帧自动收敛到新内容 ——
//! `game_runtime` 那边有一行日志说明这件事（见 `GameRuntime::update_with_scene`）。
//!
//! # 音频
//!
//! 组件与脚本不直接碰 kira：它们往 [`AudioPort`]（core 的纯数据命令束）里
//! 塞命令，`flush_audio` 在**帧末**一次性结算给 `xgame-audio` 的
//! [`AudioEngine`]。好处见 `xgame-core::audio` 模块文档 —— 这里只需记住：
//! `World::new` 一律建 **silent 引擎**（测试反复构造 World，抢设备会让行为
//! 不确定），宿主用 [`World::with_audio_engine`] 注入真设备引擎。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use xgame_audio::AudioEngine;
use xgame_core::{
    same_scene_path, AudioPort, AudioStats, EntityId, GameClock, Inspect, PhysicsHandle, PlaySound,
    Scene, SceneLoader, Transform,
};
use xgame_input::{Input, InputMap, MouseButton};
use xgame_math::glam::{Mat4, Vec2, Vec3};
use xgame_ui::{layout, update_interaction, UiEvent, UiInteraction, UiPointer, UiTree};
use xgame_physics::{
    body_from_scene, Collider, CollisionEvent, DebugLine, PhysicsConfig, PhysicsWorld, RigidBody,
};
use xgame_script::{FrameCtx, GameCtx, LogLevel, LuaHost, ScriptLog};

use crate::camera_view::CameraView;
use crate::registry::{BehaviorInstance, BehaviorRegistry};

/// 运行时的可调参数。
#[derive(Debug, Clone)]
pub struct WorldConfig {
    /// 重力。
    pub gravity: Vec3,
    /// 固定步长（秒）。60Hz 是物理的常用值。
    pub fixed_dt: f32,
    /// 物理子步数（`>1` 提升碰撞稳定性，代价是 CPU）。
    pub physics_sub_steps: u32,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            gravity: Vec3::new(0.0, -9.81, 0.0),
            fixed_dt: 1.0 / 60.0,
            physics_sub_steps: 1,
        }
    }
}

impl WorldConfig {
    /// 从项目清单的 `runtime` 节构造（重力 / 固定步长，先过容错夹取）。
    ///
    /// 编辑器 Play 与独立游戏二进制**共用这一个入口**——两处各写一遍
    /// `gravity: Vec3::from_array(...)` 就是"同一份配置两种行为"的经典起点。
    /// `sanitized()` 在这里做而不是要求调用方先做：清单是手改的文本，
    /// 非法值（`fixed_dt <= 0` = 时间静止）不能流进物理引擎。
    pub fn from_project_runtime(rt: &xgame_core::RuntimeConfig) -> Self {
        let rt = rt.sanitized();
        Self {
            gravity: Vec3::from_array(rt.gravity),
            fixed_dt: rt.fixed_dt,
            ..Self::default()
        }
    }
}

/// 一帧的统计信息（状态栏 / 性能面板用）。
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameReport {
    /// 本帧跑了几个固定步长（0 表示这一帧物理没推进）。
    pub fixed_steps: u32,
    /// 场景实体数。
    pub entities: usize,
    /// 物理刚体数。
    pub bodies: usize,
    /// 碰撞体数。
    pub colliders: usize,
}

/// UI 运行态：布局缓冲 + 交互状态（容量跨帧复用）。
#[derive(Default)]
struct UiRuntime {
    tree: UiTree,
    items: Vec<xgame_ui::LayoutItem>,
    interaction: UiInteraction,
    events: Vec<UiEvent>,
}

/// 游戏运行时世界。
pub struct World {
    scene: Scene,
    physics: PhysicsWorld,
    input: Input,
    /// Lua 脚本宿主（唯一脚本轨；Rhai 已随双轨 P3 删除，见计划文档 §六）。
    /// 日志与命令缓冲也由它持有（`LuaHost::buffers_mut`），组件轨与脚本轨共用。
    scripts: LuaHost,
    clock: GameClock,
    /// 实体 → 刚体。用 `EntityId` 而不是索引做键：脚本可以增删实体，
    /// 索引随时会变。
    body_of_entity: HashMap<EntityId, u64>,
    /// 脚本路径解析基准（项目根目录）。
    root: PathBuf,
    /// 上一帧的统计，供 UI 读取。
    last_report: FrameReport,
    /// `sync_bodies` 的复用暂存（"有 physics 但还没建刚体"的实体索引）。
    ///
    /// 做成字段而不是每次 `Vec::new()`：`sync_bodies` 每帧（每个固定步 + 每渲染帧）
    /// 都要扫一遍，稳态下这个 `Vec` 一直是空的，但字面量写法会**每帧分配一次**。
    /// 留着容量循环复用，稳态就是零分配。
    scratch_physics_indices: Vec<usize>,
    /// 场景里是否存在**运动学**刚体。
    ///
    /// 由 `sync_bodies` 那次扫描顺带记下，供 `push_kinematic_bodies` 早退用。
    /// 没有它的话，"这个场景要不要推运动学体"就得每个固定步再单独扫一遍全场 ——
    /// 纯静态 / 纯动态场景（很常见）白白付出一次 O(n)。
    has_kinematic: bool,
    /// 场景里是否存在**动态**刚体。同上，供 `pull_dynamic_bodies` 早退用。
    has_dynamic: bool,
    /// 行为组件注册表（双轨架构的 Rust 轨道）。
    behaviors: BehaviorRegistry,
    /// 实体 → 已实例化的行为组件。
    ///
    /// 注册名存的是注册表里的 `'static` 字符串：每次分发构造 `GameCtx`
    /// 时零分配，日志归属也统一成注册名。钩子函数指针在装配期就拷进了
    /// 实例（`BehaviorInstance`），分发热路径零哈希。
    behavior_instances: HashMap<EntityId, BehaviorInstanceSet>,
    /// 音频命令束：组件 / 脚本产生命令，`flush_audio` 帧末结算。
    audio_port: AudioPort,
    /// 音频引擎（kira 封装）。`World::new` 一律 silent，宿主注入真引擎。
    audio: AudioEngine,
    /// 已经 autoplay 过的实体 —— 没有它，"进场景就响"会变成每帧重播。
    /// 组件被摘除 / 实体被删时清掉对应条目（重新挂上会再响一次）。
    audio_autoplayed: HashSet<EntityId>,
    /// 上一次音频扫描时的实体数（见 `sync_audio` 的快速路径）。
    audio_scan_count: usize,
    /// 上一次扫描时场景里有没有挂音频源的实体（扫描时刷新）。
    audio_sources_seen: bool,
    /// UI 视口尺寸（物理像素）。宿主每帧设置；`(0, 0)` = 不跑 UI（无头 / 未上屏）。
    ui_viewport: (u32, u32),
    /// UI 指针（视口左上角原点，物理像素）。`None` = 指针不在视口内。
    ///
    /// 刻意**不**直接读 `input.mouse_pos()`：窗口坐标 → 视口坐标的映射只有
    /// 宿主知道（编辑器视口在面板里的位置 / DPI 缩放），运行时不该猜。
    ui_pointer: Option<Vec2>,
    /// UI 布局与交互状态（跨帧保持按下捕获）。
    ui: UiRuntime,
    /// 粒子系统（CPU 模拟，按实体 id 分桶；见 `xgame-particle`）。
    ///
    /// 住在这里而不是渲染层：粒子的位置 / 速度是**运行时状态**，无头运行
    /// 也要推进它（`--headless` 报告里能看见粒子数），渲染只是消费者。
    particles: xgame_particle::ParticleSystem,
    /// 角色上一帧的接触面（实体 → 刚体集合），用于 enter / exit 差分。
    ///
    /// 角色的运动学胶囊永远停在 skin 距离外 —— 物理引擎不会给它发接触事件，
    /// 所以角色的 enter/exit 只能靠"这一帧碰到了谁 vs 上一帧碰到了谁"算出来。
    character_contacts: HashMap<EntityId, xgame_core::CharacterContacts>,
    /// 场景装载来源（宿主注入；`None` = 这份运行时不能换场景）。
    scene_loader: Option<Box<dyn SceneLoader>>,
    /// 当前场景的项目相对路径（`None` = 宿主没告诉，脚本 `current_scene()` 返回空串）。
    ///
    /// 存**路径**而不是 `Scene::name`：路径是项目的稳定标识（`Scenes/Level2.ron`），
    /// `name` 是作者可随意改的显示名，用它做"切到哪一份"的键会一改就断。
    current_scene: Option<String>,
    /// 本帧脚本请求的切换（帧末统一应用，见 [`World::apply_pending_scene_load`]）。
    pending_scene_load: Option<String>,
    /// 累计成功切换次数（无头报告 / 诊断读数）。
    scene_loads: u32,
    /// 发布清单里的关卡顺序（`project.ron` → `build.scenes`，宿主注入）。
    ///
    /// **顺序即关卡索引**：脚本 `scene_index()` / `load_scene_at(i)` 都读它。
    /// 空 = 宿主没配清单（单场景项目 / 老项目）—— 那时脚本只能按路径切换，
    /// `scene_count()` 返回 0，与"清单里没有当前场景"是同一个可观察结果。
    scene_order: Vec<String>,
}

/// 单个实体已实例化的组件集合。
type BehaviorInstanceSet = Vec<BehaviorInstance>;

impl World {
    /// 用一份场景快照建立运行时。
    ///
    /// 收**快照**（而不是借用编辑器的场景）是刻意的：Play 模式跑起来之后，
    /// 编辑器手里的那份必须是"进入 Play 之前的样子"，退出时才能原样恢复。
    pub fn new(scene: Scene, root: impl Into<PathBuf>, config: WorldConfig) -> Self {
        let root = root.into();
        let physics = PhysicsWorld::new(PhysicsConfig {
            gravity: config.gravity,
            num_sub_steps: config.physics_sub_steps,
            timestep: config.fixed_dt,
        });

        let mut scripts = LuaHost::new();
        scripts.set_root(root.clone());
        scripts.preload(&root, &scene);

        // 音频引擎刻意 **silent**：`World::new` 在测试里被反复构造，抢声卡
        // 会让"同一段测试有时连着设备、有时没有"——行为不确定的测试等于没有。
        // 真设备引擎由宿主经 `with_audio_engine` 注入（编辑器 Play / 独立游戏）。
        let audio = AudioEngine::silent(root.clone());

        let mut world = Self {
            scene,
            physics,
            input: Input::with_default_map(),
            scripts,
            clock: GameClock::new().with_fixed_dt(config.fixed_dt),
            body_of_entity: HashMap::new(),
            root,
            last_report: FrameReport::default(),
            scratch_physics_indices: Vec::new(),
            has_kinematic: false,
            has_dynamic: false,
            behaviors: BehaviorRegistry::default(),
            behavior_instances: HashMap::new(),
            audio_port: AudioPort::new(),
            audio,
            audio_autoplayed: HashSet::new(),
            // 0 = 强迫第一帧做过一次实扫（场景非空时实体数必然不等）。
            audio_scan_count: 0,
            audio_sources_seen: false,
            ui_viewport: (0, 0),
            ui_pointer: None,
            ui: UiRuntime::default(),
            particles: xgame_particle::ParticleSystem::new(),
            character_contacts: HashMap::new(),
            scene_loader: None,
            current_scene: None,
            pending_scene_load: None,
            scene_loads: 0,
            scene_order: Vec::new(),
        };
        // 建完刚体顺手刷新加速结构：编辑器在暂停状态下也要能射线拾取。
        world.sync_bodies();
        // 组件实例化刻意不在这里做：此刻注册表还是空表，装配只会在实例表里
        // 留下"未注册"的空占坑。真正的装配发生在 `with_behavior_registry`。
        world
    }

    /// 注入行为组件注册表（Rust 轨道）。
    ///
    /// 未注入时是空表 —— 声明了 `behaviors` 的实体记日志跳过（容错哲学与
    /// 未注册脚本一致）。
    ///
    /// 重复注入 = 按新表**重新装配**（先清实例，组件的 start 标记随实例
    /// 一起作废）：插件热重载之后组件逻辑已变，旧实例留着会跑到旧代码上；
    /// 空表时期可能留下的"未注册"占坑也在这一步一并清掉。
    pub fn with_behavior_registry(mut self, registry: BehaviorRegistry) -> Self {
        self.behaviors = registry;
        self.behavior_instances.clear();
        self.instantiate_behaviors();
        self
    }

    /// 用指定的输入映射建立运行时（游戏项目可以在 `input.ron` 里自定义按键）。
    pub fn with_input_map(mut self, map: InputMap) -> Self {
        self.input.set_map(map);
        self
    }

    /// 注入真设备音频引擎（编辑器 Play / 独立游戏宿主调用）。
    ///
    /// `World::new` 里是 silent 引擎（见那里的理由）；换引擎不影响已收集
    /// 的命令 —— 下一次 `flush_audio` 就按新引擎结算。
    pub fn with_audio_engine(mut self, engine: AudioEngine) -> Self {
        self.audio = engine;
        self
    }

    /// 注入场景装载来源（宿主调用；不注入 = 这份运行时不能换场景）。
    ///
    /// 编辑器 Play / 独立游戏 / 无头运行都传 `project.clone()` ——
    /// `ProjectHandle` 是共享资产索引的轻句柄，clone 不会重新扫盘。
    ///
    /// 不注入时 [`World::load_scene`] 返回错误而不是"静默成功"：一个不能换场景的
    /// 运行时遇上 `load_scene()` 调用，最可能的成因是宿主忘了接线，
    /// 静默吞掉会让作者以为"脚本没写对"。
    pub fn with_scene_loader(mut self, loader: impl SceneLoader + 'static) -> Self {
        self.scene_loader = Some(Box::new(loader));
        self
    }

    /// 告诉运行时"起始跑的是哪份场景文件"（宿主从清单的 `entry_scene` 取）。
    ///
    /// 只影响脚本 `current_scene()` 与诊断日志 —— 场景内容本来就已经交给
    /// [`World::new`] 了。装配时漏掉这一步不会有功能故障，只是脚本读不到名字。
    pub fn with_current_scene(mut self, relative: impl Into<String>) -> Self {
        self.set_current_scene(relative);
        self
    }

    /// 同上，但**运行中**可改（编辑器 Play 在装配完之后才知道当前场景路径）。
    pub fn set_current_scene(&mut self, relative: impl Into<String>) {
        self.current_scene = Some(relative.into());
    }

    /// 注入**关卡顺序**（宿主从清单的 `build.scenes` 取，`Project::scene_order()`）。
    ///
    /// 顺序即关卡索引：脚本 `scene_index()` 查当前是第几关、`load_scene_at(i)`
    /// 按序号切。不注入时脚本仍能用 `load_scene("Scenes/X.ron")` ——
    /// 按序号推关卡是"有了清单之后更省事"的那层，不是必需品。
    pub fn with_scene_order(mut self, order: Vec<String>) -> Self {
        self.scene_order = order;
        self
    }

    /// 关卡顺序（诊断用；脚本侧看到的是同一份）。
    pub fn scene_order(&self) -> &[String] {
        &self.scene_order
    }

    /// 当前场景在关卡顺序里的下标（`None` = 不在清单里 / 没配清单）。
    pub fn scene_index(&self) -> Option<usize> {
        let current = self.current_scene.as_deref()?;
        self.scene_order
            .iter()
            .position(|path| same_scene_path(path, current))
    }

    /// 切到关卡顺序里的第 `index` 关（宿主 / 测试入口；脚本走 `load_scene_at`）。
    pub fn load_scene_at(&mut self, index: usize) -> Result<(), String> {
        let Some(path) = self.scene_order.get(index).cloned() else {
            return Err(format!(
                "load_scene_at({index}) 越界：清单里只有 {} 关（下标 0..{}）",
                self.scene_order.len(),
                self.scene_order.len()
            ));
        };
        self.load_scene(&path)
    }

    /// 设置 3D 监听者（宿主每帧调用：编辑器 / 游戏相机就是玩家的耳朵）。
    pub fn set_listener(&mut self, position: [f32; 3], forward: [f32; 3]) {
        self.audio_port.set_listener(position, forward);
    }

    /// 设置主音量（线性 `0.0..=1.0`）。
    pub fn set_master_volume(&mut self, volume: f32) {
        self.audio_port.set_master_volume(volume);
    }

    /// 音频子系统的可观测状态（状态栏 / 测试断言）。
    pub fn audio_stats(&self) -> AudioStats {
        self.audio.stats()
    }

    // -----------------------------------------------------------------------
    // 访问
    // -----------------------------------------------------------------------

    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }

    pub fn input(&self) -> &Input {
        &self.input
    }

    pub fn input_mut(&mut self) -> &mut Input {
        &mut self.input
    }

    pub fn physics(&self) -> &PhysicsWorld {
        &self.physics
    }

    pub fn physics_mut(&mut self) -> &mut PhysicsWorld {
        &mut self.physics
    }

    /// 编辑器专用：某实体身上**已装配**的组件数。
    ///
    /// 会先跑一次惰性装配（[`Self::instantiate_behaviors`]）—— Inspector 在
    /// Play 的第一帧就要画组件，而装配原本发生在 `update` 里。装配幂等、
    /// O(声明数)，重复调用无副作用。
    ///
    /// 取 `&mut self` 不是笔误：惰性装配要写 `behavior_instances`。
    pub fn behavior_count(&mut self, entity: EntityId) -> usize {
        self.instantiate_behaviors();
        self.behavior_instances
            .get(&entity)
            .map_or(0, |set| set.len())
    }

    /// 编辑器专用：按下标取一个组件的 `(注册名, 可检视对象)`。
    ///
    /// **下标制而不是"一次借出全部"**：一次 `&mut self` 只能借出一个可变引用，
    /// 而 Inspector 要逐个画字段 —— 每画一个借一次、借完即还，借用检查器天然
    /// 满意，不需要 `split_at_mut` 那类算错索引就是 UB 的把戏。
    ///
    /// 返回 `&mut dyn Inspect`（而不是 `&mut dyn Behavior`）是刻意的：
    /// 编辑器只该碰字段表，碰不到四钩子 —— 在 Play 中途改一个正在跑的组件
    /// 的生命周期方法，是没设计过的语义。
    pub fn behavior_at_mut(
        &mut self,
        entity: EntityId,
        index: usize,
    ) -> Option<(&'static str, &mut dyn Inspect)> {
        self.instantiate_behaviors();
        let set = self.behavior_instances.get_mut(&entity)?;
        let instance = set.get_mut(index)?;
        Some((instance.name, &mut *instance.component))
    }

    pub fn clock(&self) -> &GameClock {
        &self.clock
    }

    /// 当前时间缩放系数（`1.0` = 实时）。
    pub fn time_scale(&self) -> f32 {
        self.clock.time_scale()
    }

    /// 设置时间缩放系数（`0` = 暂停模拟）。钳制与非法值处理见 [`GameClock`]。
    ///
    /// 宿主侧入口（编辑器 / 独立游戏 / 测试都走这里）；脚本侧是
    /// `set_time_scale(ctx, v)`，写的是同一份时钟。生效在**下一帧**：
    /// 本帧的固定步循环已经跑完。
    pub fn set_time_scale(&mut self, scale: f32) {
        self.clock.set_time_scale(scale);
    }

    /// 上一帧的**帧间隔**（秒，已按 `time_scale` 缩放）。
    ///
    /// 宿主的动画钟必须吃这一份而不是原始 dt —— 否则脚本一暂停，
    /// 物理冻住、角色却还在原地跑步。
    pub fn frame_dt(&self) -> f32 {
        self.clock.delta_time()
    }

    /// 游戏已运行总时长（秒，已按 `time_scale` 缩放）。
    pub fn game_time(&self) -> f32 {
        self.clock.total_time()
    }

    pub fn scripts(&self) -> &LuaHost {
        &self.scripts
    }

    /// 项目根目录（资产与脚本路径的解析基准）。
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub fn last_report(&self) -> FrameReport {
        self.last_report
    }

    /// 取出场景，结束运行时。
    pub fn into_scene(self) -> Scene {
        self.scene
    }

    // -----------------------------------------------------------------------
    // 场景切换
    // -----------------------------------------------------------------------

    /// 当前场景的项目相对路径（`None` = 宿主没告诉运行时）。
    pub fn current_scene(&self) -> Option<&str> {
        self.current_scene.as_deref()
    }

    /// 累计成功切换场景的次数（无头报告 / 诊断读数）。
    pub fn scene_loads(&self) -> u32 {
        self.scene_loads
    }

    /// 立刻装载并切换到另一份场景（宿主 / 测试入口）。
    ///
    /// 骨架逻辑全在 [`World::swap_scene`]；这里只负责"先装载、成功了再换"——
    /// 装载失败时旧场景**原封不动**（半换过去的场景比重启还糟）。
    ///
    /// 脚本侧不直接调这个：钩子执行期间场景正被可变借用，只能登记请求，
    /// 由 [`World::apply_pending_scene_load`] 在帧末统一应用。
    pub fn load_scene(&mut self, relative: &str) -> Result<(), String> {
        let Some(loader) = self.scene_loader.as_ref() else {
            return Err(format!(
                "宿主没有接入场景装载器，无法加载 '{relative}'（游戏 / 编辑器 Play / 无头运行都需要 with_scene_loader）"
            ));
        };
        let scene = loader.load_scene(relative)?;
        self.swap_scene(scene, relative);
        Ok(())
    }

    /// 登记一次场景切换请求（脚本 `load_scene()` 走这里；宿主也可以用）。
    ///
    /// 一帧内多次请求只认最后一个 —— 中间那些场景永远不会被装载，
    /// 也就不该产生"装载了又立刻丢弃"的物理重建与脚本重编译。
    pub fn request_scene_load(&mut self, relative: impl Into<String>) {
        self.pending_scene_load = Some(relative.into());
    }

    /// 应用本帧脚本登记的切换请求（在 `update` 里"消费脚本命令"之后调用）。
    ///
    /// 失败只记一条错误日志、继续跑旧场景：游戏不该因为一次路径写错就整个停摆。
    fn apply_pending_scene_load(&mut self) {
        let Some(path) = self.pending_scene_load.take() else {
            return;
        };
        if let Err(err) = self.load_scene(&path) {
            self.push_runtime_log(LogLevel::Error, format!("切换场景 '{path}' 失败：{err}"));
        }
    }

    /// 换场景的**唯一**实现：把"旧场景的一切运行时痕迹"清干净再装新的。
    ///
    /// 这一步漏掉任何一项，症状都是"跨场景串味"而不是报错，所以逐项写清理由：
    ///
    /// | 状态 | 不清会怎样 |
    /// |---|---|
    /// | `body_of_entity` / 物理体 | 旧刚体没人引用，却仍参与模拟与射线查询 |
    /// | `character_contacts` | 角色接触差分拿旧帧的接触面做基线，进新场景就误报 exit |
    /// | 脚本 `started` / `states` | 按实体 id 存：新场景的实体永远不会跑 `start`（旧 id 占着标记），每实体状态也全错位 |
    /// | 粒子 | 旧发射器的粒子留在新场景里飞（而且找不到自己的发射器） |
    /// | 音频 autoplay 集合 | 新场景同一个"名字"不会再响 |
    /// | UI 布局/交互 | 悬停与按下捕获指向已不存在的实体 |
    /// | 组件实例 | 组件持有旧场景的状态，且绑在旧实体 id 上 |
    ///
    /// **不清的**：脚本编译缓存与路径驻留表（`LuaHost::reset_scene_state` 只清
    /// `started` / `states`，连日志缓冲都不碰）。于是脚本的模块级全局变量跨场景
    /// 保留 —— 与 Unity 不重载 domain 的语义一致，作者该用 `state_*` 存
    /// "随实体走"的数据。
    fn swap_scene(&mut self, scene: Scene, relative: &str) {
        let previous = self.scene.entities.len();

        // -- 1. 物理：旧刚体全丢 --
        //
        // 实体 id 是进程内单调分配的（`id.rs`），新场景的 id 一定不与旧的重合，
        // 所以"留着等 sync_bodies 清理"虽然也会清对，但那要多跑一遍全场景扫描，
        // 而且中间态里 `body_of_entity` 是一张跨场景的垃圾表。
        let stale: Vec<u64> = self.body_of_entity.drain().map(|(_, body)| body).collect();
        for body in stale {
            self.physics.remove_body(body);
        }
        self.character_contacts.clear();
        self.has_kinematic = false;
        self.has_dynamic = false;

        // -- 2. 脚本运行时状态（编译缓存与驻留表不动，理由见上）--
        //
        // 用 `reset_scene_state` 而不是 `reset_runtime_state`：后者会连日志
        // 缓冲一起清，而本帧的日志此刻还没人取走 —— 那会把"这一帧报了什么"
        // 抹掉，正好抹掉最该留下的诊断。
        self.scripts.reset_scene_state();

        // -- 3. 粒子 / UI / 音频 --
        self.particles.clear();
        self.ui = UiRuntime::default();
        self.audio_autoplayed.clear();
        // 0 = 强迫下一帧重新实扫一遍（与 `World::new` 同一手法）。
        self.audio_scan_count = 0;
        self.audio_sources_seen = false;
        self.audio_port.stop_all();

        // -- 4. 换本体 --
        self.scene = scene;
        self.current_scene = Some(relative.to_owned());
        self.scene_loads += 1;

        // -- 5. 为新场景建物理体 / 装配组件 / 编译新脚本 --
        self.behavior_instances.clear();
        self.sync_bodies();
        self.instantiate_behaviors();
        let root = self.root.clone();
        let preload = self.scripts.preload(&root, &self.scene);

        self.push_runtime_log(
            LogLevel::Info,
            format!(
                "场景已切换 → {relative}（实体 {previous} → {}）",
                self.scene.entities.len()
            ),
        );
        if !preload.failed.is_empty() {
            self.push_runtime_log(
                LogLevel::Error,
                format!("场景 '{relative}' 有脚本编译失败：{}", preload.failed.join(", ")),
            );
        }
    }

    /// 推一条**运行时自己**产生的日志（进控制台 / 无头报告的同一本账）。
    fn push_runtime_log(&mut self, level: LogLevel, message: String) {
        let (logs, _) = self.scripts.buffers_mut();
        logs.push(ScriptLog {
            level,
            script: "运行时".to_owned(),
            entity: None,
            message,
        });
    }

    // -----------------------------------------------------------------------
    // UI
    // -----------------------------------------------------------------------

    /// 设置 UI 视口（物理像素）。宿主每帧调用；`(0, 0)` = 本帧不跑 UI。
    pub fn set_ui_viewport(&mut self, viewport: (u32, u32)) {
        self.ui_viewport = viewport;
    }

    /// 设置 UI 指针（视口左上角原点、物理像素）。`None` = 指针不在视口内。
    pub fn set_ui_pointer(&mut self, pointer: Option<Vec2>) {
        self.ui_pointer = pointer;
    }

    /// 当前悬停的 UI 实体。
    pub fn ui_hovered(&self) -> Option<EntityId> {
        self.ui.interaction.hovered()
    }

    /// 本帧的 UI 事件（调试 / 测试用；玩法侧读 `UiElement::state`）。
    pub fn ui_events(&self) -> &[UiEvent] {
        &self.ui.events
    }

    /// 布局 + 命中测试，把悬停 / 按下 / 点击写回 `Entity::ui.state`。
    ///
    /// 没有 UI 实体时整段跳过（场景里 2000 个实体也只是一次 `any` 扫描）。
    fn update_ui(&mut self) {
        let (w, h) = self.ui_viewport;
        if w == 0 || h == 0 {
            self.ui.interaction.reset();
            return;
        }
        let pointer = self.ui_pointer.map(|pos| UiPointer {
            pos,
            down: self.input.mouse_down(MouseButton::Left),
            pressed: self.input.mouse_pressed(MouseButton::Left),
            released: self.input.state().mouse_released(MouseButton::Left),
        });
        let viewport = Vec2::new(w as f32, h as f32);
        self.ui.tree.build(&self.scene);
        if self.ui.tree.roots().is_empty() {
            // 没有 UI：清掉上一帧的悬停残影（实体可能刚被删掉）。
            self.ui.items.clear();
            self.ui.interaction.reset();
            return;
        }
        layout::visit(&self.scene, &self.ui.tree, viewport, &mut self.ui.items);
        update_interaction(
            &mut self.scene,
            &self.ui.items,
            pointer,
            &mut self.ui.interaction,
            &mut self.ui.events,
        );
    }

    // -----------------------------------------------------------------------
    // 帧推进
    // -----------------------------------------------------------------------

    /// 推进一帧。
    ///
    /// `dt` 是**真实帧间隔**（调用方负责夹一个上限，否则窗口被拖动几秒之后
    /// 这里会一次补上千个固定步）。
    pub fn update(&mut self, dt: f32) -> FrameReport {
        self.clock.tick(dt);
        let time = self.clock.total_time();
        let frame_dt = self.clock.delta_time();
        let fixed_dt = self.clock.fixed_dt();

        // -- 0. UI：布局 + 命中测试 --
        //
        // 必须排在脚本之前：`ui_clicked(ctx, id)` 是**本帧**状态，脚本在
        // `update` 里读它时点击标志必须已经写好。代价是"脚本本帧生成的 UI
        // 元素下一帧才能被点到"—— 与 Unity 的 EventSystem 同一取舍。
        self.update_ui();

        // -- 1. start：给尚未启动过的实体（含上一轮脚本生成的）调用一次 --
        //
        // 每帧都扫一遍是有意的：脚本 `spawn_entity` 出来的实体没有别的时机
        // 能拿到 start。场景实体数通常远小于"每帧要渲染的顶点数"，
        // 这点 O(n) 扫描不值得为它引入一张"待启动"队列。
        self.run_dispatch(Phase::Start, 0.0, time);

        // -- 2. 固定步长循环 --
        let mut fixed_steps = 0u32;
        while self.clock.consume_fixed_update() {
            self.run_dispatch(Phase::FixedUpdate, fixed_dt, time);
            self.consume_script_commands(fixed_dt, time);
            self.sync_bodies();
            self.push_kinematic_bodies();
            self.physics.step(fixed_dt);
            self.pull_dynamic_bodies();
            self.dispatch_collisions(fixed_dt, time);
            self.dispatch_character_collisions(fixed_dt, time);
            fixed_steps += 1;
        }
        // 步数上限是"每帧"的预算，必须在整轮固定步长跑完之后再重置。
        self.clock.reset_step_counter();

        // -- 3. update：每渲染帧一次 --
        self.run_dispatch(Phase::Update, frame_dt, time);
        self.consume_script_commands(frame_dt, time);

        // -- 3.1 场景切换：**唯一**合法的换场景时机 --
        //
        // 必须排在这里，三个理由缺一不可：
        // 1. 脚本钩子都跑完了 —— 钩子执行期间 `scene` 正被可变借用，
        //    中途换掉等于把脚下那份抽走；
        // 2. 固定步长循环已经结束 —— 在物理步进中途换场景会让这一步的
        //    `pull_dynamic_bodies` 把旧刚体位姿写进新场景的实体；
        // 3. 还赶得上本帧的粒子 / 音频 / UI 收拾 —— 新场景的发射器与
        //    autoplay 音源当帧就开始推进，而不用空等一帧。
        self.apply_pending_scene_load();
        self.sync_bodies();

        // -- 3.5 粒子：本帧的实体增删与脚本 burst 都已落定 --
        //
        // 放在 `update` 之后：脚本这一帧写的 `particle.burst` 当帧就能炸出来
        // （"按下开火键 → 这一帧看见火花"，而不是下一帧）。
        // 用**渲染帧 dt** 而不是固定步长：粒子是纯视觉，跟着画面走比跟着物理
        // 走更合理（与动画时钟同一取舍）；无头运行里 dt 固定，所以仍逐位可复现。
        self.particles.update(frame_dt, &mut self.scene);

        // -- 4. 音频（帧末：本帧的实体增删与物理回写都已落定） --
        self.sync_audio();
        self.flush_audio();

        // -- 5. 输入帧末清理 --
        self.input.end_frame();

        let (bodies, colliders) = self.physics.stats();
        self.last_report = FrameReport {
            fixed_steps,
            entities: self.scene.entities.len(),
            bodies,
            colliders,
        };
        self.last_report
    }

    /// 按阶段派发（组件轨 + 脚本轨）。
    ///
    /// 先 `let World { .. } = self` 把字段拆开再构造上下文，是为了把
    /// **字段级拆借**写清楚：`scene`（可变）/ `physics`（可变）/ `input`（只读）/
    /// `scripts`（可变）/ 组件侧的注册表与实例表（可变）互不重叠，编译器放行，
    /// 不需要 `RefCell` 或运行期检查。
    fn run_dispatch(&mut self, phase: Phase, delta: f32, time: f32) {
        // 组件轨（Rust）先行、脚本轨随后：两轨共用同一束状态，顺序固定成
        // "快的先跑"——组件钩子里改的状态，脚本当帧可见（反之不保证）。
        self.dispatch_behaviors(phase, delta, time);

        let World {
            scene,
            physics,
            input,
            scripts,
            audio_port,
            clock,
            current_scene,
            scene_order,
            scene_loader,
            ..
        } = self;

        let frame = FrameCtx::new(scene, delta, time)
            .with_input(input)
            .with_physics(physics)
            .with_audio(audio_port)
            .with_clock(clock)
            .with_scene_name(current_scene.as_deref().unwrap_or(""))
            .with_scene_order(scene_order.as_slice());
        let frame = match scene_loader.as_deref() {
            Some(loader) => frame.with_loader(loader),
            None => frame,
        };

        // 组件轨与脚本轨：共用 `scripts` 那一对日志 / 命令缓冲。
        match phase {
            Phase::Start => scripts.dispatch_start(frame),
            Phase::FixedUpdate => scripts.dispatch_fixed_update(frame, delta),
            Phase::Update => scripts.dispatch_update(frame),
        }
    }

    /// 组件轨分发：按 `Entity::behaviors` 逐个调用已实例化组件的钩子。
    ///
    /// # 安全论证（与脚本轨同款）
    ///
    /// 循环体不跨钩子调用持有场景借用：每轮按索引现取 `(id, behaviors 非空)`，
    /// 借用立即结束；钩子里增删实体时，下一轮重新索引 —— 删除会让后面实体
    /// 左移（最多让一个实体本轮少跑），追加的实体还没有实例（`get_mut` 落空，
    /// 自然跳过）。组件实例表在钩子期间不会变（清理只发生在命令消费时），
    /// 所以 `instances` 的借用是稳定的。
    fn dispatch_behaviors(&mut self, phase: Phase, delta: f32, time: f32) {
        // 增量装配：为新实体补实例。判据（与 `sync_bodies` 同款论证）：
        // "声明了 behaviors 的实体数 == 已实例化的实体数" 时没有新面孔 ——
        // 新实体让声明数 +1；删实体在 `destroy_behaviors` 里同步清了实例；
        // 所以不存在"变了但判据看不见"的情形。稳态成本是一次 O(n) 顺序扫描。
        let declared = self
            .scene
            .entities
            .iter()
            .filter(|e| !e.behaviors.is_empty())
            .count();
        // 没有任何组件声明（当前绝大多数项目）：整段早退，稳态只付这次扫描。
        if declared == 0 {
            return;
        }
        if declared != self.behavior_instances.len() {
            self.instantiate_behaviors();
        }

        let World {
            scene,
            physics,
            input,
            scripts,
            audio_port,
            behavior_instances,
            clock,
            current_scene,
            scene_order,
            scene_loader,
            ..
        } = self;
        let (logs, commands) = scripts.buffers_mut();

        let mut index = 0usize;
        while index < scene.entities.len() {
            let entity = &scene.entities[index];
            let id = entity.id;
            let has_behaviors = !entity.behaviors.is_empty();
            index += 1;
            if !has_behaviors {
                continue;
            }
            let Some(instances) = behavior_instances.get_mut(&id) else {
                continue;
            };
            for instance in instances.iter_mut() {
                // start-before-update：Start 阶段只对还没 start 过的组件跑，
                // 标记先记后跑，与脚本轨同款语义 —— 钩子里生成的实体等下
                // 一轮才跑 start。标记挂在组件上而不是实体集合上：热路径
                // 由此零哈希（见 registry.rs 模块文档）。
                if phase == Phase::Start {
                    if instance.started {
                        continue;
                    }
                    instance.started = true;
                }
                let mut game = GameCtx {
                    scene: &mut *scene,
                    physics: Some(&mut *physics),
                    input: Some(input),
                    audio: Some(&mut *audio_port),
                    logs: &mut *logs,
                    commands: &mut *commands,
                    source: instance.name,
                    scene_name: current_scene.as_deref().unwrap_or(""),
                    scene_order: scene_order.as_slice(),
                    loader: scene_loader.as_deref(),
                    clock: Some(&mut *clock),
                    entity: id,
                    delta,
                    time,
                };
                match phase {
                    Phase::Start => {
                        (instance.hooks.start)(&mut instance.component, &mut game, 0.0)
                    }
                    Phase::Update => {
                        (instance.hooks.update)(&mut instance.component, &mut game, delta)
                    }
                    Phase::FixedUpdate => {
                        (instance.hooks.fixed_update)(&mut instance.component, &mut game, delta)
                    }
                }
            }
        }
    }

    /// 为声明了 behaviors 却还没有实例的实体构造组件。
    ///
    /// 未注册的名字记一条警告并跳过（不进实例表）——与"脚本崩溃不拖垮引擎"
    /// 同一容错哲学；同一实体只装配一次，警告不会每帧刷屏。
    ///
    /// 构造后立刻套用实体上存的字段值（P1.5「组件字段进场景文件」）：
    /// 必须在 `start` 之前 —— 组件的 `start` 常常要读参数（"起始高度 =
    /// 某个可调值"）。条目按**注册名**查，名字对不上（组件已改名 / 手改
    /// 场景文件）由 `apply_field_values` 静默吸收，不影响其它字段。
    fn instantiate_behaviors(&mut self) {
        for entity in &self.scene.entities {
            if entity.behaviors.is_empty() || self.behavior_instances.contains_key(&entity.id) {
                continue;
            }
            let mut instances = Vec::with_capacity(entity.behaviors.len());
            for name in &entity.behaviors {
                match self.behaviors.construct_instance(name) {
                    Some(mut instance) => {
                        // 借用拆借：`entity` 是 `&self.scene` 的借用，而
                        // `instance.component` 是独立的所有权对象 —— 套参数
                        // 不碰 World 的任何字段，直接在循环里做即可。
                        if let Some(values) = entity.behavior_params.get(name.as_str()) {
                            xgame_core::inspect::apply_field_values(
                                &mut *instance.component,
                                values,
                            );
                        }
                        instances.push(instance);
                    }
                    None => {
                        log::warn!("实体 '{}' 的行为 '{}' 未注册，已跳过", entity.name, name);
                    }
                }
            }
            self.behavior_instances.insert(entity.id, instances);
        }
    }

    /// 实体被删除：先派发各组件的 `on_destroy`，再清掉实例。
    ///
    /// 调用时机是命令消费时 —— 实体**已经离开场景**（脚本轨 on_destroy 的
    /// 接线沿用同一时机），钩子里读不到自身变换，适合做"通知外部清理"。
    fn destroy_behaviors(&mut self, id: EntityId, delta: f32, time: f32) {
        let Some(instances) = self.behavior_instances.remove(&id) else {
            return;
        };
        let World {
            scene,
            physics,
            input,
            scripts,
            audio_port,
            clock,
            current_scene,
            scene_order,
            scene_loader,
            ..
        } = self;
        let (logs, commands) = scripts.buffers_mut();
        for mut instance in instances {
            let mut game = GameCtx {
                scene: &mut *scene,
                physics: Some(&mut *physics),
                input: Some(input),
                audio: Some(&mut *audio_port),
                logs: &mut *logs,
                commands: &mut *commands,
                source: instance.name,
                scene_name: current_scene.as_deref().unwrap_or(""),
                scene_order: scene_order.as_slice(),
                loader: scene_loader.as_deref(),
                clock: Some(&mut *clock),
                entity: id,
                delta,
                time,
            };
            (instance.hooks.on_destroy)(&mut instance.component, &mut game, 0.0);
        }
    }

    /// 脚本日志（编辑器控制台 / 游戏日志）。
    pub fn take_logs(&mut self) -> Vec<ScriptLog> {
        self.scripts.take_logs()
    }

    /// 重新编译所有脚本（热重载）。
    pub fn reload_scripts(&mut self) {
        self.scripts.invalidate_all();
        let root = self.root.clone();
        self.scripts.preload(&root, &self.scene);
    }

    /// 物理调试线条。
    pub fn debug_lines(&self) -> Vec<DebugLine> {
        self.physics.debug_lines()
    }

    // -----------------------------------------------------------------------
    // 粒子
    // -----------------------------------------------------------------------

    /// 本帧的粒子四边形（渲染入口；空切片 = 这一帧没有粒子）。
    pub fn particle_quads(&self) -> &[xgame_particle::ParticleQuad] {
        self.particles.quads()
    }

    /// 粒子统计（状态栏 / 无头报告 / 测试）。
    pub fn particle_stats(&self) -> xgame_particle::ParticleStats {
        self.particles.stats()
    }

    /// 粒子内容的指纹：宿主用它判断"这一帧要不要重画"。
    ///
    /// 静止时恒定、在动时每帧都变 —— 判据全在
    /// [`xgame_particle::ParticleSystem::signature`] 里，宿主别自己另算一份。
    pub fn particle_signature(&self) -> u64 {
        self.particles.signature()
    }

    /// 粒子是否还在动（帧调度的输入之一，见 `FrameInputs::animating`）。
    pub fn particles_animating(&self) -> bool {
        self.particles.animating()
    }

    // -----------------------------------------------------------------------
    // 相机
    // -----------------------------------------------------------------------

    /// 解析当前**主相机**（`None` = 场景里没有相机实体）。
    ///
    /// 三宿主（独立游戏 / 编辑器 Play / 无头截图）都从这里拿机位，规则本身
    /// 只有一处实现（[`CameraView::from_scene`] → `Scene::active_camera_index`）。
    /// 每调一次做一次线性扫描，所以**每帧调一次**、别在循环里反复调。
    pub fn camera_view(&self) -> Option<CameraView> {
        CameraView::from_scene(&self.scene)
    }

    // -----------------------------------------------------------------------
    // 内部：音频生命周期
    // -----------------------------------------------------------------------

    /// 音频同步：autoplay 首播、空间声源推位置、失联实体停音。
    ///
    /// 每渲染帧一次（不是每个固定步）：人耳对 16 ms 的声源位置更新完全
    /// 不敏感，而固定步可能一帧跑好几个 —— 音频不值得跟物理同频。
    fn sync_audio(&mut self) {
        // 快速路径（先量后改：2000 实体纯视觉场景，这条扫描实测 +5 µs/帧）。
        // 没挂过音频源、没有 autoplay 标记、实体数也没变 ⇒ 没有 autoplay /
        // 空间跟踪要维护，整段跳过。
        // 实体数变化（脚本 spawn / destroy）会重新武装一次扫描；若某个 Rust
        // 行为在运行中直接给**已有**实体贴上 `audio` 组件（不增删实体），
        // 这里扫不到 —— 动态发声的正路是脚本 `play_sound`（不经扫描），
        // 组件走场景配置，这条取舍写在测试 `audio_rescan_picks_up_new_entities`。
        let entity_count_changed = self.scene.entities.len() != self.audio_scan_count;
        if !self.audio_sources_seen && self.audio_autoplayed.is_empty() && !entity_count_changed {
            return;
        }
        self.audio_scan_count = self.scene.entities.len();

        // 先扫（只读场景）再发命令（写 port）：把"读场景"与"写命令束"
        // 分成两段，循环体里不需要为拆借发愁。
        let mut spawned: Vec<(EntityId, PlaySound)> = Vec::new();
        let mut moved: Vec<(EntityId, [f32; 3])> = Vec::new();
        let mut found_sources = false;

        for (index, entity) in self.scene.entities.iter().enumerate() {
            let Some(source) = entity.audio.as_ref() else {
                continue;
            };
            found_sources = true;
            let id = entity.id;
            let world_pos = self.scene.world_matrix(index).w_axis.truncate().to_array();
            let first_time = !self.audio_autoplayed.contains(&id);
            if source.autoplay && first_time {
                let mut play = source.to_play_sound(source.spatial.then_some(world_pos));
                // 绑定挂载实体：让 `StopEntity`（组件摘除 / 实体删除）找得到
                // 这条声音。空间声源还顺便靠它逐帧跟随实体移动。
                play.follow = Some(id);
                spawned.push((id, play));
            } else if source.spatial {
                // 已在响（或 autoplay 关着）的空间声源：逐帧推位置。
                // 脚本用 `follow` 播的声源也靠这条拿到实体的最新坐标 ——
                // 所以 follow 目标实体需要挂一个 spatial 的 `AudioSource`
                // （哪怕 `autoplay: false`），这是"实体是发声体"的声明。
                moved.push((id, world_pos));
            }
        }

        // 刷新"这个场景还挂着音频源吗"——全摘光之后下一帧起走快速路径。
        self.audio_sources_seen = found_sources;

        // 失联判定：autoplay 标记还在、但实体没了 / 组件被摘了 → 停音。
        // 按标记表逐个查（`find_entity` 是 O(n)，标记数 = 历史发声实体数，
        // 通常是个位数）——比"每帧建一个在场实体集合"便宜得多。
        let gone: Vec<EntityId> = self
            .audio_autoplayed
            .iter()
            .copied()
            .filter(|id| {
                self.scene
                    .find_entity(*id)
                    .is_none_or(|index| self.scene.entities[index].audio.is_none())
            })
            .collect();

        let port = &mut self.audio_port;
        for (id, play) in spawned {
            self.audio_autoplayed.insert(id);
            port.play(play);
        }
        for (id, position) in moved {
            port.emitter_position(id, position);
        }
        for id in gone {
            self.audio_autoplayed.remove(&id);
            port.stop_entity(id);
        }
    }

    /// 帧末结算：drain 命令束，连同当前监听者姿态与主音量交给音频引擎。
    fn flush_audio(&mut self) {
        let commands = self.audio_port.drain();
        let listener = self.audio_port.listener;
        let master_volume = self.audio_port.master_volume;
        self.audio.apply(&commands, &listener, master_volume);
    }

    // -----------------------------------------------------------------------
    // 内部：物理体生命周期
    // -----------------------------------------------------------------------

    /// 把脚本 / 组件产生的结构性改动落到物理侧。
    fn consume_script_commands(&mut self, delta: f32, time: f32) {
        let report = self.scripts.take_commands();
        if report.is_empty() {
            return;
        }

        for id in report.removed {
            // 先通知组件（on_destroy），再清刚体。
            self.destroy_behaviors(id, delta, time);
            if let Some(body) = self.body_of_entity.remove(&id) {
                self.physics.remove_body(body);
            }
            // 角色的接触面备忘随实体一起清掉。
            self.character_contacts.remove(&id);
        }

        // 生成的实体：`sync_bodies` 会统一补建（它按"有 physics 但没有刚体"来扫）。
        // 这里只需要让新实体下帧能跑 `start` —— `dispatch_start` 每帧都扫，
        // 天然覆盖。

        // 换过父节点的实体：世界位置变了，把刚体挪过去。
        for id in report.reparented {
            let Some(body) = self.body_of_entity.get(&id).copied() else {
                continue;
            };
            let Some(index) = self.scene.find_entity(id) else {
                continue;
            };
            let world = self.scene.world_transform(index);
            self.physics.teleport(body, world.position);
            self.physics.set_rotation(body, world.rotation);
        }

        // 场景切换请求：这里只登记，真正的交换在 `apply_pending_scene_load`。
        // 固定步长循环里也会调到这里，所以**不能**就地换场景 ——
        // 那样本步的 `pull_dynamic_bodies` 会把旧刚体位姿写进新场景。
        if let Some(path) = report.scene_load {
            self.request_scene_load(path);
        }
    }

    /// 让物理世界的刚体集合与场景里声明了 `physics` 的实体保持一致。
    ///
    /// # 稳态快路径（这条是性能的关键）
    ///
    /// 绝大多数帧里"哪些实体有刚体"根本没变。旧实现无条件跑完整的三段扫描：
    /// 补建扫描 + 建一张 `HashSet` 的存活集 + 清理扫描，最后还无条件
    /// `sync_queries()`（重建射线用的 BVH）。这在一个静态场景里是纯粹的浪费 ——
    /// 而且 `physics.step()` 每步已经顺带维护了查询流水线，稳态再重建一次毫无收益。
    ///
    /// 现在先做**一次无分配的扫描**：既收集"待补建"的实体，也顺便数出物理实体总数。
    /// 只要"待补建为空"且"总数与已有刚体数一致"，就能断定集合没有任何结构性变化，
    /// 清理扫描与查询重建一起跳过。
    ///
    /// 这个判据是完备的：新增实体（无论是否带 physics）会让待补建非空或总数变化；
    /// 删实体 / 去掉 physics 会让总数与刚体数不等；"一增一删抵消"的巧合里，
    /// 新增那个的 id 一定不在映射表里 → 待补建非空。所以不存在漏判。
    fn sync_bodies(&mut self) {
        // 取走暂存（避免与下面的 `&self.scene` 借用打架），跑完再放回去。
        let mut to_create = std::mem::take(&mut self.scratch_physics_indices);
        to_create.clear();

        let mut physics_count = 0usize;
        let mut has_kinematic = false;
        let mut has_dynamic = false;
        for (index, entity) in self.scene.entities.iter().enumerate() {
            // 角色实体：刚体与碰撞体由**运行时托管**（运动学位置 + 胶囊），
            // 不走 `PhysicsBody` —— 后者默认映射成速度驱动的运动学体，而
            // rapier 的角色控制器要求位置驱动。即便作者同时配了 physics，
            // 这里也以 controller 为准（见 core::character 的模块文档）。
            if entity.controller.is_some() {
                physics_count += 1;
                if !self.body_of_entity.contains_key(&entity.id) {
                    to_create.push(index);
                }
                continue;
            }
            if let Some(def) = &entity.physics {
                physics_count += 1;
                // 顺带记下"有没有运动学 / 动态体"，省掉 push / pull 各自再扫一遍。
                match def.kind {
                    xgame_core::BodyKind::Kinematic => has_kinematic = true,
                    xgame_core::BodyKind::Dynamic => has_dynamic = true,
                    xgame_core::BodyKind::Static => {}
                }
                if !self.body_of_entity.contains_key(&entity.id) {
                    to_create.push(index);
                }
            }
        }
        self.has_kinematic = has_kinematic;
        self.has_dynamic = has_dynamic;

        if to_create.is_empty() && physics_count == self.body_of_entity.len() {
            // 稳态：没有任何结构性变化。刚体集合没动，查询结构也就不用重建。
            self.scratch_physics_indices = to_create;
            return;
        }

        // -- 补建 --
        for index in to_create.drain(..) {
            let entity = &self.scene.entities[index];
            let id = entity.id;
            let world = self.scene.world_transform(index);
            let (body, collider) = if let Some(controller) = entity.controller {
                // 参数先过容错钳制：场景文件是手写的，负半径 / NaN 会让 rapier
                // 建出 NaN 胶囊，整个模拟再也回不来。
                let c = controller.sanitized();
                (
                    // 初始位置抬到"脚底正好踩在世界变换的原点"上：角色原点在
                    // 胶囊中心，直接按实体变换放会让半个身子陷进地面。
                    RigidBody::kinematic_position_at(Transform {
                        position: world.position + Vec3::Y * c.center_height(),
                        rotation: world.rotation,
                        ..Transform::IDENTITY
                    }),
                    Collider::capsule(c.half_height(), c.radius),
                )
            } else {
                let Some(def) = entity.physics else { continue };
                body_from_scene(&def, world)
            };
            let body_id = self.physics.add_body(body);
            // `body_id` 是上一行刚建出来的，必然存在；`add_collider` 这里返回 `None`
            // 只可能是引擎内部状态不一致（那里已经记了警告）。退化成"没有碰撞体"，
            // 而不是把整帧 panic 掉。
            if self.physics.add_collider(body_id, collider).is_none() {
                log::warn!("实体 {id} 的碰撞体挂载失败：该刚体将不参与碰撞");
            }

            self.body_of_entity.insert(id, body_id);
            // 脚本要靠这个组件找到刚体（`apply_force` / `velocity` 等）。
            self.scene.entities[index]
                .components
                .insert(PhysicsHandle(body_id));
        }

        // -- 清理 --
        let live: HashSet<EntityId> = self
            .scene
            .entities
            .iter()
            .filter(|e| e.physics.is_some() || e.controller.is_some())
            .map(|e| e.id)
            .collect();
        let dead: Vec<EntityId> = self
            .body_of_entity
            .keys()
            .copied()
            .filter(|id| !live.contains(id))
            .collect();
        for id in dead {
            if let Some(body) = self.body_of_entity.remove(&id) {
                self.physics.remove_body(body);
            }
        }

        // 碰撞体集合变了，射线用的加速结构要跟着刷 —— 否则编辑器在暂停状态
        // 下拾取会打到上一帧的包围体上。
        self.physics.sync_queries();

        self.scratch_physics_indices = to_create;
    }

    /// 运动学体：把脚本 / 动画摆好的世界变换推给物理。
    fn push_kinematic_bodies(&mut self) {
        // 场景里没有运动学体时，连世界矩阵都不必解算 —— 这是纯静态 / 纯动态场景
        // （很常见）的稳态路径，省掉每固定步一次 O(n) 分配 + 矩阵连乘。
        // `has_kinematic` 是 `sync_bodies`（本步稍早）那次扫描顺带记下的，不用再扫。
        if !self.has_kinematic {
            return;
        }

        let world_matrices = self.scene.world_matrices();
        for (index, entity) in self.scene.entities.iter().enumerate() {
            let Some(def) = &entity.physics else { continue };
            // 角色实体的位姿是角色控制器写的，运动学推送会把刚移动的结果覆盖掉。
            if entity.controller.is_some() {
                continue;
            }
            if def.kind != xgame_core::BodyKind::Kinematic {
                continue;
            }
            let Some(body) = self.body_of_entity.get(&entity.id).copied() else {
                continue;
            };
            let Some(matrix) = world_matrices.get(index) else {
                continue;
            };
            let (_, rotation, position) = matrix.to_scale_rotation_translation();
            self.physics.teleport(body, position);
            self.physics.set_rotation(body, rotation);
        }
    }

    /// 把这一步的碰撞事件派发给组件轨与脚本轨。
    ///
    /// # 为什么事件在这里派发而不是在 `step` 里
    ///
    /// 物理层只知道刚体句柄，`GameCtx` 只知道实体 —— 映射表在运行时手里。
    /// 派发放在固定步长的末尾：此时 `pull_dynamic_bodies` 已经把本步的位置
    /// 写回实体，钩子里读 `pos()` 拿到的是**碰撞发生后**的位置（与 Unity 的
    /// `OnCollisionEnter` 时序一致）。
    ///
    /// 事件整批取走再派发：`drain_events` 借的是 `physics`，而派发要 `&mut self`。
    fn dispatch_collisions(&mut self, delta: f32, time: f32) {
        let events: Vec<CollisionEvent> = self.physics.drain_events().collect();
        if events.is_empty() {
            return;
        }
        for event in events {
            let (Some(a), Some(b)) = (
                self.entity_of_body(event.body_a),
                self.entity_of_body(event.body_b),
            ) else {
                // 对不上实体的刚体（脚本直接建的临时刚体）没有收件人。
                continue
            };
            if a == b {
                continue;
            }
            // 双向派发：Unity 的 OnCollisionEnter 也是两边各收一次。
            self.dispatch_collision_to(a, b, event.started, delta, time);
            self.dispatch_collision_to(b, a, event.started, delta, time);
        }
    }

    /// 给一个实体派发"和 `other` 碰撞"（组件轨 + 脚本轨）。
    fn dispatch_collision_to(
        &mut self,
        entity: EntityId,
        other: EntityId,
        started: bool,
        delta: f32,
        time: f32,
    ) {
        if self.behavior_instances.contains_key(&entity) {
            let World {
                scene,
                physics,
                input,
                scripts,
                audio_port,
                behavior_instances,
                clock,
                current_scene,
                scene_order,
                scene_loader,
                ..
            } = self;
            let (logs, commands) = scripts.buffers_mut();
            if let Some(instances) = behavior_instances.get_mut(&entity) {
                for instance in instances.iter_mut() {
                    let mut game = GameCtx {
                        scene: &mut *scene,
                        physics: Some(&mut *physics),
                        input: Some(input),
                        audio: Some(&mut *audio_port),
                        logs: &mut *logs,
                        commands: &mut *commands,
                        source: instance.name,
                        scene_name: current_scene.as_deref().unwrap_or(""),
                        scene_order: scene_order.as_slice(),
                        loader: scene_loader.as_deref(),
                        clock: Some(&mut *clock),
                        entity,
                        delta,
                        time,
                    };
                    if started {
                        (instance.hooks.collision_enter)(&mut instance.component, &mut game, other);
                    } else {
                        (instance.hooks.collision_exit)(&mut instance.component, &mut game, other);
                    }
                }
            }
        }

        let World {
            scene,
            physics,
            input,
            scripts,
            audio_port,
            clock,
            current_scene,
            scene_order,
            scene_loader,
            ..
        } = self;
        // 与其它派发路径**同款注入**：早先这里只给了 input/physics/audio，
        // 结果 `on_collision_enter` 里读 `current_scene()` 恒为空串、切场景 /
        // 生成预制体全部静默失败 —— 同一份 API 在不同钩子里行为不一致，
        // 是最难查的那类 bug。
        let frame = FrameCtx::new(scene, delta, time)
            .with_input(input)
            .with_physics(physics)
            .with_audio(audio_port)
            .with_clock(clock)
            .with_scene_name(current_scene.as_deref().unwrap_or(""))
            .with_scene_order(scene_order.as_slice());
        let frame = match scene_loader.as_deref() {
            Some(loader) => frame.with_loader(loader),
            None => frame,
        };
        scripts.dispatch_collision(frame, entity, other, started);
    }

    /// 把角色的接触面变化派发成 `on_collision_enter` / `on_collision_exit`。
    ///
    /// 与 [`dispatch_collisions`](Self::dispatch_collisions) 的分工：那边处理
    /// **动态刚体**（rapier 会发真实接触事件），这边处理**角色**（运动学胶囊
    /// 永远停在 skin 距离外，物理层根本不会有接触）。两条路都汇到同一个
    /// `dispatch_collision_to`，所以脚本 / 组件看到的语义完全一致。
    fn dispatch_character_collisions(&mut self, delta: f32, time: f32) {
        // 先把差分算完再派发：派发要 `&mut self`，不能一边借场景一边改场景。
        let mut events: Vec<(EntityId, EntityId, bool)> = Vec::new();
        for index in 0..self.scene.entities.len() {
            let entity = &self.scene.entities[index];
            if entity.controller.is_none() {
                continue;
            }
            let id = entity.id;
            let now = entity
                .component::<xgame_core::CharacterState>()
                .map(|s| s.contacts)
                .unwrap_or_default();
            let before = self
                .character_contacts
                .get(&id)
                .copied()
                .unwrap_or_default();
            if now == before {
                continue;
            }
            for body in now.iter() {
                if !before.contains(body) {
                    if let Some(other) = self.entity_of_body(body) {
                        if other != id {
                            push_collision_pair(&mut events, id, other, true);
                        }
                    }
                }
            }
            for body in before.iter() {
                if !now.contains(body) {
                    if let Some(other) = self.entity_of_body(body) {
                        if other != id {
                            push_collision_pair(&mut events, id, other, false);
                        }
                    }
                }
            }
            self.character_contacts.insert(id, now);
        }
        // 已销毁角色的条目在 `consume_script_commands` 里随实体一起删 ——
        // 每帧 retain 一遍是 O(角色数²)，不值得。
        // 双向派发（Unity 同款）：两个角色互相撞上时，两边各自的接触集都会
        // 记到对方，`push_collision_pair` 已经把这一对合并成一条事件。
        for (a, b, started) in events {
            self.dispatch_collision_to(a, b, started, delta, time);
            self.dispatch_collision_to(b, a, started, delta, time);
        }
    }

    /// 刚体 id → 实体 id（线性扫：碰撞事件远比实体少，不值得为它维护反查表）。
    fn entity_of_body(&self, body: u64) -> Option<EntityId> {
        self.body_of_entity
            .iter()
            .find(|(_, b)| **b == body)
            .map(|(id, _)| *id)
    }

    /// 动态体：把物理算出的世界变换写回实体的**局部**变换。
    fn pull_dynamic_bodies(&mut self) {
        // 没有动态体时不必解算层级（同上：静态场景的稳态路径；标志位复用免一次扫描）。
        if !self.has_dynamic {
            return;
        }

        // 父索引与世界矩阵一次解算同时拿到（见 `world_matrices_with_parents`）：
        // 旧写法先 `parent_indices()` 再 `world_matrices()`，而后者内部又要算一遍
        // 父索引 —— 每个固定步都白跑一次 `hierarchy()`。
        let (parent_indices, world_matrices) = self.scene.world_matrices_with_parents();

        for index in 0..self.scene.entities.len() {
            let id = self.scene.entities[index].id;
            let Some(def) = &self.scene.entities[index].physics else {
                continue;
            };
            // 角色实体即使误配了动态刚体也不回收 —— 它的位姿归角色控制器管。
            if self.scene.entities[index].controller.is_some() {
                continue;
            }
            if def.kind != xgame_core::BodyKind::Dynamic {
                continue;
            }
            let Some(body) = self.body_of_entity.get(&id).copied() else {
                continue;
            };
            let Some(phys) = self.physics.body_transform(body) else {
                continue;
            };

            let parent_world: Mat4 = parent_indices
                .get(index)
                .copied()
                .flatten()
                .map(|p| world_matrices[p])
                .unwrap_or(Mat4::IDENTITY);

            let local = if parent_world.determinant().abs() > 1e-12 {
                Transform::from_matrix(parent_world.inverse() * phys.local_matrix())
            } else {
                phys
            };

            if let Some(entity) = self.scene.entities.get_mut(index) {
                entity.transform.position = local.position;
                entity.transform.rotation = local.rotation;
                // 缩放不动：物理体没有缩放概念，写了会把视觉尺寸吃掉。
            }
        }
    }
}

/// 把一对碰撞实体规范化（小 id 在前）后去重压入事件表。
///
/// 两个角色互相撞上时，两边各自的接触集都会记到对方 —— 不做规范化就会
/// 收到两条方向相反的同一次碰撞。进出方向由 `started` 区分，所以去重键是
/// `(小 id, 大 id, started)` 三元组。
fn push_collision_pair(
    events: &mut Vec<(EntityId, EntityId, bool)>,
    a: EntityId,
    b: EntityId,
    started: bool,
) {
    let (lo, hi) = if a.raw() <= b.raw() { (a, b) } else { (b, a) };
    if events
        .iter()
        .any(|(x, y, s)| *x == lo && *y == hi && *s == started)
    {
        return;
    }
    events.push((lo, hi, started));
}

/// 派发阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Start,
    FixedUpdate,
    Update,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use xgame_core::project::ProjectHandle;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use xgame_core::{AudioSource, Entity, EntityId, PhysicsBody, Prefab};

    fn entity_y(world: &World, name: &str) -> f32 {
        world
            .scene()
            .find_by_name(name)
            .expect("实体应当还在")
            .transform
            .position
            .y
    }

    /// 清单里的重力 / 步长必须真的流进 `WorldConfig` —— 这是项目设置面板
    /// （project.ron → 两端运行时）的落地验证。旧实现这里是 `WorldConfig::default()`，
    /// 清单改了重力也纹丝不动，所以断言自定义值会红。
    #[test]
    fn project_runtime_flows_into_world_config() {
        let rt = xgame_core::RuntimeConfig {
            gravity: [0.0, -20.0, 0.0],
            fixed_dt: 1.0 / 120.0,
        };
        let cfg = WorldConfig::from_project_runtime(&rt);
        assert_eq!(cfg.gravity, Vec3::new(0.0, -20.0, 0.0));
        assert_eq!(cfg.fixed_dt, 1.0 / 120.0);
        // substeps 不在清单里，保持默认。
        assert_eq!(cfg.physics_sub_steps, WorldConfig::default().physics_sub_steps);

        // 手改出的垃圾值不能流进物理：fixed_dt=0 是"时间静止"。
        let bad = xgame_core::RuntimeConfig {
            gravity: [0.0, f32::NAN, 0.0],
            fixed_dt: 0.0,
        };
        let cfg = WorldConfig::from_project_runtime(&bad);
        assert_eq!(cfg.fixed_dt, 1.0 / 60.0);
        assert!(cfg.gravity.is_finite());
    }

    /// 用静态地面 + 动态方块跑一段模拟，验证"动态体由物理驱动"这条链路。
    ///
    /// 这个测试覆盖的是**整条**运行时管线：建刚体 → 固定步长 → 回写局部变换。
    /// 之前每个宿主各写一遍的时候，就是在这里漏掉过父矩阵换算。
    #[test]
    fn dynamic_body_falls_onto_static_ground() {
        let mut scene = Scene::new("physics");

        let mut ground = Entity::new("ground");
        ground.physics = Some(PhysicsBody::static_box([10.0, 0.5, 10.0]));
        ground.transform.position = Vec3::new(0.0, -0.5, 0.0);
        scene.add_entity(ground);

        let mut cube = Entity::new("cube");
        cube.physics = Some(PhysicsBody::dynamic_box([0.5, 0.5, 0.5]));
        cube.transform.position = Vec3::new(0.0, 5.0, 0.0);
        scene.add_entity(cube);

        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default());

        // 3 秒足够从 5 米落下并稳定下来。
        for _ in 0..180 {
            world.update(1.0 / 60.0);
        }

        let y = entity_y(&world, "cube");
        assert!(
            (0.3..1.2).contains(&y),
            "方块应当停在地面上（中心约 0.5），实际 y = {y}"
        );

        // 地面是静态体，不该被重力拖着走。
        assert_eq!(entity_y(&world, "ground"), -0.5);
    }

    /// 一个只会往前走的角色组件（测试用）。
    #[derive(Default)]
    struct Walker;

    xgame_core::impl_no_inspect!(Walker);

    impl Lifecycle for Walker {
        fn fixed_update(&mut self, ctx: &mut Ctx, _dt: f32) {
            ctx.character_simple_move(Vec3::new(2.0, 0.0, 0.0));
            assert!(ctx.character_is_grounded(), "走上地面后应报告着地");
        }
    }

    /// 角色实体：运行时自己建运动学胶囊刚体，脚本 / 组件通过 `character_*`
    /// 驱动，实体变换跟着走。
    ///
    /// 这条测试覆盖"实体 → 刚体 → 移动 → 写回变换"整条链路，也钉住脚底
    /// 坐标约定：激活位置 (0,0,0) 的角色应当站在 y=0 的地面上，而不是半截
    /// 身子埋进去。
    #[test]
    fn character_entity_walks_on_the_ground() {
        let mut scene = Scene::new("character");
        let mut ground = Entity::new("ground");
        ground.physics = Some(PhysicsBody::static_box([20.0, 0.5, 20.0]));
        ground.transform.position = Vec3::new(0.0, -0.5, 0.0);
        scene.add_entity(ground);

        let mut player = Entity::new("player");
        player.controller = Some(xgame_core::CharacterController::default());
        player.behaviors.push("Walker".to_owned());
        // 脚底就在 y=0：角色原点按"脚底"解释（见 center_height 的文档）。
        player.transform.position = Vec3::ZERO;
        scene.add_entity(player);

        let mut registry = BehaviorRegistry::new();
        registry.register::<Walker>("Walker");
        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default())
            .with_behavior_registry(registry);

        for _ in 0..60 {
            world.update(1.0 / 60.0);
        }

        let player = world.scene().find_by_name("player").expect("角色还在");
        let p = player.transform.position;
        assert!(
            (p.x - 2.0).abs() < 0.2,
            "1 秒 × 2 m/s 应走到 x≈2，实际 x={}",
            p.x
        );
        assert!(
            p.y.abs() < 0.05,
            "应贴着地面走（脚底 y≈0），实际 y={}",
            p.y
        );
        let state = player
            .component::<xgame_core::CharacterState>()
            .copied()
            .expect("移动过就应有状态组件");
        assert!(state.grounded, "地面上应一直着地");
        assert!(state.ground_normal.y > 0.99, "平地法线应朝上");
    }

    static APEX_BITS: AtomicU32 = AtomicU32::new(0);
    static JUMPED: AtomicUsize = AtomicUsize::new(0);

    /// 着地后起跳一次的组件（测试用）。
    #[derive(Default)]
    struct Jumper;

    xgame_core::impl_no_inspect!(Jumper);

    impl Lifecycle for Jumper {
        fn fixed_update(&mut self, ctx: &mut Ctx, _dt: f32) {
            if ctx.character_is_grounded() && JUMPED.load(Ordering::Relaxed) == 0 {
                assert!(ctx.character_jump(5.0), "着地时起跳应当成功");
                JUMPED.store(1, Ordering::Relaxed);
            }
            ctx.character_simple_move(Vec3::ZERO);
            // 正数的 f32 位模式与大小同序，所以可以直接按位取 max。
            APEX_BITS.fetch_max(ctx.pos().y.to_bits(), Ordering::Relaxed);
        }
    }

    /// 跳跃：5 m/s 起跳 → 最高点约 v²/2g ≈ 1.27m → 落回地面并恢复着地。
    ///
    /// 这条钉住运行时层的竖直速度时序：起跳当帧 `character_simple_move`
    /// 不能被"落地钳制"吃掉初速，落地当帧也不能把速度清零到穿地。
    #[test]
    fn character_jump_leaves_and_returns_to_the_ground() {
        APEX_BITS.store(0, Ordering::Relaxed);
        JUMPED.store(0, Ordering::Relaxed);

        let mut scene = Scene::new("jump");
        let mut ground = Entity::new("ground");
        ground.physics = Some(PhysicsBody::static_box([20.0, 0.5, 20.0]));
        ground.transform.position = Vec3::new(0.0, -0.5, 0.0);
        scene.add_entity(ground);

        let mut player = Entity::new("player");
        player.controller = Some(xgame_core::CharacterController::default());
        player.behaviors.push("Jumper".to_owned());
        scene.add_entity(player);

        let mut registry = BehaviorRegistry::new();
        registry.register::<Jumper>("Jumper");
        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default())
            .with_behavior_registry(registry);
        for _ in 0..150 {
            world.update(1.0 / 60.0);
        }

        let apex = f32::from_bits(APEX_BITS.load(Ordering::Relaxed));
        assert!(apex > 0.8, "应跳到 ~1.27m 高，实际最高 {apex}");
        let player = world.scene().find_by_name("player").expect("角色还在");
        assert!(
            player.transform.position.y.abs() < 0.05,
            "应落回地面（脚底 y≈0），实际 y={}",
            player.transform.position.y
        );
        let state = player
            .component::<xgame_core::CharacterState>()
            .copied()
            .expect("应有状态");
        assert!(state.grounded, "落回后应恢复着地");
    }

    static HITS: AtomicUsize = AtomicUsize::new(0);
    static HIT_EXITS: AtomicUsize = AtomicUsize::new(0);
    static HIT_OTHER: AtomicU64 = AtomicU64::new(0);

    /// 串行锁：上面三个是**进程级**静态变量，两条测试并行跑会互相清计数 /
    /// 读到对方的实体 id（`cargo test` 默认 4 线程，实测 100% 必现）。
    ///
    /// 谁再写"用全局静态记录回调"的测试，也请拿这把锁 —— 或者干脆改成
    /// 组件实例自带的计数器（那才是根治，但会动被测组件的形状）。
    static COLLISION_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serial_collision_test() -> std::sync::MutexGuard<'static, ()> {
        // 前一条测试 panic 时锁会中毒；这里直接取回内部值，让后续测试照常跑
        // （否则一次失败会连锁成一片"poisoned"失败，掩盖真正的原因）。
        COLLISION_SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 记录碰撞事件的组件（测试用）。
    #[derive(Default)]
    struct CollisionProbe;

    xgame_core::impl_no_inspect!(CollisionProbe);

    impl Lifecycle for CollisionProbe {
        fn on_collision_enter(&mut self, _ctx: &mut Ctx, other: EntityId) {
            HITS.fetch_add(1, Ordering::Relaxed);
            HIT_OTHER.store(other.raw(), Ordering::Relaxed);
        }
        fn on_collision_exit(&mut self, _ctx: &mut Ctx, _other: EntityId) {
            HIT_EXITS.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 角色撞墙 → 组件收到 `on_collision_enter`，参数是角色的实体 id。
    ///
    /// 这条同时钉住一个上游细节：rapier 会为**运动学 vs 静态**的碰撞体对
    /// 生成碰撞事件（角色的胶囊是运动学位置刚体，墙是静态体）。如果哪天
    /// 升级后不再生成，这里会先红。
    #[test]
    fn character_collision_events_reach_components() {
        let _guard = serial_collision_test();
        HITS.store(0, Ordering::Relaxed);
        HIT_EXITS.store(0, Ordering::Relaxed);
        HIT_OTHER.store(0, Ordering::Relaxed);

        let mut scene = Scene::new("collide");
        let mut ground = Entity::new("ground");
        ground.physics = Some(PhysicsBody::static_box([20.0, 0.5, 20.0]));
        ground.transform.position = Vec3::new(0.0, -0.5, 0.0);
        scene.add_entity(ground);

        // 墙：占 x ∈ [2.0, 3.0]，高 3 米（足够挡住 1 秒的行走）。
        let mut wall = Entity::new("wall");
        wall.physics = Some(PhysicsBody::static_box([0.5, 1.5, 3.0]));
        wall.transform.position = Vec3::new(2.5, 1.5, 0.0);
        wall.behaviors.push("Probe".to_owned());
        scene.add_entity(wall);

        let mut player = Entity::new("player");
        player.controller = Some(xgame_core::CharacterController::default());
        player.behaviors.push("Walker".to_owned());
        scene.add_entity(player);

        let mut registry = BehaviorRegistry::new();
        registry.register::<Walker>("Walker");
        registry.register::<CollisionProbe>("Probe");
        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default())
            .with_behavior_registry(registry);

        for _ in 0..90 {
            world.update(1.0 / 60.0);
        }

        assert!(
            HITS.load(Ordering::Relaxed) >= 1,
            "角色撞墙应触发 on_collision_enter（运动学 vs 静态）"
        );
        let player_entity = world.scene().find_by_name("player").expect("角色还在");
        assert_eq!(
            HIT_OTHER.load(Ordering::Relaxed),
            player_entity.id.raw(),
            "事件的对方参数应当是角色的实体 id"
        );
        // 速度是**实际**位移反推的：撞墙后水平速度必须归零（动画混合依赖它）。
        let state = player_entity
            .component::<xgame_core::CharacterState>()
            .copied()
            .expect("应有状态");
        assert!(
            state.velocity.x.abs() < 0.1,
            "撞墙后水平速度应≈0，实际 vx={}",
            state.velocity.x
        );
    }

    /// 走进传感器（trigger）：不挡路，但要能触发 `on_collision_enter`。
    ///
    /// 角色的接触面来自 `move_shape`，而扫掠按 Unity 语义排除传感器 ——
    /// 触发区靠 `move_character` 里那次额外的重叠查询补上。
    #[test]
    fn character_enters_a_trigger_volume() {
        let _guard = serial_collision_test();
        HITS.store(0, Ordering::Relaxed);
        HIT_OTHER.store(0, Ordering::Relaxed);

        let mut scene = Scene::new("trigger");
        let mut ground = Entity::new("ground");
        ground.physics = Some(PhysicsBody::static_box([20.0, 0.5, 20.0]));
        ground.transform.position = Vec3::new(0.0, -0.5, 0.0);
        scene.add_entity(ground);

        let mut zone = Entity::new("zone");
        let mut body = PhysicsBody::static_box([0.5, 1.0, 1.0]);
        body.is_sensor = true;
        zone.physics = Some(body);
        zone.transform.position = Vec3::new(1.0, 1.0, 0.0);
        zone.behaviors.push("Probe".to_owned());
        scene.add_entity(zone);

        let mut player = Entity::new("player");
        player.controller = Some(xgame_core::CharacterController::default());
        player.behaviors.push("Walker".to_owned());
        scene.add_entity(player);

        let mut registry = BehaviorRegistry::new();
        registry.register::<Walker>("Walker");
        registry.register::<CollisionProbe>("Probe");
        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default())
            .with_behavior_registry(registry);
        for _ in 0..60 {
            world.update(1.0 / 60.0);
        }

        assert!(
            HITS.load(Ordering::Relaxed) >= 1,
            "走进触发区应触发 on_collision_enter"
        );
        let player_id = world.scene().find_by_name("player").expect("角色还在").id;
        assert_eq!(HIT_OTHER.load(Ordering::Relaxed), player_id.raw());
        assert!(
            world.scene().find_by_name("player").unwrap().transform.position.x > 1.5,
            "传感器不该挡住角色"
        );
    }

    /// 静态体不吃重力、动态体吃 —— 用数值确认 `PhysicsBody::kind` 真的接进了物理。
    #[test]
    fn static_bodies_never_fall() {
        let mut scene = Scene::new("static");
        let mut pillar = Entity::new("pillar");
        pillar.physics = Some(PhysicsBody::static_box([0.5, 3.0, 0.5]));
        pillar.transform.position = Vec3::new(0.0, 10.0, 0.0);
        scene.add_entity(pillar);

        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default());
        for _ in 0..120 {
            world.update(1.0 / 60.0);
        }
        assert_eq!(entity_y(&world, "pillar"), 10.0);
    }

    /// 脚本真的能在运行时里驱动实体。
    #[test]
    fn scripts_run_inside_the_world() {
        let dir = std::env::temp_dir().join(format!(
            "xgame_runtime_script_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("Scripts")).unwrap();
        std::fs::write(
            dir.join("Scripts/drive.lua"),
            r#"
            function start(ctx) set_pos(ctx, 1.0, 0.0, 0.0) end
            function update(ctx)
                local p = pos(ctx)
                set_pos(ctx, p.x + 0.1, p.y, p.z)
            end
            "#,
        )
        .unwrap();

        let mut scene = Scene::new("scripted");
        let mut actor = Entity::new("actor");
        actor.scripts.push("Scripts/drive.lua".to_owned());
        scene.add_entity(actor);

        let mut world = World::new(scene, &dir, WorldConfig::default());
        for _ in 0..10 {
            world.update(1.0 / 60.0);
        }

        let x = world.scene().find_by_name("actor").unwrap().transform.position.x;
        // start 摆到 1.0，之后每帧 +0.1。
        assert!((x - 2.0).abs() < 1e-3, "脚本应当把 x 推到约 2.0，实际 {x}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Lua 脚本驱动角色 + 收到碰撞事件：整条链路的端到端验证。
    ///
    /// 覆盖四段：脚本调用 `character_simple_move` → 物理层移动运动学胶囊 →
    /// 运行时把接触面差分派发回去 → 脚本的 `on_collision_enter` 收到对方实体 id。
    #[test]
    fn lua_scripts_drive_characters_and_receive_collisions() {
        let dir = std::env::temp_dir().join(format!(
            "xgame_runtime_character_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("Scripts")).unwrap();
        std::fs::write(
            dir.join("Scripts/walk.lua"),
            r#"
            function fixed_update(ctx)
                character_simple_move(ctx, 2.0, 0.0, 0.0)
            end
            "#,
        )
        .unwrap();
        std::fs::write(
            dir.join("Scripts/bump.lua"),
            r#"
            function on_collision_enter(ctx, other)
                log(ctx, "bumped " .. tostring(other) .. " grounded=" .. tostring(character_is_grounded(ctx)))
            end
            "#,
        )
        .unwrap();

        let mut scene = Scene::new("lua_character");
        let mut ground = Entity::new("ground");
        ground.physics = Some(PhysicsBody::static_box([20.0, 0.5, 20.0]));
        ground.transform.position = Vec3::new(0.0, -0.5, 0.0);
        scene.add_entity(ground);

        let mut wall = Entity::new("wall");
        wall.physics = Some(PhysicsBody::static_box([0.5, 1.5, 3.0]));
        wall.transform.position = Vec3::new(2.5, 1.5, 0.0);
        wall.scripts.push("Scripts/bump.lua".to_owned());
        scene.add_entity(wall);

        let mut player = Entity::new("player");
        player.controller = Some(xgame_core::CharacterController::default());
        player.scripts.push("Scripts/walk.lua".to_owned());
        scene.add_entity(player);

        let mut world = World::new(scene, &dir, WorldConfig::default());
        for _ in 0..90 {
            world.update(1.0 / 60.0);
        }

        let logs = world.take_logs();
        assert!(
            logs.iter().any(|l| l.message.starts_with("bumped ")),
            "墙上的脚本应收到 on_collision_enter，日志：{logs:?}"
        );
        let p = world
            .scene()
            .find_by_name("player")
            .expect("角色还在")
            .transform
            .position;
        assert!(
            (p.x - 1.68).abs() < 0.05,
            "角色应被墙挡在 x≈1.68，实际 x={}",
            p.x
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 仓库里的示例项目：清单、场景、输入映射、脚本四样都要能干净加载。
    ///
    /// 这条测试是"示例项目别腐烂"的保险丝 —— 字段改名、RON 语法写错、
    /// 脚本 API 重命名，任何一样都会在这里被抓住，而不是等用户双击时才炸。
    #[test]
    fn sample_project_assets_all_load() {
        let root = repository_sample_path();

        let project = ProjectHandle::open(&root).expect("示例项目应当能打开");
        let scene = project.load_entry_scene().expect("入口场景应当能加载");
        assert!(scene.entities.len() >= 10, "示例场景实体太少，被改坏了吗");

        let map = InputMap::load(&root.join("input.ron"))
            .expect("input.ron 应当能解析")
            .expect("input.ron 应当存在");
        assert!(map.actions.contains_key("Jump"), "示例映射应当有 Jump 动作");
        assert!(map.axes.contains_key("MoveX"));

        let mut host = LuaHost::new();
        let report = host.preload(&root, &scene);
        assert!(
            report.failed.is_empty(),
            "示例脚本编译失败: {:?}",
            report.failed
        );
        assert!(!report.loaded.is_empty(), "示例场景应当挂了脚本");
    }

    /// 把示例项目真的跑起来：物理落定、脚本生效。
    #[test]
    fn sample_project_runs() {
        let root = repository_sample_path();
        let project = ProjectHandle::open(&root).expect("打开示例项目");
        let scene = project.load_entry_scene().expect("加载场景");

        let mut world = World::new(scene, &root, WorldConfig::default());
        for _ in 0..180 {
            world.update(1.0 / 60.0);
            // 示例脚本不该在运行期报错；报错说明示例或 API 被改坏了。
            for log in world.take_logs() {
                assert_ne!(
                    log.level,
                    crate::LogLevel::Error,
                    "示例脚本 '{}' 运行时报错: {}",
                    log.script,
                    log.message
                );
            }
        }

        // 自由落体的箱子应当已经落在地面附近，而不是继续无限下坠。
        for name in ["Crate.A", "Crate.B", "Crate.C"] {
            let y = entity_y(&world, name);
            assert!(
                (-1.0..6.0).contains(&y),
                "{name} 应当已经落在地面附近，实际 y = {y}"
            );
        }

        // Spinner 挂了 Spinner.lua，3 秒之后它的 Y 轴欧拉角应当明显转过。
        let spinner = world
            .scene()
            .find_by_name("Spinner")
            .expect("Spinner 应当还在");
        let (yaw, _, _) = spinner
            .transform
            .rotation
            .to_euler(xgame_math::glam::EulerRot::YXZ);
        assert!(
            yaw.to_degrees().abs() > 30.0,
            "Spinner 应当在自转，实际 yaw = {}",
            yaw.to_degrees()
        );
    }

    /// 示例项目在仓库里的位置（相对本 crate）。
    fn repository_sample_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../projects/SampleScene")
    }

    /// 每帧都会 `sync_bodies`，必须确认它**不会**重复建刚体。
    #[test]
    fn bodies_are_not_duplicated_across_frames() {
        let mut scene = Scene::new("dup");
        let mut cube = Entity::new("cube");
        cube.physics = Some(PhysicsBody::dynamic_box([0.5; 3]));
        scene.add_entity(cube);

        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default());
        for _ in 0..30 {
            world.update(1.0 / 60.0);
        }
        let report = world.last_report();
        assert_eq!(report.bodies, 1, "30 帧之后也应当只有 1 个刚体");
        assert_eq!(report.colliders, 1);
    }

    // -----------------------------------------------------------------------
    // 组件轨（Rust Lifecycle）—— 双轨计划 P0
    // -----------------------------------------------------------------------

    use crate::{BehaviorRegistry, Ctx, Lifecycle};
    use std::sync::atomic::AtomicU32;
    use xgame_core::{FieldDesc, FieldKind, FieldValue, Inspect};

    /// 一个走起来像脚本的组件：start 抬高、update 每帧沿 x 漂移。
    #[derive(Default)]
    struct Drifter;

    impl Lifecycle for Drifter {
        fn start(&mut self, ctx: &mut Ctx) {
            let p = ctx.pos();
            ctx.set_pos(Vec3::new(p.x, 5.0, p.z));
        }
        fn update(&mut self, ctx: &mut Ctx, dt: f32) {
            let p = ctx.pos();
            ctx.set_pos(Vec3::new(p.x + dt, p.y, p.z));
        }
    }
    xgame_core::impl_no_inspect!(Drifter);

    /// 组件真的能驱动实体（Rust 轨道的最小闭环）。
    ///
    /// 旧实现（没有组件路径）下 `actor` 纹丝不动，断言必红。
    #[test]
    fn behavior_hooks_drive_entities_like_scripts() {
        let mut scene = Scene::new("behavior");
        let mut actor = Entity::new("actor");
        actor.behaviors.push("Drifter".to_owned());
        scene.add_entity(actor);

        let mut registry = BehaviorRegistry::new();
        registry.register::<Drifter>("Drifter");
        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default())
            .with_behavior_registry(registry);

        for _ in 0..10 {
            world.update(1.0 / 60.0);
        }
        let actor = world.scene().find_by_name("actor").expect("实体应当还在");
        // start 把 y 抬到 5 且只跑一次（之后 10 帧 update 不动 y）。
        assert!((actor.transform.position.y - 5.0).abs() < 1e-6, "start 应当生效且只生效一次");
        // update 每帧 +dt：10 帧 ≈ 0.167。
        assert!(
            actor.transform.position.x > 0.1,
            "update 应当把实体沿 x 推走，实际 x = {}",
            actor.transform.position.x
        );
    }

    static DESTROYED: AtomicU32 = AtomicU32::new(0);

    #[derive(Default)]
    struct Marker;

    impl Lifecycle for Marker {
        fn on_destroy(&mut self, _ctx: &mut Ctx) {
            DESTROYED.fetch_add(1, Ordering::Relaxed);
        }
    }
    xgame_core::impl_no_inspect!(Marker);

    /// 一个会销毁别人的组件（顺带验证"组件钩子里的结构改动走命令消费路径"）。
    #[derive(Default)]
    struct Bomber {
        victim: Option<xgame_core::EntityId>,
    }

    impl Lifecycle for Bomber {
        fn start(&mut self, ctx: &mut Ctx) {
            self.victim = ctx.find_entity_id("victim");
        }
        fn update(&mut self, ctx: &mut Ctx, _dt: f32) {
            if let Some(id) = self.victim.take() {
                ctx.destroy(id);
            }
        }
    }
    xgame_core::impl_no_inspect!(Bomber);

    /// 一个带可检视字段的组件。
    ///
    /// `Inspect` 是**手写**的 —— `#[derive(Inspect)]` 生成的代码引用
    /// `::xgame`，本 crate 解析不到（见 xgame-derive 的"路径假设"）。
    /// derive 生成的正是下面这三段，它的正确性由
    /// `crates/xgame/tests/inspect_derive.rs` 单独覆盖。
    #[derive(Default)]
    struct Tunable {
        speed: f32,
        label: String,
    }

    impl Lifecycle for Tunable {}

    impl Inspect for Tunable {
        fn inspect_fields(&self) -> &'static [FieldDesc] {
            static FIELDS: &[FieldDesc] = &[
                FieldDesc {
                    name: "speed",
                    kind: FieldKind::F32,
                    label: None,
                    min: Some(0.0),
                    max: Some(100.0),
                    speed: None,
                    step: None,
                    unit: "m/s",
                    tooltip: "移动速度",
                },
                FieldDesc::new("label", FieldKind::String),
            ];
            FIELDS
        }
        fn inspect_get(&self, index: usize) -> FieldValue {
            match index {
                0 => FieldValue::F32(self.speed),
                1 => FieldValue::String(self.label.clone()),
                _ => FieldKind::F32.default_value(),
            }
        }
        fn inspect_set(&mut self, index: usize, value: FieldValue) {
            match (index, value) {
                (0, FieldValue::F32(v)) => self.speed = v,
                (1, FieldValue::String(v)) => self.label = v,
                _ => {}
            }
        }
    }

    /// 编辑器在 Play 中读写组件字段的整条通路（P1）。
    ///
    /// 守三件事：装配后能按下标取到组件、写进去的值真的落到组件本体
    /// （不是编辑器侧的影子值）、越界与未装配实体安静返回而不是 panic。
    #[test]
    fn editor_can_read_and_write_component_fields() {
        let mut scene = Scene::new("inspect");
        let mut a = Entity::new("a");
        a.behaviors.push("Tunable".to_owned());
        scene.add_entity(a);
        let bare = Entity::new("bare");
        scene.add_entity(bare);

        let mut registry = BehaviorRegistry::new();
        registry.register::<Tunable>("Tunable");

        // 字段表在注册期就缓存好了 —— 编辑器 Edit 模式（还没有实例）零分配画字段的依据。
        let table = registry.field_table("Tunable").expect("字段表应当已缓存");
        assert_eq!(table.len(), 2);
        assert_eq!(table[0].name, "speed");
        assert_eq!(table[0].unit, "m/s");
        assert!(table[0].has_range(), "min + max 齐全 ⇒ 编辑器画滑块");
        assert_eq!(table[1].name, "label");
        assert!(registry.field_table("Nope").is_none());

        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default())
            .with_behavior_registry(registry);

        let id = world.scene().find_by_name("a").expect("a 应当在").id;
        let bare_id = world.scene().find_by_name("bare").expect("bare 应当在").id;
        assert_eq!(world.behavior_count(id), 1, "应当装配出一个组件");
        assert_eq!(
            world.behavior_count(bare_id),
            0,
            "没声明 behaviors 的实体不该有组件"
        );

        // 读默认值 → 改 → 写回
        let (name, comp) = world.behavior_at_mut(id, 0).expect("第 0 个组件存在");
        assert_eq!(name, "Tunable");
        assert_eq!(comp.inspect_get(0), FieldValue::F32(0.0), "默认构造 ⇒ speed = 0");
        comp.inspect_set(0, FieldValue::F32(42.0));
        comp.inspect_set(1, FieldValue::String("hello".into()));
        // 类型不匹配的写入必须被忽略（Lua 侧传错类型的常见后果）。
        comp.inspect_set(0, FieldValue::Bool(true));

        // 再读一次，确认值真的进了组件本体。
        let (_, comp) = world.behavior_at_mut(id, 0).expect("同一实体再取一次");
        assert_eq!(comp.inspect_get(0), FieldValue::F32(42.0));
        assert_eq!(comp.inspect_get(1), FieldValue::String("hello".into()));

        // 越界：安静返回 None —— Inspector 不该因为一次下标错误崩掉。
        assert!(world.behavior_at_mut(id, 7).is_none());
        assert!(world.behavior_at_mut(bare_id, 0).is_none());
    }

    /// P1.5：场景里存的组件字段值必须在**装配期**套到实例上。
    ///
    /// 这是打包玩家拿到手感参数的唯一通路（玩家没有 `edit_components`，
    /// 没有 `seed_play_from_edit_components` —— 只有 `World` 自己装配这一条）。
    /// 旧实现（装配只构造 `Default` 实例）下 speed 读回 0，断言必红。
    #[test]
    fn behavior_params_from_scene_are_applied_at_assembly() {
        use xgame_core::inspect::{apply_field_values, FieldValue};

        let mut scene = Scene::new("params");
        let mut tuned = Entity::new("tuned");
        tuned.behaviors.push("Tunable".to_owned());
        let mut fields = xgame_core::inspect::FieldValues::new();
        fields.insert("speed".to_owned(), FieldValue::F32(42.0));
        fields.insert("label".to_owned(), FieldValue::String("存盘值".into()));
        tuned.behavior_params.insert("Tunable".to_owned(), fields);
        scene.add_entity(tuned);
        // 没存参数的实体：构造默认值原样保留（缺字段 = 不动）。
        scene.add_entity(Entity::new("plain_with_behavior_b"));
        scene.entities[1].behaviors.push("Tunable".to_owned());

        let mut registry = BehaviorRegistry::new();
        registry.register::<Tunable>("Tunable");
        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default())
            .with_behavior_registry(registry);

        let id = world.scene().find_by_name("tuned").expect("tuned 应当在").id;
        let (_, comp) = world.behavior_at_mut(id, 0).expect("组件已装配");
        assert_eq!(
            comp.inspect_get(0),
            FieldValue::F32(42.0),
            "存盘的 speed 必须在装配时套上"
        );
        assert_eq!(
            comp.inspect_get(1),
            FieldValue::String("存盘值".into()),
            "存盘的 label 必须在装配时套上"
        );

        let plain_id = world.scene().find_by_name("plain_with_behavior_b").expect("在").id;
        let (_, comp) = world.behavior_at_mut(plain_id, 0).expect("组件已装配");
        assert_eq!(comp.inspect_get(0), FieldValue::F32(0.0), "没存参数 ⇒ 保留默认");
        assert_eq!(comp.inspect_get(1), FieldValue::String(String::new()));

        // 错位防线：存盘里出现组件没有的字段 / 类型不符 —— 静默吸收，
        // 不 panic、不把别的字段带歪（与 `Inspect::inspect_set` 的哲学一致）。
        let mut stale = xgame_core::inspect::FieldValues::new();
        stale.insert("deleted".to_owned(), FieldValue::F32(1.0));
        stale.insert("speed".to_owned(), FieldValue::Bool(true));
        let mut c = Tunable::default();
        apply_field_values(&mut c, &stale);
        assert_eq!(c.speed, 0.0);
    }

    /// 实体被销毁时其组件收到 `on_destroy`。
    ///
    /// 旧实现（钩子没接线）下计数停在 0，断言必红。
    #[test]
    fn destroying_an_entity_runs_on_destroy() {
        DESTROYED.store(0, Ordering::Relaxed);

        let mut scene = Scene::new("destroy");
        let mut bomber = Entity::new("bomber");
        bomber.behaviors.push("Bomber".to_owned());
        scene.add_entity(bomber);
        let mut victim = Entity::new("victim");
        victim.behaviors.push("Marker".to_owned());
        scene.add_entity(victim);

        let mut registry = BehaviorRegistry::new();
        registry.register::<Bomber>("Bomber");
        registry.register::<Marker>("Marker");
        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default())
            .with_behavior_registry(registry);

        // bomber 在第一帧的 update 里 destroy(victim)；命令在同帧被消费。
        world.update(1.0 / 60.0);

        assert_eq!(DESTROYED.load(Ordering::Relaxed), 1, "victim 的 on_destroy 应当跑一次");
        assert!(world.scene().find_by_name("victim").is_none(), "victim 应当已离开场景");
        // 第二帧不该再收到任何回调（实例已清）。
        world.update(1.0 / 60.0);
        assert_eq!(DESTROYED.load(Ordering::Relaxed), 1, "on_destroy 不该重复派发");
    }

    /// 未注册的行为名：记日志跳过，不 panic、不影响其它实体。
    #[test]
    fn unregistered_behaviors_are_skipped_not_fatal() {
        let mut scene = Scene::new("unregistered");
        let mut actor = Entity::new("actor");
        actor.behaviors.push("Ghost".to_owned());
        scene.add_entity(actor);
        let mut drifter = Entity::new("drifter");
        drifter.behaviors.push("Drifter".to_owned());
        scene.add_entity(drifter);

        let mut registry = BehaviorRegistry::new();
        registry.register::<Drifter>("Drifter"); // 故意不注册 Ghost
        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default())
            .with_behavior_registry(registry);

        for _ in 0..5 {
            world.update(1.0 / 60.0);
        }
        // 已注册的照常工作。
        let drifter = world.scene().find_by_name("drifter").unwrap();
        assert!(drifter.transform.position.x > 0.0);
        // 未注册的实体只是没有组件，场景安然无恙。
        assert!(world.scene().find_by_name("actor").is_some());
    }

    // -- 音频 ---------------------------------------------------------------

    /// 生成一个 16-bit mono PCM WAV（440 Hz，5 秒）。
    ///
    /// 时长刻意取 5 秒而不是几十毫秒：无声引擎按**墙钟**做 GC，太短的
    /// WAV 会在测试跑到一半时被当成"播完了"，断言 active/stopped 就会飘。
    fn write_test_wav(dir: &std::path::Path, name: &str) {
        const SAMPLE_RATE: u32 = 44100;
        let samples: u32 = SAMPLE_RATE * 5;
        let data_len = samples * 2;
        let mut bytes = Vec::with_capacity(44 + data_len as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        bytes.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..samples {
            let t = i as f32 / SAMPLE_RATE as f32;
            let sample = (0.5 * (2.0 * std::f32::consts::PI * 440.0 * t).sin() * i16::MAX as f32) as i16;
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(dir.join(name), bytes).unwrap();
    }

    /// autoplay 只播一次：三次 update 之后 `requests` 仍是 1。
    ///
    /// 证红点：没有 `audio_autoplayed` 标记表的实现会每帧重播（requests=3）。
    #[test]
    fn autoplay_sound_requests_play_exactly_once() {
        let mut scene = Scene::new("autoplay");
        scene.add_entity(Entity::new("beep").with_audio(AudioSource::new("Beep.wav")));
        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default());

        for _ in 0..3 {
            world.update(1.0 / 60.0);
        }
        let stats = world.audio_stats();
        assert_eq!(stats.requests, 1, "autoplay 只该在首帧请求一次");
        assert_eq!(stats.stopped, 0);
    }

    /// 真 WAV 走完整链：场景组件 → 命令束 → 解码缓存 → 活动声音。
    #[test]
    fn real_wav_autoplay_becomes_active() {
        let dir = std::env::temp_dir().join(format!(
            "xgame_runtime_audio_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        write_test_wav(&dir, "Beep.wav");

        let mut scene = Scene::new("wav");
        scene.add_entity(
            Entity::new("beep")
                .with_audio(AudioSource::new("Beep.wav").with_looping(true)),
        );
        let mut world = World::new(scene, &dir, WorldConfig::default());
        world.update(1.0 / 60.0);

        let stats = world.audio_stats();
        assert_eq!(stats.requests, 1);
        assert_eq!(stats.cached_clips, 1, "clip 应被解码并缓存");
        assert_eq!(stats.active, 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 空间声源：首帧 Play（带位置）、之后每帧 EmitterPosition（跟随实体）。
    #[test]
    fn spatial_source_pushes_emitter_positions_each_frame() {
        let mut scene = Scene::new("spatial");
        scene.add_entity(
            Entity::new("engine")
                .with_audio(AudioSource::new("Engine.wav").with_spatial(true)),
        );
        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default());

        world.update(1.0 / 60.0);
        assert_eq!(world.audio_stats().requests, 1, "首帧应发 Play");

        // 第二帧没有新请求，但空间声源要推位置 → last_batch 应为 1
        //（一条 EmitterPosition）。
        world.update(1.0 / 60.0);
        let stats = world.audio_stats();
        assert_eq!(stats.requests, 1, "不该重播");
        assert_eq!(stats.last_batch, 1, "每帧应推一次发射位置");
    }

    /// 快速路径的重扫时机：实体数变化必须重新武装扫描。
    /// 场景先没音频（首扫后进快速路径），运行中新增带音频源的实体
    /// （模拟脚本 spawn 的结构性变化）→ 下一帧必须被扫到并 autoplay。
    ///
    /// 反面刻意**不**覆盖：给已有实体在运行中贴 `audio` 组件而不动实体数
    /// —— 快速路径不会重扫，autoplay 不触发。动态发声的正路是脚本
    /// `play_sound`（不经扫描），组件走场景配置。
    #[test]
    fn audio_rescan_picks_up_new_entities() {
        let mut scene = Scene::new("rescan");
        scene.add_entity(Entity::new("ground"));
        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default());

        // 首帧：场景无音频源 → 快速路径。等一帧让首扫落定。
        world.update(1.0 / 60.0);
        assert_eq!(world.audio_stats().requests, 0);

        // 运行中新增带音频源的实体（实体数 1 → 2）。
        world.scene_mut().entities.push(
            Entity::new("beep").with_audio(AudioSource::new("Beep.wav")),
        );
        world.update(1.0 / 60.0);
        assert_eq!(
            world.audio_stats().requests,
            1,
            "实体数变化必须触发重扫，autoplay 才不会被快速路径吞掉"
        );
    }

    /// 实体被删除 / 组件被摘除：停音 + 清 autoplay 标记（重挂会再响）。
    #[test]
    fn removed_entity_or_component_stops_its_sound() {
        let dir = std::env::temp_dir().join(format!(
            "xgame_runtime_audio_cleanup_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        write_test_wav(&dir, "Beep.wav");
        write_test_wav(&dir, "Hum.wav");

        let mut scene = Scene::new("cleanup");
        scene.add_entity(Entity::new("beep").with_audio(AudioSource::new("Beep.wav")));
        scene.add_entity(Entity::new("hum").with_audio(AudioSource::new("Hum.wav")));
        let mut world = World::new(scene, &dir, WorldConfig::default());
        world.update(1.0 / 60.0);
        assert_eq!(world.audio_stats().active, 2);

        // 摘组件（保留实体）→ 只停它那一条。
        world.scene_mut().entities[1].audio = None;
        world.update(1.0 / 60.0);
        let stats = world.audio_stats();
        assert_eq!(stats.active, 1, "摘掉组件应停对应声音");
        assert_eq!(stats.stopped, 1);

        // 删实体 → 停剩下那条。
        world.scene_mut().entities.clear();
        world.update(1.0 / 60.0);
        let stats = world.audio_stats();
        assert_eq!(stats.active, 0);
        assert_eq!(stats.stopped, 2);

        // 重新挂上 → 再响一次（标记已清）。
        world
            .scene_mut()
            .entities
            .push(Entity::new("beep2").with_audio(AudioSource::new("Beep.wav")));
        world.update(1.0 / 60.0);
        assert_eq!(world.audio_stats().requests, 3, "重挂应重新 autoplay");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 脚本 → World → 音频引擎的端到端：Lua 的 `play_sound` 要变成
    /// 引擎的 requests。
    #[test]
    fn script_play_sound_reaches_the_engine() {
        let dir = std::env::temp_dir().join(format!(
            "xgame_runtime_audio_script_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("Scripts")).unwrap();
        std::fs::write(
            dir.join("Scripts/sfx.lua"),
            r#"
            function update(ctx)
                play_sound(ctx, "Beep.wav", 0.8, 1.0)
            end
            "#,
        )
        .unwrap();

        let mut scene = Scene::new("scripted_audio");
        let mut actor = Entity::new("actor");
        actor.scripts.push("Scripts/sfx.lua".to_owned());
        scene.add_entity(actor);

        let mut world = World::new(scene, &dir, WorldConfig::default());
        world.update(1.0 / 60.0);
        assert_eq!(world.audio_stats().requests, 1, "脚本的 play_sound 应到达引擎");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // 场景切换
    // -----------------------------------------------------------------------

    /// 造一个临时项目：`Scenes/A.ron`（2 实体 / 1 刚体，挂 `Scripts/switch.lua`）
    /// + `Scenes/B.ron`（3 实体 / 2 刚体，挂 `Scripts/level2.lua`）。
    ///
    /// 两份场景的实体数、刚体数、脚本**刻意都不一样** —— 切换是否真的发生，
    /// 靠数字就能判定，不必去比内存地址。
    /// `auto_switch` = `Scripts/switch.lua` 是否每帧请求切到 B。
    /// 需要"自己控制切换时机"的测试传 `false`（只保留 start 日志）。
    fn temp_scene_project(tag: &str, auto_switch: bool) -> ProjectHandle {
        let dir = std::env::temp_dir().join(format!(
            "xgame_runtime_scene_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let project = ProjectHandle::create(&dir, "SwitchDemo").expect("建临时项目");
        std::fs::create_dir_all(project.root().join("Scripts")).unwrap();

        // A：地面 + 一个纯视觉塔；脚本在 start 里报场景名。
        // `auto_switch` 时额外加一个"每帧请求切到 B"的 update。
        let switch_body = if auto_switch {
            r#"
            function start(ctx)
                log(ctx, "start:" .. current_scene(ctx))
            end
            function update(ctx)
                load_scene(ctx, "Scenes/B.ron")
            end
            "#
        } else {
            r#"
            function start(ctx)
                log(ctx, "start:" .. current_scene(ctx))
            end
            "#
        };
        std::fs::write(project.root().join("Scripts/switch.lua"), switch_body).unwrap();
        // B：地面 + 下落球 + 一个视觉标记；start 里打一条带场景名的日志。
        std::fs::write(
            project.root().join("Scripts/level2.lua"),
            r#"
            function start(ctx)
                log(ctx, "level2-start:" .. current_scene(ctx))
            end
            "#,
        )
        .unwrap();

        let mut a = Scene::new("A");
        let mut ground = Entity::new("ground");
        ground.physics = Some(PhysicsBody::static_box([10.0, 0.5, 10.0]));
        a.add_entity(ground);
        let mut tower = Entity::new("tower");
        tower.transform.position = Vec3::new(3.0, 2.0, 0.0);
        a.add_entity(tower);
        let mut switcher = Entity::new("switcher");
        switcher.scripts.push("Scripts/switch.lua".to_owned());
        a.add_entity(switcher);
        project.write_scene("Scenes/A.ron", &a).expect("写 A");

        let mut b = Scene::new("B");
        let mut ground2 = Entity::new("ground2");
        ground2.physics = Some(PhysicsBody::static_box([10.0, 0.5, 10.0]));
        b.add_entity(ground2);
        let mut ball = Entity::new("ball");
        ball.physics = Some(PhysicsBody::dynamic_box([0.5, 0.5, 0.5]));
        ball.transform.position = Vec3::new(0.0, 4.0, 0.0);
        b.add_entity(ball);
        let mut marker = Entity::new("marker");
        marker.scripts.push("Scripts/level2.lua".to_owned());
        b.add_entity(marker);
        project.write_scene("Scenes/B.ron", &b).expect("写 B");

        project
    }

    /// 切换场景：实体换成新的、旧刚体全部销毁、新场景的刚体建好。
    #[test]
    fn load_scene_swaps_entities_and_rebuilds_physics() {
        let project = temp_scene_project("swap", true);
        let scene = project.load_scene("Scenes/A.ron").expect("读取 A");
        let mut world = World::new(scene, project.root(), WorldConfig::default())
            .with_scene_loader(project.clone())
            .with_current_scene("Scenes/A.ron");

        assert_eq!(world.scene().entities.len(), 3, "A 有 3 个实体");
        assert_eq!(world.physics().stats().0, 1, "A 只有地面带刚体");
        assert_eq!(world.current_scene(), Some("Scenes/A.ron"));

        world.load_scene("Scenes/B.ron").expect("应当能加载 B");

        assert_eq!(world.scene().entities.len(), 3, "B 有 3 个实体");
        assert!(world.scene().find_by_name("tower").is_none(), "旧场景实体必须消失");
        assert!(world.scene().find_by_name("ball").is_some(), "新场景实体应当就位");
        assert_eq!(
            world.physics().stats().0,
            2,
            "旧刚体必须销毁：B 只有地面 + 球两个"
        );
        assert_eq!(world.current_scene(), Some("Scenes/B.ron"));
        assert_eq!(world.scene_loads(), 1);
    }

    /// 装不上的场景：旧场景原封不动，错误可读（不是"半换过去"）。
    #[test]
    fn failed_scene_load_keeps_the_current_scene() {
        let project = temp_scene_project("missing", true);
        let scene = project.load_scene("Scenes/A.ron").expect("读取 A");
        let mut world = World::new(scene, project.root(), WorldConfig::default())
            .with_scene_loader(project.clone())
            .with_current_scene("Scenes/A.ron");

        let err = world.load_scene("Scenes/Nope.ron").expect_err("不存在的场景应当报错");
        assert!(err.contains("Nope.ron"), "错误里要带路径，实际：{err}");

        assert_eq!(world.scene().entities.len(), 3, "旧场景必须原封不动");
        assert!(world.scene().find_by_name("tower").is_some());
        assert_eq!(world.current_scene(), Some("Scenes/A.ron"));
        assert_eq!(world.scene_loads(), 0, "失败的装载不算切换");
    }

    /// 没接装载器的运行时：脚本的 `load_scene` 必须留下一条**可诊断**的错误，
    /// 而不是静默无事发生（否则作者会以为脚本没写对）。
    #[test]
    fn scene_load_without_loader_is_reported() {
        let mut scene = Scene::new("no_loader");
        scene.add_entity(Entity::new("only"));
        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default())
            .with_current_scene("Scenes/A.ron");

        world.request_scene_load("Scenes/B.ron");
        world.update(1.0 / 60.0);

        let logs = world.take_logs();
        let hit = logs
            .iter()
            .find(|log| log.level == crate::LogLevel::Error)
            .expect("应当有一条错误日志");
        assert!(
            hit.message.contains("场景装载器"),
            "错误文案要指出缺哪一环，实际：{}",
            hit.message
        );
        assert_eq!(world.scene().entities.len(), 1, "旧场景照常跑");
        assert_eq!(world.scene_loads(), 0);
    }

    /// 脚本 `load_scene` 的端到端：帧末生效、新场景的 `start` 跑一次、
    /// `current_scene()` 报的是**新**路径。
    #[test]
    fn script_can_switch_scene_and_new_scene_starts_fresh() {
        let project = temp_scene_project("scripted", true);
        let scene = project.load_scene("Scenes/A.ron").expect("读取 A");
        let mut world = World::new(scene, project.root(), WorldConfig::default())
            .with_scene_loader(project.clone())
            .with_current_scene("Scenes/A.ron");

        let mut logs = Vec::new();
        // 第一帧：A 的 start + update 里登记切换请求；切换在**帧末**生效。
        world.update(1.0 / 60.0);
        logs.extend(world.take_logs());
        assert_eq!(world.scene_loads(), 1, "请求应当在帧末被应用");
        assert_eq!(world.current_scene(), Some("Scenes/B.ron"));

        // 第二帧：B 的 start 才跑（start 在下一帧派发，与生成的实体同一时机）。
        world.update(1.0 / 60.0);
        logs.extend(world.take_logs());

        let messages: Vec<&str> = logs.iter().map(|log| log.message.as_str()).collect();
        assert!(
            messages.contains(&"start:Scenes/A.ron"),
            "A 的 start 应当看到 A 的路径，实际 {messages:?}"
        );
        assert!(
            messages.contains(&"level2-start:Scenes/B.ron"),
            "B 的 start 应当看到 B 的路径，实际 {messages:?}"
        );
        // 切换只发生一次：`switch.lua` 只挂在 A 上，B 里没有它。
        assert_eq!(world.scene_loads(), 1, "不该反复重载");
    }

    /// 来回切换时，运行时的"旧痕迹"必须清干净：
    /// 粒子清空、`start` 重新跑（`started` 标记与每实体状态都按实体 id 存）。
    #[test]
    fn scene_switch_clears_particles_and_script_state() {
        let project = temp_scene_project("reset", false);
        let scene = project.load_scene("Scenes/A.ron").expect("读取 A");
        let mut world = World::new(scene, project.root(), WorldConfig::default())
            .with_scene_loader(project.clone())
            .with_current_scene("Scenes/A.ron");

        // 给 A 加一个发射器（走场景 API 而不是改文件：测试只关心运行时行为）。
        let mut emitter = Entity::new("flame");
        emitter.particle = Some(xgame_core::ParticleEmitter::default());
        world.scene_mut().add_entity(emitter);
        for _ in 0..5 {
            world.update(1.0 / 60.0);
        }
        assert!(
            world.particle_stats().alive > 0,
            "5 帧之后发射器应当已经出粒子"
        );
        // 这一帧的日志里有 A 的 start；之后切走再切回来，start 要**再来一次**。
        let first: Vec<String> = world.take_logs().into_iter().map(|l| l.message).collect();
        assert!(first.iter().any(|m| m == "start:Scenes/A.ron"));

        world.load_scene("Scenes/B.ron").expect("切到 B");
        assert_eq!(world.particle_stats().alive, 0, "旧场景的粒子必须清空");

        world.load_scene("Scenes/A.ron").expect("切回 A");
        world.update(1.0 / 60.0);
        let second: Vec<String> = world.take_logs().into_iter().map(|l| l.message).collect();
        assert!(
            second.iter().any(|m| m == "start:Scenes/A.ron"),
            "切回后 start 必须重新跑（started 标记按实体 id 存，不清就会永远静默），实际 {second:?}"
        );
        assert_eq!(world.scene_loads(), 2);
    }

    // -- 关卡顺序（build.scenes）--------------------------------------------

    /// 顺序即索引：脚本用 `scene_index()` 取当前关、`load_scene_at(i + 1)` 进下一关 ——
    /// 这是"关卡编排变成数据"的最小闭环，作者不用把下一关的路径写死在脚本里。
    #[test]
    fn script_advances_through_build_scene_order() {
        let project = temp_scene_project("order", false);
        // 覆盖 A 的脚本：只在第 0 关往下推一关，到了第 1 关就停。
        std::fs::write(
            project.root().join("Scripts/switch.lua"),
            r#"
            function update(ctx)
                log(ctx, "idx:" .. tostring(scene_index(ctx)) .. "/" .. scene_count(ctx))
                if scene_index(ctx) == 0 then
                    load_scene_at(ctx, 1)
                end
            end
            "#,
        )
        .unwrap();

        let order = vec!["Scenes/A.ron".to_string(), "Scenes/B.ron".to_string()];
        let scene = project.load_scene("Scenes/A.ron").expect("读取 A");
        let mut world = World::new(scene, project.root(), WorldConfig::default())
            .with_scene_loader(project.clone())
            .with_current_scene("Scenes/A.ron")
            .with_scene_order(order);

        assert_eq!(world.scene_index(), Some(0), "起始场景是第 0 关");

        world.update(1.0 / 60.0);
        let logs: Vec<String> = world.take_logs().into_iter().map(|l| l.message).collect();
        assert_eq!(world.current_scene(), Some("Scenes/B.ron"), "帧末应当已经切过去");
        assert_eq!(world.scene_index(), Some(1));
        assert_eq!(world.scene_loads(), 1);
        assert!(
            logs.iter().any(|m| m == "idx:0/2"),
            "脚本应当看到 0/2，实际 {logs:?}"
        );

        // 第二帧：B 里没有 switch.lua，不再请求切换 —— 顺序本身不该"自动推进"。
        world.update(1.0 / 60.0);
        assert_eq!(world.scene_loads(), 1, "顺序只是数据，推进由脚本决定");
    }

    /// 越界不静默：字节码没错、但关卡算错是常见 bug，必须留一条 Error。
    /// 而且要**原地不动** —— 关卡不存在时把玩家丢进空场景比重启还糟。
    #[test]
    fn load_scene_at_out_of_range_is_an_error() {
        let project = temp_scene_project("order_bad", false);
        std::fs::write(
            project.root().join("Scripts/switch.lua"),
            r#"
            function update(ctx)
                load_scene_at(ctx, 9)
            end
            "#,
        )
        .unwrap();

        let scene = project.load_scene("Scenes/A.ron").expect("读取 A");
        let mut world = World::new(scene, project.root(), WorldConfig::default())
            .with_scene_loader(project.clone())
            .with_current_scene("Scenes/A.ron")
            .with_scene_order(vec!["Scenes/A.ron".to_string()]);

        world.update(1.0 / 60.0);
        let logs = world.take_logs();
        let hit = logs
            .iter()
            .find(|log| log.level == crate::LogLevel::Error)
            .expect("越界应当有一条错误日志");
        assert!(hit.message.contains("越界"), "实际：{}", hit.message);
        assert_eq!(world.current_scene(), Some("Scenes/A.ron"), "场景必须原地不动");
        assert_eq!(world.scene_loads(), 0);
    }

    // -- 预制体实例化（spawn_prefab）----------------------------------------

    /// 造一个带预制体的临时项目：`Prefabs/Enemy.prefab.ron` 里是一棵
    /// Root(动态球) -> Child(视觉方块)，A 场景放一个会生成它的脚本。
    fn temp_prefab_project(tag: &str) -> ProjectHandle {
        let project = temp_scene_project(tag, false);
        // A 里只有一个"每帧生成一个敌人"的脚本实体。
        std::fs::write(
            project.root().join("Scripts/switch.lua"),
            r#"
            function update(ctx)
                spawn_prefab(ctx, "Prefabs/Enemy.prefab.ron", 0.0, 2.0, 0.0)
            end
            "#,
        )
        .unwrap();

        // 预制体资产：自己造一份写盘（与编辑器 Prefab 面板走同一条写入口）。
        let mut template = Scene::new("EnemyTemplate");
        let root = template.add_entity(Entity::new("Enemy"));
        let root_index = template.index_of(root).unwrap();
        template.entities[root_index].physics = Some(PhysicsBody::dynamic_ball(0.5));
        let child = template.add_entity(
            Entity::new("Gun").with_transform(Transform::from_translation(Vec3::new(0.0, 1.0, 0.0))),
        );
        let child_index = template.index_of(child).unwrap();
        template.set_parent(child_index, Some(root_index));
        project
            .write_prefab(
                "Prefabs/Enemy.prefab.ron",
                &Prefab::from_subtree(&template, root_index),
            )
            .expect("写预制体");
        project
    }

    /// 端到端：脚本生成预制体 → 整棵子树进场景 → 运行时补上刚体 → 世界位置正确。
    ///
    /// 关键在刚体数：`EntitySpawned` 命令是**每个新实体**一条，漏一条就少一
    /// 个刚体（表现是"生成的敌人不参与碰撞"），所以拿它当判据。
    #[test]
    fn spawn_prefab_instantiates_subtree_and_builds_bodies() {
        let project = temp_prefab_project("spawn_prefab");
        let scene = project.load_scene("Scenes/A.ron").expect("读取 A");
        let (base_entities, base_bodies) = {
            let world = World::new(scene, project.root(), WorldConfig::default())
                .with_scene_loader(project.clone())
                .with_current_scene("Scenes/A.ron");
            (world.scene().entities.len(), world.physics().stats().0)
        };
        assert_eq!(base_entities, 3, "A：地面 + 塔 + 脚本实体");
        assert_eq!(base_bodies, 1, "A 只有地面带刚体");

        let scene = project.load_scene("Scenes/A.ron").expect("再读 A");
        let mut world = World::new(scene, project.root(), WorldConfig::default())
            .with_scene_loader(project.clone())
            .with_current_scene("Scenes/A.ron");

        world.update(1.0 / 60.0);

        assert_eq!(
            world.scene().entities.len(),
            base_entities + 2,
            "Root + Child 都该进场景"
        );
        let bodies = world.physics().stats().0;
        assert_eq!(bodies, base_bodies + 1, "只有根带刚体，但必须真的补上");
        // 世界位置 = 预制体根位置(0,2,0) + 子节点局部(0,1,0)
        let root = world
            .scene()
            .entities
            .iter()
            .find(|e| e.name == "Enemy")
            .expect("实例根应当在场景里");
        assert_eq!(root.transform.position, Vec3::new(0.0, 2.0, 0.0));
        let gun = world
            .scene()
            .entities
            .iter()
            .find(|e| e.name == "Gun")
            .expect("子节点应当在场景里");
        let gun_index = world.scene().index_of(gun.id).unwrap();
        let world_pos = world.scene().world_matrix(gun_index).w_axis.truncate();
        assert!(
            (world_pos - Vec3::new(0.0, 3.0, 0.0)).length() < 1e-4,
            "子节点跟着根走，实际 {world_pos:?}"
        );
        // 生成是当帧的，但**没有报错**（失败路径的错误日志会在这里露出来）。
        let logs = world.take_logs();
        assert!(
            logs.iter().all(|l| l.level != crate::LogLevel::Error),
            "成功路径不该有错误日志：{logs:?}"
        );
    }

    /// 没接装载器时：`spawn_prefab` 记一条 Error 且什么都不生成
    /// （不静默、不 panic —— 半通不通的宿主最容易在这里卡住）。
    #[test]
    fn spawn_prefab_without_loader_reports_and_does_nothing() {
        let project = temp_prefab_project("spawn_prefab_noloader");
        let scene = project.load_scene("Scenes/A.ron").expect("读取 A");
        let mut world = World::new(scene, project.root(), WorldConfig::default())
            .with_current_scene("Scenes/A.ron");
        let before = world.scene().entities.len();

        world.update(1.0 / 60.0);

        assert_eq!(world.scene().entities.len(), before, "什么都不该生成");
        let logs = world.take_logs();
        assert!(
            logs.iter()
                .any(|l| l.level == crate::LogLevel::Error && l.message.contains("装载器")),
            "缺装载器必须报错而不是静默：{logs:?}"
        );
    }

    /// 没配清单（`scene_order` 为空）：`scene_index()` 是 `None`，不是 0。
    /// 返回 0 会让"第 0 关"和"没配清单"混成同一个值，脚本无法区分。
    #[test]
    fn scene_index_is_none_without_order() {
        let mut scene = Scene::new("solo");
        scene.add_entity(Entity::new("only"));
        let world = World::new(scene, std::env::temp_dir(), WorldConfig::default())
            .with_current_scene("Scenes/Only.ron");

        assert_eq!(world.scene_index(), None);
        assert!(world.scene_order().is_empty());
    }

    /// 宿主 / 测试入口也能按序号切（不进脚本），且失败时不换场景。
    #[test]
    fn world_load_scene_at_uses_order() {
        let project = temp_scene_project("order_host", false);
        let scene = project.load_scene("Scenes/A.ron").expect("读取 A");
        let mut world = World::new(scene, project.root(), WorldConfig::default())
            .with_scene_loader(project.clone())
            .with_current_scene("Scenes/A.ron")
            .with_scene_order(vec!["Scenes/A.ron".to_string(), "Scenes/B.ron".to_string()]);

        world.load_scene_at(1).expect("第 1 关应当能装载");
        assert_eq!(world.current_scene(), Some("Scenes/B.ron"));

        let err = world.load_scene_at(2).expect_err("越界必须报错");
        assert!(err.contains("越界"), "实际：{err}");
        assert_eq!(world.current_scene(), Some("Scenes/B.ron"), "失败的切换不动场景");
    }

    /// `set_time_scale(ctx, 0)` = 暂停模拟。四件事一起守，少一件就说明缩放
    /// 没有真正进入帧管线：
    ///
    /// 1. 动态体停在半空（物理固定步没跑）；
    /// 2. `frame_dt()` 归零（脚本 `delta` / 粒子 / 宿主动画钟同源）；
    /// 3. 暂停期间 `update` 仍**每帧执行**（暂停菜单要靠它响应点击 ——
    ///    这是与"编辑器 Pause 干脆不推进 World"最关键的区别）；
    /// 4. `fixed_update` **一次都不派发** —— 累计器不涨就没有固定步，
    ///    不是"派发了但 delta 为 0"。文档里写的就是这条，用它钉住。
    #[test]
    fn script_pause_freezes_physics_but_keeps_update_running() {
        let dir = std::env::temp_dir().join(format!(
            "xgame_runtime_pause_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("Scripts")).unwrap();
        std::fs::write(
            dir.join("Scripts/pause.lua"),
            r#"
            local frames = 0
            local fixed = 0
            function fixed_update(ctx)
                fixed = fixed + 1
            end
            function update(ctx)
                frames = frames + 1
                if frames == 5 then
                    set_time_scale(ctx, 0)
                end
                if time_scale(ctx) == 0 and frames % 10 == 0 then
                    log(ctx, "paused " .. tostring(frames) .. " fixed " .. tostring(fixed))
                end
            end
            "#,
        )
        .unwrap();

        let mut scene = Scene::new("pause");
        let mut cube = Entity::new("cube");
        cube.physics = Some(PhysicsBody::dynamic_box([0.5, 0.5, 0.5]));
        // 摆得足够高：真跑起来一定会明显下落，停住才可分辨。
        cube.transform.position = Vec3::new(0.0, 50.0, 0.0);
        cube.scripts.push("Scripts/pause.lua".to_owned());
        scene.add_entity(cube);

        let mut world = World::new(scene, &dir, WorldConfig::default());

        for _ in 0..5 {
            world.update(1.0 / 60.0);
        }
        assert_eq!(
            world.time_scale(),
            0.0,
            "脚本写的时间缩放必须落在运行时那一份时钟上"
        );

        // 第 5 帧的固定步在脚本之前就跑完了，所以从**下一帧**起才真正冻住。
        world.update(1.0 / 60.0);
        let paused_y = entity_y(&world, "cube");
        let paused_time = world.game_time();
        assert_eq!(world.frame_dt(), 0.0, "暂停后帧 dt 必须是 0");
        assert_eq!(world.last_report().fixed_steps, 0, "暂停后不该再跑固定步");

        for _ in 0..30 {
            let report = world.update(1.0 / 60.0);
            assert_eq!(report.fixed_steps, 0, "暂停期间每帧都不该有固定步");
        }

        assert_eq!(entity_y(&world, "cube"), paused_y, "暂停期间物理体不能动");
        assert_eq!(world.game_time(), paused_time, "暂停期间 time 不能增长");

        let paused_ticks = world
            .take_logs()
            .iter()
            .filter(|l| l.message.starts_with("paused "))
            .count();
        assert!(
            paused_ticks >= 3,
            "暂停期间 update 仍应每帧执行（暂停菜单靠它），实际 {paused_ticks} 次"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 暂停期间 `fixed_update` **一次都不派发**（不是"派发了但 delta 为 0"）——
    /// 脚本 API 文档里写的就是这条，所以单独钉一遍。
    #[test]
    fn paused_fixed_update_is_never_dispatched() {
        let dir = std::env::temp_dir().join(format!(
            "xgame_runtime_pause_fixed_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("Scripts")).unwrap();
        // 每帧把 fixed_update 的累计次数打出来：暂停后这个数必须冻住。
        std::fs::write(
            dir.join("Scripts/count.lua"),
            r#"
            local frames = 0
            local fixed = 0
            function fixed_update(ctx)
                fixed = fixed + 1
            end
            function update(ctx)
                frames = frames + 1
                if frames == 6 then
                    set_time_scale(ctx, 0)
                end
                log(ctx, "tick " .. tostring(frames) .. "|" .. tostring(fixed))
            end
            "#,
        )
        .unwrap();

        let mut scene = Scene::new("count");
        let mut probe = Entity::new("probe");
        probe.scripts.push("Scripts/count.lua".to_owned());
        scene.add_entity(probe);
        let mut world = World::new(scene, &dir, WorldConfig::default());

        for _ in 0..20 {
            world.update(1.0 / 60.0);
        }

        let logs = world.take_logs();
        let counts: Vec<i64> = logs
            .iter()
            .filter_map(|l| l.message.strip_prefix("tick "))
            .filter_map(|rest| rest.split('|').nth(1))
            .filter_map(|n| n.trim().parse().ok())
            .collect();
        assert!(counts.len() >= 15, "实际只记录到 {} 帧", counts.len());
        let at_pause = counts[5]; // 第 6 帧（`frames == 6`）写缩放，本帧固定步已跑完
        assert!(at_pause > 0, "暂停前应当有过固定步");
        assert!(
            counts[6..].iter().all(|&c| c == at_pause),
            "暂停后 fixed_update 不该再派发，实际：{counts:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 缩放 2 倍：同样 `1/60` 的帧 dt 消费**两个**固定步 —— 证明累计器吃的是
    /// 缩放后的 dt，而不是把 `time_scale` 只挂在 `time()` 这个读数上。
    #[test]
    fn time_scale_multiplies_fixed_steps() {
        let mut scene = Scene::new("speed");
        scene.add_entity(Entity::new("marker"));
        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default());

        world.set_time_scale(2.0);
        let first = world.update(1.0 / 60.0).fixed_steps;
        let second = world.update(1.0 / 60.0).fixed_steps;
        assert_eq!(first + second, 4, "两帧 × 2 倍速 = 4 个固定步");

        world.set_time_scale(1.0);
        assert_eq!(world.update(1.0 / 60.0).fixed_steps, 1, "回到实时应当是 1 步");
    }
}

