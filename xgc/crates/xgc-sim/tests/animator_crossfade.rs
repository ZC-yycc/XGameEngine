//! 交叉淡入的模拟侧口径：切换时定格旧剪辑的时钟 / 速度 / 循环方式，淡出期间
//! 旧剪辑继续按自己的时钟走，到时长后整份清空；硬切不许留下上一条过渡的残影。

use xgc_base::EntityKey;
use xgc_sim::Simulation;
use xgc_world::{AnimationState, Animator, GlobalTransform, Transform, World};

const DT: f32 = 1.0 / 60.0;

const MACHINE: &str = r#"(
    initial: "idle",
    states: [
        (name: "idle", clip: "Survey"),
        (name: "walk", clip: "Walk"),
    ],
    transitions: [
        (from: Some("idle"), to: "walk", when: Some(AtLeast(parameter: "speed", value: 0.5))),
    ],
)"#;

/// 转移自己写 `duration: 0.0` ⇒ 用 `Animator.fade_time` 兜底；这里同样为 0 ⇒ 硬切。
const MACHINE_HARD_CUT: &str = r#"(
    initial: "idle",
    states: [
        (name: "idle", clip: "Survey"),
        (name: "walk", clip: "Walk"),
    ],
    transitions: [
        (from: Some("idle"), to: "walk", when: Some(AtLeast(parameter: "speed", value: 0.5)), duration: 0.0),
    ],
)"#;

fn spawn(machine: &str, animator: Animator) -> (Simulation, EntityKey) {
    let mut world = World::new();
    let key = world.spawn((
        Transform::default(),
        GlobalTransform::default(),
        AnimationState {
            machine: machine.to_string(),
            parameters: "{\"speed\": 0.0}".to_string(),
            ..Default::default()
        },
        animator,
    ));
    (Simulation::with_fixed_dt(world, DT), key)
}

fn drive(simulation: &mut Simulation, key: EntityKey, speed: Option<f32>) {
    simulation
        .step_with(Vec::new(), |world| {
            if let Some(speed) = speed {
                if let Some(state) = world.get_mut::<AnimationState>(key) {
                    state.parameters = format!("{{\"speed\": {speed}}}");
                }
            }
        })
        .expect("步进失败");
}

#[test]
fn transition_freezes_the_old_clip_clock_and_speed() {
    let (mut simulation, key) = spawn(
        MACHINE,
        Animator {
            fade_time: 0.25,
            ..Default::default()
        },
    );
    // 第 1 帧留在 idle：Survey 已经播了一帧，切换时要定格的就是这个相位。
    drive(&mut simulation, key, None);
    drive(&mut simulation, key, Some(1.0));

    let world = simulation.into_world();
    let animator = world.get::<Animator>(key).expect("Animator 还在");
    assert_eq!(animator.clip, "Walk");
    assert_eq!(animator.blend_from, "Survey");
    assert!((animator.blend_duration - 0.25).abs() < 1e-6);
    // 旧剪辑：定格在切换前的相位（一帧），再按自己的速度推进一帧。
    assert!(
        (animator.blend_from_time - 2.0 * DT).abs() < 1e-6,
        "blend_from_time = {}",
        animator.blend_from_time
    );
    assert!((animator.blend_from_speed - 1.0).abs() < 1e-6);
    assert!(animator.blend_from_looping);
    // 新剪辑从 0 起播，当帧推进一帧。
    assert!((animator.time - DT).abs() < 1e-6);
    assert!((animator.blend_elapsed - DT).abs() < 1e-6);
}

#[test]
fn fade_ends_after_its_duration_and_clears_the_old_side() {
    let (mut simulation, key) = spawn(
        MACHINE,
        Animator {
            fade_time: 0.25,
            ..Default::default()
        },
    );
    drive(&mut simulation, key, None);
    drive(&mut simulation, key, Some(1.0));
    // 切换帧后旧侧还在：13 帧时已淡 14/60 秒，没到 0.25。
    for _ in 0..13 {
        drive(&mut simulation, key, Some(1.0));
    }
    {
        let world = simulation.world();
        let animator = world.get::<Animator>(key).unwrap();
        assert!(animator.blend_duration > 0.0);
        assert_eq!(animator.blend_from, "Survey");
    }
    // 再走 1 帧到 15/60 = 0.25 ⇒ 淡入完成，旧侧整份清空。
    drive(&mut simulation, key, Some(1.0));

    let world = simulation.into_world();
    let animator = world.get::<Animator>(key).expect("Animator 还在");
    assert_eq!(animator.blend_duration, 0.0);
    assert_eq!(animator.blend_elapsed, 0.0);
    assert!(animator.blend_from.is_empty());
    assert_eq!(animator.blend_from_time, 0.0);
    assert!((animator.blend_from_speed - 1.0).abs() < 1e-6);
    assert!(animator.blend_from_looping);
}

#[test]
fn hard_cut_clears_a_stale_fade() {
    let (mut simulation, key) = spawn(
        MACHINE_HARD_CUT,
        Animator {
            fade_time: 0.0,
            blend_from: "Stale".to_string(),
            blend_from_time: 3.0,
            blend_from_speed: 2.0,
            blend_from_looping: false,
            blend_elapsed: 0.1,
            blend_duration: 0.5,
            ..Default::default()
        },
    );
    // 第 1 帧还留在 idle，残影被 AnimatorSystem 照常推进；第 2 帧硬切时才清。
    drive(&mut simulation, key, None);
    drive(&mut simulation, key, Some(1.0));

    let world = simulation.into_world();
    let animator = world.get::<Animator>(key).expect("Animator 还在");
    assert_eq!(animator.clip, "Walk");
    assert_eq!(animator.blend_duration, 0.0);
    assert_eq!(animator.blend_elapsed, 0.0);
    assert!(animator.blend_from.is_empty());
    assert_eq!(animator.blend_from_time, 0.0);
    assert!((animator.blend_from_speed - 1.0).abs() < 1e-6);
    assert!(animator.blend_from_looping);
}
