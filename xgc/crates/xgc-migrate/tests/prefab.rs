//! 预制体实例化：新 ID、父引用重写、一次性 Spawn。

use std::collections::BTreeSet;
use std::path::Path;

use xgc_base::EntityId;
use xgc_migrate::migrate_str;
use xgc_world::{instantiate_prefab, revert_prefab, Parent, Transform, World};

#[test]
fn pillar_prefab_instantiates_with_remapped_parents() {
    let path = Path::new("../../../projects/XGame/Prefabs/Pillar.prefab.ron");
    if !path.exists() {
        eprintln!("skip: 没有旧项目 Pillar 预制体");
        return;
    }
    let (data, _) = migrate_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut world = World::new();
    world.spawn((Transform::default(),));
    let before = world.len();
    let mapping = instantiate_prefab(&mut world, &data, None).unwrap();
    assert_eq!(mapping.len(), 3);
    assert_eq!(world.len(), before + 3);
    let new_ids: BTreeSet<EntityId> = mapping.iter().map(|(_, new)| *new).collect();
    let mut parents = 0;
    for (_, new) in &mapping {
        let key = world.key(*new).unwrap();
        if let Some(parent) = world.get::<Parent>(key) {
            assert!(new_ids.contains(&parent.entity), "父引用应指向新实例");
            parents += 1;
        }
    }
    assert!(parents > 0, "Pillar 预制体应有父子关系");
}

#[test]
fn instantiated_entities_carry_source_metadata() {
    use xgc_world::PrefabInstance;

    let path = Path::new("../../../projects/XGame/Prefabs/Pillar.prefab.ron");
    if !path.exists() {
        eprintln!("skip: 没有旧项目 Pillar 预制体");
        return;
    }
    let (data, _) = migrate_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut world = World::new();
    let mapping = instantiate_prefab(&mut world, &data, Some("Prefabs/Pillar.prefab.ron")).unwrap();
    let mut roots = 0;
    for (_, new) in &mapping {
        let key = world.key(*new).unwrap();
        let instance = world.get::<PrefabInstance>(key).unwrap();
        assert_eq!(instance.source, "Prefabs/Pillar.prefab.ron");
        if instance.root {
            roots += 1;
        }
    }
    assert_eq!(roots, 1);
}

#[test]
fn revert_preserves_instance_ids_and_transforms() {
    let path = Path::new("../../../projects/XGame/Prefabs/Pillar.prefab.ron");
    if !path.exists() {
        eprintln!("skip: 没有旧项目 Pillar 预制体");
        return;
    }
    let (data, _) = migrate_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let root_old = data
        .entities
        .iter()
        .find(|entity| {
            !entity
                .components
                .iter()
                .any(|component| component.component.0 == 0x0104)
        })
        .unwrap()
        .id;
    let mut world = World::new();
    let mapping = instantiate_prefab(&mut world, &data, Some("Pillar")).unwrap();
    let root = mapping
        .iter()
        .find(|(old, _)| *old == root_old)
        .map(|(_, new)| *new)
        .unwrap();
    let key = world.key(root).unwrap();
    world.get_mut::<Transform>(key).unwrap().position = [9.0, 9.0, 9.0];
    let mut modified = data.clone();
    modified.entities.pop();
    let before = world.len();
    let reverted = revert_prefab(&mut world, &modified, root).unwrap();
    assert_eq!(reverted.len(), 2);
    assert_eq!(world.len(), before - 1);
    assert_eq!(
        world
            .get::<Transform>(world.key(root).unwrap())
            .unwrap()
            .position,
        [9.0, 9.0, 9.0]
    );
}

#[test]
fn revert_spawns_entities_added_to_asset() {
    let path = Path::new("../../../projects/XGame/Prefabs/Pillar.prefab.ron");
    if !path.exists() {
        eprintln!("skip: 没有旧项目 Pillar 预制体");
        return;
    }
    let (data, _) = migrate_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let root = data
        .entities
        .iter()
        .find(|entity| {
            !entity
                .components
                .iter()
                .any(|component| component.component.0 == 0x0104)
        })
        .unwrap()
        .clone();
    let mut world = World::new();
    let mapping = instantiate_prefab(&mut world, &data, Some("Pillar")).unwrap();
    let root_instance = mapping.iter().find(|(old, _)| *old == root.id).unwrap().1;
    let before = world.len();
    let mut modified = data.clone();
    modified.entities.push(root.with_id(EntityId::from_raw(99)));
    let reverted = revert_prefab(&mut world, &modified, root_instance).unwrap();
    assert_eq!(reverted.len(), 4);
    assert_eq!(world.len(), before + 1);
    assert!(world.world_data().entities.iter().any(|entity| {
        entity.components.iter().any(|component| {
            component.component.0 == 0x0802
                && component.fields.iter().any(|(field, value)| {
                    field == "source_id" && *value == xgc_world::Value::Entity(99)
                })
        })
    }));
}
