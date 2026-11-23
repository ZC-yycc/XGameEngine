//! 交叉淡入回归：alpha=0 完全等于旧剪辑、alpha=1 等于新剪辑、中间是真混合。
//!
//! 迁移世界的 Fox 由状态机驱动（旧端 frame-300 是 Walk→Run 的淡入帧），渲染侧
//! 必须能用旧剪辑自己的时钟采样出淡出的那半边。

use std::path::Path;

use xgc_render::{CrossfadeLayer, MeshCache, PlayedClip};

const RESOURCES: &str = "../../../projects/XGame/Resources";

/// 两份关节矩阵的最大逐分量差（列主序 4×4）。
fn pose_delta(a: &[[f32; 16]], b: &[[f32; 16]]) -> f32 {
    a.iter()
        .zip(b)
        .flat_map(|(left, right)| left.iter().zip(right))
        .map(|(left, right)| (left - right).abs())
        .fold(0.0_f32, f32::max)
}

#[test]
fn crossfade_moves_from_the_old_clip_to_the_new_clip() {
    let root = Path::new(RESOURCES);
    if !root.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let mut cache = MeshCache::new(root);
    let mesh = cache.get("Fox.gltf").unwrap().unwrap();
    let animations = mesh.animations.as_ref().unwrap();
    let joints = &mesh.skin.as_ref().expect("Fox.gltf 带蒙皮").joint_nodes;

    let old_pose = animations
        .joint_pose("Survey", 0.7, true, joints)
        .expect("旧剪辑可采样");
    let new_pose = animations
        .joint_pose("Walk", 0.4, true, joints)
        .expect("新剪辑可采样");
    let faded = |alpha: f32| {
        animations
            .joint_pose_played(
                PlayedClip {
                    clip: "Walk",
                    time: 0.4,
                    looping: true,
                    root_motion: None,
                },
                Some(CrossfadeLayer {
                    clip: "Survey",
                    time: 0.7,
                    looping: true,
                    alpha,
                }),
                None,
                &[],
                joints,
            )
            .expect("交叉淡入可采样")
    };

    let pure_old = faded(0.0);
    let pure_new = faded(1.0);
    let middle = faded(0.5);

    assert!(
        pose_delta(&pure_old, &old_pose) < 1e-5,
        "alpha=0 应等于旧剪辑（差 {}）",
        pose_delta(&pure_old, &old_pose)
    );
    assert!(
        pose_delta(&pure_new, &new_pose) < 1e-6,
        "alpha=1 应等于新剪辑（差 {}）",
        pose_delta(&pure_new, &new_pose)
    );
    assert!(
        pose_delta(&middle, &old_pose) > 1e-3,
        "alpha=0.5 不该还是旧剪辑"
    );
    assert!(
        pose_delta(&middle, &new_pose) > 1e-3,
        "alpha=0.5 不该已经是新剪辑"
    );
    // 混合结果的每个分量都必须是有限数（坏数据不许进 palette）。
    assert!(middle
        .iter()
        .all(|matrix| matrix.iter().all(|v| v.is_finite())));
}

#[test]
fn missing_old_clip_falls_back_to_the_new_one() {
    let root = Path::new(RESOURCES);
    if !root.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let mut cache = MeshCache::new(root);
    let mesh = cache.get("Fox.gltf").unwrap().unwrap();
    let animations = mesh.animations.as_ref().unwrap();
    let joints = &mesh.skin.as_ref().expect("Fox.gltf 带蒙皮").joint_nodes;
    let new_pose = animations.joint_pose("Walk", 0.4, true, joints).unwrap();
    let pose = animations
        .joint_pose_played(
            PlayedClip {
                clip: "Walk",
                time: 0.4,
                looping: true,
                root_motion: None,
            },
            Some(CrossfadeLayer {
                clip: "Nope",
                time: 0.1,
                looping: true,
                alpha: 0.0,
            }),
            None,
            &[],
            joints,
        )
        .expect("旧剪辑缺失时退回新剪辑");
    assert!(pose_delta(&pose, &new_pose) < 1e-6);
}

#[test]
fn root_motion_axis_strip_restores_the_bind_translation() {
    let root = Path::new(RESOURCES);
    if !root.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let mut cache = MeshCache::new(root);
    let mesh = cache.get("Fox.gltf").unwrap().unwrap();
    let animations = mesh.animations.as_ref().unwrap();
    let joints = &mesh.skin.as_ref().expect("Fox.gltf 带蒙皮").joint_nodes;
    let plain = animations.joint_pose("Walk", 0.6, true, joints).unwrap();
    let stripped = animations
        .joint_pose_played(
            PlayedClip {
                clip: "Walk",
                time: 0.6,
                looping: true,
                root_motion: Some(("b_Hip_01", [true, false, true])),
            },
            None,
            None,
            &[],
            joints,
        )
        .expect("根运动剥离可采样");
    // 髋骨在 Walk 里有水平位移：剥掉 X / Z 后必须真的不同。
    assert!(
        pose_delta(&stripped, &plain) > 1e-4,
        "剥离根运动轴后姿势应变化（差 {}）",
        pose_delta(&stripped, &plain)
    );
    // 三轴全关 = 不动；未知节点名同样逐位不变。
    let untouched = animations
        .joint_pose_played(
            PlayedClip {
                clip: "Walk",
                time: 0.6,
                looping: true,
                root_motion: Some(("b_Hip_01", [false, false, false])),
            },
            None,
            None,
            &[],
            joints,
        )
        .unwrap();
    assert!(pose_delta(&untouched, &plain) < 1e-6);
    let unknown = animations
        .joint_pose_played(
            PlayedClip {
                clip: "Walk",
                time: 0.6,
                looping: true,
                root_motion: Some(("missing_node", [true, true, true])),
            },
            None,
            None,
            &[],
            joints,
        )
        .unwrap();
    assert!(pose_delta(&unknown, &plain) < 1e-6);
}
