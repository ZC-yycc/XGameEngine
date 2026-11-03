//! 父子刚体各自在世界空间模拟；回写不能再次叠上父刚体本步的位移。

use xgame_core::{Camera, Entity, PhysicsBody, PhysicsHandle, Scene, Transform};
use xgame_math::glam::{Quat, Vec3};
use xgame_runtime::{World, WorldConfig};

fn hierarchy_world(bridge: bool, reversed: bool) -> World {
    let mut scene = Scene::new("physics-hierarchy");
    let mut parent = Entity::new("parent");
    parent.transform = Transform {
        position: Vec3::new(1.0, 2.0, 3.0),
        rotation: Quat::from_rotation_y(0.4),
        scale: Vec3::splat(2.0),
    };
    parent.physics = Some(PhysicsBody::dynamic_box([0.1; 3]));
    let parent_id = scene.add_entity(parent);

    let ancestor = if bridge {
        let mut middle = Entity::new("bridge");
        middle.parent = Some(parent_id);
        middle.transform.position = Vec3::new(3.0, 0.0, 0.0);
        middle.transform.rotation = Quat::from_rotation_x(-0.3);
        middle.transform.scale = Vec3::splat(0.75);
        scene.add_entity(middle)
    } else {
        parent_id
    };
    let mut child = Entity::new("child");
    child.parent = Some(ancestor);
    child.transform.position = Vec3::new(4.0, 2.0, -1.0);
    child.transform.rotation = Quat::from_rotation_z(0.2);
    child.transform.scale = Vec3::new(1.25, 0.8, 1.1);
    child.physics = Some(PhysicsBody::dynamic_box([0.1; 3]));
    let child_id = scene.add_entity(child);

    let mut camera = Entity::new("camera");
    camera.parent = Some(child_id);
    camera.transform.position = Vec3::new(0.0, 1.0, -2.0);
    camera.camera = Some(Camera::default());
    scene.add_entity(camera);
    if reversed {
        scene.entities.reverse();
    }

    let mut world = World::new(
        scene,
        std::env::temp_dir(),
        WorldConfig {
            gravity: Vec3::ZERO,
            ..WorldConfig::default()
        },
    );
    for (name, velocity, angular) in [
        (
            "parent",
            Vec3::new(2.0, 0.5, -1.0),
            Vec3::new(0.2, 0.7, -0.1),
        ),
        (
            "child",
            Vec3::new(-1.0, 0.3, 0.5),
            Vec3::new(-0.3, 0.1, 0.4),
        ),
    ] {
        let entity = world
            .scene()
            .entities
            .iter()
            .find(|e| e.name == name)
            .unwrap();
        let handle = entity.component::<PhysicsHandle>().unwrap().0;
        world.physics_mut().set_velocity(handle, velocity);
        world.physics_mut().set_angular_velocity(handle, angular);
    }
    world
}

fn assert_hierarchy_matches_physics(bridge: bool, reversed: bool) {
    let mut world = hierarchy_world(bridge, reversed);
    let initial_parent = world
        .scene()
        .entities
        .iter()
        .find(|e| e.name == "parent")
        .unwrap()
        .transform
        .position;
    for frame in 0..90 {
        let report = world.update(1.0 / 60.0);
        assert_eq!(report.fixed_steps, 1, "必须真的执行一个物理步");
        for (index, entity) in world.scene().entities.iter().enumerate() {
            let Some(handle) = entity.component::<PhysicsHandle>() else {
                continue;
            };
            let physical = world.physics().body_transform(handle.0).unwrap();
            let visual = world.scene().world_transform(index);
            assert!(
                visual.position.distance(physical.position) < 2e-4,
                "frame={frame} entity={} visual={:?} physical={:?}",
                entity.name,
                visual.position,
                physical.position,
            );
            assert!(
                visual.rotation.dot(physical.rotation).abs() > 0.99999,
                "frame={frame} entity={} 的世界朝向必须与刚体一致",
                entity.name,
            );
        }
        let child = world
            .scene()
            .entities
            .iter()
            .find(|e| e.name == "child")
            .unwrap();
        assert_eq!(
            child.transform.scale,
            Vec3::new(1.25, 0.8, 1.1),
            "局部视觉缩放不能被物理回写吃掉"
        );
        let child_phys = world
            .physics()
            .body_transform(child.component::<PhysicsHandle>().unwrap().0)
            .unwrap();
        let child_world = Transform {
            scale: child.transform.scale * if bridge { 1.5 } else { 2.0 },
            ..child_phys
        }
        .local_matrix();
        let expected_camera = child_world.transform_point3(Vec3::new(0.0, 1.0, -2.0));
        assert!(
            world
                .camera_view()
                .unwrap()
                .position
                .distance(expected_camera)
                < 3e-4
        );
    }
    let parent = world
        .scene()
        .entities
        .iter()
        .find(|e| e.name == "parent")
        .unwrap();
    assert!(
        parent.transform.position.distance(initial_parent) > 1.0,
        "父刚体必须实际运动"
    );
}

#[test]
fn dynamic_parent_and_child_match_their_independent_physics_poses() {
    assert_hierarchy_matches_physics(false, false);
    assert_hierarchy_matches_physics(false, true);
}

#[test]
fn reversed_dynamic_hierarchy_matches_physics_through_a_visual_parent() {
    assert_hierarchy_matches_physics(true, false);
    assert_hierarchy_matches_physics(true, true);
}

/// 同一负载供旧 / 新二进制背靠背运行；不把错误实现的画面差异当成性能收益。
#[test]
#[ignore = "手动运行的物理层级成本基准，不进 CI"]
fn hierarchy_update_cost() {
    for parented in [false, true] {
        let mut scene = Scene::new("hierarchy-cost");
        let root = scene.add_entity(Entity::new("root"));
        for i in 0..512 {
            let mut e = Entity::new(format!("body{i}"));
            e.parent = parented.then_some(root);
            e.transform.position = Vec3::new(i as f32 * 4.0, 0.0, 0.0);
            e.physics = Some(PhysicsBody::dynamic_box([0.1; 3]));
            scene.add_entity(e);
        }
        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default());
        for _ in 0..30 {
            world.update(1.0 / 60.0);
        }
        let before = world.scene().entities[1].transform.position;
        let mut fixed_steps = 0;
        let start = std::time::Instant::now();
        for _ in 0..300 {
            fixed_steps += world.update(1.0 / 60.0).fixed_steps;
        }
        let us = start.elapsed().as_secs_f64() * 1e6 / 300.0;
        assert_eq!(fixed_steps, 300);
        assert_eq!(world.physics().stats().0, 512);
        assert!(world.scene().entities[1].transform.position.y < before.y - 1.0);
        println!("hierarchy_update_cost[parented={parented}]: {us:.2} us/frame");
    }
}
