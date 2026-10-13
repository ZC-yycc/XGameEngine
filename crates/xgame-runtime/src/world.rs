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
//!   ├─ 消费脚本命令 / sync_bodies
//!   └─ input.end_frame()         清掉"仅本帧有效"的输入边沿
//! ```
//!
//! **动态体由物理驱动，运动学体由脚本驱动** —— 这条规则决定了上面两个方向的
//! 数据流。二者不会互相打架，因为一个刚体只属于其中一类。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use xgame_core::{EntityId, GameClock, PhysicsHandle, Scene, Transform};
use xgame_input::{Input, InputMap};
use xgame_math::glam::{Mat4, Vec3};
use xgame_physics::{body_from_scene, DebugLine, PhysicsConfig, PhysicsWorld};
use xgame_script::{FrameCtx, GameCtx, ScriptHost, ScriptLog};

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

/// 游戏运行时世界。
pub struct World {
    scene: Scene,
    physics: PhysicsWorld,
    input: Input,
    scripts: ScriptHost,
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

        let mut scripts = ScriptHost::new();
        scripts.set_root(root.clone());
        scripts.preload(&root, &scene);

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

    pub fn clock(&self) -> &GameClock {
        &self.clock
    }

    pub fn scripts(&self) -> &ScriptHost {
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
            fixed_steps += 1;
        }
        // 步数上限是"每帧"的预算，必须在整轮固定步长跑完之后再重置。
        self.clock.reset_step_counter();

        // -- 3. update：每渲染帧一次 --
        self.run_dispatch(Phase::Update, frame_dt, time);
        self.consume_script_commands(frame_dt, time);
        self.sync_bodies();

        // -- 4. 输入帧末清理 --
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
            ..
        } = self;

        let frame = FrameCtx::new(scene, delta, time)
            .with_input(input)
            .with_physics(physics);

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
            behavior_instances,
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
                    logs: &mut *logs,
                    commands: &mut *commands,
                    source: instance.name,
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
    fn instantiate_behaviors(&mut self) {
        for entity in &self.scene.entities {
            if entity.behaviors.is_empty() || self.behavior_instances.contains_key(&entity.id) {
                continue;
            }
            let mut instances = Vec::with_capacity(entity.behaviors.len());
            for name in &entity.behaviors {
                match self.behaviors.construct_instance(name) {
                    Some(instance) => instances.push(instance),
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
            ..
        } = self;
        let (logs, commands) = scripts.buffers_mut();
        for mut instance in instances {
            let mut game = GameCtx {
                scene: &mut *scene,
                physics: Some(&mut *physics),
                input: Some(input),
                logs: &mut *logs,
                commands: &mut *commands,
                source: instance.name,
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
            let Some(def) = self.scene.entities[index].physics else {
                continue;
            };
            let id = self.scene.entities[index].id;
            let world = self.scene.world_transform(index);
            let (body, collider) = body_from_scene(&def, world);
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
            .filter(|e| e.physics.is_some())
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
    use xgame_core::{Entity, PhysicsBody};
    use xgame_script::ScriptHost;

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
            dir.join("Scripts/drive.rhai"),
            r#"
            fn start(ctx) { set_pos(ctx, 1.0, 0.0, 0.0); }
            fn update(ctx) { let p = pos(ctx); set_pos(ctx, p.x + 0.1, p.y, p.z); }
            "#,
        )
        .unwrap();

        let mut scene = Scene::new("scripted");
        let mut actor = Entity::new("actor");
        actor.scripts.push("Scripts/drive.rhai".to_owned());
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

        let mut host = ScriptHost::new();
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

        // Spinner 挂了 Spinner.rhai，3 秒之后它的 Y 轴欧拉角应当明显转过。
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
    use std::sync::atomic::{AtomicU32, Ordering};

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
}

