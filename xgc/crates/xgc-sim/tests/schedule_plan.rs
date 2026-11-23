//! 执行计划回归：计划即执行顺序。
//!
//! 钉住四件事：批次连续（摊平 == 注册顺序）、同批两两无访问冲突、宿主阶段恒
//! 最后且独占、执行走计划与注册顺序逐条一致。这些性质一起保证「分批怎么合并
//! 都不改语义」，将来同批并行才有确定性输入。

use std::sync::{Arc, Mutex};

use xgc_sim::{default_system_names, PlanStage, Schedule, Simulation, System, SystemAccess};
use xgc_world::{Component, ComponentId, Name, Transform, World};

struct TestSystem {
    name: &'static str,
    access: SystemAccess,
    recorder: Option<Arc<Mutex<Vec<&'static str>>>>,
}

impl System for TestSystem {
    fn name(&self) -> &'static str {
        self.name
    }

    fn access(&self) -> SystemAccess {
        self.access.clone()
    }

    fn run(&mut self, _world: &mut World, _dt: f32) {
        if let Some(recorder) = &self.recorder {
            recorder.lock().unwrap().push(self.name);
        }
    }
}

fn writer(component: ComponentId) -> SystemAccess {
    SystemAccess {
        reads: Vec::new(),
        writes: vec![component],
    }
}

fn test_system(name: &'static str, access: SystemAccess) -> TestSystem {
    TestSystem {
        name,
        access,
        recorder: None,
    }
}

#[test]
fn default_plan_is_contiguous_and_covers_registration_order() {
    let schedule = Schedule::default_schedule(1);
    let plan = schedule.plan();
    let stages = plan.stages();

    // 宿主阶段恰好一次；默认调度里它夹在固定步与动画之间。
    assert_eq!(
        stages
            .iter()
            .filter(|stage| matches!(stage, PlanStage::Host))
            .count(),
        1
    );
    assert!(!matches!(stages.last(), Some(&PlanStage::Host)));
    assert_eq!(plan.system_order(), (0..schedule.len()).collect::<Vec<_>>());

    let accesses = schedule.accesses();
    for stage in stages {
        let PlanStage::Batch(indices) = stage else {
            continue;
        };
        assert!(
            indices.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "批次必须连续：{indices:?}"
        );
        for (position, &left) in indices.iter().enumerate() {
            for &right in &indices[position + 1..] {
                assert!(
                    !accesses[left].conflicts_with(&accesses[right]),
                    "同批系统访问冲突：{left} vs {right}"
                );
            }
        }
    }
}

#[test]
fn default_plan_snapshot() {
    // 连续贪心按注册顺序切批：能续就续、冲突就封批。这份快照同时是 `xgc plan`
    // 的输出契约，改动系统注册顺序或访问声明必须同步这里。
    // 宿主阶段夹在 ui_focus（8）与 animation_state（9）之间：脚本本帧写的动画
    // 参数，状态机同帧就要看到。
    let expected = vec![
        PlanStage::Batch(vec![0]),
        PlanStage::Batch(vec![1]),
        PlanStage::Batch(vec![2]),
        PlanStage::Batch(vec![3]),
        PlanStage::Batch(vec![4]),
        PlanStage::Batch(vec![5, 6]),
        PlanStage::Batch(vec![7]),
        PlanStage::Batch(vec![8]),
        PlanStage::Host,
        PlanStage::Batch(vec![9]),
        PlanStage::Batch(vec![10]),
        PlanStage::Batch(vec![11]),
        PlanStage::Batch(vec![12]),
        PlanStage::Batch(vec![13, 14, 15]),
    ];
    assert_eq!(Schedule::default_schedule(1).plan().stages(), expected);
}

#[test]
fn plan_is_independent_of_thread_count() {
    assert_eq!(
        Schedule::default_schedule(1).plan(),
        Schedule::default_schedule(8).plan()
    );
}

#[test]
fn batches_only_merge_adjacent_systems() {
    // 旧的全批贪心会把 0 与 2 合进同一批（非连续），计划摊平就与注册顺序不一致。
    // 连续分批只能 [0] 与 [1, 2]。
    let mut schedule = Schedule::new();
    schedule.add(test_system("writes_transform", writer(Transform::ID)));
    schedule.add(test_system("writes_transform_too", writer(Transform::ID)));
    schedule.add(test_system("writes_name", writer(Name::ID)));

    assert_eq!(
        schedule.plan().stages(),
        &[
            PlanStage::Batch(vec![0]),
            PlanStage::Batch(vec![1, 2]),
            PlanStage::Host,
        ]
    );
}

#[test]
fn systems_with_no_access_declaration_keep_their_own_batch() {
    // 空访问集 = 什么都能碰，与所有系统冲突，只能独占一段。
    let mut schedule = Schedule::new();
    schedule.add(test_system("unknown", SystemAccess::default()));
    schedule.add(test_system("known", writer(Transform::ID)));
    schedule.add(test_system("unknown_too", SystemAccess::default()));

    assert_eq!(
        schedule.plan().stages(),
        &[
            PlanStage::Batch(vec![0]),
            PlanStage::Batch(vec![1]),
            PlanStage::Batch(vec![2]),
            PlanStage::Host,
        ]
    );
}

#[test]
fn empty_schedule_gets_only_the_host_stage() {
    let mut schedule = Schedule::new();
    assert_eq!(schedule.plan().stages(), &[PlanStage::Host]);

    let recorder = Arc::new(Mutex::new(Vec::new()));
    let host = recorder.clone();
    let plan = schedule.plan();
    schedule.run_plan(&plan, &mut World::new(), 0.0, move |_| {
        host.lock().unwrap().push("host");
    });
    assert_eq!(*recorder.lock().unwrap(), vec!["host"]);
}

#[test]
fn run_plan_keeps_registration_order_and_runs_host_last() {
    let recorder = Arc::new(Mutex::new(Vec::new()));
    let mut schedule = Schedule::new();
    for name in ["a", "b", "c"] {
        schedule.add(TestSystem {
            name,
            access: SystemAccess::default(),
            recorder: Some(recorder.clone()),
        });
    }

    let host = recorder.clone();
    let plan = schedule.plan();
    schedule.run_plan(&plan, &mut World::new(), 1.0 / 60.0, move |_| {
        host.lock().unwrap().push("host");
    });

    assert_eq!(*recorder.lock().unwrap(), vec!["a", "b", "c", "host"]);
}

#[test]
fn simulation_freezes_the_default_plan_at_construction() {
    let simulation = Simulation::new(World::new());
    assert_eq!(simulation.plan(), &simulation.schedule().plan());
    assert_eq!(simulation.schedule().names(), default_system_names());
    assert_eq!(
        simulation.plan().system_order(),
        (0..simulation.schedule().len()).collect::<Vec<_>>()
    );
}
