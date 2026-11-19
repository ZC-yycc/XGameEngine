//! 角色重力：纯角色没有刚体、默认调度里也没有 `GravitySystem`，所以竖直积分必须由
//! `CharacterSystem` 负责，否则腾空后永不落地（落差大于 `snap_to_ground` 时悬在空中）。
//!
//! 单独一个测试二进制：这一组只关心"竖直自由落体 / 跳跃弧线 / 站在地面上不沉"，
//! 与坡面语义（`character_slope.rs`）和水平 collide-and-slide（`character_kcc.rs`）分开。

use xgc_sim::{CharacterSystem, Schedule};
use xgc_world::{CharacterController, Slope, Transform, Velocity, World};

const DT: f32 = 1.0 / 60.0;

fn run(world: &mut World, frames: usize) {
    let mut schedule = Schedule::new();
    schedule.add(CharacterSystem::default());
    for _ in 0..frames {
        schedule.run(world, DT);
    }
}

/// 悬空 3 m 的角色必须落到 y=0 并停下（旧实现完全不积分竖直，会悬在原地）。
#[test]
fn a_character_in_the_air_falls_and_lands_on_the_ground() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [0.0, 3.0, 0.0],
            ..Transform::default()
        },
        Velocity::default(),
        CharacterController::default(),
    ));
    run(&mut world, 120);
    let transform = world.get::<Transform>(character).unwrap();
    assert!(
        transform.position[1].abs() < 1e-3,
        "落地后 y={}",
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

/// 跳跃是完整的抛物线：先升到 1 m 以上，再落回地面并清掉速度。
#[test]
fn a_jump_arc_comes_back_down_to_the_ground() {
    let mut world = World::new();
    let character = world.spawn((
        Transform::default(),
        Velocity {
            linear: [0.0, 5.0, 0.0],
        },
        CharacterController::default(),
    ));
    let mut peak = 0.0_f32;
    let mut schedule = Schedule::new();
    schedule.add(CharacterSystem::default());
    for _ in 0..200 {
        schedule.run(&mut world, DT);
        peak = peak.max(world.get::<Transform>(character).unwrap().position[1]);
    }
    assert!(peak > 1.0, "跳跃弧线最高点只有 {peak}");
    let transform = world.get::<Transform>(character).unwrap();
    assert!(
        transform.position[1].abs() < 1e-3,
        "落地后 y={}",
        transform.position[1]
    );
    assert_eq!(world.get::<Velocity>(character).unwrap().linear[1], 0.0);
}

/// 站在地面上不能被重力拉下去（也不该每帧抖出一个向下速度）。
#[test]
fn a_grounded_character_is_not_pulled_down() {
    let mut world = World::new();
    let character = world.spawn((
        Transform::default(),
        Velocity::default(),
        CharacterController::default(),
    ));
    run(&mut world, 60);
    let transform = world.get::<Transform>(character).unwrap();
    assert_eq!(transform.position[1], 0.0);
    assert_eq!(world.get::<Velocity>(character).unwrap().linear[1], 0.0);
}

/// 走出坡面跨度（高端之外）后脚下不再有地面，角色必须掉回 y=0。
#[test]
fn a_character_walking_off_a_slope_falls_to_the_floor() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            // 30° 坡在 x=10 处高 5.7735；角色站在跨度之外、同高位置。
            position: [12.0, 5.773_503, 0.0],
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
    run(&mut world, 180);
    let transform = world.get::<Transform>(character).unwrap();
    assert!(
        transform.position[1].abs() < 1e-3,
        "掉下来后 y={}",
        transform.position[1]
    );
}

/// 站在坡面上（脚底贴着坡面高度）不是腾空，重力不能把角色从坡上拽下来。
#[test]
fn a_character_standing_on_a_slope_does_not_slide_down() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [5.0, 2.886_751_5, 0.0],
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
    run(&mut world, 60);
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

/// 默认调度下竖直位移也只能积分一次。
///
/// `VelocitySystem` 曾经给角色单独积分竖直（水平已交给 KCC），`CharacterSystem` 也开始
/// 积分竖直之后，同一帧会写两遍 `y`：自由落体看起来"重力翻倍"、跳跃高度翻倍。
/// 这里用半隐式欧拉的解析解钉死单次积分：`n` 帧后 `y = y0 - g·dt²·n(n+1)/2`。
#[test]
fn a_character_falls_once_per_frame_under_the_default_schedule() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [0.0, 3.0, 0.0],
            ..Transform::default()
        },
        Velocity::default(),
        CharacterController::default(),
    ));
    let mut schedule = Schedule::default_schedule(1);
    let frames = 10.0_f32;
    for _ in 0..frames as usize {
        schedule.run(&mut world, DT);
    }
    let expected = 3.0 - 9.81 * DT * DT * frames * (frames + 1.0) * 0.5;
    let y = world.get::<Transform>(character).unwrap().position[1];
    assert!(
        (y - expected).abs() < 1e-4,
        "默认调度把竖直积分了两遍：y={y}，单次积分应为 {expected}"
    );
}
