//! 预制体值写回和还原都按实例根与来源身份定位。

use std::f32::consts::FRAC_1_SQRT_2;

use xgc_base::EntityId;
use xgc_world::{
    apply_prefab_values, instantiate_prefab, revert_prefab, Command, Component, ComponentData,
    GlobalTransform, Name, Parent, PluginComponent, PrefabInstance, Script, Transform, Value,
    World, WorldData,
};

const SOURCE: &str = "Prefabs/fixture.json";

fn test_asset() -> WorldData {
    let mut world = World::new();
    let root = world.spawn((
        Name {
            value: "root".into(),
        },
        Transform::default(),
    ));
    let root = world.entity(root).unwrap();
    world.spawn((
        Name {
            value: "child".into(),
        },
        Parent { entity: root },
    ));
    world.world_data()
}

fn attach(world: &mut World, entity: EntityId, parent: EntityId) {
    let plan = world.plan(vec![Command::AddComponent {
        entity,
        component: ComponentData {
            component: Parent::ID,
            fields: vec![("entity".into(), Value::Entity(parent.raw()))],
        },
    }]);
    world.apply(&plan).unwrap();
}

#[test]
fn apply_values_preserves_source_identity_and_asset_space() {
    let mut prefab = World::new();
    let placement = Transform {
        position: [4.0, 0.0, 0.0],
        ..Transform::default()
    };
    let asset_root = prefab.spawn((
        Name {
            value: "root".into(),
        },
        placement,
        GlobalTransform::from_local(&placement),
    ));
    let asset_root = prefab.entity(asset_root).unwrap();
    let hinge = Transform {
        position: [0.0, 2.0, 0.0],
        ..Transform::default()
    };
    let asset_hinge = prefab.spawn((
        hinge,
        GlobalTransform::default(),
        Parent { entity: asset_root },
    ));
    let asset_hinge = prefab.entity(asset_hinge).unwrap();
    let asset_child = prefab.spawn((
        Transform::default(),
        GlobalTransform::default(),
        Parent { entity: asset_root },
    ));
    let asset_child = prefab.entity(asset_child).unwrap();
    let mut asset = prefab.world_data();
    asset.next_id += 10;
    asset.entities.reverse();
    for entity in &mut asset.entities {
        entity.components.reverse();
    }

    let mut world = World::new();
    let outside = world.spawn((Transform::default(),));
    let outside = world.entity(outside).unwrap();
    let instance = instantiate_prefab(&mut world, &asset, Some(SOURCE)).unwrap();
    let (_, root) = instance[0];
    let (_, hinge) = instance[1];
    let (_, child) = instance[2];
    attach(&mut world, root, outside);
    let root_key = world.key(root).unwrap();
    let child_key = world.key(child).unwrap();
    world.get_mut::<Name>(root_key).unwrap().value = "changed".into();
    world.get_mut::<Transform>(root_key).unwrap().position = [10.0, 20.0, 30.0];
    world.get_mut::<GlobalTransform>(root_key).unwrap().position = [100.0; 3];
    world.get_mut::<Parent>(child_key).unwrap().entity = hinge;
    world.get_mut::<Transform>(child_key).unwrap().position = [1.0, 3.0, 0.0];
    world
        .get_mut::<GlobalTransform>(child_key)
        .unwrap()
        .position = [200.0; 3];
    let before = world.world_data();

    let applied = apply_prefab_values(&world, &asset, child).unwrap();

    assert_eq!(world.world_data(), before);
    assert_eq!(applied.revision, asset.revision);
    assert_eq!(applied.next_id, asset.next_id);
    for (asset, applied) in asset.entities.iter().zip(&applied.entities) {
        assert_eq!(applied.id, asset.id);
        assert_eq!(applied.revision, asset.revision);
        assert!(applied
            .components
            .iter()
            .map(|data| data.component)
            .eq(asset.components.iter().map(|data| data.component)));
    }
    prefab.restore_world_data(&applied).unwrap();
    let root_key = prefab.key(asset_root).unwrap();
    let child_key = prefab.key(asset_child).unwrap();
    assert_eq!(prefab.get::<Name>(root_key).unwrap().value, "changed");
    assert_eq!(prefab.get::<Transform>(root_key), Some(&placement));
    assert_eq!(
        prefab.get::<GlobalTransform>(root_key).unwrap().position,
        [4.0, 0.0, 0.0]
    );
    assert!(prefab.get::<Parent>(root_key).is_none());
    assert_eq!(prefab.get::<Parent>(child_key).unwrap().entity, asset_hinge);
    assert_eq!(
        prefab.get::<Transform>(child_key).unwrap().position,
        [1.0, 3.0, 0.0]
    );
    assert_eq!(
        prefab.get::<GlobalTransform>(child_key).unwrap().position,
        [5.0, 5.0, 0.0]
    );
}

#[test]
fn apply_values_rejects_structure_and_bad_asset_identity() {
    let mut prefab = World::new();
    let asset_root = prefab.spawn((
        Transform::default(),
        Script {
            path: "Scripts/initial.lua".into(),
        },
        PluginComponent {
            registered_name: "initial".into(),
            fields: "before".into(),
        },
    ));
    let asset_root = prefab.entity(asset_root).unwrap();
    let asset = prefab.world_data();
    let mut world = World::new();
    let instance = instantiate_prefab(&mut world, &asset, Some(SOURCE)).unwrap();
    let root = instance[0].1;
    let root_key = world.key(root).unwrap();
    let mut duplicate = asset.clone();
    duplicate.entities.push(asset.entities[0].clone());
    assert!(apply_prefab_values(&world, &duplicate, root).is_err());

    world.get_mut::<PluginComponent>(root_key).unwrap().fields = "after".into();
    let changed = apply_prefab_values(&world, &asset, root).unwrap();
    prefab.restore_world_data(&changed).unwrap();
    assert_eq!(
        prefab
            .get::<PluginComponent>(prefab.key(asset_root).unwrap())
            .unwrap()
            .fields,
        "after"
    );
    world
        .get_mut::<PluginComponent>(root_key)
        .unwrap()
        .registered_name = "different".into();
    assert!(apply_prefab_values(&world, &asset, root).is_err());
    world
        .get_mut::<PluginComponent>(root_key)
        .unwrap()
        .registered_name = "initial".into();
    world.get_mut::<Script>(root_key).unwrap().path = "Scripts/different.lua".into();
    assert!(apply_prefab_values(&world, &asset, root).is_err());
    world.get_mut::<Script>(root_key).unwrap().path = "Scripts/initial.lua".into();

    let extra = world.spawn((Parent { entity: root },));
    assert!(apply_prefab_values(&world, &asset, root).is_err());
    world.despawn(extra);
    let plan = world.plan(vec![Command::RemoveComponent {
        entity: root,
        component: Script::ID,
    }]);
    world.apply(&plan).unwrap();
    assert!(apply_prefab_values(&world, &asset, root).is_err());
}

#[test]
fn revert_isolates_same_source_instances() {
    let mut asset = test_asset();
    let mut world = World::new();
    let outer_parent = world.spawn((Transform::default(),));
    let outer_parent = world.entity(outer_parent).unwrap();
    let first = instantiate_prefab(&mut world, &asset, Some(SOURCE)).unwrap();
    let second = instantiate_prefab(&mut world, &asset, Some(SOURCE)).unwrap();
    attach(&mut world, first[0].1, outer_parent);
    attach(&mut world, second[0].1, first[0].1);
    let other_root = world.entity_data(second[0].1).unwrap();
    let other_child = world.entity_data(second[1].1).unwrap();
    asset.entities.pop();

    let mapping = revert_prefab(&mut world, &asset, first[1].1).unwrap();

    assert_eq!(world.entity_data(second[0].1), Some(other_root));
    assert_eq!(world.entity_data(second[1].1), Some(other_child));
    assert_eq!(mapping, first[..1]);
    assert!(world.key(first[1].1).is_none());
    assert_eq!(
        world.get::<Parent>(world.key(first[0].1).unwrap()),
        Some(&Parent {
            entity: outer_parent
        })
    );

    let outer_root = world.entity_data(first[0].1).unwrap();
    let mapping = revert_prefab(&mut world, &asset, second[1].1).unwrap();
    assert_eq!(mapping, second[..1]);
    assert!(world.key(second[1].1).is_none());
    assert_eq!(world.entity_data(first[0].1), Some(outer_root));
    assert_eq!(
        world.get::<Parent>(world.key(second[0].1).unwrap()),
        Some(&Parent { entity: first[0].1 })
    );
}

#[test]
fn revert_parent_walk_stops_at_source_boundary() {
    let asset = test_asset();
    let mut world = World::new();
    let mapping = instantiate_prefab(&mut world, &asset, Some(SOURCE)).unwrap();
    let child = world.key(mapping[1].1).unwrap();
    world.get_mut::<PrefabInstance>(child).unwrap().source = "different".into();
    let before = world.world_data();

    assert!(revert_prefab(&mut world, &asset, mapping[1].1).is_err());
    assert_eq!(world.world_data(), before);

    let current = revert_prefab(&mut world, &asset, mapping[0].1).unwrap();
    assert_ne!(current[1].1, mapping[1].1);
    assert_eq!(
        world.entity_data(mapping[1].1),
        Some(before.entities[1].clone())
    );
}

#[test]
fn revert_restores_child_poses_in_the_instance_parent_chain() {
    let mut prefab = World::new();
    let root = prefab.spawn((Transform::default(), GlobalTransform::default()));
    let root = prefab.entity(root).unwrap();
    let child_pose = Transform {
        position: [1.0, 2.0, 3.0],
        rotation: [0.0, FRAC_1_SQRT_2, 0.0, FRAC_1_SQRT_2],
        scale: [4.0, 2.0, 1.0],
    };
    let child = prefab.spawn((
        child_pose,
        GlobalTransform::from_local(&child_pose),
        Parent { entity: root },
    ));
    let grandchild_pose = Transform {
        position: [1.0, 0.0, -2.0],
        ..Transform::default()
    };
    prefab.spawn((
        grandchild_pose,
        GlobalTransform::from_local(&grandchild_pose),
        Parent {
            entity: prefab.entity(child).unwrap(),
        },
    ));
    let mut asset = prefab.world_data();
    // 父链不能依赖文件的实体顺序，迁移后的全局值也可能只有局部变换。
    asset.entities.reverse();

    let mut world = World::new();
    let ancestor_pose = Transform {
        position: [10.0, 0.0, 2.0],
        rotation: [0.0, 0.0, FRAC_1_SQRT_2, FRAC_1_SQRT_2],
        scale: [2.0, 3.0, 1.0],
    };
    let ancestor = world.spawn((ancestor_pose, GlobalTransform::from_local(&ancestor_pose)));
    let outer_pose = Transform {
        position: [2.0, 4.0, 0.0],
        ..Transform::default()
    };
    let outer = world.spawn((
        outer_pose,
        GlobalTransform::default(),
        Parent {
            entity: world.entity(ancestor).unwrap(),
        },
    ));
    let outer = world.entity(outer).unwrap();
    let first = instantiate_prefab(&mut world, &asset, Some(SOURCE)).unwrap();
    let second = instantiate_prefab(&mut world, &asset, Some(SOURCE)).unwrap();
    attach(&mut world, first[0].1, outer);
    attach(&mut world, second[0].1, outer);
    let other_entities: Vec<_> = second
        .iter()
        .map(|(_, id)| world.entity_data(*id).unwrap())
        .collect();

    let placement = Transform {
        position: [5.0, 2.0, -1.0],
        rotation: [FRAC_1_SQRT_2, 0.0, 0.0, FRAC_1_SQRT_2],
        scale: [3.0, 1.0, 2.0],
    };
    let root_key = world.key(first[0].1).unwrap();
    *world.get_mut::<Transform>(root_key).unwrap() = placement;
    let child_key = world.key(first[1].1).unwrap();
    *world.get_mut::<Transform>(child_key).unwrap() = Transform::default();
    *world.get_mut::<GlobalTransform>(child_key).unwrap() = GlobalTransform::default();

    let outer_global =
        GlobalTransform::compose(&GlobalTransform::from_local(&ancestor_pose), &outer_pose);
    let root_global = GlobalTransform::compose(&outer_global, &placement);
    let child_global = GlobalTransform::compose(&root_global, &child_pose);
    let grandchild_global = GlobalTransform::compose(&child_global, &grandchild_pose);

    // 从实例成员触发首次还原，然后从根重复操作。
    for selected in [first[1].1, first[0].1] {
        assert_eq!(revert_prefab(&mut world, &asset, selected).unwrap(), first);
        let root = world.key(first[0].1).unwrap();
        assert_eq!(world.get::<Parent>(root), Some(&Parent { entity: outer }));
        assert_eq!(world.get::<Transform>(root), Some(&placement));
        let child = world.key(first[1].1).unwrap();
        assert_eq!(world.get::<Transform>(child), Some(&child_pose));
        assert_eq!(world.get::<GlobalTransform>(root), Some(&root_global));
        assert_eq!(world.get::<GlobalTransform>(child), Some(&child_global));
        assert_eq!(
            world.get::<GlobalTransform>(world.key(first[2].1).unwrap()),
            Some(&grandchild_global)
        );
        for data in &other_entities {
            assert_eq!(world.entity_data(data.id).as_ref(), Some(data));
        }
    }
}

#[test]
fn revert_empty_asset_leaves_world_unchanged() {
    let mut world = World::new();
    let mapping = instantiate_prefab(&mut world, &test_asset(), Some(SOURCE)).unwrap();
    let before = world.world_data();

    let result = revert_prefab(&mut world, &WorldData::default(), mapping[0].1);

    assert_eq!(world.world_data(), before);
    assert!(result.is_err());
}
