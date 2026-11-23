//! 批内并行执行：同批系统各拿一份 `WriteSet` 并行写，必须与串行逐位一致。
//!
//! 确定性契约：同批系统的写集合两两不相交（`Schedule::plan` 保证），所以并行
//! 与串行的 `state_hash` / `revision` 必须相同。默认仍是串行（每帧新建线程会
//! 破坏热路径零分配门禁），并行由 `set_parallel_threads` 显式打开。

use xgc_sim::{
    AnimationClip, AnimationLayerSystem, AnimationLibrary, AnimationPlaybackSystem,
    AudioMixerSystem, Schedule, Simulation, SkeletonPoseSystem, System, SystemAccess,
    TranslationKey,
};
use xgc_world::{
    AnimationLayers, Animator, AudioGain, AudioListener, AudioMixer, AudioSource, Bone, Component,
    Parent, Skeleton, SkeletonPose, Transform, Velocity, World, WriteSet,
};

const MACHINE_LAYERS: &str = r#"[(name:"upper", mode:Override, clip:"Walk", weight:0.0, speed:1.0, looping:true, machine:Some((initial:"Natural", parameters:{"survey":0.0}, states:[(name:"Natural", clip:"Walk"), (name:"Survey", clip:"Survey")], transitions:[(from:Some("Natural"), to:"Survey", when:Some(AtLeast(parameter:"survey", value:0.5)), duration:0.35), (from:Some("Survey"), to:"Natural", when:Some(Below(parameter:"survey", value:0.5)), duration:0.15)])), mask:["b_Spine01_02"])]"#;

/// 记录自己被哪条路径调用的写 `core.velocity` 系统。
struct RecordingBump {
    serial_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    parted_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl System for RecordingBump {
    fn name(&self) -> &'static str {
        "recording_bump"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: Vec::new(),
            writes: vec![Velocity::ID],
        }
    }

    fn supports_parted(&self) -> bool {
        true
    }

    fn run_parted(&mut self, _world: &World, writes: &mut WriteSet, _dt: f32) {
        self.parted_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        writes.for_each_mut::<Velocity>(|_, velocity| {
            velocity.linear[1] += 1.0;
        });
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        self.serial_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        world.for_each_mut::<Velocity>(|_, velocity| {
            velocity.linear[1] += 1.0;
        });
    }
}

/// 支持批内并行的系统：写 `core.velocity`。
#[derive(Clone, Copy, Debug)]
struct BumpVelocity;

impl System for BumpVelocity {
    fn name(&self) -> &'static str {
        "bump_velocity"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: Vec::new(),
            writes: vec![Velocity::ID],
        }
    }

    fn supports_parted(&self) -> bool {
        true
    }

    fn run_parted(&mut self, _world: &World, writes: &mut WriteSet, _dt: f32) {
        writes.for_each_mut::<Velocity>(|_, velocity| {
            velocity.linear[1] += 1.0;
        });
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        world.for_each_mut::<Velocity>(|_, velocity| {
            velocity.linear[1] += 1.0;
        });
    }
}

/// 支持批内并行的系统：写 `core.transform`。
#[derive(Clone, Copy, Debug)]
struct BumpPosition;

impl System for BumpPosition {
    fn name(&self) -> &'static str {
        "bump_position"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: Vec::new(),
            writes: vec![Transform::ID],
        }
    }

    fn supports_parted(&self) -> bool {
        true
    }

    fn run_parted(&mut self, _world: &World, writes: &mut WriteSet, _dt: f32) {
        writes.for_each_mut::<Transform>(|_, transform| {
            transform.position[0] += 2.0;
        });
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        world.for_each_mut::<Transform>(|_, transform| {
            transform.position[0] += 2.0;
        });
    }
}

/// 不支持批内并行的系统：写 `core.name`。
#[derive(Clone, Copy, Debug)]
struct Rename;

impl System for Rename {
    fn name(&self) -> &'static str {
        "rename"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: Vec::new(),
            writes: vec![xgc_world::Name::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        world.for_each_mut::<xgc_world::Name>(|_, name| {
            name.value.push('!');
        });
    }
}

fn bump_world(entities: usize) -> World {
    let mut world = World::new();
    for index in 0..entities {
        let mut transform = Transform::default();
        transform.position[0] = index as f32;
        world.spawn((transform, Velocity::default()));
    }
    world
}

fn run_frames(schedule: &mut Schedule, world: &mut World, frames: usize, threads: usize) {
    let plan = schedule.plan();
    for _ in 0..frames {
        schedule.run_plan_threaded(&plan, world, 1.0 / 60.0, |_| {}, threads);
    }
}

#[test]
fn parallel_batch_matches_the_serial_path() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let run = |threads: usize| {
        let serial_calls = Arc::new(AtomicUsize::new(0));
        let parted_calls = Arc::new(AtomicUsize::new(0));
        let mut schedule = Schedule::new();
        schedule.add(RecordingBump {
            serial_calls: Arc::clone(&serial_calls),
            parted_calls: Arc::clone(&parted_calls),
        });
        schedule.add(BumpPosition);
        let mut world = bump_world(64);
        run_frames(&mut schedule, &mut world, 10, threads);
        (world, serial_calls, parted_calls)
    };

    let (serial, serial_calls, parted_calls) = run(1);
    assert_eq!(
        serial_calls.load(Ordering::SeqCst),
        10,
        "串行路径必须走 run"
    );
    assert_eq!(parted_calls.load(Ordering::SeqCst), 0);

    let (parallel, serial_calls, parted_calls) = run(4);
    assert_eq!(
        parted_calls.load(Ordering::SeqCst),
        10,
        "并行路径必须走 run_parted"
    );
    assert_eq!(serial_calls.load(Ordering::SeqCst), 0);

    assert_eq!(serial.state_hash(), parallel.state_hash());
    assert_eq!(serial.revision(), parallel.revision());
}

#[test]
fn a_batch_with_one_serial_only_system_falls_back_to_serial() {
    let mut schedule = Schedule::new();
    schedule.add(BumpVelocity);
    schedule.add(Rename);
    let plan = schedule.plan();
    assert_eq!(
        plan.stages().len(),
        2,
        "两个系统应落在同一批（写集合不相交）"
    );

    let mut serial = bump_world(8);
    run_frames(&mut schedule, &mut serial, 5, 1);

    let mut schedule = Schedule::new();
    schedule.add(BumpVelocity);
    schedule.add(Rename);
    let mut parallel = bump_world(8);
    run_frames(&mut schedule, &mut parallel, 5, 8);

    assert_eq!(serial.state_hash(), parallel.state_hash());
    assert_eq!(serial.revision(), parallel.revision());
}

#[test]
fn default_schedule_parallel_matches_serial() {
    let build = || {
        let mut world = bump_world(16);
        world.spawn((
            Transform::default(),
            Animator {
                clip: "Walk".to_string(),
                autoplay: true,
                ..Animator::default()
            },
            AnimationLayers {
                layers: MACHINE_LAYERS.to_string(),
            },
        ));
        world
    };

    let mut serial = Simulation::with_fixed_dt(build(), 1.0 / 60.0);
    for _ in 0..30 {
        serial.step(Vec::new()).expect("串行步进");
    }
    for threads in [1usize, 2, 8] {
        let mut parallel = Simulation::with_fixed_dt(build(), 1.0 / 60.0);
        parallel.set_parallel_threads(threads);
        for _ in 0..30 {
            parallel.step(Vec::new()).expect("并行步进");
        }
        assert_eq!(
            serial.world().state_hash(),
            parallel.world().state_hash(),
            "{threads} 线程与串行必须逐位一致"
        );
        assert_eq!(serial.world().revision(), parallel.world().revision());
    }
}

fn playback_system() -> AnimationPlaybackSystem {
    let mut library = AnimationLibrary::new();
    library.insert(AnimationClip::new(
        "walk",
        1.0,
        vec![
            TranslationKey {
                time: 0.0,
                value: [0.0, 0.0, 0.0],
            },
            TranslationKey {
                time: 1.0,
                value: [2.0, 0.0, 0.0],
            },
        ],
    ));
    AnimationPlaybackSystem { library }
}

fn playback_world() -> World {
    let mut world = World::new();
    world.spawn((
        Transform::default(),
        Animator {
            clip: "walk".to_string(),
            time: 0.5,
            autoplay: true,
            ..Animator::default()
        },
    ));
    world
}

#[test]
fn playback_system_writes_the_same_pose_when_parted() {
    let mut serial = playback_world();
    let mut parted = playback_world();
    let mut system = playback_system();
    system.run(&mut serial, 1.0 / 60.0);

    let mut writes = parted.extract_writes(&[Transform::ID]);
    system.run_parted(&parted, &mut writes, 1.0 / 60.0);
    parted.restore_writes(writes);

    assert_eq!(serial.state_hash(), parted.state_hash());
}

#[test]
fn layer_system_writes_the_same_layers_when_parted() {
    let build = || {
        let mut world = World::new();
        world.spawn((
            AnimationLayers {
                layers: MACHINE_LAYERS.to_string(),
            },
            Animator {
                autoplay: true,
                ..Animator::default()
            },
        ));
        world
    };
    let mut serial = build();
    let mut parted = build();
    let mut system = AnimationLayerSystem;
    system.run(&mut serial, 0.1);

    let mut writes = parted.extract_writes(&[AnimationLayers::ID]);
    system.run_parted(&parted, &mut writes, 0.1);
    parted.restore_writes(writes);

    assert_eq!(serial.state_hash(), parted.state_hash());
}

/// 分区执行：批内有一部分系统不支持 `supports_parted` 时，它们仍按注册顺序
/// 串行执行，其余系统走并行；结果必须与纯串行逐位一致。
#[test]
fn partly_parted_batch_runs_the_rest_serially() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let build = || {
        let mut world = bump_world(8);
        world.spawn((
            Transform::default(),
            xgc_world::Name {
                value: "tag".to_string(),
            },
        ));
        world
    };
    let run = |threads: usize| {
        let serial_calls = Arc::new(AtomicUsize::new(0));
        let parted_calls = Arc::new(AtomicUsize::new(0));
        let mut schedule = Schedule::new();
        schedule.add(RecordingBump {
            serial_calls: Arc::clone(&serial_calls),
            parted_calls: Arc::clone(&parted_calls),
        });
        schedule.add(BumpPosition);
        schedule.add(Rename);
        let plan = schedule.plan();
        assert_eq!(
            plan.stages().len(),
            2,
            "三个系统写集合两两不相交，应落在同一批"
        );
        let mut world = build();
        run_frames(&mut schedule, &mut world, 3, threads);
        (world, serial_calls, parted_calls)
    };

    let (serial_world, serial_calls, parted_calls) = run(1);
    assert_eq!(serial_calls.load(Ordering::SeqCst), 3);
    assert_eq!(parted_calls.load(Ordering::SeqCst), 0);

    let (parallel_world, serial_calls, parted_calls) = run(4);
    assert_eq!(
        serial_calls.load(Ordering::SeqCst),
        0,
        "并行批里 RecordingBump 不该再走串行 run"
    );
    assert_eq!(
        parted_calls.load(Ordering::SeqCst),
        3,
        "两个支持 parted 的系统都该走 run_parted"
    );
    // Rename 不支持 parted，但仍在同一批里按注册顺序串行执行了三帧。
    let mut names = Vec::new();
    parallel_world.for_each::<xgc_world::Name>(|_, name| names.push(name.value.clone()));
    assert_eq!(names, vec!["tag!!!".to_string()]);
    assert_eq!(serial_world.state_hash(), parallel_world.state_hash());
    assert_eq!(serial_world.revision(), parallel_world.revision());
}

fn skeleton_world() -> World {
    let mut world = World::new();
    let root = world.spawn((
        Skeleton { joint_count: 2 },
        SkeletonPose::default(),
        Transform::default(),
    ));
    let root_id = world.entity(root).expect("骨架根实体");
    let first = world.spawn((
        Bone { index: 0 },
        Parent { entity: root_id },
        Transform {
            position: [1.0, 0.0, 0.0],
            ..Transform::default()
        },
    ));
    let first_id = world.entity(first).expect("第一根骨");
    world.spawn((
        Bone { index: 1 },
        Parent { entity: first_id },
        Transform {
            position: [0.0, 2.0, 0.0],
            ..Transform::default()
        },
    ));
    world
}

#[test]
fn skeleton_pose_system_writes_the_same_pose_when_parted() {
    let mut serial = skeleton_world();
    let mut parted = skeleton_world();
    let mut system = SkeletonPoseSystem;
    system.run(&mut serial, 0.0);

    let mut writes = parted.extract_writes(&[SkeletonPose::ID]);
    system.run_parted(&parted, &mut writes, 0.0);
    parted.restore_writes(writes);

    assert_eq!(serial.state_hash(), parted.state_hash());
    let mut text = String::new();
    serial.for_each::<SkeletonPose>(|_, pose| text = pose.matrices.clone());
    assert!(
        !text.is_empty(),
        "关节矩阵必须真的算出来，否则两路都写空串会把测试做成假绿"
    );
}

fn audio_world() -> World {
    let mut world = World::new();
    world.spawn((AudioMixer {
        master: 0.5,
        ..AudioMixer::default()
    },));
    world.spawn((AudioListener::default(), Transform::default()));
    world.spawn((
        AudioSource {
            spatial: true,
            volume: 1.0,
            ..AudioSource::default()
        },
        AudioGain::default(),
        Transform {
            position: [0.0, 0.0, 3.0],
            ..Transform::default()
        },
    ));
    world
}

#[test]
fn audio_mixer_system_writes_the_same_gain_when_parted() {
    let mut serial = audio_world();
    let mut parted = audio_world();
    let mut system = AudioMixerSystem;
    system.run(&mut serial, 0.0);

    let mut writes = parted.extract_writes(&[AudioGain::ID]);
    system.run_parted(&parted, &mut writes, 0.0);
    parted.restore_writes(writes);

    assert_eq!(serial.state_hash(), parted.state_hash());
    let mut gain = 1.0;
    serial.for_each::<AudioGain>(|_, output| gain = output.gain);
    assert!(
        (gain - 0.125).abs() < 1e-6,
        "空间化增益应为 volume/(1+3)×master 0.5 = 0.125，实际 {gain}"
    );
}
