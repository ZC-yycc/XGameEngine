//! 脚本执行上下文。
//!
//! # 为什么这里全是"擦除过的指针"
//!
//! Rhai 的自定义类型要求 `'static`（它把参数包进 `Dynamic` 交给解释器，
//! 而 `Dynamic` 不能带生命周期参数）。可脚本必须能直接读写**活的**场景、物理世界
//! 和输入状态 —— 拷一份快照进去、跑完再拷回来，脚本之间就看不到彼此刚做的改动，
//! "一个脚本移动平台、另一个脚本把角色放上去"这种最基本的协作会直接失效。
//!
//! 所以 [`ScriptCtx`] 不持有引用，只持有**地址**（`usize`）。安全性由调用方保证：
//!
//! - `ScriptCtx` 只在 [`crate::ScriptHost`] 的 `run_hook` 一次调用期间存在；
//! - 创建它的那一刻，被指向的对象（`Scene` / `PhysicsWorld` / `Input` / 日志缓冲）
//!   都由调用栈上的 `&mut` / `&` 独占持有，期间不会有第二个别名；
//! - 传给 Rhai 时按**值**传递（`ScriptCtx: Copy`）。解释器拿到的是地址的副本，
//!   所有写入仍然落到同一份真实数据上 —— 这正是我们要的。
//!
//! 用地址而不是 `*mut T` 还有一个实际好处：裸指针在 Rust 里是 `!Send + !Sync`，
//! 而 Rhai（开启 `sync` feature）要求注册的类型 `Send + Sync`。
//! `usize` + `PhantomData<fn() -> T>` 天然满足，不需要 `unsafe impl`。

use std::marker::PhantomData;

use xgame_core::{EntityId, PhysicsBody, Scene};
use xgame_input::Input;
use xgame_math::glam::Vec3;
use xgame_physics::PhysicsWorld;

// ---------------------------------------------------------------------------
// 生命周期擦除的容器
// ---------------------------------------------------------------------------

/// 擦除了生命周期的可变别名。
struct ErasedMut<T> {
    addr: usize,
    _p: PhantomData<fn() -> T>,
}

impl<T> Clone for ErasedMut<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for ErasedMut<T> {}

impl<T> ErasedMut<T> {
    fn new(value: &mut T) -> Self {
        Self {
            addr: value as *mut T as usize,
            _p: PhantomData,
        }
    }

    /// # Safety
    ///
    /// 调用者必须保证：地址仍然有效，且此刻没有其它别名借用。
    ///
    /// 返回 `&'static mut T` 而不是 `&mut T`：这里的目标对象根本不在 `self` 里，
    /// 把返回值的生命周期挂到 `&self` 上是错的 —— 那样连 `Option::map` 里
    /// 按值传进来的临时副本都借不出去（编译器会认为返回值指向局部变量）。
    /// 实际的存活期由调用方自己保证（见模块文档）。
    unsafe fn get_mut(self) -> &'static mut T {
        unsafe { &mut *(self.addr as *mut T) }
    }

    /// # Safety
    ///
    /// 调用者必须保证地址仍然有效。返回类型放宽到 `'static` 的理由同上。
    unsafe fn get(self) -> &'static T {
        unsafe { &*(self.addr as *const T) }
    }
}

/// 擦除了生命周期的不可变别名。
struct ErasedRef<T> {
    addr: usize,
    _p: PhantomData<fn() -> T>,
}

impl<T> Clone for ErasedRef<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for ErasedRef<T> {}

impl<T> ErasedRef<T> {
    fn new(value: &T) -> Self {
        Self {
            addr: value as *const T as usize,
            _p: PhantomData,
        }
    }

    /// # Safety
    ///
    /// 调用者必须保证地址仍然有效。返回类型放宽到 `'static` 的理由同 [`ErasedMut`]。
    unsafe fn get(self) -> &'static T {
        unsafe { &*(self.addr as *const T) }
    }
}

// ---------------------------------------------------------------------------
// 日志与命令
// ---------------------------------------------------------------------------

/// 脚本日志级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// 一条脚本日志。
#[derive(Debug, Clone)]
pub struct ScriptLog {
    pub level: LogLevel,
    /// 产生这条日志的脚本（项目相对路径）。
    pub script: String,
    /// 产生这条日志的实体。
    pub entity: Option<EntityId>,
    pub message: String,
}

/// 脚本对**场景结构**的改动通知。
///
/// # 为什么是"通知"而不是"延迟执行的命令"
///
/// 最早这里是一套延迟命令队列：脚本里 `spawn_entity()` 只登记，等这一轮脚本
/// 跑完再统一建实体。听起来稳妥（避免在遍历中改容器），但立刻暴露了一个
/// 致命的易用性问题：
///
/// ```rhai
/// let id = spawn_entity(ctx, "ball", "builtin://sphere");
/// set_entity_pos(ctx, id, 1.0, 2.0, 3.0);   // 实体还不存在，这句静默失效
/// ```
///
/// "先生成、再摆位置"是最自然的写法，延迟执行会让它默默不生效，作者根本猜不到原因。
///
/// 改成**立即生效**之后安全吗？安全，理由很具体：派发脚本时遍历的是一份
/// `(实体 ID, 脚本路径)` 的**快照**，从不跨脚本调用持有实体表；所有查找又是
/// "按 ID 找索引"而不是缓存索引。于是增删实体最多让后面的脚本少看到一个实体，
/// 不会有索引错位。
///
/// 那这个枚举还留着干什么？告诉**运行时**它该做什么 —— 场景是脚本改的，
/// 但刚体是运行时建的，得有人把新实体的物理体补上、把删掉的清掉。
#[derive(Debug, Clone)]
pub enum ScriptCommand {
    /// 新实体已经进入场景，运行时需要为它建立物理体（如果声明了的话）。
    EntitySpawned {
        id: EntityId,
        physics: Option<PhysicsBody>,
    },
    /// 这些实体已经离开场景（含整棵子树），运行时需要销毁对应的物理体。
    EntitiesRemoved { ids: Vec<EntityId> },
    /// 实体换了父节点，运行时需要把它的刚体移到新的世界位置。
    Reparented { child: EntityId },
}

/// 脚本请求生成的实体。
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub name: String,
    pub model: String,
    pub position: Vec3,
    pub rotation: xgame_math::glam::Quat,
    pub scale: Vec3,
    pub physics: Option<PhysicsBody>,
}

impl SpawnRequest {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            model: String::new(),
            position: Vec3::ZERO,
            rotation: xgame_math::glam::Quat::IDENTITY,
            scale: Vec3::ONE,
            physics: None,
        }
    }
}

// ---------------------------------------------------------------------------
// 每帧的借用束（安全的那一半）
// ---------------------------------------------------------------------------

/// 一次脚本派发所需的外部状态。
///
/// 这是**安全**的一层：调用方用 `&mut` / `&` 明确交出所有权，证明这些对象
/// 在本次派发期间不会被别人碰。`ScriptHost` 拿到之后才把它们擦除成地址。
pub struct FrameCtx<'a> {
    pub scene: &'a mut Scene,
    /// 输入状态。没有输入系统时（例如无头测试）为 `None`。
    pub input: Option<&'a Input>,
    /// 物理世界。没有物理时为 `None`。
    pub physics: Option<&'a mut PhysicsWorld>,
    /// 本帧真实帧间隔（秒）。
    pub delta: f32,
    /// 游戏已运行的总时长（秒）。
    pub time: f32,
}

impl<'a> FrameCtx<'a> {
    pub fn new(scene: &'a mut Scene, delta: f32, time: f32) -> Self {
        Self {
            scene,
            input: None,
            physics: None,
            delta,
            time,
        }
    }

    pub fn with_input(mut self, input: &'a Input) -> Self {
        self.input = Some(input);
        self
    }

    pub fn with_physics(mut self, physics: &'a mut PhysicsWorld) -> Self {
        self.physics = Some(physics);
        self
    }
}

// ---------------------------------------------------------------------------
// 交给脚本的上下文
// ---------------------------------------------------------------------------

/// 传给脚本函数的第一参数（脚本里习惯叫 `ctx`）。
///
/// 它是一个 `Copy` 的地址束：脚本可以随便传给别的函数、存进局部变量，
/// 不会因为"把它移走了"而报错，也不会阻止 Rust 侧的借用检查。
#[derive(Clone, Copy)]
pub struct ScriptCtx {
    scene: ErasedMut<Scene>,
    input: Option<ErasedRef<Input>>,
    physics: Option<ErasedMut<PhysicsWorld>>,
    logs: ErasedMut<Vec<ScriptLog>>,
    commands: ErasedMut<Vec<ScriptCommand>>,
    /// 这个 (实体, 脚本) 对的持久状态。见 [`ScriptCtx::state`]。
    state: Option<ErasedMut<rhai::Map>>,
    delta: f32,
    time: f32,
    /// 触发这次调用的实体（[`EntityId::raw`]）。`0` 表示没有实体上下文。
    entity: u64,
    script: ErasedRef<String>,
}

impl ScriptCtx {
    pub(crate) fn new(
        frame: &mut FrameCtx<'_>,
        entity: EntityId,
        script_name: &String,
        logs: &mut Vec<ScriptLog>,
        commands: &mut Vec<ScriptCommand>,
        state: Option<&mut rhai::Map>,
    ) -> Self {
        Self {
            scene: ErasedMut::new(frame.scene),
            input: frame.input.map(ErasedRef::new),
            physics: frame.physics.as_deref_mut().map(ErasedMut::new),
            logs: ErasedMut::new(logs),
            commands: ErasedMut::new(commands),
            state: state.map(ErasedMut::new),
            delta: frame.delta,
            time: frame.time,
            entity: entity.raw(),
            script: ErasedRef::new(script_name),
        }
    }

    /// 取这个 (实体, 脚本) 对的持久状态表。
    ///
    /// # 为什么需要它
    ///
    /// Rhai 的函数**不能写外层作用域**（顶层 `let` 只能读、不能改），所以
    /// "给自己记一个计时器 / 冷却 / 血量"在脚本里没有别的写法。这张表由引擎侧
    /// 持有、按 `(实体, 脚本)` 分开，脚本通过 `state_get` / `state_set` 访问。
    pub(crate) fn state(&self) -> Option<&mut rhai::Map> {
        // SAFETY: 状态表由 ScriptHost 独占持有，直到派发结束。
        self.state.map(|s| unsafe { s.get_mut() })
    }

    // -- 访问器 -------------------------------------------------------------

    pub(crate) fn scene(&self) -> &Scene {
        // SAFETY: 见模块头。ctx 的生命周期被 run_hook 的调用窗口包住。
        unsafe { self.scene.get() }
    }

    pub(crate) fn scene_mut(&self) -> &mut Scene {
        // SAFETY: 同上；run_hook 期间 scene 由 FrameCtx 独占。
        unsafe { self.scene.get_mut() }
    }

    pub(crate) fn physics(&self) -> Option<&PhysicsWorld> {
        // SAFETY: 同上。
        self.physics.map(|p| unsafe { p.get() })
    }

    pub(crate) fn physics_mut(&self) -> Option<&mut PhysicsWorld> {
        // SAFETY: 同上。
        self.physics.map(|p| unsafe { p.get_mut() })
    }

    pub(crate) fn input(&self) -> Option<&Input> {
        // SAFETY: 同上。
        self.input.map(|i| unsafe { i.get() })
    }

    pub(crate) fn push_log(&self, level: LogLevel, message: String) {
        let entry = ScriptLog {
            level,
            script: self.script_name().to_owned(),
            entity: self.entity_id(),
            message,
        };
        // SAFETY: 日志缓冲由 ScriptHost 独占持有，直到派发结束。
        unsafe { self.logs.get_mut() }.push(entry);
    }

    pub(crate) fn push_command(&self, command: ScriptCommand) {
        // SAFETY: 同上。
        unsafe { self.commands.get_mut() }.push(command);
    }

    pub(crate) fn script_name(&self) -> &str {
        // SAFETY: 脚本路径字符串在 `ScriptHost.scripts` 里，活得比 ctx 久。
        unsafe { self.script.get() }.as_str()
    }

    /// 触发这次调用的实体 ID；`0`（即 `None`）表示没有实体上下文。
    pub(crate) fn entity_id(&self) -> Option<EntityId> {
        if self.entity == 0 {
            None
        } else {
            Some(EntityId::from_raw(self.entity))
        }
    }

    /// 当前实体在场景里的索引。
    pub(crate) fn entity_index(&self) -> Option<usize> {
        let id = self.entity_id()?;
        self.scene().find_entity(id)
    }

    pub(crate) fn delta(&self) -> f32 {
        self.delta
    }

    pub(crate) fn time(&self) -> f32 {
        self.time
    }
}

impl std::fmt::Debug for ScriptCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptCtx")
            .field("entity", &self.entity)
            .field("delta", &self.delta)
            .field("time", &self.time)
            .finish_non_exhaustive()
    }
}
