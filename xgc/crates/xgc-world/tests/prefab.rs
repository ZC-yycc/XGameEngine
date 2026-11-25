//! 还原范围由实例根和同源父链共同决定。

use xgc_base::EntityId;
use xgc_world::{
    instantiate_prefab, revert_prefab, Command, Component, ComponentData, Name, Parent,
    PrefabInstance, Transform, Value, World, WorldData,
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
fn revert_empty_asset_leaves_world_unchanged() {
    let mut world = World::new();
    let mapping = instantiate_prefab(&mut world, &test_asset(), Some(SOURCE)).unwrap();
    let before = world.world_data();

    let result = revert_prefab(&mut world, &WorldData::default(), mapping[0].1);

    assert_eq!(world.world_data(), before);
    assert!(result.is_err());
}
