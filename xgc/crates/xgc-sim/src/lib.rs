//! XGC 最小帧循环：按帧应用命令、跑系统、逐帧记录 revision / state hash。
//!
//! 执行顺序只认 [`Schedule::plan`]：系统按注册顺序连续分批（同批两两无访问
//! 冲突），最后一段是宿主帧末阶段（脚本等 `!Send` 状态）。跨系统并行还差世界
//! 所有权拆分，同批系统当前仍串行。计划之前还有一段**帧首准备**
//! （[`Schedule::prepare`]）：只读世界、不改语义，用来把派生的运行时缓存
//! （刚体表等）建到与当前场景一致。确定性契约先钉住：同一份 Scenario（初始
//! WorldData + 每帧命令 + fixed_dt）跑两遍，trace 必须逐帧一致。

#![deny(unsafe_code)]

use serde::{Deserialize, Serialize};
use xgc_base::Diagnostic;
use xgc_world::{Command, ComponentId, UiInput, World, WorldData};

pub const TRACE_FORMAT_VERSION: u32 = 1;

/// 默认系统名；Agent 能力清单直接复用，避免手写两份。
pub fn default_system_names() -> Vec<&'static str> {
    Schedule::default_schedule(1).names()
}

fn default_fixed_dt() -> f32 {
    1.0 / 60.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameCommands {
    pub frame: u64,
    pub commands: Vec<Command>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    pub initial: WorldData,
    pub frames: u64,
    #[serde(default = "default_fixed_dt")]
    pub fixed_dt: f32,
    pub commands: Vec<FrameCommands>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameRecord {
    pub frame: u64,
    pub revision: u64,
    pub state_hash: u64,
    pub entities: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    pub format_version: u32,
    pub frames: Vec<FrameRecord>,
}

impl Trace {
    fn new() -> Self {
        Self {
            format_version: TRACE_FORMAT_VERSION,
            frames: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub enum SimError {
    Restore(String),
    InvalidPlan {
        frame: u64,
        diagnostics: Vec<Diagnostic>,
    },
    Apply {
        frame: u64,
        diagnostics: Vec<Diagnostic>,
    },
    InvalidScenario(String),
    TraceMismatch {
        frame: u64,
        expected_hash: u64,
        actual_hash: u64,
    },
}

impl std::fmt::Display for SimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Restore(message) => write!(f, "初始世界恢复失败：{message}"),
            Self::InvalidPlan { frame, diagnostics } => {
                write!(f, "第 {frame} 帧计划无效：{diagnostics:?}")
            }
            Self::Apply { frame, diagnostics } => {
                write!(f, "第 {frame} 帧应用失败：{diagnostics:?}")
            }
            Self::InvalidScenario(message) => write!(f, "场景无效：{message}"),
            Self::TraceMismatch {
                frame,
                expected_hash,
                actual_hash,
            } => write!(
                f,
                "第 {frame} 帧 state hash 不一致：期望 {expected_hash:016x}，实际 {actual_hash:016x}"
            ),
        }
    }
}

impl std::error::Error for SimError {}

/// 系统声明的读写集。计划分批用它判冲突；跨系统并行还差世界所有权拆分，
/// 同批系统当前仍按注册顺序串行执行——批是将来并行的确定性输入。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemAccess {
    pub reads: Vec<ComponentId>,
    pub writes: Vec<ComponentId>,
}

impl SystemAccess {
    /// 保守冲突判定：任一方写集合与另一方读 / 写集合相交即冲突；
    /// 访问集为空的系统按“什么都能碰”处理。
    pub fn conflicts_with(&self, other: &Self) -> bool {
        if (self.reads.is_empty() && self.writes.is_empty())
            || (other.reads.is_empty() && other.writes.is_empty())
        {
            return true;
        }
        self.writes
            .iter()
            .any(|id| other.reads.contains(id) || other.writes.contains(id))
            || other
                .writes
                .iter()
                .any(|id| self.reads.contains(id) || self.writes.contains(id))
    }
}

/// 系统：执行顺序由 [`Schedule::plan`] 决定；`name` 用于 trace / 诊断，
/// `run` 直接改世界。
pub trait System: Send {
    fn name(&self) -> &'static str;
    fn access(&self) -> SystemAccess {
        SystemAccess::default()
    }
    /// 帧首准备：命令结算之后、执行计划之前，按注册顺序调用一次。
    ///
    /// 只拿到只读世界：这里只允许重建派生缓存（刚体表 / 骨架表等），不能改
    /// 世界语义。刚体表必须在本帧速度积分之前建好——旧端 `sync_bodies()` 也是
    /// 先建体再推进物理；挪进 `run` 会让新刚体吃到本帧 `VelocitySystem` 的积分，
    /// 整条轨迹多出 g·dt²。
    fn prepare(&mut self, _world: &World) {}
    fn run(&mut self, world: &mut World, dt: f32);
}

/// 执行计划的一段：系统批，或宿主帧末阶段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanStage {
    /// 同一段内连续的一段系统：两两无访问冲突，批内按注册顺序执行。
    Batch(Vec<usize>),
    /// 宿主帧末阶段（脚本等 `!Send` 状态）。独占、恒在最后，与所有系统冲突。
    Host,
}

/// 执行计划：一段一段按顺序执行，是帧内系统顺序的唯一来源。
///
/// 只能由 [`Schedule::plan`] 构造（字段私有），避免外部拼出与调度器不一致的
/// 顺序；执行走 [`Schedule::run_plan`]。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    stages: Vec<PlanStage>,
}

impl Plan {
    pub fn stages(&self) -> &[PlanStage] {
        &self.stages
    }

    /// 摊平所有系统下标（不含宿主阶段）。合法计划应恰好是 `0..len` 的排列，
    /// 也就是与注册顺序一致。
    pub fn system_order(&self) -> Vec<usize> {
        let mut order = Vec::new();
        for stage in &self.stages {
            if let PlanStage::Batch(indices) = stage {
                order.extend_from_slice(indices);
            }
        }
        order
    }
}

#[derive(Default)]
pub struct Schedule {
    systems: Vec<Box<dyn System>>,
}

impl Schedule {
    pub fn new() -> Self {
        Self::default()
    }

    /// 默认系统顺序；Agent 能力清单与 Simulation 共用同一份。
    pub fn default_schedule(threads: usize) -> Self {
        let mut schedule = Self::new();
        schedule.add(PhysicsSystem::default());
        schedule.add(CharacterJumpSystem);
        schedule.add(VelocitySystem::new(threads));
        schedule.add(GroundCollisionSystem::default());
        schedule.add(AabbCollisionSystem);
        // 刚体走与旧引擎相同的 rapier 求解器（角动量 / 接触力矩 / 堆叠才与旧图可比）。
        // 放在这里：上面的简化系统先跑，对 rapier 接管的 `physics.body` 实体的
        // Transform 改动会被这一步整帧覆盖回物理结果。刚体表由 `prepare` 在本帧
        // 任何系统之前建好（旧端 `sync_bodies()` 同序）；别把建体挪进 `run`——
        // 那会吃到 `VelocitySystem` 本帧的积分，整条轨迹多出 g·dt²。
        schedule.add(RapierPhysicsSystem::default());
        schedule.add(AnimationStateSystem);
        schedule.add(AnimatorSystem);
        schedule.add(AnimationPlaybackSystem::default());
        schedule.add(AnimationLayerSystem::default());
        schedule.add(SkeletonPoseSystem);
        schedule.add(UiLayoutSystem::default());
        schedule.add(UiInteractionSystem);
        schedule.add(UiFocusSystem);
        schedule.add(AudioMixerSystem);
        schedule.add(HierarchySystem::default());
        schedule
    }

    pub fn add<S: System + 'static>(&mut self, system: S) {
        self.systems.push(Box::new(system));
    }

    pub fn len(&self) -> usize {
        self.systems.len()
    }

    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.systems.iter().map(|system| system.name()).collect()
    }

    pub fn accesses(&self) -> Vec<SystemAccess> {
        self.systems.iter().map(|system| system.access()).collect()
    }

    /// 生成执行计划：按注册顺序做**连续**贪心分批（同批两两无访问冲突），
    /// 最后恒追加宿主阶段。
    ///
    /// 连续性让「计划摊平后 == 注册顺序」：计划即执行顺序，分批怎么合并不改变
    /// 语义，只是给将来同批并行留的确定性切分。
    pub fn plan(&self) -> Plan {
        let mut stages = Vec::new();
        let mut batch: Vec<usize> = Vec::new();
        for (index, system) in self.systems.iter().enumerate() {
            let access = system.access();
            let compatible = batch
                .iter()
                .all(|&other| !access.conflicts_with(&self.systems[other].access()));
            if !compatible {
                stages.push(PlanStage::Batch(std::mem::take(&mut batch)));
            }
            batch.push(index);
        }
        if !batch.is_empty() {
            stages.push(PlanStage::Batch(batch));
        }
        stages.push(PlanStage::Host);
        Plan { stages }
    }

    /// 按计划执行一帧；`host` 在宿主阶段（恒最后）执行一次。
    ///
    /// `plan` 必须来自同一个 `Schedule`（只可能由 [`Self::plan`] 产生）。
    pub fn run_plan(
        &mut self,
        plan: &Plan,
        world: &mut World,
        dt: f32,
        mut host: impl FnMut(&mut World),
    ) {
        for stage in plan.stages() {
            match stage {
                PlanStage::Batch(indices) => {
                    for &index in indices {
                        self.systems[index].run(world, dt);
                    }
                }
                PlanStage::Host => host(world),
            }
        }
    }

    /// 帧首准备：命令结算之后、执行计划之前，按注册顺序给每个系统一次只读
    /// 世界的机会（重建刚体表等派生缓存）。
    ///
    /// 只读而不是 `&mut World`：准备阶段不允许改世界语义，否则「计划即唯一
    /// 执行顺序」就被绕开了。
    pub fn prepare(&mut self, world: &World) {
        for system in &mut self.systems {
            system.prepare(world);
        }
    }

    /// 按注册顺序执行所有系统，不带宿主阶段（测试 / 独立用法）。
    pub fn run(&mut self, world: &mut World, dt: f32) {
        self.prepare(world);
        let plan = self.plan();
        self.run_plan(&plan, world, dt, |_| {});
    }
}

mod aabb;
mod animation;
mod animation_layers;
mod animation_state;
mod audio;
mod character_kcc;
mod input;
mod mixer;
mod particles;
/// 物理重力：只对 dynamic 刚体按 gravity_scale 加速，static / kinematic 不动。
mod rapier_world;
mod skeleton;
mod systems;
mod ui;

pub use aabb::AabbCollisionSystem;
pub use animation::{
    load_clip_ron, mul_quat, slerp, AnimationClip, AnimationLibrary, AnimationPlaybackSystem,
    JointTrack, RotationKey, ScaleKey, TranslationKey,
};
pub use animation_layers::{parse_layers, AnimationLayerSystem, LayerDef};
pub use animation_state::{
    parse_machine, parse_parameters, AnimationStateSystem, StateDef, StateMachine, TransitionDef,
};
pub use audio::{load_wav, AudioClip, AudioLibrary, AudioMixerSystem};
pub use character_kcc::{
    build_character_controller, center_height, character_capsule, half_height, radius, rest_gap,
    slip_limit, GRAVITY_Y, GROUND_NORMAL_MIN_DOT, GROUND_PREDICTION_FACTOR, REST_GAP_FACTOR,
    SLIDE_BAND_DEGREES,
};
pub use input::{Axis, Binding, Combo, InputMap, InputState, KeyCode, MouseButton};
pub use mixer::{write_wav_mono, SoftwareMixer};
pub use particles::{ParticleStats, ParticleSystem};
pub use rapier_world::{RapierPhysicsSystem, RapierWorld, FIXED_DT};
pub use skeleton::{parse_pose, SkeletonPoseSystem};
pub use systems::{
    AnimatorSystem, CharacterJumpSystem, GravitySystem, GroundCollisionSystem, HierarchySystem,
    PhysicsSystem, VelocitySystem,
};
pub use ui::{hit_test, UiFocusSystem, UiInteractionSystem, UiLayoutSystem, UiNav};

pub struct Simulation {
    world: World,
    schedule: Schedule,
    plan: Plan,
    fixed_dt: f32,
    frame: u64,
    trace: Trace,
}

impl Simulation {
    pub fn new(world: World) -> Self {
        Self::with_fixed_dt(world, default_fixed_dt())
    }

    pub fn with_fixed_dt(world: World, fixed_dt: f32) -> Self {
        Self::with_threads(world, fixed_dt, 1)
    }

    pub fn with_threads(world: World, fixed_dt: f32, threads: usize) -> Self {
        let schedule = Schedule::default_schedule(threads);
        let plan = schedule.plan();
        Self {
            world,
            schedule,
            plan,
            fixed_dt,
            frame: 0,
            trace: Trace::new(),
        }
    }

    pub fn from_data(data: &WorldData, fixed_dt: f32) -> Result<Self, SimError> {
        let mut world = World::new();
        world.restore_world_data(data).map_err(SimError::Restore)?;
        Ok(Self::with_fixed_dt(world, fixed_dt))
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    /// 宿主每帧写入 UI 输入；没有 `ui.input` 实体时自动补一个。
    pub fn set_ui_input(&mut self, input: UiInput) {
        let mut key = None;
        self.world.for_each::<UiInput>(|existing, _| {
            if key.is_none() {
                key = Some(existing);
            }
        });
        match key.and_then(|key| self.world.get_mut::<UiInput>(key)) {
            Some(slot) => *slot = input,
            None => {
                self.world.spawn((input,));
            }
        }
    }

    pub fn ui_input(&self) -> Option<UiInput> {
        let mut found = None;
        self.world.for_each::<UiInput>(|_, input| {
            if found.is_none() {
                found = Some(*input);
            }
        });
        found
    }

    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    pub fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    /// 当前执行计划（构造时按 `schedule` 固定；`schedule` 只读，不会漂移）。
    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    pub fn fixed_dt(&self) -> f32 {
        self.fixed_dt
    }

    /// 一帧：应用命令 → 跑系统 → 记录。帧号从 0 开始。
    pub fn step(&mut self, commands: Vec<Command>) -> Result<FrameRecord, SimError> {
        self.step_with(commands, |_| {})
    }

    /// 跑一帧，并在记录 trace 前执行 `after`（脚本 / 外部输入接入点）。
    ///
    /// `after` 是计划里的宿主阶段：紧跟在所有系统之后、记录帧号之前。
    pub fn step_with(
        &mut self,
        commands: Vec<Command>,
        after: impl FnMut(&mut World),
    ) -> Result<FrameRecord, SimError> {
        let world_plan = self.world.plan(commands);
        if !world_plan.is_valid() {
            return Err(SimError::InvalidPlan {
                frame: self.frame,
                diagnostics: world_plan.diagnostics,
            });
        }
        self.world
            .apply(&world_plan)
            .map_err(|diagnostics| SimError::Apply {
                frame: self.frame,
                diagnostics,
            })?;
        // 帧首准备必须在执行计划之前：刚体表要用**本帧积分前**的位姿建体。
        self.schedule.prepare(&self.world);
        self.schedule
            .run_plan(&self.plan, &mut self.world, self.fixed_dt, after);

        let record = FrameRecord {
            frame: self.frame,
            revision: self.world.revision(),
            state_hash: self.world.state_hash(),
            entities: self.world.len(),
        };
        self.trace.frames.push(record.clone());
        self.frame += 1;
        Ok(record)
    }

    pub fn run(&mut self, frames: u64) -> Result<(), SimError> {
        for _ in 0..frames {
            self.step(Vec::new())?;
        }
        Ok(())
    }

    pub fn into_world(self) -> World {
        self.world
    }
}

/// 跑完整场景；命令在对应帧开始、记录该帧 hash 之前应用，然后跑系统。
pub fn play(scenario: &Scenario) -> Result<(World, Trace), SimError> {
    if !scenario.fixed_dt.is_finite() || scenario.fixed_dt <= 0.0 {
        return Err(SimError::InvalidScenario(format!(
            "fixed_dt 必须是有限正数，实际 {}",
            scenario.fixed_dt
        )));
    }
    for group in &scenario.commands {
        if group.frame >= scenario.frames {
            return Err(SimError::InvalidScenario(format!(
                "命令帧 {} 超出总帧数 {}",
                group.frame, scenario.frames
            )));
        }
    }

    let mut simulation = Simulation::from_data(&scenario.initial, scenario.fixed_dt)?;
    for frame in 0..scenario.frames {
        let mut commands = Vec::new();
        for group in scenario
            .commands
            .iter()
            .filter(|group| group.frame == frame)
        {
            commands.extend(group.commands.iter().cloned());
        }
        simulation.step(commands)?;
    }
    let trace = simulation.trace().clone();
    Ok((simulation.into_world(), trace))
}

/// 重跑场景并逐帧比对 trace；第一条不一致立即返回。
pub fn verify(scenario: &Scenario, expected: &Trace) -> Result<(), SimError> {
    let (_, actual) = play(scenario)?;
    if actual.frames.len() != expected.frames.len() {
        return Err(SimError::InvalidScenario(format!(
            "trace 帧数 {} 与场景 {} 不一致",
            expected.frames.len(),
            actual.frames.len()
        )));
    }
    for (expected, actual) in expected.frames.iter().zip(actual.frames.iter()) {
        if expected.frame != actual.frame || expected.state_hash != actual.state_hash {
            return Err(SimError::TraceMismatch {
                frame: actual.frame,
                expected_hash: expected.state_hash,
                actual_hash: actual.state_hash,
            });
        }
    }
    Ok(())
}
