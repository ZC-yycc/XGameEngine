//! 脚本执行上下文。
//!
//! # 为什么这里全是"擦除过的指针"
//!
//! 脚本必须能直接读写**活的**场景、物理世界和输入状态 —— 拷一份快照进去、
//! 跑完再拷回来，脚本之间就看不到彼此刚做的改动，"一个脚本移动平台、另一个
//! 脚本把角色放上去"这种最基本的协作会直接失效。
//!
//! 所以 [`ScriptCtx`] 不持有引用，只持有**地址**（`usize`）。安全性由调用方保证：
//!
//! - `ScriptCtx` 只在 [`crate::LuaHost`] 的 `run_hook` 一次调用期间存在；
//! - 创建它的那一刻，被指向的对象（[`GameCtx`]，内含 `Scene` / `PhysicsWorld` /
//!   `Input` / 日志与命令缓冲）都由调用栈上的 `&mut` / `&` 独占持有，
//!   期间不会有第二个别名；
//! - 交给脚本的是**地址的副本**（Lua 侧把槽地址装进宿主常驻槽、按
//!   `LightUserData` 轻量指针传参，见 `lua_api.rs` 的 `LuaCtx`）。
//!   解释器拿到的指向同一份真实数据 —— 所有写入都落到活数据上。
//!
//! 用地址而不是 `*mut T` 还有一个实际好处：裸指针在 Rust 里是 `!Send + !Sync`，
//! 而 mlua（开启 `send`）要求注册的类型 `Send`。
//! `usize` + `PhantomData<fn() -> T>` 天然满足，不需要 `unsafe impl`。
//!
//! # 形状
//!
//! 双轨计划 P0 把全部宿主 API 的函数体平移进了 [`GameCtx`]（`ctx.rs`），
//! 本模块的 [`ScriptCtx`] 因此瘦成两样东西：**一个 GameCtx 地址束** +
//! **一张宿主专属的持久状态表**（`state_get` / `state_set` 背后的表，
//! `mlua::Table` 类型不进引擎无关的 `GameCtx`）。

// 上面这一段解释了这个模块**为什么**要从 `&self` 里交出 `&mut`：`ScriptCtx` 是
// 一个 Copy 的"地址束"，解释器按值拷来拷去，但每个副本指向的都是同一份活数据，
// 所以访问器全都收 `&self`。clippy 的 `mut_from_ref` 针对的正是这个形状，但它
// 看到的是签名、看不到上面那套调用窗口的约定 —— 在模块级别关掉它，并在此写明理由。
#![allow(clippy::mut_from_ref)]

use std::marker::PhantomData;

use xgame_core::{EntityId, PhysicsBody, Scene};
use xgame_input::Input;
use xgame_math::glam::Vec3;
use xgame_physics::PhysicsWorld;

use crate::ctx::GameCtx;

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

    /// 从裸地址构造（用于把带生命周期参数的类型擦成 `'static` 形状，
    /// 见 `ScriptCtx::new` 的 GameCtx 地址束）。
    fn from_addr(addr: usize) -> Self {
        Self {
            addr,
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
    /// 产生这条日志的来源（脚本轨是项目相对路径，组件轨是行为名）。
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
/// ```lua
/// local id = spawn_entity(ctx, "ball", "builtin://sphere")
/// set_entity_pos(ctx, id, 1.0, 2.0, 3.0)   -- 实体还不存在，这句静默失效
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
/// 在本次派发期间不会被别人碰。[`crate::LuaHost`] 拿到之后把它连同日志 / 命令缓冲
/// 一起捆进 [`GameCtx`]，再把 `GameCtx` 擦除成地址交给解释器。
pub struct FrameCtx<'a> {
    pub scene: &'a mut Scene,
    /// 输入状态。没有输入系统时（例如无头测试）为 `None`。
    pub input: Option<&'a Input>,
    /// 物理世界。没有物理时为 `None`。
    pub physics: Option<&'a mut PhysicsWorld>,
    /// 音频命令束。宿主没接音频子系统时为 `None`（脚本的音频调用静默 no-op）。
    pub audio: Option<&'a mut xgame_core::AudioPort>,
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
            audio: None,
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

    pub fn with_audio(mut self, audio: &'a mut xgame_core::AudioPort) -> Self {
        self.audio = Some(audio);
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
/// 本体只有一个 [`GameCtx`] 地址 —— 全部宿主 API 都是它的方法
/// （见 `lua_api.rs`：每个 Lua 函数都是一行转发）。
///
/// 状态表直接持 `mlua::Table`（双轨计划 D1"Lua 值直接用 mlua 原生转换"）。
/// 双轨时期的泛型参数 `S` 已随 Rhai 删除（P3）收掉 —— 只剩一个宿主，
/// 泛型不再有意义。
pub struct ScriptCtx {
    game: ErasedMut<GameCtx<'static>>,
    /// 这个 (实体, 脚本) 对的持久状态。见 [`ScriptCtx::state`]。
    state: Option<ErasedMut<mlua::Table>>,
}

// 手写而不是 `#[derive(Clone, Copy)]`：derive 会给 `S` 加 `S: Clone` / `S: Copy`
// 约束，而 `mlua::Table` 不是 `Copy`。地址束的两个字段（`usize` + `PhantomData`）
// 对任意 `S` 都是 `Copy`，约束纯属 derive 多加的。
impl Clone for ScriptCtx {
    fn clone(&self) -> Self {
        *self
    }
}
impl Copy for ScriptCtx {}

impl ScriptCtx {
    pub(crate) fn new(game: &mut GameCtx<'_>, state: Option<&mut mlua::Table>) -> Self {
        Self {
            // 生命周期提升（`GameCtx<'a>` → `GameCtx<'static>`）只是地址束的
            // 类型层谎报，实际存活期由 `run_hook` 的调用窗口保证 —— 与
            // `ErasedMut::get_mut` 放宽返回生命周期是同一套论证（见模块文档）。
            // 地址转换是唯一不与借用检查器打架的写法：`&mut` 再 deref 无法
            // 提升类型里内嵌的生命周期参数。
            game: ErasedMut::from_addr(game as *mut GameCtx<'_> as usize),
            state: state.map(ErasedMut::new),
        }
    }

    /// 取这个 (实体, 脚本) 对的持久状态表。
    ///
    /// # 为什么需要它
    ///
    /// 顶层 `local` 是**脚本级**共享的（同一脚本的所有实体看到同一个变量，
    /// 像别的语言里的模块级静态），所以"给自己记一个计时器 / 冷却 / 血量"
    /// 这类**每实体**可变状态在脚本里没有别的落地方式。这张表由引擎侧
    /// 持有、按 `(实体, 脚本)` 分开，脚本通过 `state_get` / `state_set` 访问。
    ///
    /// 它不进 [`GameCtx`]：值类型是 `mlua::Value`（宿主专属），
    /// 引擎无关的方法层不该认识它。
    pub(crate) fn state(&self) -> Option<&mut mlua::Table> {
        // SAFETY: 状态表由宿主（LuaHost）独占持有，直到派发结束。
        self.state.map(|s| unsafe { s.get_mut() })
    }

    /// 本次调用的完整游戏上下文（全部宿主 API 都在它上面）。
    pub(crate) fn game(&self) -> &mut GameCtx<'static> {
        // SAFETY: 见模块头。ctx 的生命周期被 run_hook 的调用窗口包住。
        unsafe { self.game.get_mut() }
    }
}

impl std::fmt::Debug for ScriptCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptCtx")
            .field("entity", &self.game().entity)
            .field("delta", &self.game().delta)
            .field("time", &self.game().time)
            .finish_non_exhaustive()
    }
}
