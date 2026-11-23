//! 根运动落地回归：实体位移、循环展开、轴过滤、父节点换算。

use xgc_sim::apply_root_motion;
use xgc_world::{
    Animator, GlobalTransform, MeshRenderer, RootMotionClip, RootMotionLibrary,
    RootMotionNodeTrack, RootMotionSet, RootMotionTrack, Transform, World,
};

const MODEL: &str = "Fox.gltf";

fn set(times: Vec<f32>, values: Vec<[f32; 3]>, node: &str) -> RootMotionSet {
    let track = RootMotionTrack::new(times, values, false).expect("轨道应有效");
    RootMotionSet {
        clips: vec![RootMotionClip {
            name: "Walk".to_string(),
            duration: 1.0,
            tracks: vec![RootMotionNodeTrack {
                node: node.to_string(),
                track,
            }],
        }],
    }
}

fn library(set: RootMotionSet) -> RootMotionLibrary {
    let mut library = RootMotionLibrary::new();
    library.insert(MODEL.to_string(), set);
    library
}

fn animator(time: f32, looping: bool, root_motion: &str) -> Animator {
    Animator {
        clip: "Walk".to_string(),
        speed: 1.0,
        looping,
        autoplay: true,
        time,
        root_motion: root_motion.to_string(),
        ..Animator::default()
    }
}

fn spawn(world: &mut World, animator: Animator) -> xgc_base::EntityKey {
    world.spawn((
        Transform::default(),
        GlobalTransform::default(),
        MeshRenderer {
            model: MODEL.to_string(),
        },
        animator,
    ))
}

fn position(world: &World, key: xgc_base::EntityKey) -> [f32; 3] {
    world
        .get::<Transform>(key)
        .expect("实体有 Transform")
        .position
}

#[test]
fn moves_the_entity_by_the_clip_delta() {
    let mut world = World::new();
    let key = spawn(
        &mut world,
        animator(0.5, true, r#"(node: "hip", axes: (true, false, true))"#),
    );
    let library = library(set(
        vec![0.0, 1.0],
        vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
        "hip",
    ));
    apply_root_motion(&mut world, 0.5, &library);
    let position = position(&world, key);
    assert!((position[0] - 1.0).abs() < 1e-6, "{position:?}");
    let global = world.get::<GlobalTransform>(key).unwrap();
    assert!((global.position[0] - 1.0).abs() < 1e-6, "{global:?}");
}

#[test]
fn looping_clips_unfold_across_the_boundary() {
    let mut world = World::new();
    let key = spawn(
        &mut world,
        animator(1.5, true, r#"(node: "hip", axes: (true, false, true))"#),
    );
    let library = library(set(
        vec![0.0, 1.0],
        vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
        "hip",
    ));
    apply_root_motion(&mut world, 1.0, &library);
    let position = position(&world, key);
    assert!((position[0] - 2.0).abs() < 1e-6, "{position:?}");
}

#[test]
fn axis_filter_and_missing_config_leave_the_entity_alone() {
    let mut world = World::new();
    let filtered = spawn(
        &mut world,
        animator(0.5, true, r#"(node: "hip", axes: (false, true, false))"#),
    );
    let empty = spawn(&mut world, animator(0.5, true, ""));
    let paused = {
        let mut animator = animator(0.5, true, r#"(node: "hip", axes: (true, true, true))"#);
        animator.autoplay = false;
        spawn(&mut world, animator)
    };
    let library = library(set(
        vec![0.0, 1.0],
        vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
        "hip",
    ));
    apply_root_motion(&mut world, 0.5, &library);
    for key in [filtered, empty, paused] {
        assert_eq!(position(&world, key), [0.0; 3]);
    }
}

#[test]
fn unknown_node_name_is_a_no_op() {
    let mut world = World::new();
    let key = spawn(
        &mut world,
        animator(0.5, true, r#"(node: "missing", axes: (true, false, true))"#),
    );
    let library = library(set(
        vec![0.0, 1.0],
        vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
        "hip",
    ));
    apply_root_motion(&mut world, 0.5, &library);
    assert_eq!(position(&world, key), [0.0; 3]);
}

#[test]
fn entity_scale_and_rotation_apply_to_the_world_delta() {
    let mut world = World::new();
    let half = std::f32::consts::FRAC_PI_4;
    let global = GlobalTransform {
        scale: [0.01; 3],
        rotation: [0.0, half.sin(), 0.0, half.cos()],
        ..GlobalTransform::default()
    };
    let key = world.spawn((
        Transform {
            scale: [0.01; 3],
            ..Transform::default()
        },
        global,
        MeshRenderer {
            model: MODEL.to_string(),
        },
        animator(0.5, true, r#"(node: "hip", axes: (true, true, true))"#),
    ));
    let library = library(set(
        vec![0.0, 1.0],
        vec![[0.0, 0.0, 0.0], [0.0, 0.0, 2.0]],
        "hip",
    ));
    apply_root_motion(&mut world, 0.5, &library);
    let position = position(&world, key);
    // 半秒走完 0→1 的采样值（轨道末值 2 ⇒ t=0.5 是 1），缩放 0.01、绕 Y 90° ⇒ 世界 +X 0.01。
    assert!((position[0] - 0.01).abs() < 1e-6, "{position:?}");
    assert!(position[2].abs() < 1e-6, "{position:?}");
}

#[test]
fn blend_fade_splits_the_delta_between_the_two_clips() {
    let mut world = World::new();
    let mut animator = animator(0.5, true, r#"(node: "hip", axes: (true, false, true))"#);
    animator.blend_from = "Run".to_string();
    animator.blend_from_time = 0.5;
    animator.blend_elapsed = 0.1;
    animator.blend_duration = 0.4;
    let key = spawn(&mut world, animator);
    let track = RootMotionTrack::new(
        vec![0.0, 1.0],
        vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
        false,
    )
    .unwrap();
    let run_track = RootMotionTrack::new(
        vec![0.0, 1.0],
        vec![[0.0, 0.0, 0.0], [6.0, 0.0, 0.0]],
        false,
    )
    .unwrap();
    let set = RootMotionSet {
        clips: vec![
            RootMotionClip {
                name: "Walk".to_string(),
                duration: 1.0,
                tracks: vec![RootMotionNodeTrack {
                    node: "hip".to_string(),
                    track,
                }],
            },
            RootMotionClip {
                name: "Run".to_string(),
                duration: 1.0,
                tracks: vec![RootMotionNodeTrack {
                    node: "hip".to_string(),
                    track: run_track,
                }],
            },
        ],
    };
    let library = library(set);
    apply_root_motion(&mut world, 0.5, &library);
    // 旧剪辑（Run）这半秒走 3 单位、新剪辑（Walk）走 1 单位，alpha = 0.25 ⇒ 3*0.75 + 1*0.25 = 2.5。
    let position = position(&world, key);
    assert!((position[0] - 2.5).abs() < 1e-5, "{position:?}");
}
