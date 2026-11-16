//! 帧循环回归：同场景两跑 trace 一致、篡改 trace 能被发现、场景 JSON 往返。

use xgc_base::EntityId;
use xgc_sim::{play, verify, FrameCommands, Scenario, SimError, Simulation};
use xgc_world::{Command, ComponentId, Name, Transform, Value, Velocity, World};

fn build_scenario() -> Scenario {
    let mut world = World::new();
    for i in 0..3 {
        world.spawn((
            Transform {
                position: [i as f32, 0.0, 0.0],
                ..Transform::default()
            },
            Name {
                value: format!("entity_{i}"),
            },
        ));
    }
    let initial = world.world_data();
    let source_data = world.entity_data(EntityId::from_raw(0)).unwrap();

    let commands = vec![
        FrameCommands {
            frame: 0,
            commands: vec![Command::SetField {
                entity: EntityId::from_raw(0),
                component: ComponentId(0x0101),
                field: "position".to_string(),
                value: Value::Vec3([1.0, 0.0, 0.0]),
            }],
        },
        FrameCommands {
            frame: 1,
            commands: vec![Command::Spawn {
                data: source_data.with_id(EntityId::from_raw(100)),
            }],
        },
        FrameCommands {
            frame: 2,
            commands: vec![Command::Despawn {
                entity: EntityId::from_raw(1),
            }],
        },
    ];

    Scenario {
        initial,
        frames: 3,
        fixed_dt: 1.0 / 60.0,
        commands,
    }
}

#[test]
fn same_scenario_replays_to_the_same_trace() {
    let scenario = build_scenario();
    let (first_world, first) = play(&scenario).unwrap();
    let (second_world, second) = play(&scenario).unwrap();

    assert_eq!(first, second);
    assert_eq!(first_world.state_hash(), second_world.state_hash());
    assert_eq!(first.frames.len(), 3);
    assert_eq!(first.frames[0].revision, 4);
    assert_eq!(first.frames[1].revision, 5);
    assert_eq!(first.frames[2].revision, 6);
    assert_eq!(first.frames[2].entities, 3);
    verify(&scenario, &first).unwrap();
}

#[test]
fn tampered_trace_is_rejected() {
    let scenario = build_scenario();
    let (_, trace) = play(&scenario).unwrap();
    let mut tampered = trace;
    tampered.frames[1].state_hash ^= 1;
    let err = verify(&scenario, &tampered).unwrap_err();
    assert!(matches!(err, SimError::TraceMismatch { frame: 1, .. }));
}

#[test]
fn scenario_json_roundtrip() {
    let scenario = build_scenario();
    let json = serde_json::to_string_pretty(&scenario).unwrap();
    let decoded: Scenario = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, scenario);

    let (world, trace) = play(&decoded).unwrap();
    assert_eq!(trace.frames.len(), 3);
    assert_eq!(world.len(), 3);
}

#[test]
fn velocity_system_moves_transform() {
    let mut world = World::new();
    world.spawn((
        Transform::default(),
        Velocity {
            linear: [1.0, 0.0, 0.0],
        },
    ));
    let mut simulation = Simulation::with_fixed_dt(world, 1.0);
    assert_eq!(simulation.schedule().names(), vec!["velocity"]);
    simulation.step(Vec::new()).unwrap();
    assert_eq!(simulation.trace().frames.len(), 1);

    let world = simulation.into_world();
    let mut found = None;
    world.for_each::<Transform>(|_, transform| found = Some(transform.position[0]));
    assert_eq!(found, Some(1.0));
}

#[test]
fn command_frame_outside_scenario_is_rejected() {
    let mut scenario = build_scenario();
    scenario.frames = 1;
    let err = play(&scenario).err().expect("应当拒绝越界命令帧");
    assert!(matches!(err, SimError::InvalidScenario(_)));
}
