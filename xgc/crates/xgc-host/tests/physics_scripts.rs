use std::path::PathBuf;

use xgc_base::EntityKey;
use xgc_host::ScriptedRunner;
use xgc_sim::GRAVITY_Y;
use xgc_world::{PhysicsBody, Script, Transform, Velocity, World};

struct ScriptRoot(PathBuf);

impl ScriptRoot {
    fn new(name: &str, source: &str) -> Self {
        let root = Self(
            std::env::temp_dir().join(format!("xgc_host_physics_{name}_{}", std::process::id())),
        );
        std::fs::create_dir_all(&root.0).unwrap();
        std::fs::write(root.0.join("physics.lua"), source).unwrap();
        root
    }

    fn runner(&self, world: World) -> ScriptedRunner {
        ScriptedRunner::with_assets(world, xgc_host::asset_paths(Some(&self.0), &[])).0
    }
}

impl Drop for ScriptRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn velocity(runner: &ScriptedRunner, entity: EntityKey) -> [f32; 3] {
    runner.world().get::<Velocity>(entity).unwrap().linear
}

fn position(runner: &ScriptedRunner, entity: EntityKey) -> [f32; 3] {
    runner.world().get::<Transform>(entity).unwrap().position
}

fn assert_near(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-5,
        "预期 {expected}，实得 {actual}"
    );
}

#[test]
fn lua_sets_a_dynamic_body_velocity_once_then_gravity_continues() {
    let root = ScriptRoot::new(
        "dynamic",
        r#"
        local steps = 0
        function update(ctx)
            steps = steps + 1
            if steps == 2 then
                local id = self_id(ctx)
                set_velocity(ctx, id, 3, 6.5, -2)
                local v = velocity(ctx, id)
                assert(v.x == 3 and v.y == 6.5 and v.z == -2)
            end
        end
        "#,
    );
    let mut world = World::new();
    let body = world.spawn((
        Transform {
            position: [0.0, 10.0, 0.0],
            ..Transform::default()
        },
        PhysicsBody::default(),
        Velocity::default(),
        Script {
            paths: vec!["physics.lua".into()],
        },
    ));
    let mut runner = root.runner(world);
    let dt = runner.fixed_dt();
    let gravity_delta = GRAVITY_Y * dt;
    runner.step().unwrap();
    assert_near(velocity(&runner, body)[1], gravity_delta);
    runner.step().unwrap();
    let before = position(&runner, body);

    runner.step().unwrap();
    let after = position(&runner, body);
    let first_velocity = velocity(&runner, body);
    assert_near(first_velocity[1], 6.5 + gravity_delta);
    assert_near(first_velocity[0], 3.0);
    assert_near(first_velocity[2], -2.0);
    assert_near(after[0] - before[0], 3.0 * dt);
    assert_near(after[2] - before[2], -2.0 * dt);
    // Rapier 缺省的四个子步逐次累加重力，单步位移是 v₀·dt + g·dt²·5/8。
    let gravity_displacement = gravity_delta * dt * 5.0 / 8.0;
    assert_near(after[1] - before[1], 6.5 * dt + gravity_displacement);

    runner.step().unwrap();
    let second_velocity = velocity(&runner, body);
    assert_near(second_velocity[1], first_velocity[1] + gravity_delta);
    let next = position(&runner, body);
    assert_near(next[0] - after[0], 3.0 * dt);
    assert_near(
        next[1] - after[1],
        first_velocity[1] * dt + gravity_displacement,
    );
    assert_near(next[2] - after[2], -2.0 * dt);
}

#[test]
fn collision_hook_sets_solver_velocity_without_an_ecs_velocity_component() {
    let root = ScriptRoot::new(
        "sensor",
        r#"
        local entered = false
        function on_collision_enter(ctx, other)
            assert(not entered and other ~= self_id(ctx))
            assert(math.abs(pos(ctx).x) < 1e-5)
            entered = true
            set_velocity(ctx, self_id(ctx), -60, 0, 0)
            set_velocity(ctx, self_id(ctx), 120, 0, 0)
        end
        function on_collision_exit(ctx, other)
            assert(entered and other ~= self_id(ctx))
            assert(math.abs(pos(ctx).x - 2.0) < 1e-5)
            set_velocity(ctx, self_id(ctx), 0, 0, 0)
        end
        "#,
    );
    let mut world = World::new();
    let transform = Transform {
        position: [0.0, 4.0, 0.0],
        ..Transform::default()
    };
    world.spawn((
        transform,
        PhysicsBody {
            kind: "static".into(),
            is_sensor: true,
            ..PhysicsBody::default()
        },
    ));
    let body = world.spawn((
        transform,
        PhysicsBody {
            gravity_scale: 0.0,
            ..PhysicsBody::default()
        },
        Script {
            paths: vec!["physics.lua".into()],
        },
    ));
    let body_id = world.entity(body).unwrap();
    let mut runner = root.runner(world);
    let mut distances = Vec::new();
    let mut edges = Vec::new();
    for frame in 1..=5 {
        runner.step().unwrap();
        distances.push(position(&runner, body)[0]);
        edges.extend(
            runner
                .world()
                .collision_events()
                .iter()
                .filter(|event| event.entity == body_id)
                .map(|event| (frame, event.enter)),
        );
    }
    eprintln!("沿 X 轴的位置 {distances:?}，动体接触边沿 {edges:?}");
    // 进入回调的最后一条速度指令让动体在第二步穿出。Rapier 同步发出退出，
    // 退出回调清掉的速度在第三步生效，防止动体多走一个固定步。
    assert_eq!(edges, [(1, true), (2, false)]);
    for (distance, expected) in distances.into_iter().zip([0.0, 2.0, 2.0, 2.0, 2.0]) {
        assert_near(distance, expected);
    }
    assert!(runner.world().get::<Velocity>(body).is_none());
}
