use xgc_base::EntityId;
use xgc_sim::{HierarchySystem, System};
use xgc_world::{Component, GlobalTransform, Parent, Transform, Value, World};

#[test]
fn hierarchy_system_leaves_cycles_alone() {
    let mut world = World::new();
    let unchanged = GlobalTransform {
        position: [99.0, 3.0, 6.0],
        ..GlobalTransform::default()
    };
    let first_key = world.spawn((Transform::default(), Parent::default(), unchanged));
    let first = world.entity(first_key).unwrap();
    let second_key = world.spawn((Transform::default(), Parent { entity: first }, unchanged));
    let second = world.entity(second_key).unwrap();
    world.get_mut::<Parent>(first_key).unwrap().entity = second;
    let dependent = world.spawn((Transform::default(), Parent { entity: second }, unchanged));
    let root = world.spawn((Transform::default(), unchanged));
    let root_id = world.entity(root).unwrap();

    let mut hierarchy = HierarchySystem::default();
    let before = world.revision();
    let cycle_keys = [first_key, second_key, dependent];
    let cycle_revisions = cycle_keys.map(|key| {
        let id = world.entity(key).unwrap();
        world.entity_revision(id).unwrap()
    });
    hierarchy.run(&mut world, 0.0);
    assert_eq!(world.revision(), before + 3);
    assert_eq!(
        world.get::<GlobalTransform>(root),
        Some(&GlobalTransform::default())
    );
    for (key, revision) in cycle_keys.into_iter().zip(cycle_revisions) {
        assert_eq!(world.get::<GlobalTransform>(key), Some(&unchanged));
        assert_eq!(
            world.entity_revision(world.entity(key).unwrap()),
            Some(revision)
        );
    }

    // 类型化写 Parent 不推进 revision，仍要在下一次传播解开整条分支。
    world.get_mut::<Parent>(first_key).unwrap().entity = root_id;
    assert_eq!(world.revision(), before + 3);
    hierarchy.run(&mut world, 0.0);
    assert_eq!(world.revision(), before + 15);
    for key in cycle_keys {
        assert_eq!(
            world.get::<GlobalTransform>(key),
            Some(&GlobalTransform::default())
        );
    }
}

#[test]
fn hierarchy_resolves_parent_changes_and_reused_slots() {
    let mut world = World::new();
    let missing = EntityId::from_raw(42);
    let child = world.spawn((
        Transform {
            position: [1.0, 0.0, 0.0],
            ..Transform::default()
        },
        Parent { entity: missing },
        GlobalTransform::default(),
    ));
    let parent_local = Transform {
        position: [10.0, 0.0, 0.0],
        ..Transform::default()
    };
    let parent = world.spawn((parent_local, GlobalTransform::default()));
    let parent_id = world.entity(parent).unwrap();
    let only_local = world.spawn((parent_local,));
    let only_global = world.spawn((GlobalTransform::from_local(&parent_local),));
    let mut hierarchy = HierarchySystem::default();

    // 缺父、父不参与传播、父缺本地变换都应让子实体得到相同的位置。
    for alternate in [
        missing,
        world.entity(only_local).unwrap(),
        world.entity(only_global).unwrap(),
    ] {
        world.get_mut::<Parent>(child).unwrap().entity = alternate;
        hierarchy.run(&mut world, 0.0);
        assert_eq!(
            world.get::<GlobalTransform>(child).unwrap().position[0],
            1.0
        );
    }
    assert_eq!(
        world.get::<GlobalTransform>(only_global),
        Some(&GlobalTransform::default())
    );

    world.get_mut::<Parent>(child).unwrap().entity = parent_id;
    hierarchy.run(&mut world, 0.0);
    assert_eq!(
        world.get::<GlobalTransform>(child).unwrap().position[0],
        11.0
    );
    assert!(world.despawn(parent));
    let replacement = world.spawn((parent_local, GlobalTransform::default()));
    assert_eq!(replacement.index(), parent.index());
    assert_ne!(replacement.generation(), parent.generation());
    hierarchy.run(&mut world, 0.0);
    assert_eq!(
        world.get::<GlobalTransform>(child).unwrap().position[0],
        1.0
    );
    assert_eq!(
        world.get::<GlobalTransform>(replacement).unwrap().position[0],
        10.0
    );
}

#[test]
fn hierarchy_writeback_preserves_reflected_float_bits() {
    let local = std::hint::black_box(Transform {
        position: [-0.0, f32::from_bits(0x7f81_0011), f32::INFINITY],
        rotation: [1.0, -0.0, 0.0, 0.0],
        scale: [1.0, f32::NEG_INFINITY, 0.25],
    });
    let mut world = World::new();
    let key = world.spawn((local, GlobalTransform::default()));
    let reference = world.spawn((GlobalTransform::default(),));
    let reference_id = world.entity(reference).unwrap();
    HierarchySystem::default().run(&mut world, 0.0);

    // 与实际的反射入口比较位模式，覆盖普通相等比较看不见的 -0 和 NaN。
    for (field, value) in [
        ("position", Value::Vec3(local.position.map(f64::from))),
        ("rotation", Value::Quat(local.rotation.map(f64::from))),
        ("scale", Value::Vec3(local.scale.map(f64::from))),
    ] {
        world
            .set_field(reference_id, GlobalTransform::ID, field, &value)
            .unwrap();
    }
    let actual = world.get::<GlobalTransform>(key).unwrap();
    let expected = world.get::<GlobalTransform>(reference).unwrap();
    assert_eq!(
        actual.position.map(f32::to_bits),
        expected.position.map(f32::to_bits)
    );
    assert_eq!(
        actual.rotation.map(f32::to_bits),
        expected.rotation.map(f32::to_bits)
    );
    assert_eq!(
        actual.scale.map(f32::to_bits),
        expected.scale.map(f32::to_bits)
    );
}
