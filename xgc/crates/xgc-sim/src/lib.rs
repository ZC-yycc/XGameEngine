//! XGC 最小帧循环：按帧应用命令、跑系统、逐帧记录 revision / state hash。
//!
//! 调度目前是**固定顺序**（注册顺序）；访问声明与并行合并在下一刀。确定性契约
//! 先钉住：同一份 Scenario（初始 WorldData + 每帧命令 + fixed_dt）跑两遍，
//! trace 必须逐帧一致。

#![deny(unsafe_code)]

use serde::{Deserialize, Serialize};
use xgc_base::Diagnostic;
use xgc_world::{Command, ComponentId, World, WorldData};

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

/// 系统声明的读写集。M1 先做元数据与冲突判断；并行执行只发生在系统内部
/// （按 archetype 分片），跨系统并行在下一刀。
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

/// 系统：M1 只有顺序执行；`name` 用于 trace / 诊断，`run` 直接改世界。
pub trait System: Send {
    fn name(&self) -> &'static str;
    fn access(&self) -> SystemAccess {
        SystemAccess::default()
    }
    fn run(&mut self, world: &mut World, dt: f32);
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
        schedule.add(CharacterSystem);
        schedule.add(AabbCollisionSystem);
        schedule.add(AnimatorSystem);
        schedule.add(UiLayoutSystem::default());
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

    /// 按注册顺序贪心分批：同批系统两两无访问冲突。执行仍按注册顺序，
    /// 这个计划是给下一步跨系统并行用的确定性输入。
    pub fn batches(&self) -> Vec<Vec<usize>> {
        let mut batches: Vec<Vec<usize>> = Vec::new();
        for (index, system) in self.systems.iter().enumerate() {
            let access = system.access();
            let mut placed = false;
            for batch in &mut batches {
                let compatible = batch
                    .iter()
                    .all(|&other| !access.conflicts_with(&self.systems[other].access()));
                if compatible {
                    batch.push(index);
                    placed = true;
                    break;
                }
            }
            if !placed {
                batches.push(vec![index]);
            }
        }
        batches
    }

    pub fn run(&mut self, world: &mut World, dt: f32) {
        for system in &mut self.systems {
            system.run(world, dt);
        }
    }
}

/// 物理重力：只对 dynamic 刚体按 gravity_scale 加速，static / kinematic 不动。
mod animation;
mod systems;

pub use animation::{load_clip_ron, AnimationClip, AnimationLibrary, TranslationKey};
pub use systems::{
    AabbCollisionSystem, AnimatorSystem, CharacterJumpSystem, CharacterSystem, GravitySystem,
    GroundCollisionSystem, HierarchySystem, PhysicsSystem, UiLayoutSystem, VelocitySystem,
};

pub struct Simulation {
    world: World,
    schedule: Schedule,
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
        Self {
            world,
            schedule,
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

    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    pub fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    pub fn fixed_dt(&self) -> f32 {
        self.fixed_dt
    }

    /// 一帧：应用命令 → 跑系统 → 记录。帧号从 0 开始。
    pub fn step(&mut self, commands: Vec<Command>) -> Result<FrameRecord, SimError> {
        let plan = self.world.plan(commands);
        if !plan.is_valid() {
            return Err(SimError::InvalidPlan {
                frame: self.frame,
                diagnostics: plan.diagnostics,
            });
        }
        self.world
            .apply(&plan)
            .map_err(|diagnostics| SimError::Apply {
                frame: self.frame,
                diagnostics,
            })?;
        self.schedule.run(&mut self.world, self.fixed_dt);

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
