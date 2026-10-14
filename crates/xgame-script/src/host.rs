//! 脚本宿主：编译缓存、生命周期派发、命令回收。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rhai::{Dynamic, Engine, Map, Scope, AST};

use xgame_core::{EntityId, Scene};

use crate::api;
use crate::context::{FrameCtx, LogLevel, ScriptCommand, ScriptCtx, ScriptLog};
use crate::ctx::GameCtx;
use crate::intern::PathIntern;

/// 脚本可以实现的生命周期钩子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    /// 实体进入运行时后调用一次。
    Start,
    /// 每个渲染帧调用一次。
    Update,
    /// 每个固定步长调用一次（物理步进同频）。
    FixedUpdate,
    /// 实体被销毁前调用一次。
    Destroy,
}

impl Hook {
    /// 脚本里对应的函数名。
    pub fn fn_name(self) -> &'static str {
        match self {
            Hook::Start => "start",
            Hook::Update => "update",
            Hook::FixedUpdate => "fixed_update",
            Hook::Destroy => "on_destroy",
        }
    }
}

/// 一个脚本定义了哪些钩子（编译期算一次，之后每帧只查这几个 bool）。
#[derive(Debug, Clone, Copy, Default)]
struct Hooks {
    start: bool,
    update: bool,
    fixed_update: bool,
    destroy: bool,
}

impl Hooks {
    fn has(&self, hook: Hook) -> bool {
        match hook {
            Hook::Start => self.start,
            Hook::Update => self.update,
            Hook::FixedUpdate => self.fixed_update,
            Hook::Destroy => self.destroy,
        }
    }
}

/// 一份编译好的脚本。
///
/// 不存路径了：名字走驻留表（`intern.resolve`），状态表键走驻留 id ——
/// 结构上不再需要第二份 `String` 副本。
struct CompiledScript {
    ast: AST,
    hooks: Hooks,
    /// 脚本**顶层语句**跑完之后的作用域。
    ///
    /// 必须留着它：`call_fn` 只执行函数体，**不会**执行脚本顶层的
    /// `const` / `let`。少了这份 scope，脚本里写
    ///
    /// ```rhai
    /// const SPEED = 5.0;
    /// fn update(ctx) { set_pos(ctx, SPEED * delta(ctx), 0.0, 0.0); }
    /// ```
    ///
    /// 会在函数体里报 "Variable not found: SPEED" —— 而脚本并不会崩，
    /// 只是这个钩子每帧静默失效。这是 Rhai 最容易踩的一个坑，
    /// 引擎侧把它兜住，用户就不用知道"顶层语句要自己先 eval 一遍"这回事了。
    ///
    /// 顺带的语义：顶层 `let` 声明的计数器是**这份脚本共享**的（同一脚本的
    /// 所有实体看到同一个变量），像别的语言里的模块级静态变量。
    scope: Scope<'static>,
}

/// [`ScriptHost::preload`] 的结果。
#[derive(Debug, Clone, Default)]
pub struct PreloadReport {
    pub loaded: Vec<String>,
    pub failed: Vec<String>,
}

/// 这一轮脚本产生的、需要**运行时**接手的事件。
///
/// 场景本身已经被脚本改完了（见 [`ScriptCommand`]）；这份报告只是告诉运行时
/// 哪些实体的物理体需要补建 / 清理 / 重新定位。
#[derive(Debug, Clone, Default)]
pub struct CommandReport {
    /// 新生成的实体（运行时按需建物理体）。
    pub spawned: Vec<EntityId>,
    /// 被移除的实体，含整棵子树。
    pub removed: Vec<EntityId>,
    /// 换了父节点的实体（运行时应把刚体移到新的世界位置）。
    pub reparented: Vec<EntityId>,
}

impl CommandReport {
    /// 这份报告里有没有事情需要运行时处理。
    pub fn is_empty(&self) -> bool {
        self.spawned.is_empty() && self.removed.is_empty() && self.reparented.is_empty()
    }
}

/// Rhai 脚本宿主。
///
/// 一个 `ScriptHost` 对应**一次运行**（编辑器的 Play 会话，或独立游戏的一局）。
/// 它自己编译脚本、缓存 AST、派发生命周期回调，并把脚本请求的结构性改动
/// 攒成命令交给运行时执行。
pub struct ScriptHost {
    engine: Engine,
    /// 项目相对路径 → 编译结果。
    scripts: HashMap<String, CompiledScript>,
    /// 编译失败过的路径。单独记一份，避免每帧对着同一个坏文件反复读盘 + 报错。
    failed: HashSet<String>,
    /// 已经跑过 `start` 的实体。
    started: HashSet<EntityId>,
    /// 每个 (实体, 脚本) 对的持久状态（`state_get` / `state_set` 背后的表）。
    /// 键里的脚本是**驻留 id**（`u32`）：`String` 键意味着每钩子克隆一次路径，
    /// 1000 实体实测 ≈77.8 µs/帧，见 [`PathIntern`] 模块文档。
    states: HashMap<(EntityId, u32), Map>,
    /// 脚本路径驻留表（热路径零 `String` 分配）。
    ///
    /// **永不清空**：状态表按 id 找状态，`invalidate` / `invalidate_all`
    /// 只清编译缓存 —— 驻留表一清，热重载后每实体的状态就静默错位了。
    intern: PathIntern,
    /// 待回收的日志与命令。
    logs: Vec<ScriptLog>,
    commands: Vec<ScriptCommand>,
    /// 脚本路径的解析基准（项目根目录）。
    root: Option<PathBuf>,
}

impl Default for ScriptHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptHost {
    pub fn new() -> Self {
        let mut engine = Engine::new();

        // -- 沙箱限制 --
        //
        // 脚本是**用户内容**，一个死循环就能把游戏卡死。Rhai 的操作计数器
        // 会在上限处抛错并中断这次调用 —— 效果类似"这个脚本这帧没跑"，
        // 而不是整个进程无响应。上限取 20 万：常见脚本一帧只有几百次操作，
        // 留足余量；真跑到 20 万说明它必然写错了。
        engine.set_max_operations(200_000);
        // 递归深度：防止脚本互相调用（或自己调自己）把栈打爆。
        engine.set_max_call_levels(32);
        // 字符串 / 数组大小：防止一个 `while` 里 `push` 出几 GB 数据。
        engine.set_max_string_size(1 << 20);
        engine.set_max_array_size(65_536);

        api::register(&mut engine);

        Self {
            engine,
            scripts: HashMap::new(),
            failed: HashSet::new(),
            started: HashSet::new(),
            states: HashMap::new(),
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

    /// 同一个脚本是否已经编译过（编辑器里给"脚本文件存在但没编过"这类提示用）。
    pub fn is_loaded(&self, path: &str) -> bool {
        self.scripts.contains_key(path)
    }

    /// 丢弃所有编译结果，下次派发时重新读取。
    ///
    /// 热重载的入口：编辑器（或独立游戏的开发模式）在检测到脚本文件变化后调用。
    ///
    /// **不碰驻留表**（[`PathIntern`]）：状态表按驻留 id 找状态，id 一变
    /// 每实体的计时器 / 冷却就静默错位了。回归测试：
    /// `state_survives_hot_reload_even_when_intern_order_changes`。
    pub fn invalidate_all(&mut self) {
        self.scripts.clear();
        self.failed.clear();
    }

    /// 让某个脚本失效（同样不清驻留表，理由见 [`ScriptHost::invalidate_all`]）。
    pub fn invalidate(&mut self, path: &str) {
        self.scripts.remove(path);
        self.failed.remove(path);
    }

    /// 清空**运行时**状态（`start` 是否跑过），保留编译缓存。
    ///
    /// 退出 Play 模式、重新进入时调用：实体的 `start` 应当再跑一遍，
    /// 但脚本文件没必要重新编译。
    pub fn reset_runtime_state(&mut self) {
        self.started.clear();
        self.states.clear();
        self.logs.clear();
        self.commands.clear();
    }

    /// 编译场景里用到的全部 Rhai 脚本（`.lua` 归 Lua 宿主，这里跳过）。
    pub fn preload(&mut self, root: &Path, scene: &Scene) -> PreloadReport {
        self.root = Some(root.to_path_buf());
        let mut report = PreloadReport::default();
        let mut seen: HashSet<&str> = HashSet::new();

        let mut paths: Vec<String> = Vec::new();
        for entity in &scene.entities {
            for path in &entity.scripts {
                if is_rhai(path) && seen.insert(path.as_str()) {
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

    /// 对尚未跑过 `start` 的实体派发一次 `start`（`.lua` 归 Lua 宿主）。
    pub fn dispatch_start(&mut self, mut frame: FrameCtx<'_>) {
        // 队列存驻留 id（`u32`）而不是 `String`：实体一旦跑过 start，这个队列
        // 就不会再建，所以这里省的主要是"首次进入场景那一帧"的分配 ——
        // 但与 `collect_script_queue` 保持同一种载体，免得两处各记一套账。
        let mut queue: Vec<(EntityId, u32)> = Vec::new();
        for entity in &frame.scene.entities {
            if entity.scripts.is_empty() || self.started.contains(&entity.id) {
                continue;
            }
            for path in &entity.scripts {
                if is_rhai(path) {
                    queue.push((entity.id, self.intern.intern(path)));
                }
            }
        }
        // 先把 `started` 全记上再执行：脚本 A 的 `start` 里如果生成了实体，
        // 那个实体应当等下一轮才跑 start（它的脚本可能还没加载完）。
        for (id, _) in &queue {
            self.started.insert(*id);
        }
        for (id, path_id) in queue {
            self.run_hook(&mut frame, id, path_id, Hook::Start);
        }
    }

    /// 派发一帧 `update`。
    pub fn dispatch_update(&mut self, mut frame: FrameCtx<'_>) {
        let queue = collect_script_queue(frame.scene, &mut self.intern);
        for (id, path_id) in queue {
            self.run_hook(&mut frame, id, path_id, Hook::Update);
        }
    }

    /// 派发一次 `fixed_update`（与物理固定步长同频）。
    pub fn dispatch_fixed_update(&mut self, mut frame: FrameCtx<'_>, fixed_dt: f32) {
        frame.delta = fixed_dt;
        let queue = collect_script_queue(frame.scene, &mut self.intern);
        for (id, path_id) in queue {
            self.run_hook(&mut frame, id, path_id, Hook::FixedUpdate);
        }
    }

    /// 在实体被移除**之前**派发 `on_destroy`。
    ///
    /// `scripts` 由调用方传进来：实体一旦从场景里摘掉，它的脚本列表也就没了。
    pub fn dispatch_destroy(&mut self, frame: &mut FrameCtx<'_>, entity: EntityId, scripts: &[String]) {
        for path in scripts {
            if is_rhai(path) {
                let path_id = self.intern.intern(path);
                self.run_hook(frame, entity, path_id, Hook::Destroy);
            }
        }
        self.started.remove(&entity);
    }

    /// 实体离开场景时清掉它的运行时痕迹（`started` 标记与状态表）。
    ///
    /// `EntityId` 不会复用，这里主要是防止长时间运行的游戏把状态表撑大。
    pub fn forget_entity(&mut self, entity: EntityId) {
        self.started.remove(&entity);
        self.states.retain(|(id, _), _| *id != entity);
    }

    /// 派发一个钩子。`path_id` 是 [`PathIntern`] 给的稳定 id（队列里只带它，
    /// 热路径零 `String` 克隆；要 `&str` 的地方用 `intern.resolve` 换回来）。
    fn run_hook(&mut self, frame: &mut FrameCtx<'_>, entity: EntityId, path_id: u32, hook: Hook) {
        // 不能先 `let path = self.intern.resolve(...)` 再传进 `&mut self` 的
        // 方法：那条 `&str` 借的是 self 的一个字段，借用检查器看不出"它就是
        // self 身上的一块"，只会认为整个 self 被借走。所以加载这一步在
        // 方法**内部**自己解析 id；钩子体再单独 resolve 一次（O(1) 下标）。
        if !self.ensure_loaded_id(path_id) {
            return;
        }
        // 字段级拆借：`path` 借 `self.intern`（共享），`script` 借 `self.scripts`
        // （可写），下面还要动 `states` / `logs` / `commands` / `engine` ——
        // 全是互不重叠的字段，编译器放行，所以这条 `&str` 能一路活到钩子跑完。
        let path = self.intern.resolve(path_id);
        let Some(script) = self.scripts.get_mut(path) else {
            return;
        };
        if !script.hooks.has(hook) {
            return;
        }

        // 字段级拆借：scripts（可写）/ engine（可写）/ logs、commands、states
        // （可写）是互不重叠的字段，这里必须一次借清，否则 `&mut self` 的辅助
        // 方法会把整份 self 借走。
        //
        // `GameCtx` 持有 logs / commands 的**真实借用**（活到下面 `call_fn`
        // 返回、`game` 析构为止）；`self.engine` 是另一个字段，同时取它的
        // `&mut` 不冲突。交给解释器的 `ScriptCtx` 则只是 `game` 的地址
        // （见 context.rs 的说明），不会额外延长任何借用。
        //
        // scope 先 take 出来、跑完再放回去：借 `script.scope` 的同时还要借
        // `script.ast`，两个可变/不可变借用叠在一起过不了借用检查。
        let mut scope = std::mem::replace(&mut script.scope, Scope::new());

        // 每个 (实体, 脚本) 一份的状态表。键里的脚本是驻留 id（`u32`）——
        // 以前这里每次克隆一个路径 `String`（1000 实体实测 ≈77.8 µs/帧），
        // 换成 id 后语义不变（id 与路径一一对应、热重载也不变，见 `intern.rs`）。
        let state_key = (entity, path_id);
        let state = self.states.entry(state_key).or_default();

        // 把本次钩子所需的全部状态捆成 GameCtx（API 真相源，见 ctx.rs），
        // 再擦除成地址交给解释器。字段级拆借：frame 的 scene / physics 与
        // self 的 logs / commands / states / scripts / intern 互不重叠，编译器放行。
        let mut game = GameCtx {
            scene: &mut *frame.scene,
            physics: frame.physics.as_deref_mut(),
            input: frame.input,
            logs: &mut self.logs,
            commands: &mut self.commands,
            source: path,
            entity,
            delta: frame.delta,
            time: frame.time,
        };
        let ctx = ScriptCtx::new(&mut game, Some(state));
        let fn_name = hook.fn_name();
        let result = self
            .engine
            .call_fn::<Dynamic>(&mut scope, &script.ast, fn_name, (ctx,));
        script.scope = scope;

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

    /// 日志与命令缓冲的直接访问 —— 组件轨（Rust `Lifecycle`）的分发与脚本轨
    /// 共用这两条缓冲：日志进同一个控制台、结构改动走同一条消费路径。
    pub fn buffers_mut(&mut self) -> (&mut Vec<ScriptLog>, &mut Vec<ScriptCommand>) {
        (&mut self.logs, &mut self.commands)
    }

    /// 取走脚本产生的结构性改动通知（场景已经改完了，这里只处理物理侧）。
    ///
    /// 每帧派发之后调一次，把报告交给运行时。
    pub fn take_commands(&mut self) -> CommandReport {
        let commands = std::mem::take(&mut self.commands);
        let mut report = CommandReport::default();
        for command in commands {
            match command {
                ScriptCommand::EntitySpawned { id, .. } => report.spawned.push(id),
                ScriptCommand::EntitiesRemoved { ids } => report.removed.extend(ids),
                ScriptCommand::Reparented { child } => report.reparented.push(child),
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
        // 先驻留拿到 id、再转 id 版：`ensure_loaded_id` 要在方法**体内**借
        // `self.intern` 解析路径，从外面把那条 `&str` 递进去会与 `&mut self`
        // 打架（借用检查器看不出"它就是 self 身上的一块"）。
        let path_id = self.intern.intern(path);
        self.ensure_loaded_id(path_id)
    }

    /// [`ensure_loaded`] 的驻留 id 形参版（派发热路径用：队列里只有 id）。
    fn ensure_loaded_id(&mut self, path_id: u32) -> bool {
        // 借 `self.intern`（共享）；下面动的全是别的字段（scripts / failed /
        // logs / engine / root），字段级拆借放行。
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
                message: "脚本宿主还没有项目根目录，无法解析脚本路径".to_owned(),
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

        match self.engine.compile(&source) {
            Ok(ast) => {
                let hooks = collect_hooks(&ast);

                // 先跑一遍顶层语句：把 `const` / `let` 装进 scope，
                // 函数体里才引用得到（原因见 `CompiledScript::scope`）。
                let mut scope = Scope::new();
                if let Err(err) = self
                    .engine
                    .eval_ast_with_scope::<Dynamic>(&mut scope, &ast)
                {
                    self.logs.push(ScriptLog {
                        level: LogLevel::Error,
                        script: path.to_owned(),
                        entity: None,
                        message: format!("脚本顶层语句执行失败: {err}"),
                    });
                    self.failed.insert(path.to_owned());
                    return false;
                }

                self.scripts.insert(
                    path.to_owned(),
                    CompiledScript {
                        ast,
                        hooks,
                        scope,
                    },
                );
                true
            }
            Err(err) => {
                self.logs.push(ScriptLog {
                    level: LogLevel::Error,
                    script: path.to_owned(),
                    entity: None,
                    message: format!("脚本编译失败: {err}"),
                });
                self.failed.insert(path.to_owned());
                false
            }
        }
    }
}

/// 收集场景里所有 (实体, Rhai 脚本) 对，顺序即执行顺序。
///
/// `.lua` 归 Lua 宿主（见 [`crate::lua_host::is_lua`]）—— 不在这里过滤的话，
/// 混挂场景里每个 `.lua` 都会被 Rhai 当成语法错误，每帧刷一条"脚本编译失败"。
///
/// 路径当场驻留成 `u32` 再进队列：`String` 版每帧每实体克隆一次路径，
/// 1000 实体实测比驻留贵 ≈32.6 µs/帧（见 [`PathIntern`] 模块文档）。
fn collect_script_queue(scene: &Scene, intern: &mut PathIntern) -> Vec<(EntityId, u32)> {
    let mut queue = Vec::new();
    for entity in &scene.entities {
        for path in &entity.scripts {
            if is_rhai(path) {
                queue.push((entity.id, intern.intern(path)));
            }
        }
    }
    queue
}

/// 这条脚本路径归 Rhai 宿主管（`.lua` 的否定）—— 两轨合起来恰好覆盖全部脚本。
fn is_rhai(path: &str) -> bool {
    !crate::lua_host::is_lua(path)
}

/// 从编译结果里找出定义了哪些钩子。
///
/// 用 AST 的元数据而不是"调用失败再看错误"：后者会把"钩子不存在"和
/// "钩子内部调用了不存在的函数"混成一类，前者是正常的，后者必须报错。
fn collect_hooks(ast: &AST) -> Hooks {
    let mut hooks = Hooks::default();
    for f in ast.iter_functions() {
        if f.access.is_private() {
            continue;
        }
        match f.name {
            "start" => hooks.start = true,
            "update" => hooks.update = true,
            "fixed_update" => hooks.fixed_update = true,
            "on_destroy" => hooks.destroy = true,
            _ => {}
        }
    }
    hooks
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
                "xgame_script_{tag}_{}_{}",
                std::process::id(),
                // 同一进程里多个测试并行跑，用纳秒时间戳区分目录。
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(dir.join("Scripts")).unwrap();
            Self { dir }
        }

        fn write_script(&self, name: &str, source: &str) -> String {
            let relative = format!("Scripts/{name}.rhai");
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

    /// 按扩展名分流：Rhai 宿主**不碰** `.lua`。
    ///
    /// 双轨计划要求"两轨按扩展名分流，同一个 `Entity::scripts` 可以混挂"。
    /// 反向（Lua 宿主跳过 `.rhai`）已有 `is_lua` 保证；这边若不过滤，混挂场景里
    /// 每个 `.lua` 都会被 Rhai 当成语法错误 → 每帧一条"脚本编译失败"日志，
    /// 且 preload 把它们记进 `failed`（正常的 Lua 脚本被打成"坏脚本"）。
    #[test]
    fn rhai_host_skips_lua_scripts() {
        let project = TempProject::new("rhai_skip_lua");
        // 真写一个 `.lua` 文件（内容按 Rhai 语法就是错的，正好验"不该被它编译"）。
        let lua = "Scripts/other.lua";
        std::fs::write(
            project.dir.join(lua),
            "function update(ctx) end",
        )
        .unwrap();

        let mut scene = scene_with_script(&project.write_script("ok", "fn update(ctx) {}"));
        scene.entities[0].scripts.push(lua.to_owned());

        let mut host = ScriptHost::new();
        let report = host.preload(&project.dir, &scene);

        assert!(
            report.failed.is_empty(),
            "`.lua` 不该被 Rhai 宿主当成编译失败: {:?}",
            report.failed
        );
        assert!(
            !report.loaded.iter().any(|p| p.ends_with(".lua")),
            "`.lua` 不该进 Rhai 的 loaded: {:?}",
            report.loaded
        );

        // 派发也不该产生任何错误日志（否则混挂场景每帧刷屏）。
        let mut scene = scene_with_script(&project.write_script("ok2", "fn update(ctx) {}"));
        scene.entities[0].scripts.push(lua.to_owned());
        let mut host = ScriptHost::new();
        host.preload(&project.dir, &scene);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));
        assert!(
            host.logs.is_empty(),
            "Rhai 派发不该碰 `.lua`，日志应为空: {:?}",
            host.logs
        );
    }

    #[test]
    fn hooks_are_detected_from_the_ast() {
        let host = ScriptHost::new();
        let ast = host
            .engine
            .compile("fn start(ctx) {} fn update(ctx) {} fn helper() {}")
            .unwrap();
        let hooks = collect_hooks(&ast);
        assert!(hooks.start, "没识别出 start");
        assert!(hooks.update, "没识别出 update");
        assert!(!hooks.destroy);

        // 也确认一下 Rhai 到底报告了什么（失败时能一眼看出名字大小写之类的问题）。
        let names: Vec<String> = ast.iter_functions().map(|f| f.name.to_owned()).collect();
        assert!(names.contains(&"start".to_owned()), "AST 里的函数: {names:?}");
    }

    #[test]
    fn script_hooks_run_and_mutate_the_scene() {
        let project = TempProject::new("hooks");
        let path = project.write_script(
            "spin",
            r#"
            fn start(ctx) { log(ctx, "started"); }
            fn update(ctx) {
                let p = pos(ctx);
                set_pos(ctx, p.x + 1.0, p.y, p.z);
            }
            "#,
        );

        let mut scene = scene_with_script(&path);
        let mut host = ScriptHost::new();
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
        // 第二次 take 应当是空的（日志确实被取走了，不是复制一份）。
        assert!(host.take_logs().is_empty());
    }

    #[test]
    fn start_runs_once_per_entity() {
        let project = TempProject::new("start_once");
        let path = project.write_script(
            "counter",
            r#"
            fn start(ctx) { let p = pos(ctx); set_pos(ctx, p.x + 1.0, 0.0, 0.0); }
            "#,
        );

        let mut scene = scene_with_script(&path);
        let mut host = ScriptHost::new();
        host.preload(&project.dir, &scene);

        for _ in 0..3 {
            host.dispatch_start(FrameCtx::new(&mut scene, 0.016, 0.0));
        }
        assert_eq!(scene.entities[0].transform.position.x, 1.0);
    }

    /// 顶层 `const` 必须在函数体里可见。
    ///
    /// 这条曾经是坏的：`call_fn` 不会执行脚本顶层语句，常量根本没进作用域，
    /// 于是 `const SPEED = 5.0;` + 函数里用 `SPEED` 会报 "Variable not found"，
    /// 而脚本又不会崩 —— 只是静默失效。`ScriptHost` 现在会先
    /// `eval_ast_with_scope` 一遍顶层，把 scope 留住。
    #[test]
    fn global_constants_are_visible_inside_functions() {
        let project = TempProject::new("globals");
        let path = project.write_script(
            "globals",
            r#"
            const SPEED = 4.0;
            fn update(ctx) {
                let p = pos(ctx);
                set_pos(ctx, p.x + SPEED, p.y, p.z);
            }
            "#,
        );

        let mut scene = scene_with_script(&path);
        let mut host = ScriptHost::new();
        host.preload(&project.dir, &scene);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));

        assert_eq!(
            scene.entities[0].transform.position.x, 4.0,
            "顶层 const 应当能在函数里用"
        );
        assert!(host.logs.is_empty(), "不该有错误日志: {:?}", host.logs);
    }

    /// `state_get_or` / `state_set` 要能跨帧保持，而且要**按实体分开**。
    ///
    /// 为什么必须有这套 API：Rhai 的函数不能写外层作用域，顶层 `let counter = 0;`
    /// 在函数里 `counter += 1` 会直接报错。脚本的"计时器 / 冷却 / 血量"没有别的
    /// 落地方式，所以状态由引擎侧按 `(实体, 脚本)` 托管。
    ///
    /// 两个实体各跑 3 帧：如果状态是脚本级共享的，A 会走到 5、B 走到 6；
    /// 各自都是 3 才说明是按实体分开的。
    #[test]
    fn per_entity_state_persists_and_is_isolated() {
        let project = TempProject::new("state");
        let path = project.write_script(
            "state",
            r#"
            fn update(ctx) {
                let n = state_get_or(ctx, "n", 0.0);
                state_set(ctx, "n", n + 1.0);
                let p = pos(ctx);
                set_pos(ctx, n + 1.0, p.y, p.z);
            }
            "#,
        );

        let mut scene = Scene::new("state");
        for name in ["a", "b"] {
            let mut entity = Entity::new(name);
            entity.scripts.push(path.clone());
            scene.add_entity(entity);
        }

        let mut host = ScriptHost::new();
        host.preload(&project.dir, &scene);
        for _ in 0..3 {
            host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));
        }

        assert!(host.logs.is_empty(), "不该有错误日志: {:?}", host.logs);
        assert_eq!(scene.entities[0].transform.position.x, 3.0, "实体 a");
        assert_eq!(scene.entities[1].transform.position.x, 3.0, "实体 b");
    }

    /// 热重载后每实体的状态不能错位（驻留 id 是状态表的键）。
    ///
    /// 场景里 a 挂 `counter_a.rhai`（计数 +1）、b 挂 `counter_b.rhai`（计数 +100）。
    /// 跑一帧后 `invalidate_all` 并**把实体顺序倒过来** —— 下一轮的驻留顺序
    /// 随之改变（b 的脚本先被驻留）。状态必须还跟着"实体 + 自己的脚本"走：
    /// a 应当是 2、b 应当是 200。
    ///
    /// 红对照是**热重载时顺手清空驻留表**这个看似合理的实现：清掉之后 b 先
    /// 被驻留成 id 0，两条状态全部落空、各自从 0 重计（a=1、b=100），这条
    /// 断言就红。状态错位是**静默**的（键的类型两版一样，错了无从发现），
    /// 所以必须靠测试兜底；它是 `PathIntern` "表永不清空"约定的执行者。
    #[test]
    fn state_survives_hot_reload_even_when_intern_order_changes() {
        let project = TempProject::new("reload_state");
        let path_a = project.write_script(
            "counter_a",
            r#"
            fn update(ctx) {
                let n = state_get_or(ctx, "n", 0.0);
                let n2 = n + 1.0;
                state_set(ctx, "n", n2);
                set_pos(ctx, n2, 0.0, 0.0);
            }
            "#,
        );
        let path_b = project.write_script(
            "counter_b",
            r#"
            fn update(ctx) {
                let n = state_get_or(ctx, "n", 0.0);
                let n2 = n + 100.0;
                state_set(ctx, "n", n2);
                set_pos(ctx, n2, 0.0, 0.0);
            }
            "#,
        );

        let mut scene = Scene::new("reload_state");
        for (name, path) in [("a", &path_a), ("b", &path_b)] {
            let mut entity = Entity::new(name);
            entity.scripts.push(path.clone());
            scene.add_entity(entity);
        }

        let mut host = ScriptHost::new();
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
            fn update(ctx) { set_anim_param(ctx, "speed", 2.5); }
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

        let mut host = ScriptHost::new();
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

    /// `set_anim_additive_weight` 写叠加层权重并钳位；没配叠加层 → 静默 no-op。
    #[test]
    fn set_anim_additive_weight_writes_and_clamps() {
        use xgame_core::AnimAdditive;

        let project = TempProject::new("additive_weight");
        let path = project.write_script(
            "hitshake",
            r#"
            fn update(ctx) { set_anim_additive_weight(ctx, 3.0); }
            "#,
        );

        let mut scene = Scene::new("additive_weight");
        let mut with = Entity::new("with_layer");
        with.scripts.push(path.clone());
        let mut a = xgame_core::Animator::new("Walk");
        a.additive = Some(AnimAdditive {
            clip: "Run".into(),
            ..Default::default()
        });
        with.animator = Some(a);
        scene.add_entity(with);
        let mut without = Entity::new("without_layer");
        without.scripts.push(path.clone());
        scene.add_entity(without);

        let mut host = ScriptHost::new();
        host.preload(&project.dir, &scene);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));
        assert!(host.logs.is_empty(), "不该有错误日志: {:?}", host.logs);

        let add = scene.entities[0]
            .animator
            .as_ref()
            .unwrap()
            .additive
            .as_ref()
            .unwrap();
        assert!(
            (add.weight - 1.0).abs() < 1e-6,
            "权重应钳在 1.0，实得 {}",
            add.weight
        );
        assert!(
            scene.entities[1]
                .animator
                .as_ref()
                .is_none_or(|a2| a2.additive.is_none()),
            "没配叠加层的实体静默 no-op"
        );
    }

    /// `set_anim_layer_weight` 按层名写权重并钳位；重名取第一个命中；
    /// 未知名字 / 没挂 animator → 静默 no-op。
    #[test]
    fn set_anim_layer_weight_matches_by_name_and_clamps() {
        use xgame_core::AnimLayer;

        let project = TempProject::new("layer_weight");
        let path = project.write_script(
            "upper",
            r#"
            fn update(ctx) {
                set_anim_layer_weight(ctx, "upper", 3.0);
                set_anim_layer_weight(ctx, "ghost", 0.5);   // 不存在的层：静默
            }
            "#,
        );

        let mut with = Entity::new("with_layers");
        with.scripts.push(path.clone());
        let mut a = xgame_core::Animator::new("Walk");
        a.layers = vec![
            AnimLayer {
                name: "legs".into(),
                weight: 0.4,
                ..Default::default()
            },
            AnimLayer {
                name: "upper".into(),
                weight: 0.1,
                ..Default::default()
            },
            AnimLayer {
                name: "upper".into(), // 重名：不该被改到
                weight: 0.9,
                ..Default::default()
            },
        ];
        with.animator = Some(a);
        let mut scene = Scene::new("layer_weight");
        scene.add_entity(with);
        let mut without = Entity::new("without_layers");
        without.scripts.push(path.clone());
        scene.add_entity(without);

        let mut host = ScriptHost::new();
        host.preload(&project.dir, &scene);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));
        assert!(host.logs.is_empty(), "不该有错误日志: {:?}", host.logs);

        let layers = scene.entities[0].animator.as_ref().unwrap().layers.as_slice();
        assert_eq!(layers[0].name, "legs", "别的层不该被误写");
        assert!((layers[0].weight - 0.4).abs() < 1e-6, "别的层权重保持");
        assert!(
            (layers[1].weight - 1.0).abs() < 1e-6,
            "同名第一命中应钳在 1.0，实得 {}",
            layers[1].weight
        );
        assert!(
            (layers[2].weight - 0.9).abs() < 1e-6,
            "重名的第二个不该被写"
        );
        assert!(
            scene.entities[1]
                .animator
                .as_ref()
                .is_none_or(|a2| a2.layers.is_empty()),
            "没配层栈的实体静默 no-op"
        );
    }

    #[test]
    fn script_errors_are_reported_without_killing_the_frame() {
        let project = TempProject::new("runtime_error");
        let path = project.write_script(
            "boom",
            r#"
            fn update(ctx) { let x = 1 / 0; set_pos(ctx, x, 0.0, 0.0); }
            "#,
        );

        let mut scene = scene_with_script(&path);
        let mut host = ScriptHost::new();
        let report = host.preload(&project.dir, &scene);
        // 除以零在 Rhai 里是**运行时**错误，编译是过的。
        assert!(report.failed.is_empty());

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
        let mut scene = scene_with_script("Scripts/nope.rhai");
        let mut host = ScriptHost::new();
        let report = host.preload(&project.dir, &scene);
        assert_eq!(report.failed.len(), 1);

        // 再派发也不该 panic，也不该重复报错。
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));
        assert_eq!(host.take_logs().len(), 1);
    }

    #[test]
    fn spawn_commands_use_the_preallocated_id() {
        let project = TempProject::new("spawn");
        let path = project.write_script(
            "spawner",
            r#"
            fn start(ctx) {
                let id = spawn_entity(ctx, "ball", "builtin://sphere");
                set_entity_pos(ctx, id, 3.0, 4.0, 5.0);
            }
            "#,
        );

        let mut scene = scene_with_script(&path);
        let mut host = ScriptHost::new();
        let preload = host.preload(&project.dir, &scene);
        assert!(preload.failed.is_empty(), "编译失败: {:?}", preload.failed);
        host.dispatch_start(FrameCtx::new(&mut scene, 0.016, 0.0));

        // 生成是**立即**生效的：脚本里 spawn 之后的每一句都已经能看到新实体。
        let report = host.take_commands();
        assert_eq!(report.spawned.len(), 1);

        let spawned_id = report.spawned[0];
        let index = scene.find_entity(spawned_id).expect("返回的 ID 应当就是真实实体 ID");
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
            fn update(ctx) {
                let target = entity_id(ctx, "victim");
                if target != 0 { destroy(ctx, target); }
            }
            "#,
        );

        let mut scene = Scene::new("test");
        let mut bomb = Entity::new("bomb");
        bomb.scripts.push(path.clone());
        scene.add_entity(bomb);

        let mut victim = Entity::new("victim");
        victim.scripts.clear();
        scene.add_entity(victim);

        let mut host = ScriptHost::new();
        host.preload(&project.dir, &scene);
        host.dispatch_update(FrameCtx::new(&mut scene, 0.016, 0.0));

        // 删除同样立即生效；报告里带上被删的 ID，运行时据此清刚体。
        let report = host.take_commands();
        assert_eq!(report.removed.len(), 1);
        assert!(scene.find_by_name("victim").is_none());
    }
}
