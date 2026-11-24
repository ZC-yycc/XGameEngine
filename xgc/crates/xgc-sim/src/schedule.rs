//! 执行计划与调度器：分段（宿主阶段前后）、分批、批内并行。
//!
//! 分批的唯一真相源在这里：`Schedule::plan` 按访问声明做连续贪心，宿主阶段
//! 插在第一个 `runs_after_host()` 系统之前。批是并行的确定性输入，批内系统
//! 写集合两两不相交（冲突判定见 [`SystemAccess::conflicts_with`]）。

use xgc_world::{ComponentId, World, WriteSet};

use crate::{
    AabbCollisionSystem, AnimationLayerSystem, AnimationPlaybackSystem, AnimationStateSystem,
    AnimatorSystem, AudioMixerSystem, CharacterJumpSystem, GroundCollisionSystem, HierarchySystem,
    PhysicsSystem, RapierPhysicsSystem, SkeletonPoseSystem, UiFocusSystem, UiInteractionSystem,
    UiLayoutSystem, VelocitySystem,
};

/// 默认系统名；Agent 能力清单直接复用，避免手写两份。
pub fn default_system_names() -> Vec<&'static str> {
    Schedule::default_schedule(1).names()
}

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

    /// 是否必须排在**宿主阶段之后**执行。
    ///
    /// 旧端帧序是「固定步 → `Update` 钩子 → 动画 tick」：脚本本帧写的动画参数，
    /// 状态机同帧就要看到。执行计划把宿主阶段插在第一个 `runs_after_host()`
    /// 系统之前，默认调度里就是动画系统之前；不参与宿主阶段的系统保持注册顺序。
    fn runs_after_host(&self) -> bool {
        false
    }

    /// 批内并行执行能力：`run_parted` 只拿只读世界 + 自己那份 `WriteSet`。
    ///
    /// 返回 false（默认）时调度器不会把该批拆成并行，整批仍按注册顺序串行。
    fn supports_parted(&self) -> bool {
        false
    }

    /// 批内并行执行入口；只有 [`System::supports_parted`] 为真时才会被调用。
    fn run_parted(&mut self, _world: &World, _writes: &mut WriteSet, _dt: f32) {}

    fn run(&mut self, world: &mut World, dt: f32);
}

/// 执行计划的一段：系统批，或宿主帧末阶段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanStage {
    /// 同一段内连续的一段系统：两两无访问冲突，批内按注册顺序执行。
    Batch(Vec<usize>),
    /// 宿主阶段（脚本等 `!Send` 状态）。独占，插在第一个声明
    /// [`System::runs_after_host`] 的系统之前（默认调度里是动画系统之前）；
    /// 没有系统声明时落在最后。与所有系统冲突。
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
    /// 批内并行线程数：1 = 串行（默认）。>1 时同批系统必须都实现
    /// `System::run_parted`，否则整批退回串行。
    parallel_threads: usize,
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
        schedule.add(AabbCollisionSystem::default());
        // 刚体走与旧引擎相同的 rapier 求解器（角动量 / 接触力矩 / 堆叠才与旧图可比）。
        // 放在这里：上面的简化系统先跑，对 rapier 接管的 `physics.body` 实体的
        // Transform 改动会被这一步整帧覆盖回物理结果。刚体表由 `prepare` 在本帧
        // 任何系统之前建好（旧端 `sync_bodies()` 同序）；别把建体挪进 `run`——
        // 那会吃到 `VelocitySystem` 本帧的积分，整条轨迹多出 g·dt²。
        schedule.add(RapierPhysicsSystem::default());
        // UI 布局 / 命中在宿主阶段之前（旧端 `update_ui()` 排在 Update 钩子之前）。
        schedule.add(UiLayoutSystem::default());
        schedule.add(UiInteractionSystem);
        schedule.add(UiFocusSystem);
        // 宿主阶段在这里：脚本本帧写的动画参数，状态机同帧就要看到（旧端
        // `Update` 钩子先于动画 tick）。下面这些系统都声明 runs_after_host()。
        schedule.add(AnimationStateSystem);
        schedule.add(AnimatorSystem);
        schedule.add(AnimationPlaybackSystem::default());
        schedule.add(AnimationLayerSystem);
        schedule.add(SkeletonPoseSystem);
        schedule.add(AudioMixerSystem);
        schedule.add(HierarchySystem::default());
        schedule
    }

    pub fn add<S: System + 'static>(&mut self, system: S) {
        self.systems.push(Box::new(system));
    }

    /// 打开批内并行执行（默认 1 = 串行）。确定性契约不变：同批系统的写集合
    /// 两两不相交，并行与串行的 `state_hash` / `revision` 必须逐位一致。
    pub fn set_parallel_threads(&mut self, threads: usize) {
        self.parallel_threads = threads.max(1);
    }

    pub fn parallel_threads(&self) -> usize {
        self.parallel_threads.max(1)
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
    /// 宿主阶段插在第一个 `runs_after_host()` 系统之前；没有这种系统时落在最后。
    ///
    /// 连续性让「计划摊平后 == 注册顺序」：计划即执行顺序，分批怎么合并不改变
    /// 语义，只是给将来同批并行留的确定性切分。宿主阶段的位置是语义的一部分：
    /// 旧端「固定步 → Update 钩子 → 动画 tick」，脚本写的动画参数要当帧生效。
    pub fn plan(&self) -> Plan {
        let mut stages = Vec::new();
        let mut batch: Vec<usize> = Vec::new();
        let mut host_placed = false;
        for (index, system) in self.systems.iter().enumerate() {
            if !host_placed && system.runs_after_host() {
                if !batch.is_empty() {
                    stages.push(PlanStage::Batch(std::mem::take(&mut batch)));
                }
                stages.push(PlanStage::Host);
                host_placed = true;
            }
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
        if !host_placed {
            stages.push(PlanStage::Host);
        }
        Plan { stages }
    }

    /// 按计划执行一帧；`host` 在计划的宿主阶段执行一次。
    ///
    /// `plan` 必须来自同一个 `Schedule`（只可能由 [`Self::plan`] 产生）。
    pub fn run_plan(
        &mut self,
        plan: &Plan,
        world: &mut World,
        dt: f32,
        host: impl FnMut(&mut World),
    ) {
        let threads = self.parallel_threads();
        self.run_plan_threaded(plan, world, dt, host, threads);
    }

    /// 按计划执行一帧，显式指定批内并行线程数（1 = 串行）。
    ///
    /// 并行只发生在「批内系统全部 `supports_parted`」的段；宿主阶段仍在调用线程
    /// 上执行。同批写集合不相交 ⇒ 与串行路径的 `state_hash` / `revision` 一致。
    pub fn run_plan_threaded(
        &mut self,
        plan: &Plan,
        world: &mut World,
        dt: f32,
        mut host: impl FnMut(&mut World),
        threads: usize,
    ) {
        let threads = threads.max(1);
        for stage in plan.stages() {
            match stage {
                PlanStage::Batch(indices) => self.run_batch(indices, world, dt, threads),
                PlanStage::Host => host(world),
            }
        }
    }

    /// 执行一段系统批：批内声明了 `supports_parted` 的系统走并行，其余按注册
    /// 顺序串行；并行时把各自写的列取成 `WriteSet`，共享只读世界、每个系统一个
    /// 线程，join 后统一还原列。
    ///
    /// 批内两两无访问冲突，所以「先串后并」与纯串行的读写结果等价；`revision`
    /// 只数 bump 次数、与先后无关，`state_hash` 也不含它。
    ///
    /// 线程 panic 时先还原已 join 的列再原样传播；未 join 的线程由
    /// `std::thread::scope` 负责收尾（此时世界可能缺列，调用方应放弃本帧）。
    fn run_batch(&mut self, indices: &[usize], world: &mut World, dt: f32, threads: usize) {
        let mut serial: Vec<usize> = Vec::new();
        let mut parted: Vec<usize> = Vec::new();
        for &index in indices {
            if self.systems[index].supports_parted() {
                parted.push(index);
            } else {
                serial.push(index);
            }
        }
        for &index in &serial {
            self.systems[index].run(world, dt);
        }
        let parallel = threads > 1 && parted.len() > 1;
        if !parallel {
            for &index in &parted {
                self.systems[index].run(world, dt);
            }
            return;
        }

        let mut sets: Vec<(usize, WriteSet)> = Vec::with_capacity(parted.len());
        for &index in &parted {
            let writes = self.systems[index].access().writes;
            sets.push((index, world.extract_writes(&writes)));
        }
        let mut jobs: Vec<(&mut dyn System, WriteSet)> = Vec::with_capacity(indices.len());
        for (index, system) in self.systems.iter_mut().enumerate() {
            if let Some(position) = sets.iter().position(|(owner, _)| *owner == index) {
                let (_, set) = sets.remove(position);
                jobs.push((system.as_mut(), set));
            }
        }

        let world_ref: &World = &*world;
        let mut restored: Vec<WriteSet> = Vec::with_capacity(jobs.len());
        let mut panicked = None;
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(jobs.len());
            for (system, set) in jobs {
                handles.push(scope.spawn(move || {
                    let mut set = set;
                    system.run_parted(world_ref, &mut set, dt);
                    set
                }));
            }
            for handle in handles {
                match handle.join() {
                    Ok(set) => restored.push(set),
                    Err(payload) => {
                        if panicked.is_none() {
                            panicked = Some(payload);
                        }
                    }
                }
            }
        });
        for set in restored {
            world.restore_writes(set);
        }
        if let Some(payload) = panicked {
            std::panic::resume_unwind(payload);
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
