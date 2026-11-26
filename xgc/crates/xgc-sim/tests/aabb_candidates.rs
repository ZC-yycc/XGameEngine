//! 候选对回归。逐值对照命令使用 `trace_mixed_aabb_values -- --exact --nocapture`。

mod support;

use std::fmt::Write as _;

use xgc_base::EntityKey;
use xgc_sim::{AabbCollisionSystem, System, FIXED_DT};
use xgc_world::{CollisionEvent, PhysicsBody, Transform, Velocity, World};

fn box_body(kind: &str) -> PhysicsBody {
    PhysicsBody {
        kind: kind.into(),
        shape: "cuboid".into(),
        ..PhysicsBody::default()
    }
}

fn spawn(world: &mut World, p: [f32; 3], v: [f32; 3], body: PhysicsBody) -> EntityKey {
    world.spawn((
        Transform {
            position: p,
            ..Transform::default()
        },
        Velocity { linear: v },
        body,
    ))
}

fn run(world: &mut World, system: &mut AabbCollisionSystem) {
    world.clear_collision_events();
    system.run(world, FIXED_DT);
}

fn events(world: &World, a: EntityKey, b: EntityKey, enter: bool) -> [CollisionEvent; 2] {
    let a = world.entity(a).unwrap();
    let b = world.entity(b).unwrap();
    [
        CollisionEvent {
            entity: a,
            other: b,
            enter,
        },
        CollisionEvent {
            entity: b,
            other: a,
            enter,
        },
    ]
}

#[test]
fn separation_discovers_a_later_pair_without_revisiting_the_first() {
    let mut world = World::new();
    let moving = spawn(&mut world, [0.0; 3], [-3.0, 5.0, 7.0], box_body("dynamic"));
    let left = spawn(&mut world, [-0.75, 0.0, 0.0], [0.0; 3], box_body("static"));
    let right = spawn(&mut world, [1.10, 0.0, 0.0], [0.0; 3], box_body("static"));
    let mut system = AabbCollisionSystem::default();

    run(&mut world, &mut system);
    // 第一对把盒子推到 x=.25。新出现的右接触把它推回；旧顺序不会回访左接触。
    let right_penetration = 1.0 - (0.25_f32 - 1.10).abs();
    assert_eq!(
        world.get::<Transform>(moving).unwrap().position,
        [0.25 - right_penetration, 0.0, 0.0]
    );
    assert_eq!(
        world.get::<Velocity>(moving).unwrap().linear,
        [0.0, 5.0, 7.0]
    );
    assert_eq!(
        world.collision_events(),
        [
            events(&world, moving, left, true),
            events(&world, moving, right, true)
        ]
        .concat()
    );

    world.get_mut::<Transform>(moving).unwrap().position[0] = 20.0;
    run(&mut world, &mut system);
    assert_eq!(
        world.collision_events(),
        [
            events(&world, moving, left, false),
            events(&world, moving, right, false)
        ]
        .concat()
    );
}

#[test]
fn repeated_separation_preserves_the_order_of_remaining_pairs() {
    let mut world = World::new();
    let moving = spawn(&mut world, [0.0; 3], [0.0; 3], box_body("dynamic"));
    let mut expected = Vec::new();
    for x in [-0.75, -0.5, -0.25, 0.0, 0.25, 2.10] {
        let fixed = spawn(&mut world, [x, 0.0, 0.0], [0.0; 3], box_body("static"));
        expected.extend(events(&world, moving, fixed, true));
    }

    run(&mut world, &mut AabbCollisionSystem::default());
    // 前五次分离逐次向右推出 .25。最后一对只在第五次分离后才重叠。
    let right_penetration = 1.0 - (1.25_f32 - 2.10).abs();
    assert_eq!(
        world.get::<Transform>(moving).unwrap().position,
        [1.25 - right_penetration, 0.0, 0.0]
    );
    assert_eq!(world.collision_events(), expected);
}

#[test]
fn sphere_contact_survives_distance_squared_underflow() {
    let mut world = World::new();
    let sensor = PhysicsBody {
        shape: "ball".into(),
        radius: 1.0e-30,
        is_sensor: true,
        ..box_body("dynamic")
    };
    let left = spawn(&mut world, [0.0; 3], [0.0; 3], sensor.clone());
    let right = spawn(
        &mut world,
        [1.0e-23, 0.0, 0.0],
        [0.0; 3],
        PhysicsBody {
            kind: "static".into(),
            ..sensor
        },
    );
    run(&mut world, &mut AabbCollisionSystem::default());
    // 精确距离大于半径和，但旧窄相计算的 f32 距离平方为零。
    assert_eq!(world.collision_events(), events(&world, left, right, true));
}

#[test]
fn trace_mixed_aabb_values() {
    // 取自优化前同机运行，覆盖各实体的浮点位模式和按序发出的有向事件。
    const EXHAUSTIVE_TRACE: [u64; 9] = [
        0xe9ec_ec41_99a7_c64e,
        0x762d_dbdf_1f85_8bf5,
        0x1d8f_b75f_6164_4767,
        0x4038_1e06_64d3_e052,
        0x3f10_fc78_c8e8_21b2,
        0x8125_62fb_d648_7348,
        0xa9f7_efb4_aa3a_b33e,
        0xebc5_bcbd_1fff_6c6c,
        0xc27c_c4de_e8c9_da8d,
    ];
    let mut world = World::new();
    let mut keys = Vec::new();
    let mut seed = 83u32;
    let mut random = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((seed >> 24) as f32 / 128.0) - 1.0
    };
    for index in 0..64 {
        let mut body = box_body(["dynamic", "dynamic", "static", "kinematic"][index % 4]);
        body.shape = ["cuboid", "ball", "capsule"][index % 3].into();
        body.radius = 0.3 + 0.3 * random().abs();
        body.is_sensor = index % 5 == 0;
        keys.push(spawn(
            &mut world,
            [random() * 3.0, random(), random() * 3.0],
            [random(), random(), random()],
            body,
        ));
    }
    // 量级与非有限数据由窄相原有语义裁定，传感器保留这些输入的位模式。
    let mut invalid = box_body("static");
    invalid.is_sensor = true;
    invalid.half_extents = [f32::INFINITY, -0.25, 0.8];
    let exceptional = spawn(
        &mut world,
        [f32::from_bits(0x7fc01234), 0.0, 0.0],
        [-0.0; 3],
        invalid,
    );
    keys.push(exceptional);
    let mut massive = box_body("dynamic");
    massive.half_extents = [1.1e25, 0.5, 0.5];
    massive.is_sensor = true;
    keys.push(spawn(&mut world, [1.0e25, 0.0, 0.0], [0.0; 3], massive));

    let mut system = AabbCollisionSystem::default();
    let mut saw_enter = false;
    let mut saw_exit = false;
    for (frame, expected) in EXHAUSTIVE_TRACE.into_iter().enumerate() {
        for key in keys.iter().take(64).step_by(3).copied() {
            // 刻意跨过多个旧接触，下一帧只看最终位置（当前模型是离散检测）。
            let p = &mut world.get_mut::<Transform>(key).unwrap().position;
            p[0] += random() * 4.0;
            p[2] += random() * 2.0;
        }
        run(&mut world, &mut system);
        let mut values = String::new();
        for key in keys.iter().copied() {
            let position = world
                .get::<Transform>(key)
                .unwrap()
                .position
                .map(f32::to_bits);
            let speed = world.get::<Velocity>(key).unwrap().linear.map(f32::to_bits);
            writeln!(
                values,
                "aabb value frame={frame} id={} position={position:08x?} velocity={speed:08x?}",
                world.entity(key).unwrap().raw()
            )
            .unwrap();
        }
        for event in world.collision_events() {
            saw_enter |= event.enter;
            saw_exit |= !event.enter;
            writeln!(
                values,
                "aabb event frame={frame} a={} b={} enter={}",
                event.entity.raw(),
                event.other.raw(),
                event.enter
            )
            .unwrap();
        }
        let actual = values
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
            });
        print!("{values}");
        assert_eq!(actual, expected, "第 {frame} 帧的位置、速度或事件顺序变化");
    }
    assert!(saw_enter && saw_exit, "差分场景必须覆盖进入和离开接触");
}

#[test]
fn moving_candidates_reuse_storage_after_warmup() {
    let mut world = World::new();
    let mut moving = Vec::new();
    for index in 0..128 {
        let x = index as f32 * 6.0;
        moving.push((
            x,
            spawn(&mut world, [x, 0.0, 0.0], [0.0; 3], box_body("dynamic")),
        ));
        spawn(
            &mut world,
            [x - 0.75, 0.0, 0.0],
            [0.0; 3],
            box_body("static"),
        );
        spawn(
            &mut world,
            [x + 1.10, 0.0, 0.0],
            [0.0; 3],
            box_body("static"),
        );
    }
    let mut system = AabbCollisionSystem::default();
    let mut step = || {
        for &(x, key) in &moving {
            world.get_mut::<Transform>(key).unwrap().position[0] = x;
        }
        run(&mut world, &mut system);
    };
    step();
    step();
    support::reset_counter();
    for _ in 0..60 {
        step();
    }
    assert_eq!(support::snapshot(), Some((0, 0)));
}
