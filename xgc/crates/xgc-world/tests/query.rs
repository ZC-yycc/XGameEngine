//! 多组件查询与状态哈希回归。

use xgc_reflect::{ComponentId, Value};
use xgc_world::{Transform, Velocity, World};

#[test]
fn for_each2_only_visits_archetypes_with_both_components() {
    let mut world = World::new();
    world.spawn((Transform::default(),));
    world.spawn((
        Transform {
            position: [2.0, 0.0, 0.0],
            ..Transform::default()
        },
        Velocity {
            linear: [1.0, 0.0, 0.0],
        },
    ));
    world.spawn((Velocity::default(),));

    let mut seen = Vec::new();
    world.for_each2::<Transform, Velocity>(|_, transform, velocity| {
        seen.push((transform.position[0], velocity.linear[0]));
    });
    assert_eq!(seen, vec![(2.0, 1.0)]);
}

#[test]
fn state_hash_is_deterministic_and_tracks_field_changes() {
    fn build() -> u64 {
        let mut world = World::new();
        let key = world.spawn((Transform::default(), Velocity::default()));
        let id = world.entity(key).unwrap();
        world
            .set_field(
                id,
                ComponentId(0x0101),
                "position",
                &Value::Vec3([4.0, 5.0, 6.0]),
            )
            .unwrap();
        world.state_hash()
    }

    assert_eq!(build(), build());

    let mut world = World::new();
    let key = world.spawn((Transform::default(),));
    let id = world.entity(key).unwrap();
    let before = world.state_hash();
    world
        .set_field(
            id,
            ComponentId(0x0101),
            "position",
            &Value::Vec3([1.0, 0.0, 0.0]),
        )
        .unwrap();
    assert_ne!(world.state_hash(), before);
}
