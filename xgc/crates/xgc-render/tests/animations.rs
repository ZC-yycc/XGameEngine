//! Fox.gltf 自带动画的解析与采样回归：迁移世界的狐狸没有骨骼实体，
//! 动画只能从模型里读；这里钉住剪辑清单与采样行为，防止解析悄悄退化。

use std::path::Path;

use xgc_render::{Interpolation, MeshCache};

const RESOURCES: &str = "../../../projects/XGame/Resources";

#[test]
fn fox_gltf_exposes_survey_walk_and_run_clips() {
    let root = Path::new(RESOURCES);
    if !root.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let mut cache = MeshCache::new(root);
    let mesh = cache.get("Fox.gltf").unwrap().unwrap();
    let animations = mesh.animations.as_ref().expect("Fox.gltf 自带动画剪辑");
    assert!(!animations.nodes.is_empty());
    for name in ["Survey", "Walk", "Run"] {
        let clip = animations
            .clip(name)
            .unwrap_or_else(|| panic!("缺剪辑 {name}"));
        assert!(clip.duration.is_finite() && clip.duration > 0.0);
        assert_eq!(clip.channels.len(), 21, "旧端 Fox 的每个剪辑 21 条通道");
        assert!(clip
            .channels
            .iter()
            .all(|channel| channel.interpolation == Interpolation::Linear));
    }
}

#[test]
fn walk_clip_samples_distinct_joint_poses() {
    let root = Path::new(RESOURCES);
    if !root.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let mut cache = MeshCache::new(root);
    let mesh = cache.get("Fox.gltf").unwrap().unwrap();
    let animations = mesh.animations.as_ref().unwrap();
    let joints = &mesh.skin.as_ref().expect("Fox.gltf 带蒙皮").joint_nodes;
    assert!(!joints.is_empty());

    let start = animations
        .joint_pose("Walk", 0.0, true, joints)
        .expect("Walk 在 t=0 可采样");
    assert_eq!(start.len(), joints.len());
    let middle = animations
        .joint_pose("Walk", 0.5, true, joints)
        .expect("Walk 在 t=0.5 可采样");
    assert_eq!(middle.len(), joints.len());
    assert!(
        start
            .iter()
            .zip(&middle)
            .any(|(a, b)| a.iter().zip(b).any(|(x, y)| (x - y).abs() > 1e-6)),
        "t=0 与 t=0.5 的关节矩阵应不同"
    );
}

#[test]
fn missing_clip_or_bad_time_returns_none() {
    let root = Path::new(RESOURCES);
    if !root.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let mut cache = MeshCache::new(root);
    let mesh = cache.get("Fox.gltf").unwrap().unwrap();
    let animations = mesh.animations.as_ref().unwrap();
    let joints = &mesh.skin.as_ref().unwrap().joint_nodes;
    assert!(animations.joint_pose("Nope", 0.0, true, joints).is_none());
    assert!(animations
        .joint_pose("Walk", f32::NAN, true, joints)
        .is_none());
    assert!(animations.joint_pose("Walk", 0.0, true, &[]).is_none());
}

#[test]
fn additive_layer_adds_delta_on_top_of_the_base_pose() {
    use xgc_render::AdditiveLayer;

    let root = Path::new(RESOURCES);
    if !root.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let mut cache = MeshCache::new(root);
    let mesh = cache.get("Fox.gltf").unwrap().unwrap();
    let animations = mesh.animations.as_ref().unwrap();
    let joints = &mesh.skin.as_ref().unwrap().joint_nodes;

    let base = animations
        .joint_pose("Walk", 0.4, true, joints)
        .expect("Walk 可采样");
    // 权重 0：增量被完全乘掉，必须与纯基础姿势逐位一致（旧端 blend_additive 同式）。
    let zero = animations
        .joint_pose_with_additive(
            "Walk",
            0.4,
            true,
            Some(AdditiveLayer {
                clip: "Run",
                time: 0.5,
                looping: true,
                weight: 0.0,
            }),
            joints,
        )
        .expect("叠加层可采样");
    assert_eq!(zero, base, "weight=0 的叠加层不能改动基础姿势");

    // 权重 1：Run 相对它第 0 帧的增量要真的加上去。
    let full = animations
        .joint_pose_with_additive(
            "Walk",
            0.4,
            true,
            Some(AdditiveLayer {
                clip: "Run",
                time: 0.5,
                looping: true,
                weight: 1.0,
            }),
            joints,
        )
        .expect("叠加层可采样");
    assert!(
        full.iter()
            .zip(&base)
            .any(|(a, b)| a.iter().zip(b).any(|(x, y)| (x - y).abs() > 1e-4)),
        "weight=1 的叠加层应改变关节矩阵"
    );

    // 叠加剪辑不存在：退回基础姿势，不报错、不把角色画没。
    let missing = animations
        .joint_pose_with_additive(
            "Walk",
            0.4,
            true,
            Some(AdditiveLayer {
                clip: "NoSuchClip",
                time: 0.5,
                looping: true,
                weight: 1.0,
            }),
            joints,
        )
        .expect("叠加剪辑缺失时仍应返回基础姿势");
    assert_eq!(missing, base);
}

#[test]
fn box_glb_has_no_animations() {
    let root = Path::new(RESOURCES);
    if !root.join("Box.glb").exists() {
        eprintln!("skip: 没有 Box.glb");
        return;
    }
    let mut cache = MeshCache::new(root);
    let mesh = cache.get("Box.glb").unwrap().unwrap();
    assert!(
        mesh.animations.is_none(),
        "Box.glb 没有 animations，别凭空造剪辑"
    );
}
