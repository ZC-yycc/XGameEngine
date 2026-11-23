//! 帧循环回归：同场景两跑 trace 一致、篡改 trace 能被发现、场景 JSON 往返。

use xgc_base::EntityId;
use xgc_sim::{
    play, verify, AabbCollisionSystem, AnimatorSystem, CharacterJumpSystem, FrameCommands,
    GravitySystem, GroundCollisionSystem, PhysicsSystem, PlanStage, Scenario, Schedule, SimError,
    Simulation, System, SystemAccess, VelocitySystem,
};
use xgc_world::{
    Animator, CharacterController, Command, Component, ComponentId, GlobalTransform, Name, Parent,
    PhysicsBody, Transform, Value, Velocity, World,
};

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
    assert_eq!(
        simulation.schedule().names(),
        vec![
            "physics",
            "character_jump",
            "velocity",
            "ground",
            "aabb",
            "rapier",
            "animation_state",
            "animation",
            "animation_apply",
            "animation_layers",
            "skeleton_pose",
            "ui_layout",
            "ui_interaction",
            "ui_focus",
            "audio_mixer",
            "hierarchy"
        ]
    );
    simulation.step(Vec::new()).unwrap();
    assert_eq!(simulation.trace().frames.len(), 1);

    let world = simulation.into_world();
    let mut found = None;
    world.for_each::<Transform>(|_, transform| found = Some(transform.position[0]));
    assert_eq!(found, Some(1.0));
}

#[test]
fn velocity_system_declares_access_and_parallel_hash_matches() {
    let simulation = Simulation::new(World::new());
    let access = &simulation.schedule().accesses()[2];
    // 读 `Velocity` + 读 `CharacterController`（角色水平位移归 KCC，这里要跳过它）。
    assert_eq!(access.reads, vec![ComponentId(0x0102), ComponentId(0x0402)]);
    assert_eq!(access.writes, vec![ComponentId(0x0101)]);

    fn run(threads: usize) -> (u64, f64) {
        let mut world = World::new();
        for i in 0..2_000 {
            let transform = Transform {
                position: [i as f32, 0.0, 0.0],
                ..Transform::default()
            };
            if i % 2 == 0 {
                world.spawn((
                    transform,
                    Velocity {
                        linear: [1.0, 0.5, -1.0],
                    },
                ));
            } else {
                world.spawn((
                    transform,
                    Velocity {
                        linear: [1.0, 0.5, -1.0],
                    },
                    Name {
                        value: format!("e{i}"),
                    },
                ));
            }
        }
        let mut simulation = Simulation::with_threads(world, 1.0 / 60.0, threads);
        simulation.run(10).unwrap();
        let mut sum = 0.0f64;
        simulation
            .world()
            .for_each::<Transform>(|_, transform| sum += transform.position[0] as f64);
        (simulation.world().state_hash(), sum)
    }

    let one = run(1);
    assert_eq!(one, run(2));
    assert_eq!(one, run(8));
}

#[test]
fn plan_groups_only_non_conflicting_consecutive_systems() {
    struct NameSystem;

    impl System for NameSystem {
        fn name(&self) -> &'static str {
            "name"
        }

        fn access(&self) -> SystemAccess {
            SystemAccess {
                reads: Vec::new(),
                writes: vec![Name::ID],
            }
        }

        fn run(&mut self, world: &mut World, _dt: f32) {
            world.for_each_mut::<Name>(|_, name| name.value.push('!'));
        }
    }

    let mut schedule = Schedule::new();
    schedule.add(VelocitySystem::default());
    schedule.add(NameSystem);
    schedule.add(GravitySystem::new([0.0, -9.81, 0.0]));

    let plan = schedule.plan();
    assert_eq!(
        plan.stages(),
        &[
            PlanStage::Batch(vec![0, 1]),
            PlanStage::Batch(vec![2]),
            PlanStage::Host,
        ]
    );
    assert_eq!(plan.system_order(), vec![0, 1, 2]);
    assert!(schedule.accesses()[0].conflicts_with(&schedule.accesses()[2]));
    assert!(!schedule.accesses()[0].conflicts_with(&schedule.accesses()[1]));
}

#[test]
fn gravity_system_writes_velocity() {
    let mut world = World::new();
    world.spawn((Velocity::default(),));
    let mut schedule = Schedule::new();
    schedule.add(GravitySystem::new([0.0, -2.0, 0.0]));
    schedule.run(&mut world, 1.0);

    let mut found = None;
    world.for_each::<Velocity>(|_, velocity| found = Some(velocity.linear[1]));
    assert_eq!(found, Some(-2.0));
}

#[test]
fn hierarchy_system_composes_parent_transforms() {
    let mut world = World::new();
    let parent_key = world.spawn((
        Transform {
            position: [10.0, 0.0, 0.0],
            ..Transform::default()
        },
        GlobalTransform::default(),
    ));
    let parent = world.entity(parent_key).unwrap();
    let child_key = world.spawn((
        Transform {
            position: [1.0, 0.0, 0.0],
            ..Transform::default()
        },
        Parent { entity: parent },
        GlobalTransform::default(),
    ));
    let child = world.entity(child_key).unwrap();

    let mut simulation = Simulation::with_fixed_dt(world, 1.0 / 60.0);
    simulation.step(Vec::new()).unwrap();
    let world = simulation.into_world();

    assert_eq!(
        world.get_field(child, GlobalTransform::ID, "position"),
        Some(Value::Vec3([11.0, 0.0, 0.0]))
    );
    assert_eq!(
        world.get_field(parent, GlobalTransform::ID, "position"),
        Some(Value::Vec3([10.0, 0.0, 0.0]))
    );
}

#[test]
fn hierarchy_system_leaves_cycles_alone() {
    let mut world = World::new();
    let first_key = world.spawn((
        Transform::default(),
        Parent::default(),
        GlobalTransform::default(),
    ));
    let second_key = world.spawn((
        Transform::default(),
        Parent::default(),
        GlobalTransform::default(),
    ));
    let first = world.entity(first_key).unwrap();
    let second = world.entity(second_key).unwrap();
    let _ = world.set_field(first, Parent::ID, "entity", &Value::Entity(second.raw()));
    let _ = world.set_field(second, Parent::ID, "entity", &Value::Entity(first.raw()));

    let mut simulation = Simulation::with_fixed_dt(world, 1.0 / 60.0);
    simulation.step(Vec::new()).unwrap();
    let world = simulation.into_world();
    assert_eq!(
        world.get_field(first, GlobalTransform::ID, "position"),
        Some(Value::Vec3([0.0; 3]))
    );
}

#[test]
fn hierarchy_scratch_reuse_is_stable_across_frames() {
    let mut world = World::new();
    let parent_key = world.spawn((
        Transform {
            position: [10.0, 0.0, 0.0],
            ..Transform::default()
        },
        GlobalTransform::default(),
    ));
    let parent = world.entity(parent_key).unwrap();
    let child_key = world.spawn((
        Transform {
            position: [1.0, 0.0, 0.0],
            ..Transform::default()
        },
        Parent { entity: parent },
        GlobalTransform::default(),
    ));
    let child = world.entity(child_key).unwrap();

    let mut simulation = Simulation::with_fixed_dt(world, 1.0 / 60.0);
    let mut hashes = Vec::new();
    for _ in 0..3 {
        simulation.step(Vec::new()).unwrap();
        hashes.push(simulation.world().state_hash());
    }
    assert!(hashes.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(
        simulation
            .world()
            .get_field(child, GlobalTransform::ID, "position"),
        Some(Value::Vec3([11.0, 0.0, 0.0]))
    );
}

#[test]
fn command_frame_outside_scenario_is_rejected() {
    let mut scenario = build_scenario();
    scenario.frames = 1;
    let err = play(&scenario).err().expect("应当拒绝越界命令帧");
    assert!(matches!(err, SimError::InvalidScenario(_)));
}

#[test]
fn animation_system_advances_and_loops() {
    let mut world = World::new();
    let entity = world.spawn((Animator {
        autoplay: true,
        speed: 2.0,
        duration: 1.0,
        looping: true,
        ..Animator::default()
    },));
    let mut schedule = Schedule::new();
    schedule.add(AnimatorSystem);
    schedule.run(&mut world, 0.3);
    assert!((world.get::<Animator>(entity).unwrap().time - 0.6).abs() < 1e-5);
    schedule.run(&mut world, 0.3);
    assert!((world.get::<Animator>(entity).unwrap().time - 0.2).abs() < 1e-5);
}

#[test]
fn physics_only_dynamic_bodies_fall() {
    let mut world = World::new();
    let dynamic = world.spawn((
        Velocity::default(),
        PhysicsBody {
            kind: "dynamic".to_string(),
            gravity_scale: 1.0,
            ..PhysicsBody::default()
        },
    ));
    let static_body = world.spawn((
        Velocity::default(),
        PhysicsBody {
            kind: "static".to_string(),
            gravity_scale: 1.0,
            ..PhysicsBody::default()
        },
    ));
    let mut schedule = Schedule::new();
    schedule.add(PhysicsSystem::default());
    schedule.run(&mut world, 0.5);
    let dynamic = world.get::<Velocity>(dynamic).unwrap();
    let static_body = world.get::<Velocity>(static_body).unwrap();
    assert!((dynamic.linear[1] + 4.905).abs() < 1e-4, "{dynamic:?}");
    assert_eq!(static_body.linear, [0.0; 3]);
}

#[test]
fn ground_collision_stops_dynamic_body() {
    let mut world = World::new();
    let entity = world.spawn((
        Transform {
            position: [0.0, 5.0, 0.0],
            ..Transform::default()
        },
        Velocity {
            linear: [0.0, -10.0, 0.0],
        },
        PhysicsBody {
            shape: "cuboid".to_string(),
            half_extents: [0.5, 0.5, 0.5],
            ..PhysicsBody::default()
        },
    ));
    let mut schedule = Schedule::new();
    schedule.add(VelocitySystem::new(1));
    schedule.add(GroundCollisionSystem::default());
    for _ in 0..20 {
        schedule.run(&mut world, 0.1);
    }
    let transform = world.get::<Transform>(entity).unwrap();
    let velocity = world.get::<Velocity>(entity).unwrap();
    assert!((transform.position[1] - 0.5).abs() < 1e-4);
    assert_eq!(velocity.linear[1], 0.0);
}

#[test]
fn aabb_separates_overlapping_dynamic_boxes() {
    let mut world = World::new();
    let a = world.spawn((
        Transform {
            position: [0.0, 0.5, 0.0],
            ..Transform::default()
        },
        PhysicsBody::default(),
    ));
    let b = world.spawn((
        Transform {
            position: [0.2, 0.5, 0.0],
            ..Transform::default()
        },
        PhysicsBody::default(),
    ));
    let mut schedule = Schedule::new();
    schedule.add(AabbCollisionSystem);
    schedule.run(&mut world, 0.016);
    let a = world.get::<Transform>(a).unwrap();
    let b = world.get::<Transform>(b).unwrap();
    assert!((a.position[0] - b.position[0]).abs() >= 1.0 - 1e-5);
    assert!((a.position[0] + b.position[0] - 0.2).abs() < 1e-5);
}

#[test]
fn spheres_separate_along_center_line() {
    let mut world = World::new();
    let a = world.spawn((
        Transform {
            position: [0.0, 1.0, 0.0],
            ..Transform::default()
        },
        PhysicsBody {
            shape: "ball".to_string(),
            radius: 0.5,
            ..PhysicsBody::default()
        },
    ));
    let b = world.spawn((
        Transform {
            position: [0.5, 1.0, 0.0],
            ..Transform::default()
        },
        PhysicsBody {
            shape: "ball".to_string(),
            radius: 0.5,
            ..PhysicsBody::default()
        },
    ));
    let mut schedule = Schedule::new();
    schedule.add(AabbCollisionSystem);
    schedule.run(&mut world, 0.016);
    let a = world.get::<Transform>(a).unwrap();
    let b = world.get::<Transform>(b).unwrap();
    assert!(((a.position[0] - b.position[0]).abs() - 1.0).abs() < 1e-5);
    assert!((a.position[0] + b.position[0] - 0.5).abs() < 1e-5);
}

#[test]
fn character_jump_consumes_request_when_grounded() {
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        Velocity::default(),
        CharacterController {
            jump: true,
            grounded: true,
            jump_velocity: 6.0,
            ..CharacterController::default()
        },
    ));
    let mut schedule = Schedule::new();
    schedule.add(CharacterJumpSystem);
    schedule.run(&mut world, 0.016);
    let velocity = world.get::<Velocity>(entity).unwrap();
    let controller = world.get::<CharacterController>(entity).unwrap();
    assert_eq!(velocity.linear[1], 6.0);
    assert!(!controller.jump);
    assert!(!controller.grounded);
}

#[test]
fn simulation_step_with_runs_external_hook_before_trace() {
    let mut world = World::new();
    let entity = world.spawn((Transform::default(),));
    let mut simulation = Simulation::new(world);
    let record = simulation
        .step_with(Vec::new(), |world| {
            if let Some(transform) = world.get_mut::<Transform>(entity) {
                transform.position = [7.0, 8.0, 9.0];
            }
        })
        .unwrap();
    assert_eq!(record.state_hash, simulation.world().state_hash());
    assert_eq!(
        simulation
            .world()
            .get::<Transform>(entity)
            .unwrap()
            .position,
        [7.0, 8.0, 9.0]
    );
}
