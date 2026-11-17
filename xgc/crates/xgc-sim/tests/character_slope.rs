//! 角色坡面语义：`physics.character` 的 `slope_limit` / `snap_to_ground`（旧引擎同一档位语义）。
//!
//! 单独一个集成测试二进制：拆分前 `character.rs` 已贴着 600 行软上限，且这些用例要的
//! 世界构造（坡 + 角色，无刚体）与台阶 / 阻挡那组不同。

use xgc_sim::{CharacterSystem, Schedule, VelocitySystem};
use xgc_world::{CharacterController, Slope, Transform, Velocity, World};
/// 60° 斜坡（rise 17.32 / size.x 10）对 45° 上限的角色是墙，不是地面。
///
/// 旧实现只按坡面高度抬升，`slope_limit` 完全没被读——角色会直接站上 60° 陡坡。
#[test]
fn character_refuses_to_climb_a_slope_above_its_limit() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [1.0, 0.0, 5.0],
            ..Transform::default()
        },
        Velocity {
            linear: [3.0, 0.0, 0.0],
        },
        CharacterController::default(),
    ));
    world.spawn((Slope {
        origin: [0.0, 0.0, 0.0],
        size: [10.0, 10.0],
        rise: 17.32,
    },));
    let mut schedule = Schedule::new();
    schedule.add(CharacterSystem);
    schedule.run(&mut world, 0.016);
    let transform = world.get::<Transform>(character).unwrap();
    // 坡底在 x=0，角色半径 0.3 → 停在 -0.3；y 保持 0（没被抬上坡面）。
    assert!(
        (transform.position[0] + 0.3).abs() < 1e-4,
        "x={}",
        transform.position[0]
    );
    assert!(
        transform.position[1].abs() < 1e-5,
        "y={}",
        transform.position[1]
    );
    assert_eq!(world.get::<Velocity>(character).unwrap().linear[0], 0.0);
}

/// 30° 斜坡在 45° 上限内，照旧是地面——挡住"一刀切拒绝所有坡面"的过度实现。
#[test]
fn character_walks_up_a_slope_within_its_limit() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [5.0, 0.0, 5.0],
            ..Transform::default()
        },
        Velocity::default(),
        CharacterController::default(),
    ));
    world.spawn((Slope {
        origin: [0.0, 0.0, 0.0],
        size: [10.0, 10.0],
        rise: 5.773_503,
    },));
    let mut schedule = Schedule::new();
    schedule.add(CharacterSystem);
    schedule.run(&mut world, 0.016);
    let transform = world.get::<Transform>(character).unwrap();
    assert!(
        (transform.position[1] - 2.886_751_5).abs() < 1e-4,
        "y={}",
        transform.position[1]
    );
    assert!(
        world
            .get::<CharacterController>(character)
            .unwrap()
            .grounded
    );
}

/// 下坡时脚底离地不到 `snap_to_ground` 就按回地面，`grounded` 不能抖。
#[test]
fn character_snaps_down_to_the_ground_it_is_walking_over() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            // 坡面在 x=5 处高 2.8868，角色悬在上方 0.1（默认吸附距离 0.2）。
            position: [5.0, 2.986_751_5, 5.0],
            ..Transform::default()
        },
        Velocity {
            linear: [0.0, -0.5, 0.0],
        },
        CharacterController::default(),
    ));
    world.spawn((Slope {
        origin: [0.0, 0.0, 0.0],
        size: [10.0, 10.0],
        rise: 5.773_503,
    },));
    let mut schedule = Schedule::new();
    schedule.add(CharacterSystem);
    schedule.run(&mut world, 0.016);
    let transform = world.get::<Transform>(character).unwrap();
    assert!(
        (transform.position[1] - 2.886_751_5).abs() < 1e-4,
        "y={}",
        transform.position[1]
    );
    let velocity = world.get::<Velocity>(character).unwrap();
    assert_eq!(velocity.linear[1], 0.0);
    assert!(
        world
            .get::<CharacterController>(character)
            .unwrap()
            .grounded
    );
}

/// 吸附只吃向下的速度：起跳当帧不能被按回地面。
#[test]
fn snapping_does_not_cancel_a_jump() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [5.0, 2.936_751_5, 5.0],
            ..Transform::default()
        },
        Velocity {
            linear: [0.0, 5.0, 0.0],
        },
        CharacterController::default(),
    ));
    world.spawn((Slope {
        origin: [0.0, 0.0, 0.0],
        size: [10.0, 10.0],
        rise: 5.773_503,
    },));
    let mut schedule = Schedule::new();
    schedule.add(CharacterSystem);
    schedule.run(&mut world, 0.016);
    let transform = world.get::<Transform>(character).unwrap();
    assert!(
        (transform.position[1] - 2.936_751_5).abs() < 1e-5,
        "y={}",
        transform.position[1]
    );
    assert!(
        !world
            .get::<CharacterController>(character)
            .unwrap()
            .grounded
    );
}

/// 从高端（+X）走进陡坡：必须推到背面，不能一帧传送过整个斜坡。
///
/// 只按坡底推的实现在这里会把 x=10 的角色直接放到 x=-0.3（跨过 10 m 的坡面）。
#[test]
fn character_stays_outside_the_high_end_of_a_steep_slope() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [10.0, 0.0, 5.0],
            ..Transform::default()
        },
        Velocity {
            linear: [-3.0, 0.0, 0.0],
        },
        CharacterController::default(),
    ));
    world.spawn((Slope {
        origin: [0.0, 0.0, 0.0],
        size: [10.0, 10.0],
        rise: 17.32,
    },));
    let mut schedule = Schedule::new();
    schedule.add(CharacterSystem);
    schedule.run(&mut world, 0.016);
    let transform = world.get::<Transform>(character).unwrap();
    // 坡面在 x=10 收口，背面墙在 x=10+半径 0.3。
    assert!(
        (transform.position[0] - 10.3).abs() < 1e-4,
        "x={}",
        transform.position[0]
    );
    assert_eq!(world.get::<Velocity>(character).unwrap().linear[0], 0.0);
}

/// 坡面架在 5 m 高平台上时，坡底外推不能变成坡面之外的隐形墙。
///
/// `base_x_at` 不夹进坡面跨度的话，x=1 的角色会被推到 -3.187（离坡面 3 米多）。
#[test]
fn a_raised_slope_does_not_push_from_outside_its_own_span() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [1.0, 0.0, 5.0],
            ..Transform::default()
        },
        Velocity {
            linear: [1.0, 0.0, 0.0],
        },
        CharacterController::default(),
    ));
    world.spawn((Slope {
        origin: [0.0, 5.0, 0.0],
        size: [10.0, 10.0],
        rise: 17.32,
    },));
    let mut schedule = Schedule::new();
    schedule.add(CharacterSystem);
    schedule.run(&mut world, 0.016);
    let transform = world.get::<Transform>(character).unwrap();
    assert!(
        (transform.position[0] + 0.3).abs() < 1e-4,
        "x={}",
        transform.position[0]
    );
}

/// 上限本身可调：`slope_limit = 60` 时 60° 坡就是地面（钉住"按度比较"而不是写死 45）。
#[test]
fn a_slope_at_the_configured_limit_is_walkable() {
    let mut world = World::new();
    // 上限直接取坡面自己的倾角：钉住"正好等于上限也可走"（写死 60 与 17.32/10 的
    // 59.9996° 差一点，`<=` 改成 `<` 也照样绿）。
    let slope = Slope {
        origin: [0.0, 0.0, 0.0],
        size: [10.0, 10.0],
        rise: 17.32,
    };
    let controller = CharacterController {
        slope_limit: slope.degrees(),
        ..CharacterController::default()
    };
    let character = world.spawn((
        Transform {
            position: [5.0, 0.0, 5.0],
            ..Transform::default()
        },
        Velocity::default(),
        controller,
    ));
    world.spawn((slope,));
    let mut schedule = Schedule::new();
    schedule.add(CharacterSystem);
    schedule.run(&mut world, 0.016);
    let transform = world.get::<Transform>(character).unwrap();
    assert!(
        (transform.position[1] - 8.66).abs() < 1e-4,
        "y={}",
        transform.position[1]
    );
}

/// `snap_to_ground = 0` 表示不吸附：悬空 0.1 m 的角色不能被按回地面。
#[test]
fn zero_snap_distance_leaves_an_airborne_character_alone() {
    let mut world = World::new();
    let controller = CharacterController {
        snap_to_ground: 0.0,
        ..CharacterController::default()
    };
    let character = world.spawn((
        Transform {
            position: [5.0, 0.1, 5.0],
            ..Transform::default()
        },
        Velocity {
            linear: [0.0, -1.0, 0.0],
        },
        controller,
    ));
    let mut schedule = Schedule::new();
    schedule.add(CharacterSystem);
    schedule.run(&mut world, 0.016);
    let transform = world.get::<Transform>(character).unwrap();
    assert!(
        (transform.position[1] - 0.1).abs() < 1e-5,
        "y={}",
        transform.position[1]
    );
    assert!(
        !world
            .get::<CharacterController>(character)
            .unwrap()
            .grounded
    );
}

/// 下坡（rise <= 0）没有能卡人的立面，多陡都当地面：架在 10 m 平台上时脚底贴着坡面。
#[test]
fn descending_slopes_stay_walkable_whatever_their_angle() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [2.0, 0.0, 5.0],
            ..Transform::default()
        },
        Velocity::default(),
        CharacterController::default(),
    ));
    world.spawn((Slope {
        origin: [0.0, 10.0, 0.0],
        size: [10.0, 10.0],
        rise: -17.32,
    },));
    let mut schedule = Schedule::new();
    schedule.add(CharacterSystem);
    schedule.run(&mut world, 0.016);
    let transform = world.get::<Transform>(character).unwrap();
    // 坡面在 x=2 处高 10 - 17.32 * 0.2 = 6.536。
    assert!(
        (transform.position[1] - 6.536).abs() < 1e-3,
        "y={}",
        transform.position[1]
    );
    assert!(
        world
            .get::<CharacterController>(character)
            .unwrap()
            .grounded
    );
}

/// 真的走上去：`VelocitySystem` 推进 x、`CharacterSystem` 跟着坡面抬 y，连续多帧成立。
#[test]
fn character_keeps_climbing_a_walkable_slope_over_several_frames() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [0.0, 0.0, 5.0],
            ..Transform::default()
        },
        Velocity {
            linear: [2.0, 0.0, 0.0],
        },
        CharacterController::default(),
    ));
    world.spawn((Slope {
        origin: [0.0, 0.0, 0.0],
        size: [10.0, 10.0],
        rise: 5.773_503,
    },));
    let mut schedule = Schedule::new();
    schedule.add(VelocitySystem::default());
    schedule.add(CharacterSystem);
    for _ in 0..6 {
        schedule.run(&mut world, 0.1);
    }
    let transform = world.get::<Transform>(character).unwrap();
    assert!(
        (transform.position[0] - 1.2).abs() < 1e-3,
        "x={}",
        transform.position[0]
    );
    // 30° 坡：y = rise * (x / size.x) = 5.773503 * 0.12 = 0.69282。
    assert!(
        (transform.position[1] - 0.692_820_4).abs() < 1e-3,
        "y={}",
        transform.position[1]
    );
    assert!(
        world
            .get::<CharacterController>(character)
            .unwrap()
            .grounded
    );
}

/// `slope_limit = NaN` 落回缺省 45°：否则 `degrees() <= NaN` 恒假，连平地都被当墙。
#[test]
fn a_nan_slope_limit_falls_back_to_the_default_limit() {
    let mut world = World::new();
    let controller = CharacterController {
        slope_limit: f32::NAN,
        ..CharacterController::default()
    };
    let character = world.spawn((
        Transform {
            position: [5.0, 0.0, 5.0],
            ..Transform::default()
        },
        Velocity::default(),
        controller,
    ));
    world.spawn((Slope {
        origin: [0.0, 0.0, 0.0],
        size: [10.0, 10.0],
        rise: 5.773_503,
    },));
    let mut schedule = Schedule::new();
    schedule.add(CharacterSystem);
    schedule.run(&mut world, 0.016);
    let transform = world.get::<Transform>(character).unwrap();
    assert!(
        (transform.position[1] - 2.886_751_5).abs() < 1e-4,
        "y={}",
        transform.position[1]
    );
}

/// 角色陷在坡体高处（`feet > origin.y`，正常行走到不了、spawn / 脚本能到）时也去最近出口。
///
/// 用坡面几何中点选边的话这里会推到坡背（x=10.3，横移 4.8 m），最近的其实是坡底 3.7416。
#[test]
fn a_character_inside_a_steep_slope_goes_to_the_nearer_side() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [5.5, 7.0, 5.0],
            ..Transform::default()
        },
        Velocity::default(),
        CharacterController::default(),
    ));
    world.spawn((Slope {
        origin: [0.0, 0.0, 0.0],
        size: [10.0, 10.0],
        rise: 17.32,
    },));
    let mut schedule = Schedule::new();
    schedule.add(CharacterSystem);
    schedule.run(&mut world, 0.016);
    let transform = world.get::<Transform>(character).unwrap();
    // base = 7 * 10 / 17.32 = 4.041571，减半径 0.3 = 3.741571。
    assert!(
        (transform.position[0] - 3.741_571).abs() < 1e-4,
        "x={}",
        transform.position[0]
    );
}

/// 从可走坡面往下走：靠 `snap_to_ground` 的吸附分支逐帧贴住坡面（vy 全程为 0）。
#[test]
fn character_walks_down_a_walkable_slope_over_several_frames() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [8.0, 4.618_803, 5.0],
            ..Transform::default()
        },
        Velocity {
            linear: [-2.0, 0.0, 0.0],
        },
        CharacterController::default(),
    ));
    world.spawn((Slope {
        origin: [0.0, 0.0, 0.0],
        size: [10.0, 10.0],
        rise: 5.773_503,
    },));
    let mut schedule = Schedule::new();
    schedule.add(VelocitySystem::default());
    schedule.add(CharacterSystem);
    for _ in 0..6 {
        schedule.run(&mut world, 0.1);
    }
    let transform = world.get::<Transform>(character).unwrap();
    assert!(
        (transform.position[0] - 6.8).abs() < 1e-3,
        "x={}",
        transform.position[0]
    );
    assert!(
        (transform.position[1] - 3.925_982).abs() < 1e-3,
        "y={}",
        transform.position[1]
    );
    assert!(world.get::<Velocity>(character).unwrap().linear[1] == 0.0);
    assert!(
        world
            .get::<CharacterController>(character)
            .unwrap()
            .grounded
    );
}
