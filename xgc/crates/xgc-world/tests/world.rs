//! XGC 世界行为回归：生成 / 回收 / 查询 / 确定性。

use std::time::Instant;

use xgc_world::{Name, Transform, Velocity, World};

#[test]
fn spawn_query_and_despawn_10k() {
    let mut world = World::new();
    let start = Instant::now();
    let mut keys = Vec::with_capacity(10_000);
    for i in 0..10_000 {
        let transform = Transform {
            position: [i as f32, 0.0, 0.0],
            ..Transform::default()
        };
        keys.push(world.spawn((transform, Velocity::default())));
    }
    let spawn = start.elapsed();

    assert_eq!(world.len(), 10_000);
    assert_eq!(world.count::<Transform>(), 10_000);
    assert_eq!(world.count::<Velocity>(), 10_000);

    let query_start = Instant::now();
    let mut sum = 0.0f64;
    world.for_each::<Transform>(|_, transform| sum += transform.position[0] as f64);
    let query = query_start.elapsed();
    assert_eq!(sum, (0..10_000).sum::<i32>() as f64);

    let mut moved = 0usize;
    world.for_each_mut::<Velocity>(|_, velocity| {
        velocity.linear[0] += 1.0;
        moved += 1;
    });
    assert_eq!(moved, 10_000);

    let despawn_start = Instant::now();
    for key in keys.iter().step_by(2) {
        assert!(world.despawn(*key));
    }
    let despawn = despawn_start.elapsed();
    assert_eq!(world.len(), 5_000);
    assert_eq!(world.count::<Transform>(), 5_000);
    assert_eq!(world.count::<Velocity>(), 5_000);

    println!("10k spawn={spawn:?} query={query:?} despawn_half={despawn:?}");
}

#[test]
fn despawn_middle_keeps_others_and_updates_moved_row() {
    let mut world = World::new();
    let a = world.spawn((Transform {
        position: [0.0, 0.0, 0.0],
        ..Transform::default()
    },));
    let b = world.spawn((Transform {
        position: [1.0, 0.0, 0.0],
        ..Transform::default()
    },));
    let c = world.spawn((Transform {
        position: [2.0, 0.0, 0.0],
        ..Transform::default()
    },));

    assert!(world.despawn(b));
    assert!(world.get::<Transform>(b).is_none());
    assert_eq!(world.get::<Transform>(a).unwrap().position[0], 0.0);
    assert_eq!(world.get::<Transform>(c).unwrap().position[0], 2.0);
    assert_eq!(world.len(), 2);

    let mut seen = Vec::new();
    world.for_each::<Transform>(|_, transform| seen.push(transform.position[0]));
    seen.sort_by(f32::total_cmp);
    assert_eq!(seen, vec![0.0, 2.0]);
}

#[test]
fn stale_handle_is_rejected_after_slot_reuse() {
    let mut world = World::new();
    let old = world.spawn((Transform::default(),));
    assert!(world.despawn(old));
    let new = world.spawn((Transform::default(),));
    assert_eq!(old.index(), new.index());
    assert_ne!(old.generation(), new.generation());
    assert!(world.get::<Transform>(old).is_none());
    assert!(world.get::<Transform>(new).is_some());
}

#[test]
fn archetypes_split_by_component_set() {
    let mut world = World::new();
    world.spawn((Transform::default(),));
    world.spawn((Transform::default(), Velocity::default()));
    assert_eq!(world.archetype_count(), 2);
    assert_eq!(world.count::<Transform>(), 2);
    assert_eq!(world.count::<Velocity>(), 1);

    let mut velocity_seen = 0;
    world.for_each::<Velocity>(|_, _| velocity_seen += 1);
    assert_eq!(velocity_seen, 1);
}

#[test]
fn revision_advances_on_structural_change() {
    let mut world = World::new();
    let start = world.revision();
    let key = world.spawn((Transform::default(),));
    assert_eq!(world.revision(), start + 1);
    assert!(world.despawn(key));
    assert_eq!(world.revision(), start + 2);
    assert!(!world.despawn(key));
    assert_eq!(world.revision(), start + 2);
}

#[test]
fn non_copy_component_roundtrip() {
    let mut world = World::new();
    let key = world.spawn((Name {
        value: "Player".to_string(),
    },));
    assert_eq!(world.get::<Name>(key).unwrap().value, "Player");
    world.get_mut::<Name>(key).unwrap().value.push('!');
    assert_eq!(world.get::<Name>(key).unwrap().value, "Player!");
}

#[test]
fn ids_and_iteration_order_are_deterministic() {
    fn run() -> Vec<(u64, f32)> {
        let mut world = World::new();
        for i in 0..1_000 {
            world.spawn((Transform {
                position: [i as f32, 0.0, 0.0],
                ..Transform::default()
            },));
        }
        let mut seen = Vec::new();
        world.for_each::<Transform>(|key, transform| {
            seen.push((world.entity(key).unwrap().raw(), transform.position[0]));
        });
        seen
    }

    assert_eq!(run(), run());
}
