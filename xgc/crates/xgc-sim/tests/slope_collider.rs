//! 坡面（`physics.slope`）的 rapier 碰撞体回归：形状要与解析坡面对齐、要真的挡住人、
//! 改了坡面参数要重建。
//!
//! 背景：`Slope` 是纯解析高度函数，ECS 侧（贴地 / 陡坡判定）读它；rapier 侧看不到解析
//! 函数，只能看碰撞体。这一刀补的正是"两侧看同一个坡面"，所以断言分两头：射线下探到
//! 的高度必须等于 `Slope::height_at`，角色走到陡坡处必须被挡在坡脚。

use xgc_sim::{RapierPhysicsSystem, RapierWorld, Schedule, VelocitySystem, FIXED_DT};
use xgc_world::{CharacterController, PhysicsBody, Slope, Transform, Velocity, World};

fn slope(origin: [f32; 3], size: [f32; 2], rise: f32) -> Slope {
    Slope { origin, size, rise }
}

fn step_times(world: &mut World, frames: usize) {
    let mut schedule = Schedule::new();
    schedule.add(VelocitySystem::default());
    schedule.add(RapierPhysicsSystem::default());
    for _ in 0..frames {
        schedule.run(world, FIXED_DT);
    }
}

/// 60° 陡坡（`slope_limit` 默认 45°，KCC 爬不上去）必须把角色挡在坡脚。
///
/// 没有碰撞体时角色会径直穿到坡的另一侧（x≈5），这正是本用例要钉的差别。
#[test]
fn a_character_is_stopped_by_a_slope_collider() {
    let mut world = World::new();
    world.spawn((slope([0.0, 0.0, 0.0], [10.0, 10.0], 17.32),));
    let character = world.spawn((
        Transform {
            position: [-3.0, 0.0, 0.0],
            ..Transform::default()
        },
        Velocity {
            linear: [4.0, 0.0, 0.0],
        },
        CharacterController::default(),
    ));
    step_times(&mut world, 120);
    let x = world.get::<Transform>(character).unwrap().position[0];
    assert!(x < 0.2, "角色穿过了陡坡碰撞体：x={x}");
    assert!(x > -1.5, "角色没走到坡脚：x={x}");
}

/// 碰撞体上表面必须落在解析坡面上：射线探到的高度与 `Slope::height_at` 逐点对齐。
#[test]
fn slope_collider_matches_the_analytic_surface() {
    let mut world = World::new();
    let def = slope([0.0, 0.0, 0.0], [10.0, 10.0], 4.0);
    world.spawn((def,));
    let mut rapier = RapierWorld::new();
    rapier.step(&mut world, FIXED_DT);
    for x in [1.0_f32, 3.5, 7.0, 9.5] {
        let expected = def.height_at(x, 5.0).expect("解析坡面在跨度内");
        let hit = rapier
            .ground_under(x, 5.0, 100.0)
            .unwrap_or_else(|| panic!("x={x} 处射线没打到坡面碰撞体"));
        assert!(
            (hit - expected).abs() < 1e-3,
            "x={x} 处碰撞体高度 {hit} 与解析坡面 {expected} 不一致"
        );
    }
}

/// 改坡面参数（这里直接改组件，与编辑器写组件同一条路径）后碰撞体必须重建。
#[test]
fn a_slope_collider_rebuilds_when_the_slope_changes() {
    let mut world = World::new();
    let def = slope([0.0, 0.0, 0.0], [10.0, 10.0], 2.0);
    let key = world.spawn((def,));
    let mut rapier = RapierWorld::new();
    rapier.step(&mut world, FIXED_DT);
    let before = rapier.ground_under(5.0, 5.0, 100.0).expect("初始坡面");
    assert!((before - 1.0).abs() < 1e-3, "初始高度 {before}");

    world.get_mut::<Slope>(key).unwrap().rise = 6.0;
    rapier.step(&mut world, FIXED_DT);
    let after = rapier.ground_under(5.0, 5.0, 100.0).expect("重建后的坡面");
    assert!(
        (after - 3.0).abs() < 1e-3,
        "改坡面后碰撞体没重建：高度仍是 {after}（应 3.0）"
    );
}

/// 带 `physics.body` 的实体由显式刚体托管几何，坡面模块必须让位。
///
/// 否则同一个坡会同时有"自动楔形"和用户自己的碰撞体两层，探地 / 扫掠会摸到不该有的面。
#[test]
fn a_slope_with_an_explicit_body_gets_no_automatic_collider() {
    let mut world = World::new();
    world.spawn((
        slope([0.0, 0.0, 0.0], [10.0, 10.0], 4.0),
        PhysicsBody {
            kind: "static".to_string(),
            shape: "cuboid".to_string(),
            ..PhysicsBody::default()
        },
        Transform {
            position: [50.0, 0.0, 0.0],
            ..Transform::default()
        },
    ));
    let mut rapier = RapierWorld::new();
    rapier.step(&mut world, FIXED_DT);
    assert!(
        rapier.ground_under(5.0, 5.0, 100.0).is_none(),
        "带 physics.body 的坡面不该再建自动碰撞体"
    );
}

/// 尺寸退化 / 参数非有限的坡面不建碰撞体，别把垃圾几何交给 rapier。
#[test]
fn degenerate_slopes_are_skipped() {
    let mut world = World::new();
    world.spawn((slope([0.0, 0.0, 0.0], [10.0, 10.0], 4.0),));
    world.spawn((slope([0.0, 0.0, 50.0], [0.0, 10.0], 4.0),));
    world.spawn((slope([0.0, 0.0, 100.0], [10.0, 10.0], f32::NAN),));
    let mut rapier = RapierWorld::new();
    rapier.step(&mut world, FIXED_DT);
    assert!(
        rapier.ground_under(5.0, 5.0, 100.0).is_some(),
        "合法的坡面必须建出碰撞体"
    );
    // 宽度为 0 的坡若不校验，会退化成 x≈0.05 处一块竖直薄板（局部 X 轴转到世界 Y），
    // 所以直接用那块板的位置探测：没有校验时这里会命中 y≈4。
    assert!(
        rapier.ground_under(0.05, 55.0, 100.0).is_none(),
        "宽度为 0 的坡面不该有碰撞体"
    );
    assert!(
        rapier.ground_under(5.0, 105.0, 100.0).is_none(),
        "rise 为 NaN 的坡面不该有碰撞体"
    );
}

/// 下坡（`rise < 0`）也要对齐：法线方向随之翻转，是符号最容易写错的一档。
#[test]
fn a_downhill_slope_collider_matches_the_analytic_surface() {
    let mut world = World::new();
    let def = slope([0.0, 0.0, 0.0], [10.0, 10.0], -4.0);
    world.spawn((def,));
    let mut rapier = RapierWorld::new();
    rapier.step(&mut world, FIXED_DT);
    for x in [1.0_f32, 5.0, 9.0] {
        let expected = def.height_at(x, 5.0).expect("解析坡面在跨度内");
        let hit = rapier
            .ground_under(x, 5.0, 100.0)
            .unwrap_or_else(|| panic!("x={x} 处射线没打到下坡碰撞体"));
        assert!(
            (hit - expected).abs() < 1e-3,
            "x={x} 处下坡碰撞体高度 {hit} 与解析坡面 {expected} 不一致"
        );
    }
}
