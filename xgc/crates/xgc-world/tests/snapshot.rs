//! 世界快照 / 恢复回归：ID、组件、revision、状态哈希都要一致。

use xgc_base::EntityId;
use xgc_reflect::{ComponentId, Value};
use xgc_world::{Name, Transform, Velocity, World};

fn build_world(count: i32) -> (World, Vec<EntityId>, u64) {
    let mut world = World::new();
    let mut ids = Vec::with_capacity(count as usize);
    for i in 0..count {
        let key = world.spawn((
            Transform {
                position: [i as f32, 0.0, 0.0],
                ..Transform::default()
            },
            Velocity {
                linear: [0.0, 0.0, -1.0],
            },
            Name {
                value: format!("entity_{i}"),
            },
        ));
        ids.push(world.entity(key).unwrap());
    }
    let hash = world.state_hash();
    (world, ids, hash)
}

#[test]
fn world_snapshot_restore_roundtrip() {
    let (mut world, ids, before_hash) = build_world(200);
    let before_len = world.len();
    let before_revision = world.revision();
    let snapshot = world.snapshot();
    assert_eq!(snapshot.entities().len(), before_len);
    assert_eq!(snapshot.revision(), before_revision);

    for id in ids.iter().take(5) {
        world
            .set_field(
                *id,
                ComponentId(0x0101),
                "position",
                &Value::Vec3([9.0, 9.0, 9.0]),
            )
            .unwrap();
    }
    world.despawn(world.key(ids[10]).unwrap());
    world.spawn((Transform::default(),));
    assert_ne!(world.state_hash(), before_hash);

    world.restore_snapshot(&snapshot);
    assert_eq!(world.len(), before_len);
    assert_eq!(world.state_hash(), before_hash);
    assert_eq!(world.revision(), before_revision);
    assert_eq!(
        world.get_field(ids[0], ComponentId(0x0103), "value"),
        Some(Value::String("entity_0".to_string()))
    );
    for id in &ids {
        assert!(world.key(*id).is_some());
    }
}

#[test]
fn snapshot_restores_into_an_empty_world() {
    let (world, ids, hash) = build_world(50);
    let snapshot = world.snapshot();
    drop(world);

    let mut other = World::new();
    other.restore_snapshot(&snapshot);
    assert_eq!(other.len(), ids.len());
    assert_eq!(other.state_hash(), hash);
    for id in &ids {
        assert!(other.key(*id).is_some());
    }
}
