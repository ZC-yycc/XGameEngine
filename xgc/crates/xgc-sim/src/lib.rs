//! XGC 最小帧循环：按帧应用命令、跑系统、逐帧记录 revision / state hash。
//!
//! 执行顺序只认 [`Schedule::plan`]：系统按注册顺序连续分批（同批两两无访问
//! 冲突），最后一段是宿主帧末阶段（脚本等 `!Send` 状态）。批内并行要先把写集合
//! 从世界取出（`World::extract_writes`），由 `Schedule::set_parallel_threads` 显式
//! 打开，默认仍串行。计划之前还有一段**帧首准备**
//! （[`Schedule::prepare`]）：只读世界、不改语义，用来把派生的运行时缓存
//! （刚体表等）建到与当前场景一致。确定性契约先钉住：同一份 Scenario（初始
//! WorldData + 每帧命令 + fixed_dt）跑两遍，trace 必须逐帧一致。

#![deny(unsafe_code)]

mod error;
mod scenario;

mod animation_events;

pub use error::SimError;
pub use scenario::{FrameCommands, FrameRecord, Scenario, Trace, TRACE_FORMAT_VERSION};

use scenario::default_fixed_dt;
use xgc_world::{Command, HostUiInput, World, WorldData};

mod schedule;

pub use schedule::{default_system_names, Plan, PlanStage, Schedule, System, SystemAccess};

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
mod root_motion;
mod skeleton;
mod systems;
mod ui;

pub use animation::{
    load_clip_ron, mul_quat, slerp, AnimationClip, AnimationLibrary, AnimationPlaybackSystem,
    JointTrack, RotationKey, ScaleKey, TranslationKey,
};
pub use animation_layers::AnimationLayerSystem;
pub use animation_state::AnimationStateSystem;
pub use audio::{load_wav, AudioClip, AudioLibrary, AudioMixerSystem};
pub use character_kcc::{
    build_character_controller, center_height, character_capsule, half_height, radius, rest_gap,
    slip_limit, GRAVITY_Y, GROUND_NORMAL_MIN_DOT, GROUND_PREDICTION_FACTOR, REST_GAP_FACTOR,
    SLIDE_BAND_DEGREES,
};
pub use input::{
    ActionInput, Axis, Binding, Combo, InputEdge, InputMap, InputState, KeyCode, MouseButton,
};
pub use mixer::{write_wav_mono, SoftwareMixer};
pub use particles::{ParticleStats, ParticleSystem};
pub use rapier_world::{ColliderOverlap, OverlapBody, RapierPhysicsSystem, RapierWorld, FIXED_DT};
pub use root_motion::apply_root_motion;
pub use skeleton::{parse_pose, SkeletonPoseSystem};
pub use systems::{
    AnimatorSystem, CharacterJumpSystem, GravitySystem, GroundCollisionSystem, HierarchySystem,
    PhysicsSystem, VelocitySystem,
};
pub use ui::{
    hit_test, UiFocusSystem, UiInteractionSystem, UiLayoutSystem, UiNav, DEFAULT_UI_VIEWPORT,
};
pub use xgc_world::{StateDef, StateMachine, TransitionCondition, TransitionDef};

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

    /// 帧末、资产相关的派生修正用的可变入口（例如根运动：轨道表来自模型资产，
    /// 不属于默认调度的零资产依赖契约，由宿主在 `step_with` 返回后应用）。
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// 宿主在固定步前写入设备输入。无头回放仍可读取场景里的 `ui.input`。
    pub fn set_ui_input(&mut self, input: HostUiInput) {
        self.world.set_host_ui_input(input);
    }

    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    pub fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    /// 打开批内并行执行（默认 1 = 串行）。同批系统需实现 `System::run_parted`；
    /// 打开后每帧按批新建线程，热路径分配门禁下不要把默认值调成 >1。
    pub fn set_parallel_threads(&mut self, threads: usize) {
        self.schedule.set_parallel_threads(threads);
    }

    pub fn parallel_threads(&self) -> usize {
        self.schedule.parallel_threads()
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

    /// 跑一帧，在计划里的宿主阶段执行回调并记录步末状态。
    pub fn step_with(
        &mut self,
        commands: Vec<Command>,
        host: impl FnMut(&mut World),
    ) -> Result<FrameRecord, SimError> {
        let frame = self.advance_with(commands, host)?;
        let record = FrameRecord {
            frame,
            revision: self.world.revision(),
            state_hash: self.world.state_hash(),
            entities: self.world.len(),
        };
        self.trace.frames.push(record.clone());
        Ok(record)
    }

    /// 与 `step_with` 共用固定步，供自行采样状态的宿主避免持续累积 trace。
    pub fn advance_with(
        &mut self,
        commands: Vec<Command>,
        host: impl FnMut(&mut World),
    ) -> Result<u64, SimError> {
        // 碰撞事件是"本步"的瞬态数据：先清上一帧，再让物理系统写。
        self.world.clear_collision_events();
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
            .run_plan(&self.plan, &mut self.world, self.fixed_dt, host);

        let frame = self.frame;
        self.frame += 1;
        Ok(frame)
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
