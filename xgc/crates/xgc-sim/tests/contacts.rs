//! 默认固定步的 Rapier 接触求解必须让刚体落稳。

use xgc_base::EntityKey;
use xgc_sim::Simulation;
use xgc_world::{PhysicsBody, Transform, Velocity, World};

fn spawn_box(world: &mut World, y: f32, kind: &str, half_extents: [f32; 3]) -> EntityKey {
    world.spawn((
        Transform {
            position: [0.0, y, 0.0],
            ..Transform::default()
        },
        PhysicsBody {
            kind: kind.into(),
            shape: "cuboid".into(),
            half_extents,
            ..PhysicsBody::default()
        },
        Velocity::default(),
    ))
}

fn advance(sim: &mut Simulation, frames: usize) {
    for _ in 0..frames {
        sim.advance_with(Vec::new(), |_| {}).unwrap();
    }
}

fn assert_resting(sim: &Simulation, key: EntityKey, expected_y: f32) -> [f32; 3] {
    let world = sim.world();
    let position = world.get::<Transform>(key).unwrap().position;
    let velocity = world.get::<Velocity>(key).unwrap().linear;
    assert!(
        (position[1] - expected_y).abs() < 0.05,
        "应停在 y={expected_y} 附近，实际位置 {position:?}"
    );
    assert!(
        velocity.into_iter().all(|axis| axis.abs() < 1e-3),
        "支撑稳定后速度仍非零：{velocity:?}"
    );
    position
}

#[test]
fn falling_boxes_settle_on_a_static_box_and_each_other() {
    let mut world = World::new();
    // 支撑面抬到 y=1，隐式地面无法使测试通过。
    spawn_box(&mut world, 0.5, "static", [3.0, 0.5, 3.0]);
    let lower = spawn_box(&mut world, 4.0, "dynamic", [0.5; 3]);
    let upper = spawn_box(&mut world, 5.5, "dynamic", [0.5; 3]);
    let mut sim = Simulation::new(world);
    advance(&mut sim, 360);

    let positions = [
        (lower, 1.5, assert_resting(&sim, lower, 1.5)),
        (upper, 2.5, assert_resting(&sim, upper, 2.5)),
    ];
    advance(&mut sim, 120);
    for (key, expected_y, before) in positions {
        let after = assert_resting(&sim, key, expected_y);
        assert!(
            before
                .into_iter()
                .zip(after)
                .all(|(before, after)| (after - before).abs() < 1e-3),
            "静止后位姿漂移：{before:?} → {after:?}"
        );
    }
}
