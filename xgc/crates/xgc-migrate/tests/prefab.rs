//! 预制体实例化：新 ID、父引用重写、一次性 Spawn。

use std::collections::BTreeSet;
use std::path::Path;

use xgc_base::EntityId;
use xgc_migrate::{instantiate_prefab, migrate_str};
use xgc_world::{Parent, Transform, World};

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
