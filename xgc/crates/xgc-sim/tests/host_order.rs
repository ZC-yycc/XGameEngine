//! 宿主阶段位置回归：脚本必须跑在固定步之后、动画系统之前。
//!
//! 旧引擎帧序是「固定步循环 → `Update` 钩子 → 动画 tick」：脚本本帧写的动画
//! 参数，状态机同帧就要看到。宿主阶段恒最后会让状态机切换晚一帧（第 300 帧
//! dump 里 `parameters.speed=3.0` 而 `state` 仍是 Walk 即此）。

use std::sync::{Arc, Mutex};

use xgc_sim::{PlanStage, Schedule, Simulation, System};
use xgc_world::{
    AnimationState, Animator, GlobalTransform, StateDef, StateMachine, Transform,
    TransitionCondition, TransitionDef, World,
};

struct Recorder {
    name: &'static str,
    after_host: bool,
    log: Arc<Mutex<Vec<&'static str>>>,
}

impl System for Recorder {
    fn name(&self) -> &'static str {
        self.name
    }

    fn runs_after_host(&self) -> bool {
        self.after_host
    }

    fn run(&mut self, _world: &mut World, _dt: f32) {
        self.log.lock().unwrap().push(self.name);
    }
}

#[test]
fn default_plan_moves_the_host_stage_before_animation() {
    let schedule = Schedule::default_schedule(1);
    let names = schedule.names();
    let plan = schedule.plan();
    let stages = plan.stages();
    let host = stages
        .iter()
        .position(|stage| matches!(stage, PlanStage::Host))
        .expect("默认调度必须有宿主阶段");
    assert!(
        host + 1 < stages.len(),
        "宿主阶段后面必须有系统（动画系统），实际 {stages:?}"
    );

    let first_after = match &stages[host + 1] {
        PlanStage::Batch(indices) => indices.clone(),
        other => panic!("宿主阶段后面应是系统批，实际 {other:?}"),
    };
    let after_names: Vec<&str> = first_after.iter().map(|&index| names[index]).collect();
    assert!(
        after_names.contains(&"animation_state"),
        "宿主阶段后面第一批应先跑动画状态机，实际 {after_names:?}"
    );

    let before: Vec<&str> = stages[..host]
        .iter()
        .flat_map(|stage| match stage {
            PlanStage::Batch(indices) => indices
                .iter()
                .map(|&index| names[index])
                .collect::<Vec<_>>(),
            PlanStage::Host => Vec::new(),
        })
        .collect();
    assert!(
        before.contains(&"rapier"),
        "宿主阶段之前应包含固定步，实际 {before:?}"
    );
    assert!(
        !before.contains(&"animation_state") && !before.contains(&"animation"),
        "动画系统不能在宿主阶段之前，实际 {before:?}"
    );
}

#[test]
fn host_runs_between_the_fixed_simulation_and_post_host_systems() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut schedule = Schedule::new();
    schedule.add(Recorder {
        name: "sim",
        after_host: false,
        log: log.clone(),
    });
    schedule.add(Recorder {
        name: "animation",
        after_host: true,
        log: log.clone(),
    });
    let plan = schedule.plan();
    let mut world = World::new();
    schedule.run_plan(&plan, &mut world, 0.0, |_| {
        log.lock().unwrap().push("host");
    });
    assert_eq!(*log.lock().unwrap(), vec!["sim", "host", "animation"]);
}

#[test]
fn script_parameters_reach_the_state_machine_in_the_same_frame() {
    let machine = StateMachine {
        initial: "idle".into(),
        states: [("idle", "Survey"), ("walk", "Walk")]
            .into_iter()
            .map(|(name, clip)| StateDef {
                name: name.into(),
                clip: clip.into(),
                looping: true,
                speed: 1.0,
            })
            .collect(),
        transitions: vec![TransitionDef {
            from: Some("idle".into()),
            to: "walk".into(),
            when: Some(TransitionCondition::AtLeast {
                parameter: "speed".into(),
                value: 0.5,
            }),
            duration: 0.0,
        }],
    };
    let mut world = World::new();
    let key = world.spawn((
        Transform::default(),
        GlobalTransform::default(),
        AnimationState {
            machine,
            parameters: [("speed".into(), 0.0)].into(),
            ..Default::default()
        },
        Animator::default(),
    ));
    let mut simulation = Simulation::with_fixed_dt(world, 1.0 / 60.0);
    simulation
        .step_with(Vec::new(), |world| {
            world
                .get_mut::<AnimationState>(key)
                .unwrap()
                .parameters
                .insert("speed".into(), 1.0);
        })
        .expect("步进失败");
    let animator = simulation.world().get::<Animator>(key).unwrap();
    assert_eq!(
        animator.clip, "Walk",
        "宿主阶段在动画之前时，脚本本帧写的参数必须当帧生效"
    );
}
