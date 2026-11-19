//! 角色的竖直 / 地面 / 坡面回归：竖直自由落体、跳跃弧线、贴地不沉、可行走坡、平台边缘、
//! 落到箱顶、斜坡站立不滑、确定性。
//!
//! 背景：这一刀把角色竖直（重力、地面探测、抬离地面、贴地吸附、台阶）从 ECS 的
//! `CharacterSystem` / `CharacterStepSystem` 搬进 rapier 角色阶段，所以断言口径从"解析坡面
//! 高度函数 + y=0"变成"碰撞体 + 隐式 y=0 平面"：坡面用 `physics.slope` 的静态楔形碰撞体，
//! 平地仍是 `y = 0`（XGC 还没有地面碰撞体）。摆这类用例时给每帧重写速度，模拟脚本的输入。
//!
//! 坡面上的**静息脚底高度**比解析坡面高 `(radius + rest_gap)·(sec θ − 1)`：胶囊是圆柱侧壁 +
//! 半球底，沿法线保持 `rest_gap` 的间隙时，脚底（胶囊中心减 `center_height` 再减 `rest_gap`）
//! 就会比坡面高出这一段 —— 30° 坡约 5.0 cm、45° 坡约 13.3 cm，平地（θ = 0）为 0。
//! 这是"胶囊 + 间隙"几何的固有量，不是误差：旧端也保留同样的 `rest_gap` 间隙（它的间隙公式
//! 少算了半高在法线上的投影，实际悬空更高，约 14 cm），所以断言按公式钉住而不是钉在解析坡面上。

use xgc_sim::{Schedule, FIXED_DT};
use xgc_world::{CharacterController, PhysicsBody, Slope, Transform, Velocity, World};

fn slope(origin: [f32; 3], size: [f32; 2], rise: f32) -> Slope {
    Slope { origin, size, rise }
}

fn static_box(position: [f32; 3], half: [f32; 3]) -> (Transform, PhysicsBody) {
    (
        Transform {
            position,
            ..Transform::default()
        },
        PhysicsBody {
            kind: "static".into(),
            shape: "cuboid".into(),
            half_extents: half,
            ..PhysicsBody::default()
        },
    )
}

fn run_default(world: &mut World, frames: usize) {
    let mut schedule = Schedule::default_schedule(1);
    for _ in 0..frames {
        schedule.run(world, FIXED_DT);
    }
}

fn position(world: &World, key: xgc_base::EntityKey) -> [f32; 3] {
    world.get::<Transform>(key).unwrap().position
}

/// 空中落体：初始高度 3，重力把脚底拉回 `y = 0` 并清零向下速度。
#[test]
fn a_character_in_the_air_falls_to_the_ground() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [0.0, 3.0, 0.0],
            ..Transform::default()
        },
        Velocity::default(),
        CharacterController::default(),
    ));
    run_default(&mut world, 120);
    let p = position(&world, character);
    assert!(p[1].abs() < 1e-3, "落地后 y={}", p[1]);
    let velocity = *world.get::<Velocity>(character).unwrap();
    assert_eq!(velocity.linear[1], 0.0, "落地后竖直速度必须清零");
    assert!(
        world
            .get::<CharacterController>(character)
            .unwrap()
            .grounded,
        "落地后必须判定为着地"
    );
}

/// 跳跃弧线：给一个向上初速，抛物线到顶（> 1 m）再落回地面。
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
    let mut peak: f32 = 0.0;
    let mut schedule = Schedule::default_schedule(1);
    for _ in 0..200 {
        schedule.run(&mut world, FIXED_DT);
        peak = peak.max(position(&world, character)[1]);
    }
    assert!(peak > 1.0, "跳跃峰值只有 {peak}");
    let p = position(&world, character);
    assert!(p[1].abs() < 1e-3, "跳跃后没落回地面：y={}", p[1]);
    assert_eq!(world.get::<Velocity>(character).unwrap().linear[1], 0.0);
}

/// 站在地面上不许被重力一点点拉下去（重力只在腾空时累加）。
#[test]
fn a_grounded_character_is_not_pulled_down() {
    let mut world = World::new();
    let character = world.spawn((Transform::default(), CharacterController::default()));
    run_default(&mut world, 60);
    let p = position(&world, character);
    assert!(p[1].abs() < 1e-6, "站在地面上却被拉下去了：y={}", p[1]);
    assert!(
        world
            .get::<CharacterController>(character)
            .unwrap()
            .grounded
    );
}

/// 站在可行走坡面上不动：不会被重力拽着沿坡下滑（下滑需要主动沿切向推）。
#[test]
fn a_character_standing_on_a_slope_does_not_slide_down() {
    let mut world = World::new();
    let def = slope([0.0, 0.0, 0.0], [10.0, 10.0], 5.773_503);
    world.spawn((def,));
    let height = def.height_at(5.0, 5.0).expect("坡面跨度内");
    let character = world.spawn((
        Transform {
            position: [5.0, height, 5.0],
            ..Transform::default()
        },
        Velocity::default(),
        CharacterController::default(),
    ));
    run_default(&mut world, 60);
    let p = position(&world, character);
    // 30° 坡的静息悬空量：(0.3 + 0.022)·(sec 30° − 1) ≈ 0.0498 m。
    let rest = height + 0.322 * (1.0 / 30.0_f32.to_radians().cos() - 1.0);
    assert!(
        (p[1] - rest).abs() < 2e-3,
        "站在坡面上的静息高度不对：y={} 期望 {rest}（解析坡面 {height} + 胶囊间隙）",
        p[1]
    );
    assert!(
        world
            .get::<CharacterController>(character)
            .unwrap()
            .grounded
    );
}

/// 30° 可行走坡：沿坡往上推，脚底要跟着坡面抬起来。
#[test]
fn a_character_walks_up_a_walkable_slope() {
    let mut world = World::new();
    let def = slope([0.0, 0.0, 0.0], [10.0, 10.0], 5.773_503);
    world.spawn((def,));
    let character = world.spawn((
        Transform {
            position: [1.0, def.height_at(1.0, 5.0).unwrap(), 5.0],
            ..Transform::default()
        },
        Velocity {
            linear: [2.0, 0.0, 0.0],
        },
        CharacterController::default(),
    ));
    let mut schedule = Schedule::default_schedule(1);
    // 上坡过程中脚底必须在坡面之上、也不能悬空超过一个大间隙（爬坡是沿坡面滑，不是飞）。
    for _ in 0..60 {
        if let Some(v) = world.get_mut::<Velocity>(character) {
            v.linear[0] = 2.0;
        }
        schedule.run(&mut world, FIXED_DT);
        let p = position(&world, character);
        let surface = def.height_at(p[0], p[2]).expect("还在坡面跨度内");
        let clearance = p[1] - surface;
        assert!(
            (-5e-2..=8e-2).contains(&clearance),
            "上坡时脚底与坡面的间隙异常：{clearance}（x={}, y={}, 坡面={surface}）",
            p[0],
            p[1]
        );
    }
    // 停下后回到「胶囊沿法线保持 rest_gap」的静息高度：30° 坡约 5.0 cm。
    for _ in 0..60 {
        if let Some(v) = world.get_mut::<Velocity>(character) {
            v.linear[0] = 0.0;
        }
        schedule.run(&mut world, FIXED_DT);
    }
    let p = position(&world, character);
    let surface = def.height_at(p[0], p[2]).expect("还在坡面跨度内");
    let expected = surface + 0.322 * (1.0 / 30.0_f32.to_radians().cos() - 1.0);
    assert!(p[0] > 2.0, "没走上坡：x={}", p[0]);
    assert!(
        p[0] < 3.5,
        "上坡速度异常：x={}（一步跨过整段说明台阶 / 滑动路径出了问题）",
        p[0]
    );
    assert!(
        (p[1] - expected).abs() < 2e-3,
        "停下后没跟住坡面：y={} 期望 {expected}（坡面 {surface} + 胶囊间隙）",
        p[1]
    );
}

/// 走出坡面跨度后掉回 `y = 0`（没有地面碰撞体，隐式平面兜底）。
#[test]
fn a_character_walking_off_a_slope_falls_to_the_floor() {
    let mut world = World::new();
    let def = slope([0.0, 0.0, 0.0], [10.0, 10.0], 5.773_503);
    world.spawn((def,));
    let character = world.spawn((
        Transform {
            position: [8.0, def.height_at(8.0, 5.0).unwrap(), 5.0],
            ..Transform::default()
        },
        Velocity {
            linear: [4.0, 0.0, 0.0],
        },
        CharacterController::default(),
    ));
    let mut schedule = Schedule::default_schedule(1);
    for _ in 0..180 {
        if let Some(v) = world.get_mut::<Velocity>(character) {
            v.linear[0] = 4.0;
        }
        schedule.run(&mut world, FIXED_DT);
    }
    let p = position(&world, character);
    assert!(p[0] > 10.0, "没走出坡面跨度：x={}", p[0]);
    assert!(p[1].abs() < 1e-2, "走出坡面后没掉回地面：y={}", p[1]);
}

/// 从空中落到箱顶：箱顶 0.5，落到上面站稳（不与隐式平面 y=0 混淆）。
#[test]
fn a_character_falls_onto_a_box_top() {
    let mut world = World::new();
    let (transform, body) = static_box([0.0, 0.25, 0.0], [1.0, 0.25, 1.0]);
    world.spawn((transform, body));
    let character = world.spawn((
        Transform {
            position: [0.0, 2.0, 0.0],
            ..Transform::default()
        },
        Velocity::default(),
        CharacterController::default(),
    ));
    run_default(&mut world, 120);
    let p = position(&world, character);
    assert!((p[1] - 0.5).abs() < 1e-2, "没站上箱顶：y={}", p[1]);
    assert!(
        world
            .get::<CharacterController>(character)
            .unwrap()
            .grounded
    );
}

/// 同一世界跑两遍，位置逐位相同（KCC + 自实现台阶 / 吸附都不许引入不确定性）。
#[test]
fn character_vertical_motion_is_deterministic() {
    fn shoot() -> Vec<u32> {
        let mut world = World::new();
        let def = slope([0.0, 0.0, 0.0], [10.0, 10.0], 5.773_503);
        world.spawn((def,));
        let character = world.spawn((
            Transform {
                position: [1.0, 2.0, 0.0],
                ..Transform::default()
            },
            Velocity {
                linear: [2.0, 0.0, 0.0],
            },
            CharacterController::default(),
        ));
        let mut schedule = Schedule::default_schedule(1);
        for _ in 0..60 {
            if let Some(v) = world.get_mut::<Velocity>(character) {
                v.linear[0] = 2.0;
            }
            schedule.run(&mut world, FIXED_DT);
        }
        position(&world, character)
            .iter()
            .map(|v| v.to_bits())
            .collect()
    }
    assert_eq!(shoot(), shoot(), "两次运行的位置必须逐位相同");
}

/// 下坡行走：贴地吸附必须把角色压回坡面，而不是让它一路小跳 / 悬空。
///
/// 坡面比 `y = 0` 高（`origin.y = 10`），所以解析平面不会抢在坡面之前当"地面"；
/// 30° 下坡 + 2 m/s 的水平输入每帧掉 `tan30° * 2 * dt ≈ 1.9 cm`，正好落在
/// `rest_gap < gap <= snap_to_ground` 的吸附窗口里。去掉吸附分支时角色会先腾空、
/// `grounded` 掉成 false，这条用例即变红。
#[test]
fn a_character_walking_down_a_slope_stays_on_the_ground() {
    let mut world = World::new();
    let def = slope([0.0, 10.0, 0.0], [10.0, 10.0], -5.773_503);
    world.spawn((def,));
    let start_x = 1.0;
    let hover = 0.322_f32 * (1.0 / (30.0_f32).to_radians().cos() - 1.0);
    let character = world.spawn((
        Transform {
            position: [start_x, def.height_at(start_x, 5.0).unwrap() + hover, 5.0],
            ..Transform::default()
        },
        Velocity::default(),
        CharacterController::default(),
    ));
    let mut schedule = Schedule::default_schedule(1);
    // 先走一帧让 `grounded` 落地（刚 spawn 的实体是 `false`，第一帧后才由探地决定）。
    schedule.run(&mut world, FIXED_DT);
    for frame in 0..90 {
        let x = position(&world, character)[0];
        let ground = def.height_at(x, 5.0).unwrap();
        let y = position(&world, character)[1];
        // 逐帧都算"踩在地上"：胶囊沿法线保持 rest_gap，脚底比坡面高出静息高度那一段。
        let hover = 0.322_f32 * (1.0 / (30.0_f32).to_radians().cos() - 1.0);
        assert!(
            (y - (ground + hover)).abs() < 5e-2,
            "第 {frame} 帧没踩在坡面上：y={y}，坡面 {ground}，静息 {hover}"
        );
        assert!(
            world
                .get::<CharacterController>(character)
                .unwrap()
                .grounded,
            "第 {frame} 帧下坡时不该离地：x={x}，y={y}"
        );
        if let Some(v) = world.get_mut::<Velocity>(character) {
            v.linear[0] = 2.0;
        }
        schedule.run(&mut world, FIXED_DT);
    }
    let p = position(&world, character);
    assert!(p[0] > 2.0, "下坡没前进：x={}", p[0]);
}

/// 陡坡站不住：坡度超过 `slope_limit + 10°` 时角色沿坡面滑下去，而不是钉在坡面上。
///
/// 这对应旧端的 `min_slope_slide_angle = (slope_limit + 10).min(89)`：`slope_limit` 以内能爬，
/// `slope_limit..+10°` 是迟滞带（爬不上去也不主动下滑），再陡才滑。修之前这里着地就把竖直
/// 速度清零，重力进不来，角色会永远"站"在 60° 坡面上。
#[test]
fn a_character_standing_on_a_steep_slope_slides_down() {
    let mut world = World::new();
    let def = slope([0.0, 0.0, 0.0], [10.0, 10.0], 17.320_508);
    world.spawn((def,));
    let start_x = 1.0;
    let hover = 0.322_f32 * (1.0 / (60.0_f32).to_radians().cos() - 1.0);
    let character = world.spawn((
        Transform {
            position: [start_x, def.height_at(start_x, 5.0).unwrap() + hover, 5.0],
            ..Transform::default()
        },
        Velocity::default(),
        CharacterController::default(),
    ));
    let mut schedule = Schedule::default_schedule(1);
    for _ in 0..30 {
        schedule.run(&mut world, FIXED_DT);
    }
    let early = position(&world, character);
    assert!(
        early[0] < start_x - 1e-2,
        "30 帧后必须已经开始沿坡下滑：x {} -> {}",
        start_x,
        early[0]
    );
    // 下滑过程中必须**一直贴着坡面**：把这帧的竖直位移清零（也就是不跳过"陡坡当墙"分支）
    // 会让角色退化成"水平挪一步 → 悬空 → 掉回坡面"的阶梯式下滑，横向位移与最终落地都
    // 仍然成立，只有逐帧贴着坡面这一条能抓出来（`steep_contact` 的 `!slipping` 门禁）。
    for _ in 0..150 {
        schedule.run(&mut world, FIXED_DT);
        let p = position(&world, character);
        if p[0] > 0.0 {
            let surface = def.height_at(p[0], 5.0).unwrap();
            let clearance = p[1] - surface;
            assert!(
                (-0.05..0.45).contains(&clearance),
                "下滑途中必须贴着坡面：x={} y={} 离坡面 {}",
                p[0],
                p[1],
                clearance
            );
            assert!(
                world
                    .get::<CharacterController>(character)
                    .unwrap()
                    .grounded,
                "下滑途中必须始终踩在坡面上：x={} y={}",
                p[0],
                p[1]
            );
        }
    }
    let p = position(&world, character);
    assert!(p[1].abs() < 1e-2, "滑到坡底后脚底必须落回地面：y={}", p[1]);
}

/// 迟滞带：45° + 10° 以内（这里 50°）的坡爬不上去，但也不会自己滑下来。
///
/// 没有 `+10°` 迟滞带的话，站在 46° 坡上的角色每帧都在"滑一点 / 被探地拉回"之间抖，
/// 或者直接滑走；这条用例钉住"迟滞带内站得住"。
#[test]
fn a_slope_within_the_slide_band_holds_the_character() {
    let mut world = World::new();
    let def = slope([0.0, 0.0, 0.0], [10.0, 10.0], 11.917_536);
    world.spawn((def,));
    let start_x = 2.0;
    let hover = 0.322_f32 * (1.0 / (50.0_f32).to_radians().cos() - 1.0);
    let character = world.spawn((
        Transform {
            position: [start_x, def.height_at(start_x, 5.0).unwrap() + hover, 5.0],
            ..Transform::default()
        },
        Velocity::default(),
        CharacterController::default(),
    ));
    let start = position(&world, character);
    run_default(&mut world, 120);
    let p = position(&world, character);
    assert!(
        (p[0] - start[0]).abs() < 2e-2,
        "迟滞带内的坡不该下滑：x {} -> {}",
        start[0],
        p[0]
    );
    assert!(
        (p[1] - start[1]).abs() < 2e-2,
        "迟滞带内的坡不该下沉：y {} -> {}",
        start[1],
        p[1]
    );
}
