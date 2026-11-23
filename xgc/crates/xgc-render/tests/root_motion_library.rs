//! 宿主共用的根运动轨道表：三个宿主（CLI / editor / player）都从
//! `MeshCache::root_motion_library` 建表，这里钉住"什么世界会建出表"，
//! 免得某天漏接后只有编辑器里角色不走路。

use std::path::Path;

use xgc_render::MeshCache;
use xgc_world::{Animator, MeshRenderer, Transform, World};

const RESOURCES: &str = "../../../projects/XGame/Resources";

fn fox_world(root_motion: &str) -> World {
    let mut world = World::new();
    world.spawn((
        Transform::default(),
        MeshRenderer {
            model: "Fox.gltf".to_string(),
        },
        Animator {
            clip: "Walk".to_string(),
            autoplay: true,
            root_motion: root_motion.to_string(),
            ..Animator::default()
        },
    ));
    world
}

#[test]
fn a_world_with_root_motion_builds_the_library() {
    let root = Path::new(RESOURCES);
    if !root.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let world = fox_world("(node:\"b_Hip_01\",axes:(true,false,true))");
    let mut meshes = MeshCache::new(root);
    let library = meshes.root_motion_library(&world);
    assert_eq!(library.len(), 1, "Fox 有根运动配置，表里应有它的轨道");
    let set = library.set("Fox.gltf").expect("模型应有根运动轨道");
    assert!(!set.is_empty(), "轨道集不能为空");
}

#[test]
fn a_world_without_root_motion_gets_an_empty_library() {
    let root = Path::new(RESOURCES);
    if !root.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let mut meshes = MeshCache::new(root);
    // 没有 Animator 的世界。
    let mut plain = World::new();
    plain.spawn((
        Transform::default(),
        MeshRenderer {
            model: "Fox.gltf".to_string(),
        },
    ));
    assert!(meshes.root_motion_library(&plain).is_empty());
    // 有 Animator 但没配根运动（空串 / 空节点名都算没配）。
    let off = fox_world("");
    assert!(meshes.root_motion_library(&off).is_empty());
    let empty_node = fox_world("(node:\"\",axes:(true,false,true))");
    assert!(meshes.root_motion_library(&empty_node).is_empty());
}
