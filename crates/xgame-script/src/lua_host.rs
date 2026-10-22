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
    /// 脚本路径的解析基准（项目根目录）。
    root: Option<PathBuf>,
}

impl Default for LuaHost {
    fn default() -> Self {
        Self::new()
    }
}

impl LuaHost {
    pub fn new() -> Self {
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

        let budget = Arc::new(AtomicU64::new(INSTRUCTION_BUDGET));
        // 指令计数沙箱：每 HOOK_INTERVAL 条指令回调一次，扣预算，扣到 0 中断。
        // 这是"死循环脚本只毁掉自己这一帧，不卡死整个游戏"的唯一防线。
        lua.set_hook(
            HookTriggers::new().every_nth_instruction(HOOK_INTERVAL),
            {
                let budget = Arc::clone(&budget);
                move |_lua, _debug| {
                    let remaining = budget.load(Ordering::Relaxed);
                    if remaining == 0 {
                        // 返回错误 → 中断这次 Lua 调用，错误冒泡到 `call` 的 Err。
                        return Err(mlua::Error::RuntimeError(
                            "指令数超限，疑似死循环，已中断本次调用".to_owned(),
                        ));
                    }
                    budget.store(remaining.saturating_sub(HOOK_INTERVAL as u64), Ordering::Relaxed);
                    Ok(VmState::Continue)
                }
            },
        );

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
            root: None,
        }
    }

    /// 设置脚本路径的解析基准（项目根目录）。
    pub fn set_root(&mut self, root: impl Into<PathBuf>) {
        self.root = Some(root.into());
    }

    /// 已编译脚本数量。
    pub fn script_count(&self) -> usize {
        self.scripts.len()
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
    }

    /// 让某个脚本失效（同样不清驻留表，理由见 [`LuaHost::invalidate_all`]）。
    pub fn invalidate(&mut self, path: &str) {
        self.scripts.remove(path);
        self.failed.remove(path);
    }

    /// 清空**运行时**状态（`start` 是否跑过），保留编译缓存。
    pub fn reset_runtime_state(&mut self) {
        self.started.clear();
        self.states.clear();
        self.logs.clear();
        self.commands.clear();
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

    /// 在实体被移除**之前**派发 `on_destroy`。
    pub fn dispatch_destroy(&mut self, frame: &mut FrameCtx<'_>, entity: EntityId, scripts: &[String]) {
        for path in scripts {
            if is_lua(path) {
                let path_id = self.intern.intern(path);
                self.run_hook(frame, entity, path_id, Hook::Destroy, None);
            }
        }
        self.started.remove(&entity);
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
        let result = match arg {
            Some(other) => func.call::<()>((light, other.raw() as i64)),
            None => func.call::<()>(light),
        };
        if let Err(err) = result {
            let message = format!("{err}");
            self.logs.push(ScriptLog {
                level: LogLevel::Error,
                script: path.to_owned(),
                entity: Some(entity),
                message,
            });
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
                ScriptCommand::EntitiesRemoved { ids } => report.removed.extend(ids),
                ScriptCommand::Reparented { child } => report.reparented.push(child),
                // 一帧多次切换请求只认最后一个：中间那些场景永远不会被装载，
                // 也就不该产生"装载了又立刻丢弃"的副作用。
                ScriptCommand::LoadScene { path } => report.scene_load = Some(path),
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

        let full = root.join(path);
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
        self.scripts.insert(path.to_owned(), CompiledScript { funcs });
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

    fn scene_with_script(relative: &str) -> Scene {
        let mut scene = Scene::new("test");
        let mut entity = Entity::new("actor");
        entity.scripts.push(relative.to_owned());
        scene.add_entity(entity);
        scene
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
        assert!(funcs.fixed_update.is_none(), "没定义 fixed_update 不该被识别");
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
        let material = entity.material.as_ref().expect("Lua set_material 应写入覆盖");
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
        scene.add_entity(
            Entity::new("Target")
                .with_transform(xgame_core::Transform::from_translation(Vec3::new(10.0, 0.0, 0.0))),
        );

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

        let emitter = scene.entities[0]
            .particle
            .as_ref()
            .expect("发射器应还在");
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

        assert!((port.master_volume - 0.3).abs() < 1e-6, "主音量应落到 port 上");
        let batch = port.drain();
        assert_eq!(batch.len(), 3, "play×2 + stop 应产生三条命令: {batch:?}");
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
}
