//! Lua 脚本宿主：编译缓存、生命周期派发、命令回收。
//!
//! 宿主 API 的真相源是 [`crate::GameCtx`] 方法层（`ctx.rs`），本模块只管
//! 编译缓存与生命周期派发。差异全部来自 Lua 的语言习惯：
//!
//! - **钩子是环境表里的全局函数**。每个脚本在**自己的环境表**里执行（沙箱），
//!   `start` / `update` / `fixed_update` / `on_destroy` 从那张表里查。
//! - **顶层 `local` 是脚本级共享的**（同一脚本的所有实体看到同一个值，
//!   像别的语言里的模块级静态）。每实体的状态走 `state_get` / `state_set`
//!   （由 `LuaCtx` 携带的每 (实体, 脚本) 一张表）。
//! - **指令计数沙箱**：每帧每钩子一次调用的步数上限，超了就中断这次调用
//!   （效果类似"这个脚本这帧没跑"），而不是卡死进程。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use mlua::{Function, HookTriggers, Lua, StdLib, Table, Value, VmState};
use xgame_core::{EntityId, Scene};

use crate::context::{FrameCtx, LogLevel, ScriptCommand, ScriptCtx, ScriptLog};
use crate::ctx::GameCtx;
use crate::host::{CommandReport, Hook, PreloadReport};
use crate::intern::PathIntern;
use crate::lua_api::{self, LuaCtx};

/// 每帧每钩子一次调用的指令上限。
///
/// 常见脚本一帧只有几千条指令，留足余量；真跑到这个数说明它必然写错了（死循环）。
/// 20 万条指令对一帧的胶水逻辑绰绰有余（历史值：双轨时期与 Rhai 侧
/// `set_max_operations(200_000)` 对齐，删 Rhai 后保留原数量级）。
const INSTRUCTION_BUDGET: u64 = 200_000;

/// 指令计数钩子的触发间隔（每 N 条指令检查一次预算）。
///
/// 太小（=1）会让每条指令都回调进 Rust，拖慢正常脚本；太大则超限后多跑一会儿
/// 才中断。1024 是"正常脚本几乎无感、死循环在微秒级被掐断"的折中。
const HOOK_INTERVAL: u32 = 1024;

/// 单个脚本 VM 的内存上限（字节）。
///
/// 与 [`INSTRUCTION_BUDGET`] 是一对沙箱：那个挡"算不完"（死循环），这个挡
/// "存不下"（`local t = {} while true do t[#t+1] = string.rep("x", 1 << 20) end`）。
/// 没有这条上限时，一个失控脚本能把进程内存吃光 —— 结局是操作系统 OOM 杀进程，
/// 编辑器里未保存的场景跟着一起没。脚本是**用户内容**，防护不能靠"作者会写对"。
///
/// 128 MiB 刻意设得很宽松：脚本 API 的持久状态表、编译后的 chunk、几百个实体的
/// 状态加起来通常不到 1 MiB。留这么大是为了不误伤"用 Lua 搓了张大表"的正当玩法，
/// 同时又让无限增长在跨过某个数量级时立刻失败。
pub const MEMORY_LIMIT: usize = 128 * 1024 * 1024;

/// 报错计数表超过这个条数就扫一遍、把已不在场景里的实体条目删掉。
///
/// 条目只在"某个 (实体, 脚本, 钩子) 报过错且之后没成功过"时存在，正常项目
/// 远达不到这个量级，所以这条清理路径几乎不会走；设上限是为了防住
/// "每帧 spawn + 报错 + destroy"这种病态脚本把表撑大。
const ERROR_STREAK_SOFT_LIMIT: usize = 1024;

/// 四个生命周期钩子的函数句柄（编译期从环境表取一次）。
///
/// 以前这里是一组 bool（`Hooks`），钩子函数每帧每实体现从 env 表查
/// （`env.get` 实测 ~194 ns/次，1000 实体 ≈192 µs/帧，占 no-op 派发 17%）。
/// 而 `detect_funcs` 本来就要把函数 `raw_get` 出来验存在性 —— 句柄直接留下，
/// 热路径零查表。`Function` 是 mlua 的 GC 句柄，跨帧持有安全（脚本失效时
/// 随 [`CompiledScript`] 一起丢弃）。
struct HookFuncs {
    start: Option<Function>,
    update: Option<Function>,
    fixed_update: Option<Function>,
    collision_enter: Option<Function>,
    collision_exit: Option<Function>,
    destroy: Option<Function>,
}

impl HookFuncs {
    fn get(&self, hook: Hook) -> Option<&Function> {
        match hook {
            Hook::Start => self.start.as_ref(),
            Hook::Update => self.update.as_ref(),
            Hook::FixedUpdate => self.fixed_update.as_ref(),
            Hook::CollisionEnter => self.collision_enter.as_ref(),
            Hook::CollisionExit => self.collision_exit.as_ref(),
            Hook::Destroy => self.destroy.as_ref(),
        }
    }
}

/// 一份编译好的 Lua 脚本。
///
/// 不存路径了：名字走驻留表（`intern.resolve`），状态表键走驻留 id ——
/// 结构上不再需要第二份 `String` 副本。
///
/// 也不存环境表：钩子函数自身的 `_ENV` upvalue 持有它（Lua 的沙箱机制），
/// 宿主侧丢掉 `Table` 句柄不影响脚本内的全局查找回落。
struct CompiledScript {
    funcs: HookFuncs,
}

/// Lua 脚本宿主。
///
/// 一个 `LuaHost` 对应**一次运行**（编辑器的 Play 会话，或独立游戏的一局）。
/// 它自己编译脚本、缓存环境表、派发生命周期回调，并把脚本请求的结构性改动
/// 攒成命令交给运行时执行。
pub struct LuaHost {
    lua: Lua,
    /// 剩余指令预算（钩子每触发一次扣，扣到 0 就中断这次调用）。
    budget: Arc<AtomicU64>,
    /// 项目相对路径 → 编译结果。
    scripts: HashMap<String, CompiledScript>,
    /// 编译失败过的路径。单独记一份，避免每帧对着同一个坏文件反复读盘 + 报错。
    failed: HashSet<String>,
    /// 已经跑过 `start` 的实体。
    started: HashSet<EntityId>,
    /// 每个 (实体, 脚本) 对的持久状态表。键里的脚本是**驻留 id**（`u32`）——
    /// `String` 键 = 每钩子克隆一次路径（1000 实体实测 ≈77.8 µs/帧，
    /// 见 [`PathIntern`] 模块文档）。
    states: HashMap<(EntityId, u32), Table>,
    /// ctx 传参的常驻槽（见 [`LuaCtx`] 类型文档）。
    ///
    /// 为什么是 `Box`：槽的地址必须**跨调用稳定** —— 脚本拿到的是它的指针，
    /// 每钩子现址分配会给出不同地址（而且现造 userdata 实测 ~284 ns/次，
    /// 1000 实体 ×2 钩子 ≈568 µs/帧）。`MaybeUninit` 是因为首次写入前
    /// 没有合法初值（`LuaCtx` 需要一个活的 `GameCtx` 才能构造）。
    ctx_slot: Box<std::mem::MaybeUninit<LuaCtx>>,
    /// 脚本路径驻留表（热路径零 `String` 分配）。**永不清空** ——
    /// 状态表按 id 找状态，热重载（`invalidate*`）只清编译缓存；
    /// 理由与回归测试见 [`crate::intern`] 与
    /// `LuaHost::invalidate_all` 的说明（`state_survives_hot_reload_*`）。
    intern: PathIntern,
    /// 待回收的日志与命令。
    logs: Vec<ScriptLog>,
    commands: Vec<ScriptCommand>,
    /// 连续报错计数（限流用）：`(实体, 脚本驻留 id, 钩子序号)` → 连续次数。
    ///
    /// 一个每帧都出错的脚本如果不限流，就会 60 条/秒地刷日志面板，并且每次
    /// `format!` 都在热路径上分配。这里按**指数退避**只报第 1、2、4、8…次，
    /// 中间的静默；同一钩子成功执行一次就清零，下次再错重新算。
    ///
    /// **不**做"错够几次就禁用脚本"：那会悄悄改变游戏行为，比刷屏更难查。
    error_streak: HashMap<(EntityId, u32, u8), u32>,
    /// 脚本路径的解析基准（项目根目录）。
    root: Option<PathBuf>,
    /// 附加搜索根（`project.ron` 的 `content.search_paths`）。
    ///
    /// 语义与门禁都在 [`xgame_core::resolve_read_path`]：先直接命中、再按顺序
    /// 试这些目录；不合规的条目运行时跳过（`--check` 会报）。
    search_paths: Vec<String>,
    /// 内存上限（字节）。只用于**报错文案**：真正的强制在 `lua.set_memory_limit`。
    memory_limit: usize,
}

impl Default for LuaHost {
    fn default() -> Self {
        Self::new()
    }
}

impl LuaHost {
    pub fn new() -> Self {
        Self::with_memory_limit(MEMORY_LIMIT)
    }

    /// 同 [`Self::new`]，但自定义脚本 VM 的内存上限（字节）。
    ///
    /// 生产路径只有 [`MEMORY_LIMIT`] 这一个值；参数化是为了让测试能用几 MiB 的
    /// 小限额验证"超限会抛错"，不必为了触发上限去真的吃几百兆内存。
    pub fn with_memory_limit(limit: usize) -> Self {
        // 沙箱：只开安全的标准库。关掉 io / os / package / debug / ffi ——
        // 脚本是**用户内容**（可能是下载的 mod），不该能读写文件、起进程、
        // 或绕过宿主的类型检查。coroutine 开着（Lua 并发惯用法，且无副作用）。
        let libs = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE;
        let lua = match Lua::new_with(libs, mlua::LuaOptions::default()) {
            Ok(lua) => lua,
            Err(err) => {
                // Lua VM 都建不起来是环境级故障（vendored 编译坏了），
                // 不该静默。退化成"空 VM + 记一条错误"，让调用方的每个脚本
                // 都报"加载失败"，而不是 panic 拖垮宿主进程。
                log::error!("创建 Lua VM 失败: {err}");
                Lua::new()
            }
        };

        // 内存上限：与指令预算互补的那一半沙箱。
        //
        // 指令预算挡的是"算不完"（死循环），这个挡的是"存不下"（
        // `while true do t[#t+1] = {} end`）。没有它，一个失控脚本会把进程内存
        // 吃光，结局是操作系统 OOM 杀进程 —— 编辑器里未保存的场景一起没。
        // 设置失败只会发生在"分配器不归 mlua 管"的模块模式，打一条警告即可：
        // 不能因为设不上限就让引擎跑不起来。
        if let Err(err) = lua.set_memory_limit(limit) {
            log::warn!("设置脚本内存上限失败（{err}），脚本将可以无限占用内存");
        }

        let budget = Arc::new(AtomicU64::new(INSTRUCTION_BUDGET));
        // 指令计数沙箱：每 HOOK_INTERVAL 条指令回调一次，扣预算，扣到 0 中断。
        // 这是"死循环脚本只毁掉自己这一帧，不卡死整个游戏"的唯一防线。
        lua.set_hook(HookTriggers::new().every_nth_instruction(HOOK_INTERVAL), {
            let budget = Arc::clone(&budget);
            move |_lua, _debug| {
                let remaining = budget.load(Ordering::Relaxed);
                if remaining == 0 {
                    // 返回错误 → 中断这次 Lua 调用，错误冒泡到 `call` 的 Err。
                    return Err(mlua::Error::RuntimeError(
                        "指令数超限，疑似死循环，已中断本次调用".to_owned(),
                    ));
                }
                budget.store(
                    remaining.saturating_sub(HOOK_INTERVAL as u64),
                    Ordering::Relaxed,
                );
                Ok(VmState::Continue)
            }
        });

        // 把整套装进全局表（失败只可能是 VM 状态异常）。
        if let Err(err) = lua_api::register(&lua) {
            log::error!("注册 Lua 宿主 API 失败: {err}");
        }

        Self {
            lua,
            budget,
            scripts: HashMap::new(),
            failed: HashSet::new(),
            started: HashSet::new(),
            states: HashMap::new(),
            ctx_slot: Box::new(std::mem::MaybeUninit::uninit()),
            intern: PathIntern::new(),
            logs: Vec::new(),
            commands: Vec::new(),
            error_streak: HashMap::new(),
            root: None,
            search_paths: Vec::new(),
            memory_limit: limit,
        }
    }

    /// 脚本 VM 当前占用的内存（字节）。诊断与回归测试用。
    pub fn used_memory(&self) -> usize {
        self.lua.used_memory()
    }

    /// 本次 VM 的内存上限（字节）。
    pub fn memory_limit(&self) -> usize {
        self.memory_limit
    }

    /// 把一次 Lua 错误整理成给作者看的一句话。
    ///
    /// 内存超限要**说清楚是超限**：Lua 的原文是 `not enough memory`，作者看到
    /// 这句只会怀疑引擎坏了，而不是"我这张表太大了"。
    fn describe_error(&self, err: &mlua::Error) -> String {
        match err {
            mlua::Error::MemoryError(_) => format!(
                "脚本内存超限（上限 {} MiB），已中断本次调用。常见成因是在钩子里不断往表里塞东西",
                self.memory_limit / (1024 * 1024)
            ),
            other => format!("{other}"),
        }
    }

    /// 设置脚本路径的解析基准（项目根目录）。
    pub fn set_root(&mut self, root: impl Into<PathBuf>) {
        self.root = Some(root.into());
    }

    /// 设置附加搜索根（`project.ron` 的 `content.search_paths`）。
    ///
    /// 只影响**还没加载**的脚本：已经编译进缓存的不重读（与热重载的语义一致 ——
    /// 换搜索路径不是"脚本内容变了"）。
    pub fn set_search_paths(&mut self, paths: Vec<String>) {
        self.search_paths = paths;
    }

    /// 已编译脚本数量。
    pub fn script_count(&self) -> usize {
        self.scripts.len()
    }

    /// 当前持有的 `(实体, 脚本)` 状态表数量（诊断与生命周期回归测试用）。
    pub fn entity_state_count(&self) -> usize {
        self.states.len()
    }

    /// 同一个脚本是否已经编译过。
    pub fn is_loaded(&self, path: &str) -> bool {
        self.scripts.contains_key(path)
    }

    /// 丢弃所有编译结果，下次派发时重新读取（热重载入口）。
    ///
    /// **不碰驻留表**（[`PathIntern`]）：状态表按驻留 id 找状态，id 一变
    /// 每实体的状态就静默错位了。回归测试见本文件的
    /// `state_survives_hot_reload_even_when_intern_order_changes`。
    pub fn invalidate_all(&mut self) {
        self.scripts.clear();
        self.failed.clear();
        // 脚本内容变了，报错序列要从头算 —— 否则改好之后的第一条错误会被
        // 当作"第 N 次"静默掉，正好把用户最想看的那条吞了。
        self.error_streak.clear();
    }

    /// 让某个脚本失效（同样不清驻留表，理由见 [`LuaHost::invalidate_all`]）。
    pub fn invalidate(&mut self, path: &str) {
        self.scripts.remove(path);
        self.failed.remove(path);
        // 同上：这个脚本的报错计数作废（键里的驻留 id 无法反查路径，只能全清）。
        self.error_streak.clear();
    }

    /// 清空**运行时**状态（`start` 是否跑过），保留编译缓存。
    pub fn reset_runtime_state(&mut self) {
        self.started.clear();
        self.states.clear();
        self.logs.clear();
        self.commands.clear();
        // 新的 Play 会话应当重新报错；否则第一次错误会被上一次会话的计数吞掉。
        self.error_streak.clear();
    }

    /// 场景切换：只清与**实体**绑定的运行时状态（`start` 标记 + 每实体状态表）。
    ///
    /// 与 [`LuaHost::reset_runtime_state`] 的差别是**不碰日志与命令缓冲** ——
    /// 换场景发生在帧末，此时本帧的日志还没被人取走（编辑器控制台 / 无头报告
    /// 都在帧后读），连它们一起清掉就会把"这一帧到底报了什么错"抹掉。
    /// 命令缓冲同理：其中可能有本帧最后一条 `spawn` 通知还没被消费。
    ///
    /// **不清编译缓存与驻留表**：脚本模块级全局变量跨场景保留（Unity 不重载
    /// domain 的语义），驻留 id 永不清是 `intern.rs` 的硬约束。
    pub fn reset_scene_state(&mut self) {
        self.started.clear();
        self.states.clear();
        // 实体 ID 随场景整体换掉，旧的报错计数对新场景没有意义。
        self.error_streak.clear();
    }

    /// 编译场景里用到的全部 `.lua` 脚本。
    ///
    /// 只认 `.lua`（扩展名判定见 [`is_lua`]）—— 非 `.lua` 的路径直接跳过
    /// （历史上 Rhai 轨的遗留，删 Rhai 后不再有第二个宿主接管）。
    pub fn preload(&mut self, root: &Path, scene: &Scene) -> PreloadReport {
        self.root = Some(root.to_path_buf());
        let mut report = PreloadReport::default();
        let mut seen: HashSet<&str> = HashSet::new();

        let mut paths: Vec<String> = Vec::new();
        for entity in &scene.entities {
            for path in &entity.scripts {
                if is_lua(path) && seen.insert(path.as_str()) {
                    paths.push(path.clone());
                }
            }
        }

        for path in paths {
            if self.ensure_loaded(&path) {
                report.loaded.push(path);
            } else {
                report.failed.push(path);
            }
        }
        report
    }

    // -----------------------------------------------------------------------
    // 生命周期派发
    // -----------------------------------------------------------------------

    /// 对尚未跑过 `start` 的实体派发一次 `start`。
    pub fn dispatch_start(&mut self, mut frame: FrameCtx<'_>) {
        // 队列存驻留 id 而不是 `String`（热路径零路径 `String` 克隆，
        // 见 [`PathIntern`]）。
        let mut queue: Vec<(EntityId, u32)> = Vec::new();
        for entity in &frame.scene.entities {
            if self.started.contains(&entity.id) {
                continue;
            }
            for path in &entity.scripts {
                if is_lua(path) {
                    queue.push((entity.id, self.intern.intern(path)));
                }
            }
        }
        // 先把 `started` 全记上再执行：脚本 A 的 `start` 里如果生成了实体，
        // 那个实体应当等下一轮才跑 start。
        for (id, _) in &queue {
            self.started.insert(*id);
        }
        for (id, path_id) in queue {
            self.run_hook(&mut frame, id, path_id, Hook::Start, None);
        }
    }

    /// 派发一帧 `update`。
    pub fn dispatch_update(&mut self, mut frame: FrameCtx<'_>) {
        let queue = collect_lua_queue(frame.scene, &mut self.intern);
        for (id, path_id) in queue {
            self.run_hook(&mut frame, id, path_id, Hook::Update, None);
        }
    }

    /// 派发一次 `fixed_update`（与物理固定步长同频）。
    pub fn dispatch_fixed_update(&mut self, mut frame: FrameCtx<'_>, fixed_dt: f32) {
        frame.delta = fixed_dt;
        let queue = collect_lua_queue(frame.scene, &mut self.intern);
        for (id, path_id) in queue {
            self.run_hook(&mut frame, id, path_id, Hook::FixedUpdate, None);
        }
    }

    /// 在实体被移除**之后**派发 `on_destroy`，随后释放该实体的全部脚本状态。
    ///
    /// 此时钩子仍可读自己的 `state_*`，但自身已不在场景中；这与 Rust 组件轨
    /// 的销毁语义一致。`scripts` 来自移除命令保存的清单，不能再到场景里反查。
    pub fn dispatch_destroy(
        &mut self,
        frame: &mut FrameCtx<'_>,
        entity: EntityId,
        scripts: &[String],
    ) {
        for path in scripts {
            if is_lua(path) {
                let path_id = self.intern.intern(path);
                self.run_hook(frame, entity, path_id, Hook::Destroy, None);
            }
        }
        self.forget_entity(entity);
    }

    /// 派发一次碰撞事件（`started` = 进入接触）。
    ///
    /// 只派发给**事件涉及的那个实体**：对方实体作为参数传进去，不是收件人 ——
    /// 否则一次碰撞会把全场脚本都叫一遍。
    pub fn dispatch_collision(
        &mut self,
        mut frame: FrameCtx<'_>,
        entity: EntityId,
        other: EntityId,
        started: bool,
    ) {
        let hook = if started {
            Hook::CollisionEnter
        } else {
            Hook::CollisionExit
        };
        // 队列里存驻留 id（与其它派发同款：热路径不克隆路径字符串）。
        let mut queue: Vec<(EntityId, u32)> = Vec::new();
        if let Some(e) = frame.scene.entities.iter().find(|e| e.id == entity) {
            for path in &e.scripts {
                if is_lua(path) {
                    queue.push((entity, self.intern.intern(path)));
                }
            }
        }
        for (id, path_id) in queue {
            self.run_hook(&mut frame, id, path_id, hook, Some(other));
        }
    }

    /// 实体离开场景时清掉它的运行时痕迹（`started` 标记与状态表）。
    pub fn forget_entity(&mut self, entity: EntityId) {
        self.started.remove(&entity);
        self.states.retain(|(id, _), _| *id != entity);
    }

    /// 派发一个钩子。`path_id` 是驻留 id（队列里只带它，热路径零 `String`
    /// 克隆；要 `&str` 的地方用 `intern.resolve` 换回来）。
    fn run_hook(
        &mut self,
        frame: &mut FrameCtx<'_>,
        entity: EntityId,
        path_id: u32,
        hook: Hook,
        arg: Option<EntityId>,
    ) {
        // 前面的回调可能已经删除这个实体；Destroy 使用移除通知，允许实体不在场。
        if !matches!(hook, Hook::Destroy) && frame.scene.find_entity(entity).is_none() {
            return;
        }
        // 实体被删后它的报错计数会残留；超过软上限时扫一遍清掉（见常量说明）。
        if self.error_streak.len() > ERROR_STREAK_SOFT_LIMIT {
            let scene = &*frame.scene;
            self.error_streak
                .retain(|key, _| scene.find_entity(key.0).is_some());
        }
        // 借 `self.intern` 得到的 `&str` 传不进 `&mut self` 的方法（借用检查器
        // 看不出"它就是 self 身上的一块"），所以加载这一步在方法内部自己解析 id；
        // 钩子体再单独 resolve 一次（O(1) 下标）。
        if !self.ensure_loaded_id(path_id) {
            return;
        }
        // 字段级拆借：`path` 借 `self.intern`（共享），下面要动的
        // scripts / states / logs / commands / budget / ctx_slot / lua 全是别的字段。
        let path = self.intern.resolve(path_id);
        let Some(script) = self.scripts.get(path) else {
            return;
        };
        // 钩子函数句柄是编译期缓存的（`detect_funcs` 留下的），热路径不再
        // 查 env 表 —— 每钩子 ~194 ns ×1000 实体 ≈192 µs/帧，量证见 PLAN.md 二十。
        let Some(func) = script.funcs.get(hook) else {
            return;
        };

        // 重置本次调用的指令预算（沙箱）。
        self.budget.store(INSTRUCTION_BUDGET, Ordering::Relaxed);

        // 每 (实体, 脚本) 一份的状态表：键里的脚本是驻留 id（`u32`）——
        // 以前这里每钩子克隆一个路径 String（1000 实体实测 ≈77.8 µs/帧），
        // id 与路径一一对应且热重载也不变（见 `intern.rs`），语义等价。
        let state_key = (entity, path_id);
        let state = self.states.entry(state_key).or_insert_with(|| {
            self.lua
                .create_table()
                .expect("创建 Lua 状态表失败（VM 内存耗尽）")
        });

        // 把本次钩子所需的全部状态捆成 GameCtx（API 真相源，见 ctx.rs）。
        let mut game = GameCtx {
            scene: &mut *frame.scene,
            physics: frame.physics.as_deref_mut(),
            input: frame.input,
            audio: frame.audio.as_deref_mut(),
            logs: &mut self.logs,
            commands: &mut self.commands,
            source: path,
            scene_name: frame.scene_name,
            scene_order: frame.scene_order,
            loader: frame.loader,
            clock: frame.clock.as_deref_mut(),
            save: frame.save.as_deref_mut(),
            entity,
            delta: frame.delta,
            time: frame.time,
        };
        let ctx = ScriptCtx::new(&mut game, Some(state));
        // 写进常驻槽、传槽的轻量指针（见 `LuaCtx` 类型文档）——现造 userdata
        // 实测 ~284 ns/次，1000 实体 ×2 钩子 ≈568 µs/帧，方案对比见 PLAN.md 二十。
        let light = LuaCtx::as_light(self.ctx_slot.write(LuaCtx::from_script_ctx(ctx)));

        // 调用钩子。错误（含沙箱超限）记成日志，不 panic、不中断这一帧。
        // 带参数的钩子（碰撞）多传一个"对方实体 id"：Lua 里是普通整数，
        // 与脚本侧 `self_id()` / `find_entity()` 的口径一致。
        let result = {
            let _window = lua_api::ContextWindow::enter(&self.lua, light);
            match arg {
                Some(other) => func.call::<()>((light, other.raw() as i64)),
                None => func.call::<()>(light),
            }
        };
        if let Err(err) = result {
            let key = (entity, path_id, hook as u8);
            let streak = self.error_streak.entry(key).or_insert(0);
            *streak += 1;
            let count = *streak;
            // 指数退避：第 1、2、4、8、16… 次各报一条，中间的静默。100 帧持续
            // 报错也只是 7 条，而 1 次的偶发出错照样看得见。
            if count.is_power_of_two() {
                let described = self.describe_error(&err);
                let message = if count == 1 {
                    described
                } else {
                    format!("{described}（该钩子已连续报错 {count} 次，后续只在 2 的幂次再报）")
                };
                self.logs.push(ScriptLog {
                    level: LogLevel::Error,
                    script: path.to_owned(),
                    entity: Some(entity),
                    message,
                });
            }
        } else if !self.error_streak.is_empty() {
            // 成功执行一次 → 清零。`is_empty` 短路是刻意的：正常运行时这张表
            // 是空的，热路径上不该为"可能没出错"付一次哈希查找。
            self.error_streak.remove(&(entity, path_id, hook as u8));
        }
    }

    // -----------------------------------------------------------------------
    // 命令与日志
    // -----------------------------------------------------------------------

    /// 日志与命令缓冲的直接访问 —— 组件轨与脚本轨共用这两条缓冲。
    pub fn buffers_mut(&mut self) -> (&mut Vec<ScriptLog>, &mut Vec<ScriptCommand>) {
        (&mut self.logs, &mut self.commands)
    }

    /// 取走脚本产生的结构性改动通知（场景已经改完了，这里只处理物理侧）。
    pub fn take_commands(&mut self) -> CommandReport {
        let commands = std::mem::take(&mut self.commands);
        let mut report = CommandReport::default();
        for command in commands {
            match command {
                ScriptCommand::EntitySpawned { id, .. } => report.spawned.push(id),
                ScriptCommand::EntitiesRemoved { entities } => report.removed.extend(entities),
                ScriptCommand::Reparented { child } => report.reparented.push(child),
                ScriptCommand::Relayered { id, layer } => report.relayered.push((id, layer)),
                // 一帧多次切换请求只认最后一个：中间那些场景永远不会被装载，
                // 也就不该产生"装载了又立刻丢弃"的副作用。
                ScriptCommand::LoadScene { path } => report.scene_load = Some(path),
                // 与场景切换同款：一帧只做最后一次请求。
                ScriptCommand::SaveGame { slot } => report.save_game = Some(slot),
                ScriptCommand::LoadGame { slot } => report.load_game = Some(slot),
            }
        }
        report
    }

    /// 取走本帧产生的日志。
    pub fn take_logs(&mut self) -> Vec<ScriptLog> {
        std::mem::take(&mut self.logs)
    }

    /// 用编译好的脚本预加载一个已知路径（编辑器"重新加载脚本"用）。
    pub fn ensure_loaded(&mut self, path: &str) -> bool {
        // 先驻留拿到 id、再转 id 版（`&str` 借的是 self 的一个字段，
        // 没法跨 `&mut self` 方法调用传进去）。
        let path_id = self.intern.intern(path);
        self.ensure_loaded_id(path_id)
    }

    /// [`ensure_loaded`] 的驻留 id 形参版（派发热路径用：队列里只有 id）。
    fn ensure_loaded_id(&mut self, path_id: u32) -> bool {
        // 借 `self.intern`（共享）；下面动的全是别的字段，字段级拆借放行。
        let path = self.intern.resolve(path_id);
        if self.scripts.contains_key(path) {
            return true;
        }
        if self.failed.contains(path) {
            return false;
        }

        let Some(root) = self.root.clone() else {
            self.logs.push(ScriptLog {
                level: LogLevel::Error,
                script: path.to_owned(),
                entity: None,
                message: "Lua 宿主还没有项目根目录，无法解析脚本路径".to_owned(),
            });
            self.failed.insert(path.to_owned());
            return false;
        };

        let full = match xgame_core::resolve_read_path(&root, path, &self.search_paths) {
            Ok(full) => full,
            Err(err) => {
                // 路径门禁：`../..` / 绝对路径 / Windows 不认的写法在这里就挡住，
                // 绝不落到 `fs::read_to_string`。报错里带原因与试过的目录。
                self.logs.push(ScriptLog {
                    level: LogLevel::Error,
                    script: path.to_owned(),
                    entity: None,
                    message: format!("脚本路径不可用 '{path}': {err}"),
                });
                self.failed.insert(path.to_owned());
                return false;
            }
        };
        let source = match std::fs::read_to_string(&full) {
            Ok(source) => source,
            Err(err) => {
                self.logs.push(ScriptLog {
                    level: LogLevel::Error,
                    script: path.to_owned(),
                    entity: None,
                    message: format!("读取脚本失败 '{}': {err}", full.display()),
                });
                self.failed.insert(path.to_owned());
                return false;
            }
        };

        // 每个脚本一张独立的环境表（沙箱边界），`__index` 指向真全局，
        // 这样脚本能读到宿主 API，但写入只落在自己那张表里（脚本间互不污染）。
        let env = match self.lua.create_table() {
            Ok(t) => t,
            Err(err) => {
                self.logs.push(ScriptLog {
                    level: LogLevel::Error,
                    script: path.to_owned(),
                    entity: None,
                    message: format!("创建脚本环境表失败: {err}"),
                });
                self.failed.insert(path.to_owned());
                return false;
            }
        };
        // 真元表而不是"往 env 里塞个叫 __index 的字段"：Lua 只在**元表**里查
        // `__index`，env 里同名字段是普通字段、对读取毫无作用（踩过：脚本里
        // 所有宿主 API 都变成 nil）。metatable.__index = _G 让读未定义名字
        // 回落到真全局（宿主 API 就在那），而写入仍落在 env 自己这张表里
        // （脚本间互不污染）。
        let meta = match self.lua.create_table() {
            Ok(t) => t,
            Err(err) => {
                self.logs.push(ScriptLog {
                    level: LogLevel::Error,
                    script: path.to_owned(),
                    entity: None,
                    message: format!("创建脚本环境元表失败: {err}"),
                });
                self.failed.insert(path.to_owned());
                return false;
            }
        };
        // `set_metatable` 返回 `()`（元表设置本身不会失败），所以先 set 再挂。
        if let Err(err) = meta.set("__index", self.lua.globals()) {
            self.logs.push(ScriptLog {
                level: LogLevel::Error,
                script: path.to_owned(),
                entity: None,
                message: format!("配置脚本环境失败: {err}"),
            });
            self.failed.insert(path.to_owned());
            return false;
        }
        env.set_metatable(Some(meta));
        // 顶层执行也是独立调用，不能继承其他脚本或钩子耗尽的预算。
        self.budget.store(INSTRUCTION_BUDGET, Ordering::Relaxed);

        // 加载并执行。Lua 没有 Rhai 那种"编译成 AST 再单独跑顶层"的两段式，
        // 直接 `load(...).set_environment(env).exec()` 就是"编译 + 跑顶层"。
        // 顶层定义的 `function update(ctx)` 会落进 env（因为 env 是这张 chunk 的 _ENV）。
        if let Err(err) = self
            .lua
            .load(source.as_bytes())
            .set_name(path.to_owned())
            .set_environment(env.clone())
            .exec()
        {
            self.logs.push(ScriptLog {
                level: LogLevel::Error,
                script: path.to_owned(),
                entity: None,
                message: format!("脚本加载失败: {err}"),
            });
            self.failed.insert(path.to_owned());
            return false;
        }

        let funcs = detect_funcs(&env);
        self.scripts
            .insert(path.to_owned(), CompiledScript { funcs });
        true
    }
}

/// 这条脚本路径是不是 Lua（按扩展名判定；非 `.lua` 的路径在
/// [`LuaHost::preload`] / 派发队列里直接跳过）。
fn is_lua(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lua"))
}

/// 收集场景里所有 (实体, Lua 脚本) 对，顺序即执行顺序。
///
/// 路径当场驻留成 `u32` 再进队列（`String` 版每帧每实体克隆一次，
/// 1000 实体 ≈32.6 µs/帧的纯克隆，见 [`PathIntern`] 模块文档）。
fn collect_lua_queue(scene: &Scene, intern: &mut PathIntern) -> Vec<(EntityId, u32)> {
    let mut queue = Vec::new();
    for entity in &scene.entities {
        for path in &entity.scripts {
            if is_lua(path) {
                queue.push((entity.id, intern.intern(path)));
            }
        }
    }
    queue
}

/// 从环境表里取出四个钩子的函数句柄（不存在的为 `None`）。
///
/// 用"这个名字在 env 里是不是函数"而不是"调用失败再看错误"：后者会把
/// "钩子不存在"和"钩子内部报错"混成一类。四个钩子名见 [`crate::Hook::fn_name`]。
fn detect_funcs(env: &Table) -> HookFuncs {
    let grab = |name: &str| match env.raw_get::<Value>(name) {
        // `raw_get` 绕过 `__index`：我们要看的是**这个脚本自己**有没有定义钩子，
        // 而不是"全局有没有一个同名函数"（宿主 API 里没有同名的，但保险起见）。
        Ok(Value::Function(f)) => Some(f),
        _ => None,
    };
    HookFuncs {
        start: grab("start"),
        update: grab("update"),
        fixed_update: grab("fixed_update"),
        collision_enter: grab("on_collision_enter"),
        collision_exit: grab("on_collision_exit"),
        destroy: grab("on_destroy"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::SpawnRequest;
    use xgame_core::Entity;

    /// 建一个用完就删的临时项目目录。
    struct TempProject {
        dir: PathBuf,
    }

    impl TempProject {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "xgame_lua_{tag}_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(dir.join("Scripts")).unwrap();
            Self { dir }
        }

        fn write_script(&self, name: &str, source: &str) -> String {
            let relative = format!("Scripts/{name}.lua");
            std::fs::write(self.dir.join(&relative), source).unwrap();
            relative
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn removed_entity_does_not_run_queued_update() {
        let project = TempProject::new("removed_queue");
        let killer = project.write_script(
            "killer",
            "function update(ctx) destroy(ctx, entity_id(ctx, 'victim')) end",
        );
        let victim = project.write_script(
            "victim",
            "function update(ctx) log(ctx, 'stale update') end",
        );
        let mut scene = scene_with_script(&killer);
        let mut entity = Entity::new("victim");
        entity.scripts.push(victim);
        scene.add_entity(entity);
        let mut host = LuaHost::new();
        host.set_root(&project.dir);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.0, 0.0));
        assert_eq!(scene.entities.len(), 1);
        assert!(
            host.take_logs().is_empty(),
            "已删除实体不能继续执行快照里的 update"
        );
    }

    #[test]
    fn saved_context_is_rejected_outside_hook_window() {
        let project = TempProject::new("inactive_context");
        let script =
            project.write_script("save_ctx", "function update(ctx) _G.saved_ctx = ctx end");
        let mut scene = scene_with_script(&script);
        let mut host = LuaHost::new();
        host.set_root(&project.dir);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.0, 0.0));
        let saved: Value = host.lua.globals().get("saved_ctx").unwrap();
        // 只尝试转换，不调用 GameCtx，旧实现的红灯也不能解引用失效的栈地址。
        assert!(<LuaCtx as mlua::FromLua>::from_lua(saved, &host.lua).is_err());
    }

    #[test]
    fn top_level_budget_is_independent_after_an_exhausted_script() {
        let project = TempProject::new("top_level_budget");
        let bad = project.write_script("exhausted", "while true do end");
        let good = project.write_script(
            "healthy",
            "local sum = 0; for i = 1, 10000 do sum = sum + i end",
        );
        let mut host = LuaHost::new();
        host.set_root(&project.dir);
        assert!(!host.ensure_loaded(&bad));
        assert!(
            host.ensure_loaded(&good),
            "前一个脚本耗尽预算不能污染下一个脚本的顶层执行"
        );
    }

    fn scene_with_script(relative: &str) -> Scene {
        let mut scene = Scene::new("test");
        let mut entity = Entity::new("actor");
        entity.scripts.push(relative.to_owned());
        scene.add_entity(entity);
        scene
    }

    /// 越界脚本路径在**读取之前**就被拒绝：不读盘、不执行，报错说清原因。
    ///
    /// 旧实现是 `root.join(path)`：`../outside.lua` 会真的被读进来并执行 ——
    /// 一份手写 / 下载来的场景就能让运行时跑项目外的代码。`--check` 会点名这种
    /// 引用，但**不跑检查的宿主（独立游戏）照样会读**，所以门禁必须在读取处。
    #[test]
    fn script_path_escape_is_rejected_before_reading() {
        let project = TempProject::new("path_escape");
        // 项目**外**放一份脚本，用 `..` 指过去。
        let outside = project.dir.parent().unwrap().join(format!(
            "xgame_escape_{}_{}.lua",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&outside, "function update(ctx) log(ctx, 'ESCAPED') end").unwrap();
        let relative = format!("../{}", outside.file_name().unwrap().to_string_lossy());

        let mut scene = scene_with_script(&relative);
        let mut host = LuaHost::new();
        host.set_root(&project.dir);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.0, 0.0));

        let logs = host.take_logs();
        assert!(
            logs.iter()
                .any(|l| l.level == LogLevel::Error && l.message.contains("脚本路径不可用")),
            "越界路径必须报「脚本路径不可用」，实际: {logs:?}"
        );
        assert!(
            !logs.iter().any(|l| l.message.contains("ESCAPED")),
            "项目外的脚本一个字都不许执行: {logs:?}"
        );
        std::fs::remove_file(&outside).unwrap();
    }

    /// `search_paths` 是脚本解析的兜底目录：`Scripts/` 之外的共享脚本能被找到。
    #[test]
    fn search_paths_resolve_scripts_outside_scripts_dir() {
        let project = TempProject::new("search_paths");
        std::fs::create_dir_all(project.dir.join("Mods/Shared")).unwrap();
        std::fs::write(
            project.dir.join("Mods/Shared/shared.lua"),
            "function update(ctx) log(ctx, 'SHARED RAN') end",
        )
        .unwrap();

        let mut scene = scene_with_script("shared.lua");
        let mut host = LuaHost::new();
        host.set_root(&project.dir);
        host.set_search_paths(vec!["Mods/Shared".into()]);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.0, 0.0));

        let logs = host.take_logs();
        assert!(
            logs.iter().any(|l| l.message.contains("SHARED RAN")),
            "兜底目录里的脚本应当被执行: {logs:?}"
        );
    }

    /// 越界的 `search_paths` 条目被跳过 —— 它不能把搜索范围扩到项目外。
    #[test]
    fn invalid_search_path_cannot_widen_script_search() {
        let project = TempProject::new("bad_search");
        // 刻意在项目**外**放一份与待加载脚本**同名**的文件：只有真的把 `..`
        // 当成搜索根，它才会被找到。
        let outside = project.dir.parent().unwrap().join("nope.lua");
        std::fs::write(&outside, "function update(ctx) log(ctx, 'ESCAPED') end").unwrap();

        let mut scene = scene_with_script("nope.lua");
        let mut host = LuaHost::new();
        host.set_root(&project.dir);
        host.set_search_paths(vec!["..".into(), "C:/Windows".into()]);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.0, 0.0));

        let logs = host.take_logs();
        assert!(
            logs.iter()
                .any(|l| l.level == LogLevel::Error && l.message.contains("脚本路径不可用")),
            "缺失脚本必须报错（而不是从越界目录里找到）: {logs:?}"
        );
        assert!(
            !logs.iter().any(|l| l.message.contains("ESCAPED")),
            "越界搜索根不得命中: {logs:?}"
        );
        std::fs::remove_file(&outside).unwrap();
    }

    /// 成功执行一次就把报错计数清零 —— 恢复之后再次出错必须重新报。
    ///
    /// 没有这一条，"限流"很容易退化成"每个脚本一辈子只报一次"，那等于把
    /// 修复后复发的问题永久藏起来。
    #[test]
    fn a_recovered_script_reports_its_next_error_again() {
        let project = TempProject::new("error_recover");
        // 第 3 帧故意不报错，其余帧报错。
        let script = project.write_script(
            "flaky",
            "function update(ctx)\n\
             \x20 _G.n = (_G.n or 0) + 1\n\
             \x20 if _G.n == 3 then return end\n\
             \x20 error('boom')\n\
             end",
        );
        let mut scene = scene_with_script(&script);
        let mut host = LuaHost::new();
        host.set_root(&project.dir);

        let mut per_frame = Vec::new();
        for _ in 0..4 {
            host.dispatch_update(FrameCtx::new(&mut scene, 0.0, 0.0));
            per_frame.push(host.take_logs().len());
        }
        assert_eq!(
            per_frame,
            vec![1, 1, 0, 1],
            "第 3 帧成功后计数应清零，第 4 帧的错误要重新报出来（实际 {per_frame:?}）"
        );
    }

    /// 限流后的日志必须说明"还在错、已经错了几次"。
    ///
    /// 只报第 1 次而不带计数的话，用户会以为一次性错误已经好了 —— 持续故障
    /// 反而更难发现，那就是限流的副作用盖过了收益。
    #[test]
    fn throttled_error_says_how_many_times_it_failed() {
        let project = TempProject::new("error_count_in_text");
        let script = project.write_script("broken", "function update(ctx) error('boom') end");
        let mut scene = scene_with_script(&script);
        let mut host = LuaHost::new();
        host.set_root(&project.dir);

        let mut messages = Vec::new();
        for _ in 0..4 {
            host.dispatch_update(FrameCtx::new(&mut scene, 0.0, 0.0));
            for log in host.take_logs() {
                messages.push(log.message);
            }
        }
        assert!(
            messages.iter().any(|m| m.contains("连续报错 2 次")),
            "第 2 次的日志应当带上次数，实际 {messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("连续报错 4 次")),
            "第 4 次的日志应当带上次数，实际 {messages:?}"
        );
        assert!(
            messages.len() < 4,
            "4 帧里应当只报 1/2/4 三次，实际 {messages:?}"
        );
    }

    /// 一个每帧都报错的脚本不能每帧刷一条日志。
    ///
    /// 这是"错误熔断"的最小形态：持续故障只报有限的几次，而不是 60 条/秒。
    /// 不限流的后果有两个，都不轻：日志面板瞬间被淹掉（用户看不到别的问题），
    /// 而且每次 `format!("{err}")` 都在热路径上分配 —— 一个坏脚本就能把稳态
    /// 零分配整个破坏掉。
    ///
    /// 注意**不**做"报错几次就禁用脚本"：那会悄悄改变游戏行为，比刷屏更难查。
    ///
    /// 顺带钉住另一件事：错误**必须至少被看见一次**，否则限流就成了隐瞒。
    #[test]
    fn repeated_script_errors_are_throttled() {
        let project = TempProject::new("error_streak");
        let script = project.write_script("broken", "function update(ctx) error('boom') end");
        let mut scene = scene_with_script(&script);
        let mut host = LuaHost::new();
        host.set_root(&project.dir);

        let mut total = 0;
        for _ in 0..100 {
            host.dispatch_update(FrameCtx::new(&mut scene, 0.0, 0.0));
            total += host.take_logs().len();
        }
        assert!(total > 0, "报错必须至少被记录一次，否则限流等于隐瞒故障");
        assert!(
            total <= 10,
            "持续报错必须被限流：100 帧里报了 {total} 条（应当是指数退避，量级在个位数）"
        );
    }

    /// 失控脚本必须撞上限，而不是把进程内存吃光。
    ///
    /// 用 8 MiB 的限额（生产是 128 MiB）走同一条路径：为了触发上限去真的吃
    /// 128 MiB 会把测试变成内存炸弹，而限额是参数化的，测小的就是测大的。
    ///
    /// 没有这条上限时的结局不是"脚本报错"，是**操作系统 OOM 杀进程** ——
    /// 编辑器里没保存的场景跟着一起没。所以这条测试断言的不是错误文本好看，
    /// 而是"进程还活着且内存被挡住了"。
    #[test]
    fn a_runaway_script_hits_the_memory_limit_instead_of_the_oom_killer() {
        let project = TempProject::new("memory_limit");
        let script = project.write_script(
            "hog",
            // 每次 1 MiB × 32 次 = 32 MiB，是限额的 4 倍。
            "function update(ctx)\n\
             local chunks = {}\n\
             for i = 1, 32 do chunks[i] = string.rep('x', 1024 * 1024) end\n\
             end",
        );
        let mut scene = scene_with_script(&script);
        let limit = 8 * 1024 * 1024;
        let mut host = LuaHost::with_memory_limit(limit);
        host.set_root(&project.dir);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.0, 0.0));

        let logs = host.take_logs();
        assert!(
            logs.iter().any(|log| log.message.contains("内存超限")),
            "必须报「内存超限」而不是让分配过去，实际日志 {logs:?}"
        );
        assert!(
            host.used_memory() <= limit,
            "内存必须被真正挡住：上限 {limit} B，实际 {} B",
            host.used_memory()
        );
    }

    /// 超限只毁掉**那一次调用**，VM 与后续帧照常工作。
    ///
    /// 这条是"上限可用"与"上限有毒"的分界线：如果一次超限之后 VM 就废了，
    /// 作者会看到"改完脚本还是全都不动了"，比 OOM 更难查。
    #[test]
    fn the_vm_recovers_after_a_memory_limit_hit() {
        let project = TempProject::new("memory_recover");
        let script = project.write_script(
            "burst",
            "local frame = 0\n\
             function update(ctx)\n\
             frame = frame + 1\n\
             if frame == 1 then\n\
             local chunks = {}\n\
             for i = 1, 32 do chunks[i] = string.rep('x', 1024 * 1024) end\n\
             else\n\
             log(ctx, 'recovered')\n\
             end\n\
             end",
        );
        let mut scene = scene_with_script(&script);
        let mut host = LuaHost::with_memory_limit(8 * 1024 * 1024);
        host.set_root(&project.dir);

        host.dispatch_update(FrameCtx::new(&mut scene, 0.0, 0.0));
        let first = host.take_logs();
        assert!(
            first.iter().any(|log| log.message.contains("内存超限")),
            "第一帧应当超限，实际 {first:?}"
        );

        host.dispatch_update(FrameCtx::new(&mut scene, 0.0, 0.0));
        let second = host.take_logs();
        assert!(
            second.iter().any(|log| log.message.contains("recovered")),
            "第二帧必须恢复正常执行，实际 {second:?}"
        );
        assert!(
            host.used_memory() < host.memory_limit(),
            "回收之后的占用应当低于上限，实际 {} B",
            host.used_memory()
        );
    }

    #[test]
    fn is_lua_splits_by_extension() {
        assert!(is_lua("Scripts/a.lua"));
        assert!(is_lua("Scripts/a.LUA"), "扩展名大小写不敏感");
        assert!(!is_lua("Scripts/a.rhai"), "非 .lua 扩展名不是 Lua");
        assert!(!is_lua("Scripts/noext"), "无扩展名不是 Lua");
        assert!(!is_lua("Scripts/lua"), "纯名字不是扩展名");
    }

    #[test]
    fn funcs_are_detected_from_env() {
        let host = LuaHost::new();
        let env = host.lua.create_table().unwrap();
        // 只定义两个钩子 + 一个普通函数。
        host.lua
            .load(&b"function start() end function update() end function helper() end"[..])
            .set_name("@cache_probe.lua")
            .set_environment(env.clone())
            .exec()
            .unwrap();
        let funcs = detect_funcs(&env);
        assert!(funcs.start.is_some(), "没识别出 start");
        assert!(funcs.update.is_some(), "没识别出 update");
        assert!(
            funcs.fixed_update.is_none(),
            "没定义 fixed_update 不该被识别"
        );
        assert!(funcs.destroy.is_none(), "没定义 on_destroy 不该被识别");
    }

    /// Lua 侧的材质 API 必须真的接在同名方法上（防止"注册了但没接"）。
    #[test]
    fn material_api_is_reachable_from_lua() {
        let project = TempProject::new("material_api");
        let path = project.write_script(
            "material",
            r#"
            function start(ctx)
                local id = self_id(ctx)
                set_material_asset(ctx, id, "Resources/Materials/Wood.mat")
                set_material(ctx, id, 0.5, 0.6, 0.7, 1.0, 0.25, 0.75)
                log(ctx, material_asset(ctx, id))
            end
            "#,
        );
        let mut scene = scene_with_script(&path);
        let mut host = LuaHost::new();
        let report = host.preload(&project.dir, &scene);
        assert!(report.failed.is_empty(), "编译失败: {:?}", report.failed);
        host.dispatch_start(FrameCtx::new(&mut scene, 0.016, 0.0));

        let entity = &scene.entities[0];
        assert_eq!(
            entity.material_asset.as_deref(),
            Some("Resources/Materials/Wood.mat")
        );
        let material = entity
            .material
            .as_ref()
            .expect("Lua set_material 应写入覆盖");
        assert_eq!(material.base_color, Some([0.5, 0.6, 0.7, 1.0]));
        assert_eq!(material.metallic, Some(0.25));
        assert_eq!(material.roughness, Some(0.75));

        let logs = host.take_logs();
        assert!(
            logs.iter()
                .any(|l| l.message.contains("Resources/Materials/Wood.mat")),
            "material_asset 应能读回 Lua: {logs:?}"
        );
    }

    /// Lua 侧的相机 API：`look_at` 必须真的把 **+Z** 转向目标，
    /// `set_active_camera` 必须是"独占式"切换（否则第二台相机永远轮不到）。
    #[test]
    fn camera_api_is_reachable_from_lua() {
        use xgame_core::Camera;
        use xgame_math::glam::Vec3;

        let project = TempProject::new("camera_api");
        let path = project.write_script(
            "follow_cam",
            r#"
            function start(ctx)
                local me = self_id(ctx)
                -- 先看"哪台是主相机"：应当是第一台（active 默认 true）。
                log(ctx, tostring(active_camera(ctx)))
                -- 独占式切换到自己。
                set_active_camera(ctx, me)
                -- 看向另一个实体（世界坐标）。
                look_at_entity(ctx, entity_id(ctx, "Target"))
                set_camera_fov(ctx, me, 40.0)
                log(ctx, tostring(camera_fov(ctx, me)))
            end
            "#,
        );

        let mut scene = Scene::new("cam_api");
        // 第一台相机：抢在脚本实体之前，用来验证"切换"是真的生效。
        let other = scene.add_entity(Entity::new("Other Cam").with_camera(Camera::default()));
        let mut cam = Entity::new("Follow Cam").with_camera(Camera::default());
        cam.scripts.push(path.clone());
        let me = scene.add_entity(cam);
        // 目标放在世界 +X 方向，相机在原点 → 看向它之后 forward 应变成 +X。
        scene.add_entity(Entity::new("Target").with_transform(
            xgame_core::Transform::from_translation(Vec3::new(10.0, 0.0, 0.0)),
        ));

        let mut host = LuaHost::new();
        let report = host.preload(&project.dir, &scene);
        assert!(report.failed.is_empty(), "编译失败: {:?}", report.failed);
        host.dispatch_start(FrameCtx::new(&mut scene, 0.016, 0.0));

        let me_index = scene.index_of(me).unwrap();
        let other_index = scene.index_of(other).unwrap();
        assert!(
            scene.entities[me_index].camera.as_ref().unwrap().active,
            "set_active_camera 没把目标相机打开"
        );
        assert!(
            !scene.entities[other_index].camera.as_ref().unwrap().active,
            "set_active_camera 必须是独占的：其它相机要关掉"
        );

        let forward = scene.entities[me_index].transform.rotation * Vec3::Z;
        assert!(
            (forward - Vec3::X).length() < 1e-4,
            "look_at_entity 没把 +Z 转向目标，实际 forward = {forward:?}"
        );
        assert_eq!(
            scene.entities[me_index].camera.as_ref().unwrap().fov,
            40.0,
            "set_camera_fov 没落盘"
        );

        let logs: Vec<String> = host.take_logs().into_iter().map(|l| l.message).collect();
        assert!(
            logs.iter().any(|m| m == &other.raw().to_string()),
            "active_camera 应报告第一台相机（切换之前）: {logs:?}"
        );
        assert!(
            logs.iter().any(|m| m == "40.0"),
            "camera_fov 应能读回（Lua 5.4 的 tostring(40.0) = \"40.0\"）: {logs:?}"
        );
    }

    /// Lua 侧的粒子 API：开关 / burst / rate 必须都落在组件上。
    /// burst 特别验一次"可累积"—— 这是脚本手感的来源。
    #[test]
    fn particle_api_is_reachable_from_lua() {
        use xgame_core::ParticleEmitter;

        let project = TempProject::new("particle_api");
        let path = project.write_script(
            "torch",
            r#"
            function start(ctx)
                local id = self_id(ctx)
                log(ctx, tostring(particle_emitting(ctx, id)))
                set_particle_emitting(ctx, id, false)
                set_particle_rate(ctx, id, 30.0)
                particle_burst(ctx, id, 8)
                particle_burst(ctx, id, 8)
            end
            "#,
        );
        let mut scene = scene_with_script(&path);
        scene.entities[0].particle = Some(ParticleEmitter::fire());
        let mut host = LuaHost::new();
        let report = host.preload(&project.dir, &scene);
        assert!(report.failed.is_empty(), "编译失败: {:?}", report.failed);
        host.dispatch_start(FrameCtx::new(&mut scene, 0.016, 0.0));

        let emitter = scene.entities[0].particle.as_ref().expect("发射器应还在");
        assert!(!emitter.emitting, "Lua 关掉了发射");
        assert_eq!(emitter.rate, 30.0);
        assert_eq!(emitter.burst, 16, "两次 burst 必须累积");

        let logs = host.take_logs();
        assert!(
            logs.iter().any(|l| l.message == "true"),
            "particle_emitting 应读回 true: {logs:?}"
        );
    }

    #[test]
    fn script_hooks_run_and_mutate_the_scene() {
        let project = TempProject::new("hooks");
        let path = project.write_script(
            "spin",
            r#"
            function start(ctx) log(ctx, "started") end
            function update(ctx)
                local p = pos(ctx)
                set_pos(ctx, p.x + 1.0, p.y, p.z)
            end
            "#,
        );

        let mut scene = scene_with_script(&path);
        let mut host = LuaHost::new();
        let report = host.preload(&project.dir, &scene);
        assert!(report.failed.is_empty(), "编译失败: {:?}", report.failed);

        host.dispatch_start(FrameCtx::new(&mut scene, 0.016, 0.0));
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));

        assert_eq!(scene.entities[0].transform.position.x, 2.0);

        let logs = host.take_logs();
        assert!(
            logs.iter().any(|l| l.message == "started"),
            "start 里的 log 应当被收上来: {logs:?}"
        );
        assert!(host.take_logs().is_empty());
    }

    /// 脚本调音频 API：命令必须经 port 收上来 —— 这是"脚本层不碰 kira、
    /// 只产生纯数据命令"这条分层承诺的端到端证红（函数未注册时 Lua 报
    /// "attempt to call a nil value"，port 会是空的）。
    #[test]
    fn script_audio_calls_reach_the_port() {
        let project = TempProject::new("audio");
        let path = project.write_script(
            "sfx",
            r#"
            function update(ctx)
                play_sound(ctx, "Resources/Audio/Beep.wav", 0.5, 1.0)
                play_sound_at(ctx, "Resources/Audio/Boom.wav", 1.0, 2.0, 3.0, 0.8)
                stop_sound(ctx, "Resources/Audio/Beep.wav")
                set_listener(ctx, 5.0, 5.0, 5.0, 0.0, 0.0, -1.0)
                set_master_volume(ctx, 0.3)
                -- 音量总线：按名归组 + 改组音量 + 回读（未知总线必须是 nil）
                play_sound_bus(ctx, "Resources/Audio/Song.wav", "bgm", 1.0, 1.0)
                set_bus_volume(ctx, "bgm", 0.4)
                if bus_volume(ctx, "bgm") == nil then
                    error("bus_volume('bgm') 应返回数字")
                end
                if bus_volume(ctx, "nope") ~= nil then
                    error("未知总线的 bus_volume 应返回 nil")
                end
            end
            "#,
        );
        let mut scene = scene_with_script(&path);
        let mut host = LuaHost::new();
        let report = host.preload(&project.dir, &scene);
        assert!(report.failed.is_empty(), "编译失败: {:?}", report.failed);

        let mut port = xgame_core::AudioPort::new();
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0).with_audio(&mut port));

        let errors: Vec<String> = host
            .take_logs()
            .iter()
            .filter(|l| matches!(l.level, crate::context::LogLevel::Error))
            .map(|l| l.message.clone())
            .collect();
        assert!(errors.is_empty(), "脚本调音频 API 不应报错: {errors:?}");

        assert!(
            (port.master_volume() - 0.3).abs() < 1e-6,
            "主音量应落到 port 上"
        );
        assert!(
            (port.bus_volume(xgame_core::AudioBus::Bgm) - 0.4).abs() < 1e-6,
            "总线音量应落到 port 上"
        );
        let batch = port.drain();
        assert_eq!(
            batch.len(),
            4,
            "play×2 + stop + play_sound_bus 应产生四条命令: {batch:?}"
        );
        assert!(
            matches!(&batch[3], xgame_core::AudioCommand::Play(p)
                if p.bus == xgame_core::AudioBus::Bgm && p.clip == "Resources/Audio/Song.wav"),
            "第 4 条应是归入 Bgm 的 Play: {:?}",
            batch[3]
        );
    }

    #[test]
    fn start_runs_once_per_entity() {
        let project = TempProject::new("start_once");
        let path = project.write_script(
            "counter",
            r#"
            function start(ctx) local p = pos(ctx); set_pos(ctx, p.x + 1.0, 0.0, 0.0) end
            "#,
        );

        let mut scene = scene_with_script(&path);
        let mut host = LuaHost::new();
        host.preload(&project.dir, &scene);

        for _ in 0..3 {
            host.dispatch_start(FrameCtx::new(&mut scene, 0.016, 0.0));
        }
        assert_eq!(scene.entities[0].transform.position.x, 1.0);
    }

    /// 顶层 `local` 常量必须在函数体里可见。
    ///
    /// Lua 的 chunk 以 env 为 `_ENV`，顶层 `local SPEED` 是 update 的 upvalue，
    /// 函数体里天然可见。
    #[test]
    fn global_constants_are_visible_inside_functions() {
        let project = TempProject::new("globals");
        let path = project.write_script(
            "globals",
            r#"
            local SPEED = 4.0
            function update(ctx)
                local p = pos(ctx)
                set_pos(ctx, p.x + SPEED, p.y, p.z)
            end
            "#,
        );

        let mut scene = scene_with_script(&path);
        let mut host = LuaHost::new();
        host.preload(&project.dir, &scene);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));

        assert_eq!(
            scene.entities[0].transform.position.x, 4.0,
            "顶层 local 应当能在函数里用"
        );
        assert!(host.logs.is_empty(), "不该有错误日志: {:?}", host.logs);
    }

    /// `state_get_or` / `state_set` 跨帧保持，且**按实体分开**。
    ///
    /// Lua 函数**能**写 upvalue，但这组 API 仍按 `(实体, 脚本)` 分开 ——
    /// upvalue 是脚本级共享的（同一脚本的所有实体看到同一个值），"每实体一份
    /// 血量"这类需求只能走 state_*。两个实体各跑 3 帧，各自都应是 3。
    #[test]
    fn per_entity_state_persists_and_is_isolated() {
        let project = TempProject::new("state");
        let path = project.write_script(
            "state",
            r#"
            function update(ctx)
                local n = state_get_or(ctx, "n", 0.0)
                state_set(ctx, "n", n + 1.0)
                local p = pos(ctx)
                set_pos(ctx, n + 1.0, p.y, p.z)
            end
            "#,
        );

        let mut scene = Scene::new("state");
        for name in ["a", "b"] {
            let mut entity = Entity::new(name);
            entity.scripts.push(path.clone());
            scene.add_entity(entity);
        }

        let mut host = LuaHost::new();
        host.preload(&project.dir, &scene);
        for _ in 0..3 {
            host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));
        }

        assert!(host.logs.is_empty(), "不该有错误日志: {:?}", host.logs);
        assert_eq!(scene.entities[0].transform.position.x, 3.0, "实体 a");
        assert_eq!(scene.entities[1].transform.position.x, 3.0, "实体 b");
    }

    /// 热重载后每实体的状态不能错位（驻留 id 是状态表的键）—— 防的是
    /// "热重载时顺手清空驻留表"这个看似合理的错误实现：清掉之后下一轮按
    /// 新顺序重发 id，两条状态全部落空、从 0 重计，下面"状态应延续"的断言
    /// 就红。详见 [`crate::intern`] 模块文档。
    #[test]
    fn state_survives_hot_reload_even_when_intern_order_changes() {
        let project = TempProject::new("reload_state");
        let path_a = project.write_script(
            "counter_a",
            r#"
            function update(ctx)
                local n = state_get_or(ctx, "n", 0.0)
                local n2 = n + 1.0
                state_set(ctx, "n", n2)
                set_pos(ctx, n2, 0.0, 0.0)
            end
            "#,
        );
        let path_b = project.write_script(
            "counter_b",
            r#"
            function update(ctx)
                local n = state_get_or(ctx, "n", 0.0)
                local n2 = n + 100.0
                state_set(ctx, "n", n2)
                set_pos(ctx, n2, 0.0, 0.0)
            end
            "#,
        );

        let mut scene = Scene::new("reload_state");
        for (name, path) in [("a", &path_a), ("b", &path_b)] {
            let mut entity = Entity::new(name);
            entity.scripts.push(path.clone());
            scene.add_entity(entity);
        }

        let mut host = LuaHost::new();
        host.preload(&project.dir, &scene);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));
        assert_eq!(scene.entities[0].transform.position.x, 1.0, "帧 1：a");
        assert_eq!(scene.entities[1].transform.position.x, 100.0, "帧 1：b");

        // 热重载 + 倒序：下一轮驻留顺序 b 先于 a（id 若被重发就会错位）。
        host.invalidate_all();
        scene.entities.reverse();

        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));
        assert!(host.logs.is_empty(), "重编译不该报错: {:?}", host.logs);
        assert_eq!(
            scene.entities[0].transform.position.x, 200.0,
            "热重载后 b（现在排第一）的状态应延续"
        );
        assert_eq!(
            scene.entities[1].transform.position.x, 2.0,
            "热重载后 a 的状态应延续"
        );
    }

    /// `set_anim_param` 把值写进**自己实体**的机器参数表。
    #[test]
    fn set_anim_param_writes_the_owning_entities_machine() {
        use xgame_core::{AnimMachine, AnimState};

        let project = TempProject::new("anim_param");
        let path = project.write_script(
            "locomotion",
            r#"
            function update(ctx) set_anim_param(ctx, "speed", 2.5) end
            "#,
        );

        let mut machine = AnimMachine {
            entry: "Idle".into(),
            ..Default::default()
        };
        machine.states.push(AnimState {
            name: "Idle".into(),
            ..Default::default()
        });

        let mut scene = Scene::new("anim_param");
        for name in ["with_machine", "without_machine"] {
            let mut entity = Entity::new(name);
            entity.scripts.push(path.clone());
            if name == "with_machine" {
                let mut a = xgame_core::Animator::new("Walk");
                a.machine = Some(machine.clone());
                entity.animator = Some(a);
            }
            scene.add_entity(entity);
        }

        let mut host = LuaHost::new();
        host.preload(&project.dir, &scene);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));
        assert!(host.logs.is_empty(), "不该有错误日志: {:?}", host.logs);

        let m = scene.entities[0]
            .animator
            .as_ref()
            .unwrap()
            .machine
            .as_ref()
            .unwrap();
        assert_eq!(
            m.parameters.get("speed").copied(),
            Some(2.5),
            "带机器的实体应写入参数"
        );
        // 没挂机器的实体静默 no-op —— 脚本照常跑，不报错。
        assert!(scene.entities[1]
            .animator
            .as_ref()
            .is_none_or(|a| a.machine.is_none()));
    }

    /// 动画剪辑的脚本控制：直接换基础剪辑 / 调速度与淡入 / 换叠加层与层栈剪辑。
    ///
    /// 六个函数在旧实现里都不存在 —— 这条测试先红，再补注册与 `GameCtx` 方法。
    #[test]
    fn script_controls_base_clip_additive_and_layers() {
        use xgame_core::{AnimAdditive, AnimLayer};

        let project = TempProject::new("anim_control");
        let path = project.write_script(
            "drive",
            r#"
            function start(ctx)
                set_anim_clip(ctx, "Run")
                set_anim_speed(ctx, 2.0)
                set_anim_looping(ctx, false)
                set_anim_fade_time(ctx, 0.05)
                set_anim_additive_clip(ctx, "Survey")
                set_anim_layer_clip(ctx, "upper", "Run")
            end
            "#,
        );

        let mut scene = Scene::new("anim_control");
        let mut entity = Entity::new("fox");
        entity.scripts.push(path);
        let mut animator = xgame_core::Animator::new("Walk");
        animator.additive = Some(AnimAdditive {
            clip: "Walk".into(),
            ..Default::default()
        });
        animator.layers.push(AnimLayer {
            name: "upper".into(),
            clip: "Walk".into(),
            ..Default::default()
        });
        entity.animator = Some(animator);
        scene.add_entity(entity);

        let mut host = LuaHost::new();
        host.preload(&project.dir, &scene);
        host.dispatch_start(FrameCtx::new(&mut scene, 0.016, 0.0));
        assert!(host.logs.is_empty(), "脚本不该报错: {:?}", host.logs);

        let animator = scene.entities[0].animator.as_ref().expect("animator");
        assert_eq!(animator.clip, "Run", "基础剪辑应被脚本切换");
        assert_eq!(animator.speed, 2.0, "速度应写到自己的 animator");
        assert!(!animator.looping, "循环开关应被脚本改掉");
        assert!(
            (animator.fade_time - 0.05).abs() < 1e-6,
            "淡入时长应被脚本改掉"
        );
        assert_eq!(
            animator.additive.as_ref().unwrap().clip,
            "Survey",
            "叠加层剪辑应被脚本切换"
        );
        assert_eq!(animator.layers[0].clip, "Run", "层栈剪辑应被脚本按层名切换");
    }

    #[test]
    fn script_errors_are_reported_without_killing_the_frame() {
        let project = TempProject::new("runtime_error");
        let path = project.write_script(
            "boom",
            r#"
            function update(ctx) local x = 1 / 0; set_pos(ctx, x, 0.0, 0.0) end
            "#,
        );

        let mut scene = scene_with_script(&path);
        let mut host = LuaHost::new();
        let report = host.preload(&project.dir, &scene);
        // 除以零在 Lua 里是 **inf**（合法值），不是错误 —— 这里换成真正的
        // 运行时错误：调用 nil。
        let _ = report;
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));
        // 上面这句不该 panic；inf 也不该报错（Lua 语义）。
        assert!(host.logs.is_empty(), "除以零不该报错: {:?}", host.logs);
    }

    #[test]
    fn calling_nil_is_a_runtime_error_not_a_panic() {
        let project = TempProject::new("call_nil");
        let path = project.write_script(
            "bad",
            r#"
            function update(ctx) local f = nil; f() end
            "#,
        );

        let mut scene = scene_with_script(&path);
        let mut host = LuaHost::new();
        let report = host.preload(&project.dir, &scene);
        assert!(report.failed.is_empty(), "编译是过的: {:?}", report.failed);

        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));

        let logs = host.take_logs();
        assert!(
            logs.iter().any(|l| l.level == LogLevel::Error),
            "运行时报错应当进日志: {logs:?}"
        );
        // 关键：脚本报错之后，场景依然可读可写（引擎没有被带崩）。
        assert_eq!(scene.entities.len(), 1);
    }

    #[test]
    fn missing_script_is_a_load_failure_not_a_panic() {
        let project = TempProject::new("missing");
        let mut scene = scene_with_script("Scripts/nope.lua");
        let mut host = LuaHost::new();
        let report = host.preload(&project.dir, &scene);
        assert_eq!(report.failed.len(), 1);

        // 再派发也不该 panic，也不该重复报错。
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));
        assert_eq!(host.take_logs().len(), 1);
    }

    /// 沙箱：`io` / `os` / `require` 在裁剪后的标准库里应当是 nil。
    ///
    /// 这是"脚本是用户内容，不该能读写文件 / 起进程"的回归网。
    #[test]
    fn sandbox_removes_dangerous_stdlib() {
        let project = TempProject::new("sandbox");
        let path = project.write_script(
            "probe",
            r#"
            function update(ctx)
                if io ~= nil then error(ctx, "io 泄漏") end
                if os ~= nil then error(ctx, "os 泄漏") end
                if require ~= nil then error(ctx, "require 泄漏") end
                if debug ~= nil then error(ctx, "debug 泄漏") end
                log(ctx, "sandbox ok")
            end
            "#,
        );

        let mut scene = scene_with_script(&path);
        let mut host = LuaHost::new();
        host.preload(&project.dir, &scene);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));

        let logs = host.take_logs();
        assert!(
            !logs.iter().any(|l| l.level == LogLevel::Error),
            "危险标准库应当不可见: {logs:?}"
        );
        assert!(
            logs.iter().any(|l| l.message == "sandbox ok"),
            "脚本应当正常跑完: {logs:?}"
        );
    }

    /// 死循环脚本必须被指令计数沙箱掐断，而不是卡死测试进程。
    #[test]
    fn infinite_loop_is_interrupted_by_the_budget() {
        let project = TempProject::new("loop");
        let path = project.write_script(
            "spin",
            r#"
            function update(ctx) while true do end end
            "#,
        );

        let mut scene = scene_with_script(&path);
        let mut host = LuaHost::new();
        host.preload(&project.dir, &scene);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));

        let logs = host.take_logs();
        assert!(
            logs.iter().any(|l| l.message.contains("指令数超限")),
            "死循环应当被沙箱中断并报错: {logs:?}"
        );
    }

    #[test]
    fn spawn_commands_use_the_preallocated_id() {
        let project = TempProject::new("spawn");
        let path = project.write_script(
            "spawner",
            r#"
            function start(ctx)
                local id = spawn_entity(ctx, "ball", "builtin://sphere")
                set_entity_pos(ctx, id, 3.0, 4.0, 5.0)
            end
            "#,
        );

        let mut scene = scene_with_script(&path);
        let mut host = LuaHost::new();
        let preload = host.preload(&project.dir, &scene);
        assert!(preload.failed.is_empty(), "编译失败: {:?}", preload.failed);
        host.dispatch_start(FrameCtx::new(&mut scene, 0.016, 0.0));

        // 生成是**立即**生效的：脚本里 spawn 之后的每一句都已经能看到新实体。
        let report = host.take_commands();
        assert_eq!(report.spawned.len(), 1);

        let spawned_id = report.spawned[0];
        let index = scene
            .find_entity(spawned_id)
            .expect("返回的 ID 应当就是真实实体 ID");
        assert_eq!(scene.entities[index].name, "ball");
        assert_eq!(scene.entities[index].transform.position.x, 3.0);
        assert_eq!(scene.entities[index].model, "builtin://sphere");

        // 顺带覆盖一下 SpawnRequest 的默认值走法。
        let default_request = SpawnRequest::new("x");
        assert_eq!(default_request.model, "");
    }

    /// 预制体实例化：从 Lua 一路走到场景，并且失败（没装载器）时脚本侧
    /// 拿到的是 `nil` 而不是一个假 id。
    ///
    /// 这里用一个内存假装载器（真路径 `ProjectHandle` 由 runtime 的测试覆盖，
    /// 见 `world.rs` 的同名场景）。
    #[test]
    fn spawn_prefab_is_reachable_from_lua() {
        let project = TempProject::new("spawn_prefab");
        let path = project.write_script(
            "spawner",
            r#"
            function start(ctx)
                id = spawn_prefab(ctx, "Prefabs/Enemy.prefab.ron", 1.0, 2.0, 3.0)
                if id ~= nil then
                    set_entity_pos(ctx, id, 4.0, 5.0, 6.0)
                end
            end
            "#,
        );

        let mut scene = scene_with_script(&path);
        let mut host = LuaHost::new();
        let preload = host.preload(&project.dir, &scene);
        assert!(preload.failed.is_empty(), "编译失败: {:?}", preload.failed);

        // 没注入装载器：失败路径，脚本里 `id ~= nil` 应为假。
        host.dispatch_start(FrameCtx::new(&mut scene, 0.016, 0.0));
        let spawned = host.take_commands().spawned;
        assert!(spawned.is_empty(), "没有装载器时什么都不该生成");
        assert_eq!(scene.entities.len(), 1, "只有原来那个 actor");
        assert!(
            host.take_logs()
                .iter()
                .any(|l| l.level == LogLevel::Error && l.message.contains("装载器")),
            "失败要留下可诊断的日志"
        );

        // 注入装载器：整棵子树进场景，脚本里紧接着那句 set_entity_pos 生效。
        let loader = crate::ctx::tests::FakePrefabLoader {
            prefab: Some(crate::ctx::tests::sample_prefab()),
        };
        let mut scene2 = scene_with_script(&path);
        let mut host2 = LuaHost::new();
        assert!(host2.preload(&project.dir, &scene2).failed.is_empty());
        let frame = FrameCtx::new(&mut scene2, 0.016, 0.0).with_loader(&loader);
        host2.dispatch_start(frame);
        let spawned = host2.take_commands().spawned;
        assert_eq!(spawned.len(), 2, "Root + Child 两条生成命令");
        assert_eq!(scene2.entities.len(), 3, "actor + 两个新实体");
        let root = scene2.find_entity(spawned[0]).expect("根实体应存在");
        assert_eq!(
            scene2.entities[root].transform.position,
            xgame_math::glam::Vec3::new(4.0, 5.0, 6.0),
            "脚本里 spawn 之后的 set_entity_pos 必须当帧生效"
        );
        assert!(scene2.entities[root]
            .prefab
            .as_ref()
            .is_some_and(|p| p.is_root));
    }

    #[test]
    fn destroy_command_removes_the_subtree() {
        let project = TempProject::new("destroy");
        let path = project.write_script(
            "bomb",
            r#"
            function update(ctx)
                local target = entity_id(ctx, "victim")
                if target ~= 0 then destroy(ctx, target) end
            end
            "#,
        );

        let mut scene = Scene::new("test");
        let mut bomb = Entity::new("bomb");
        bomb.scripts.push(path.clone());
        scene.add_entity(bomb);

        let mut victim = Entity::new("victim");
        victim.scripts.clear();
        scene.add_entity(victim);

        let mut host = LuaHost::new();
        host.preload(&project.dir, &scene);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));

        // 删除同样立即生效；报告里带上被删的 ID，运行时据此清刚体。
        let report = host.take_commands();
        assert_eq!(report.removed.len(), 1);
        assert!(scene.find_by_name("victim").is_none());
    }

    /// `time_scale` / `set_time_scale` 必须读写**同一份时钟**（宿主交进来的那份），
    /// 且坏值处理与 `GameClock` 一致：NaN 忽略、负值钳 0。
    ///
    /// 反例是很容易犯的：在脚本层缓一个 `time_scale` 副本，于是"设了暂停、
    /// 下一帧又自己变回 1.0"，或者 `0.0/0.0` 把系数悄悄打回默认。
    #[test]
    fn lua_time_scale_round_trips_through_the_host_clock() {
        let project = TempProject::new("time_scale");
        let path = project.write_script(
            "speed",
            r#"
            function update(ctx)
                if time_scale(ctx) == 1.0 then log(ctx, "default_ok") end
                set_time_scale(ctx, 0.25)
                if time_scale(ctx) == 0.25 then log(ctx, "set_ok") end
                set_time_scale(ctx, 0.0 / 0.0)   -- NaN：必须忽略，保住 0.25
                if time_scale(ctx) == 0.25 then log(ctx, "nan_ignored") end
                set_time_scale(ctx, -5.0)        -- 负值：钳到 0
                if time_scale(ctx) == 0.0 then log(ctx, "neg_clamped") end
            end
            "#,
        );
        let mut scene = scene_with_script(&path);
        let mut host = LuaHost::new();
        let report = host.preload(&project.dir, &scene);
        assert!(report.failed.is_empty(), "编译失败: {:?}", report.failed);

        let mut clock = xgame_core::GameClock::new();
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0).with_clock(&mut clock));

        let logs: Vec<String> = host.take_logs().into_iter().map(|l| l.message).collect();
        for expected in ["default_ok", "set_ok", "nan_ignored", "neg_clamped"] {
            assert!(
                logs.iter().any(|m| m == expected),
                "缺标记 {expected}，实际日志：{logs:?}"
            );
        }
        assert_eq!(clock.time_scale(), 0.0, "最后一次写入应落在宿主时钟上");
    }

    /// 从注册源码里提取 `globals.set("name", ...)` 的名字。
    ///
    /// 不靠运行期枚举全局表：Lua 基础库也有 `error` 等函数，而引擎恰好覆盖了
    /// 同名全局；用集合差分会把被覆盖的名字漏掉。直接读注册源码能精确拿到
    /// 引擎自己注册的 178 个名字。
    fn registered_global_names(source: &str) -> std::collections::BTreeSet<String> {
        let mut names = std::collections::BTreeSet::new();
        let mut rest = source;
        while let Some(position) = rest.find("globals.set(") {
            rest = &rest[position + "globals.set(".len()..];
            let after = rest.trim_start();
            let after = after
                .strip_prefix('"')
                .unwrap_or_else(|| panic!("globals.set 必须使用字符串字面量名: {after:.80}"));
            let end = after
                .find('"')
                .unwrap_or_else(|| panic!("globals.set 名字缺右引号: {after:.80}"));
            let name = &after[..end];
            assert!(!name.is_empty(), "globals.set 名字不能为空");
            assert!(names.insert(name.to_string()), "重复注册全局函数: {name}");
            rest = &after[end + 1..];
        }
        names
    }

    /// 机器可读清单必须与 `lua_api.rs` / `ui_api.rs` 的注册名**逐名一致**。
    ///
    /// 这是清单的防漂移主钉子：新增 / 改名 / 删除脚本 API 时，如果只改注册而
    /// 不改 `xgame-core::script_api`，这里立刻红。运行期再确认每个名字真的
    /// 是 Lua 函数，防止“源码里有、注册失败”的静默漏注册。
    #[test]
    fn script_api_manifest_matches_registered_globals() {
        let source = format!(
            "{}\n{}",
            include_str!("lua_api.rs"),
            include_str!("ui_api.rs")
        );
        let actual = registered_global_names(&source);
        let doc = xgame_core::script_api::script_api();
        let expected: std::collections::BTreeSet<String> =
            doc.function_names().map(str::to_string).collect();

        assert_eq!(
            actual, expected,
            "脚本 API 清单与 lua_api/ui_api 的注册名不一致"
        );
        assert_eq!(
            actual.len(),
            178,
            "注册名数量应为 178（2026-10-30 加手柄 3 个 + 触摸 2 个：touch_count / touch）"
        );

        let host = LuaHost::new();
        let globals = host.lua.globals();
        for name in &actual {
            let value: Value = globals
                .get(name.as_str())
                .unwrap_or_else(|error| panic!("读全局 {name} 失败: {error}"));
            assert!(
                matches!(value, Value::Function(_)),
                "{name} 在 Lua 里不是函数"
            );
        }
    }
}
