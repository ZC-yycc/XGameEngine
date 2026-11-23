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
