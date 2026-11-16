//! 可序列化世界数据回归：世界 ↔ WorldData 往返、JSON 往返。

use xgc_world::{Name, Transform, Velocity, World};

fn build_world(count: i32) -> World {
    let mut world = World::new();
    for i in 0..count {
        world.spawn((
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
    }
    world
}

#[test]
fn world_data_roundtrip_preserves_ids_revisions_and_hash() {
    let world = build_world(100);
    let data = world.world_data();
    assert_eq!(data.entities.len(), 100);
    assert_eq!(data.next_id, 100);

    let mut restored = World::new();
    restored.restore_world_data(&data).unwrap();
    assert_eq!(restored.len(), 100);
    assert_eq!(restored.revision(), world.revision());
    assert_eq!(restored.state_hash(), world.state_hash());
    assert_eq!(restored.world_data(), data);
}

#[test]
fn world_data_json_roundtrip() {
    let world = build_world(10);
    let data = world.world_data();
    let json = serde_json::to_string_pretty(&data).unwrap();
    let decoded: xgc_world::WorldData = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, data);

    let mut restored = World::new();
    restored.restore_world_data(&decoded).unwrap();
    assert_eq!(restored.state_hash(), world.state_hash());
}
