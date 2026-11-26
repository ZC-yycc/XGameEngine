//! S2 Physics Yard 的 CPU 基线。每个格子是一个动态盒、一个地面和三个静态障碍。
//! 默认固定步包含物理、同步与其它系统，场景哈希和日志在计时外。
//!
//! `cargo test --locked --offline -j 2 --release -p xgc-sim --test physics_yard -- --ignored --nocapture --test-threads=1`
//! 可以按测试名选档。中高档默认有界，`XGC_S2_SAMPLES` 可覆盖采样数；少于 100 帧时
//! p99 是样本最大值。组场是内存内创建实体和调度，首步另计刚体表构建。

mod support;

use std::time::{Duration, Instant};

use xgc_base::EntityKey;
use xgc_sim::{AabbCollisionSystem, RapierPhysicsSystem, Simulation, System, FIXED_DT};
use xgc_world::{BodyKind, GlobalTransform, PhysicsBody, Transform, Velocity, World};

const WARMUP_STEPS: u64 = 18;
const MID_BOXES: usize = 5000;
const MID_COLUMNS: usize = 100;

#[test]
#[ignore = "需要 Release；每个档位有独立的 CPU 计时"]
fn physics_yard_low() {
    measure_yard("low", 500, 25, 120);
}

#[test]
#[ignore = "需要 Release；按 S2 规格运行 5000 个动态盒和 20000 个静态体"]
fn physics_yard_mid() {
    measure_yard("mid", MID_BOXES, MID_COLUMNS, 8);
}

#[test]
#[ignore = "需要 Release；规模较大，可以用 XGC_S2_SAMPLES 做短探针"]
fn physics_yard_high() {
    measure_yard("high", 10_000, 125, 6);
}

#[test]
#[ignore = "需要 Release；对中档的 AABB 和 Rapier 做独立诊断采样"]
fn physics_yard_mid_systems() {
    let (mut rapier_world, _) = yard(MID_BOXES, MID_COLUMNS);
    let mut rapier = RapierPhysicsSystem::default();
    for _ in 0..WARMUP_STEPS {
        rapier.prepare(&rapier_world);
        rapier.run(&mut rapier_world, FIXED_DT);
    }

    // 复制接触已经建立的同一份姿态。单独计时只用于定位开销，不能相加当作整帧。
    let mut aabb_world = World::new();
    aabb_world
        .restore_world_data(&rapier_world.world_data())
        .unwrap();
    let mut aabb = AabbCollisionSystem::default();
    for _ in 0..2 {
        aabb_world.clear_collision_events();
        aabb.run(&mut aabb_world, FIXED_DT);
    }

    let samples = sample_count(24);
    let aabb = sample_steps(samples, |_| {
        aabb_world.clear_collision_events();
        aabb.run(&mut aabb_world, FIXED_DT);
    });
    aabb.report("S2 mid AABB isolated");
    let rapier = sample_steps(samples, |_| {
        rapier.prepare(&rapier_world);
        rapier.run(&mut rapier_world, FIXED_DT);
    });
    rapier.report("S2 mid Rapier prepare+run isolated");
}

fn measure_yard(tier: &str, dynamic: usize, columns: usize, samples: usize) {
    let start = Instant::now();
    let (world, first_box) = yard(dynamic, columns);
    let mut simulation = Simulation::new(world);
    let setup = start.elapsed();
    assert_eq!(simulation.fixed_dt(), FIXED_DT);
    assert_eq!(simulation.parallel_threads(), 1);
    assert!(simulation.schedule().names().contains(&"rapier"));
    let initial_hash = simulation.world().state_hash();
    let start = Instant::now();
    assert_eq!(simulation.advance_with(Vec::new(), |_| {}).unwrap(), 0);
    let first_step = start.elapsed();
    for frame in 1..WARMUP_STEPS {
        assert_eq!(simulation.advance_with(Vec::new(), |_| {}).unwrap(), frame);
    }
    let warmup_hash = simulation.world().state_hash();
    assert_ne!(initial_hash, warmup_hash, "预热期间刚体应落到地面");

    let measured = sample_steps(sample_count(samples), |index| {
        let frame = simulation.advance_with(Vec::new(), |_| {}).unwrap();
        assert_eq!(frame, WARMUP_STEPS + index as u64);
    });
    let world = simulation.world();
    let mut counts = (0, 0);
    world.for_each2::<PhysicsBody, Transform>(|_, body, _| match body.kind() {
        BodyKind::Dynamic => counts.0 += 1,
        BodyKind::Static => counts.1 += 1,
        BodyKind::Kinematic => panic!("S2 不应生成运动学刚体"),
    });
    assert_eq!(counts, (dynamic, dynamic * 4));
    assert_eq!(world.len(), counts.0 + counts.1);
    let final_y = world.get::<Transform>(first_box).unwrap().position[1];
    assert!(
        (0.45..=0.55).contains(&final_y),
        "盒子应停在地面上，实际 {final_y}"
    );
    assert!(simulation.trace().frames.is_empty());
    let end_hash = world.state_hash();
    eprintln!(
        "S2 {tier}: entities={} dynamic={} static={} dt={FIXED_DT:.9} setup_ms={:.3} first_step_ms={:.3} warmup={WARMUP_STEPS} initial_hash={initial_hash:016x} warmup_hash={warmup_hash:016x} end_hash={end_hash:016x} first_box_y={final_y:.6}",
        world.len(), counts.0, counts.1, ms(setup), ms(first_step)
    );
    measured.report(&format!("S2 {tier} default step"));
}

fn yard(dynamic: usize, columns: usize) -> (World, EntityKey) {
    let mut world = World::new();
    let mut first_box = None;
    let fixed = PhysicsBody {
        kind: "static".into(),
        shape: "cuboid".into(),
        half_extents: [1.25, 0.25, 1.25],
        ..PhysicsBody::default()
    };
    let obstacle = PhysicsBody {
        half_extents: [0.2, 0.4, 0.2],
        ..fixed.clone()
    };
    let moving = PhysicsBody {
        shape: "cuboid".into(),
        ..PhysicsBody::default()
    };

    // 按格子登记，固定实体 ID 和碰撞体插入顺序。盒子离开地面落下，避免量到空接触场景。
    for index in 0..dynamic {
        let x = (index % columns) as f32 * 3.0;
        let z = (index / columns) as f32 * 3.0;
        spawn_fixed(&mut world, [x, -0.25, z], &fixed);
        for [dx, dz] in [[-1.0, -1.0], [-1.0, 1.0], [1.0, -1.0]] {
            spawn_fixed(&mut world, [x + dx, 0.4, z + dz], &obstacle);
        }
        let transform = Transform {
            position: [x, 0.63 + (index % 7) as f32 * 0.013, z],
            ..Transform::default()
        };
        let global = GlobalTransform::from_local(&transform);
        let key = world.spawn((transform, global, Velocity::default(), moving.clone()));
        first_box.get_or_insert(key);
    }
    (world, first_box.unwrap())
}

fn spawn_fixed(world: &mut World, position: [f32; 3], body: &PhysicsBody) {
    let transform = Transform {
        position,
        ..Transform::default()
    };
    let global = GlobalTransform::from_local(&transform);
    world.spawn((transform, global, body.clone()));
}

fn sample_count(default: usize) -> usize {
    match std::env::var("XGC_S2_SAMPLES") {
        Ok(value) => value
            .parse()
            .ok()
            .filter(|&count| count > 0)
            .expect("XGC_S2_SAMPLES 必须是正整数"),
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => panic!("读取 XGC_S2_SAMPLES 失败：{error}"),
    }
}

struct Samples {
    elapsed: Vec<Duration>,
    allocations: u64,
    bytes: u64,
}

impl Samples {
    fn report(&self, label: &str) {
        let percentile = |p: usize| ms(self.elapsed[(self.elapsed.len() * p).div_ceil(100) - 1]);
        eprintln!(
            "{label}: samples={} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} allocations={} bytes={}",
            self.elapsed.len(),
            percentile(50),
            percentile(95),
            percentile(99),
            self.allocations,
            self.bytes
        );
    }
}

fn sample_steps(count: usize, mut step: impl FnMut(usize)) -> Samples {
    let mut elapsed = Vec::with_capacity(count);
    support::reset_counter();
    for index in 0..count {
        let start = Instant::now();
        step(index);
        elapsed.push(start.elapsed());
    }
    let (allocations, bytes) = support::snapshot().expect("量测线程的计数器");
    elapsed.sort_unstable();
    Samples {
        elapsed,
        allocations,
        bytes,
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
