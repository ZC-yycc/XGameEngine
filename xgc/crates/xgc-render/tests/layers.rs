//! 动画混合层回归：Override / Additive × 关节遮罩在模型局部姿势上生效。
//!
//! 遮罩是子树根名单（与旧端 `mask_covers` 同口径）；这里用 Fox 的
//! `b_Spine01_02` 当分界线：脊柱及其子孙被覆盖，髋骨 / 尾巴 / 腿必须逐位不变。

use std::path::Path;

use xgc_render::{LayerSample, MeshCache, PlayedClip};
use xgc_world::LayerMode;

const RESOURCES: &str = "../../../projects/XGame/Resources";
// Fox 关节顺序（glTF skin.joints）：6 = b_Head_05（用于 additive 断言的可读索引）。

fn pose_delta(a: &[[f32; 16]], b: &[[f32; 16]]) -> f32 {
    let mut max = 0.0f32;
    for (left, right) in a.iter().zip(b) {
        for (x, y) in left.iter().zip(right) {
            max = max.max((x - y).abs());
        }
    }
    max
}

fn joint_delta(a: &[f32; 16], b: &[f32; 16]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max)
}

/// 遮罩 `b_Spine01_02` 覆盖的关节下标（Fox skin：b_Spine01_02 及其子孙 = 3..=12）。
const COVERED: std::ops::RangeInclusive<usize> = 3..=12;

fn max_delta(a: &[[f32; 16]], b: &[[f32; 16]], indices: impl Iterator<Item = usize>) -> f32 {
    indices
        .map(|index| joint_delta(&a[index], &b[index]))
        .fold(0.0f32, f32::max)
}

fn load_fox() -> Option<(MeshCache, String)> {
    let root = Path::new(RESOURCES);
    if !root.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return None;
    }
    let cache = MeshCache::new(root);
    Some((cache, "Fox.gltf".to_string()))
}

fn walked(
    animations: &xgc_render::AnimationSet,
    joints: &[usize],
    layers: &[LayerSample],
) -> Vec<[f32; 16]> {
    animations
        .joint_pose_played(
            PlayedClip {
                clip: "Walk",
                time: 0.4,
                looping: true,
                root_motion: None,
            },
            None,
            None,
            layers,
            joints,
        )
        .expect("Walk 可采样")
}

#[test]
fn override_layer_only_touches_masked_joints() {
    let Some((mut cache, model)) = load_fox() else {
        return;
    };
    let mesh = cache.get(&model).unwrap().unwrap();
    let animations = mesh.animations.as_ref().unwrap();
    let joints = &mesh.skin.as_ref().unwrap().joint_nodes;
    let base = animations.joint_pose("Walk", 0.4, true, joints).unwrap();
    let layered = walked(
        animations,
        joints,
        &[LayerSample {
            mode: LayerMode::Override,
            clip: "Survey".into(),
            time: 0.7,
            looping: true,
            weight: 1.0,
            fade: None,
            mask: vec!["b_Spine01_02".into()],
        }],
    );
    let covered = max_delta(&base, &layered, COVERED.clone());
    assert!(
        covered > 1e-3,
        "遮罩内关节必须被覆盖（max delta {covered}）"
    );
    let uncovered = max_delta(
        &base,
        &layered,
        (0..base.len()).filter(|index| !COVERED.contains(index)),
    );
    assert!(
        uncovered < 1e-6,
        "遮罩外关节必须逐位不变（max delta {uncovered}）"
    );
}

#[test]
fn override_weight_zero_matches_the_base_pose() {
    let Some((mut cache, model)) = load_fox() else {
        return;
    };
    let mesh = cache.get(&model).unwrap().unwrap();
    let animations = mesh.animations.as_ref().unwrap();
    let joints = &mesh.skin.as_ref().unwrap().joint_nodes;
    let base = animations.joint_pose("Walk", 0.4, true, joints).unwrap();
    let layered = walked(
        animations,
        joints,
        &[LayerSample {
            mode: LayerMode::Override,
            clip: "Survey".into(),
            time: 0.7,
            looping: true,
            weight: 0.0,
            fade: None,
            mask: vec!["b_Spine01_02".into()],
        }],
    );
    assert!(
        pose_delta(&base, &layered) < 1e-6,
        "权重 0 必须逐位等于基础姿势"
    );
}

#[test]
fn empty_base_clip_starts_from_the_bind_pose() {
    let Some((mut cache, model)) = load_fox() else {
        return;
    };
    let mesh = cache.get(&model).unwrap().unwrap();
    let animations = mesh.animations.as_ref().unwrap();
    let joints = &mesh.skin.as_ref().unwrap().joint_nodes;
    let expected = animations.joint_pose("Survey", 0.7, true, joints).unwrap();
    let pose = animations
        .joint_pose_played(
            PlayedClip {
                clip: "",
                time: 0.0,
                looping: true,
                root_motion: None,
            },
            None,
            None,
            &[LayerSample {
                mode: LayerMode::Override,
                clip: "Survey".into(),
                time: 0.7,
                looping: true,
                weight: 1.0,
                fade: None,
                mask: Vec::new(),
            }],
            joints,
        )
        .expect("只有层栈也能出姿势");
    assert!(pose_delta(&expected, &pose) < 1e-5);
}

#[test]
fn additive_layer_adds_delta_on_masked_joints_only() {
    let Some((mut cache, model)) = load_fox() else {
        return;
    };
    let mesh = cache.get(&model).unwrap().unwrap();
    let animations = mesh.animations.as_ref().unwrap();
    let joints = &mesh.skin.as_ref().unwrap().joint_nodes;
    let base = animations.joint_pose("Walk", 0.4, true, joints).unwrap();
    let layered = walked(
        animations,
        joints,
        &[LayerSample {
            mode: LayerMode::Additive,
            clip: "Survey".into(),
            time: 0.7,
            looping: true,
            weight: 1.0,
            fade: None,
            mask: vec!["b_Spine01_02".into()],
        }],
    );
    let covered = max_delta(&base, &layered, COVERED.clone());
    assert!(
        covered > 1e-3,
        "additive 层要在遮罩内加增量（max delta {covered}）"
    );
    let uncovered = max_delta(
        &base,
        &layered,
        (0..base.len()).filter(|index| !COVERED.contains(index)),
    );
    assert!(uncovered < 1e-6, "遮罩外不加增量（max delta {uncovered}）");
}

#[test]
fn missing_layer_clip_falls_back_to_the_base_pose() {
    let Some((mut cache, model)) = load_fox() else {
        return;
    };
    let mesh = cache.get(&model).unwrap().unwrap();
    let animations = mesh.animations.as_ref().unwrap();
    let joints = &mesh.skin.as_ref().unwrap().joint_nodes;
    let base = animations.joint_pose("Walk", 0.4, true, joints).unwrap();
    let layered = walked(
        animations,
        joints,
        &[LayerSample {
            mode: LayerMode::Override,
            clip: "Nope".into(),
            time: 0.7,
            looping: true,
            weight: 1.0,
            fade: None,
            mask: Vec::new(),
        }],
    );
    assert!(pose_delta(&base, &layered) < 1e-6, "缺剪辑的层静默跳过");
}
