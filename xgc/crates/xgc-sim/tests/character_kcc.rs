//! 角色横向移动接入 rapier `KinematicCharacterController` 的回归。
//!
//! 这些用例大多显式搭一条最小调度（`VelocitySystem` + `RapierPhysicsSystem`），不跑默认
//! 18 个系统：本刀只改"水平位移归谁"，竖直 / 坡面仍由既有系统负责，混进来会让断言
//! 分不清是哪一层的错。例外是三条默认调度用例（陡坡挤入 / 低台阶抬升 / 带刚体的角色不被 ECS 角色系统写）——它们专门钉系统**顺序**与归属。

use xgc_sim::{RapierPhysicsSystem, Schedule, VelocitySystem, FIXED_DT};
use xgc_world::{CharacterController, Command, PhysicsBody, Slope, Transform, Velocity, World};

/// 绕 Y 轴旋转 `degrees` 的四元数（`Transform.rotation` 是 `[x, y, z, w]`）。
fn yaw(degrees: f32) -> [f32; 4] {
    let half = degrees.to_radians() * 0.5;
    [0.0, half.sin(), 0.0, half.cos()]
}

fn step_times(world: &mut World, frames: usize) {
    let mut schedule = Schedule::new();
    schedule.add(VelocitySystem::default());
    schedule.add(RapierPhysicsSystem::default());
    for _ in 0..frames {
        schedule.run(world, FIXED_DT);
    }
}

/// 只给实体挂一个静态刚体（位置由实体自己的 `Transform` 决定）。
fn static_body(half: [f32; 3]) -> PhysicsBody {
    PhysicsBody {
        kind: "static".to_string(),
        shape: "cuboid".to_string(),
        half_extents: half,
        ..PhysicsBody::default()
    }
}

fn static_box(position: [f32; 3], half: [f32; 3], rotation: [f32; 4]) -> (Transform, PhysicsBody) {
    (
        Transform {
            position,
            rotation,
            ..Transform::default()
        },
        PhysicsBody {
            kind: "static".to_string(),
            shape: "cuboid".to_string(),
            half_extents: half,
            ..PhysicsBody::default()
        },
    )
}

#[test]
fn character_stops_at_a_wall_instead_of_passing_through() {
    let mut world = World::new();
    let character = world.spawn((
        Transform::default(),
        Velocity {
            linear: [6.0, 0.0, 0.0],
        },
        CharacterController::default(),
    ));
    let (wall_transform, wall_body) = static_box([2.0, 1.0, 0.0], [0.05, 1.0, 2.0], yaw(0.0));
    world.spawn((wall_transform, wall_body));
    step_times(&mut world, 60);
    let transform = world.get::<Transform>(character).unwrap();
    // 墙在 x=2，墙面向内 0.05 → 角色半径 0.3 的胶囊中心停在 1.95 之前。
    assert!(
        transform.position[0] < 1.96,
        "角色穿墙了：x={}",
        transform.position[0]
    );
    assert!(
        transform.position[0] > 1.5,
        "角色根本没走到墙边：x={}",
        transform.position[0]
    );
    // 撞墙后水平速度回写成"实际位移 / dt"，与旧端口径一致。
    assert_eq!(world.get::<Velocity>(character).unwrap().linear[0], 0.0);
}

/// 斜放（20°）的墙必须让角色**沿墙面滑开**，不能像旧 AABB 推出那样顶死在最近轴上。
///
/// 旧实现（`CharacterBlockSystem`）只看 `PhysicsBody.half_extents` 的轴对齐盒，斜墙的
/// AABB 面比真实表面更靠外，角色一碰就被按最近轴推回并清掉该轴速度，z 永远不会变。
#[test]
fn character_slides_along_a_rotated_wall() {
    let mut world = World::new();
    let character = world.spawn((
        Transform::default(),
        Velocity {
            linear: [3.0, 0.0, 0.0],
        },
        CharacterController::default(),
    ));
    let (wall_transform, wall_body) = static_box([1.5, 1.0, 0.0], [0.5, 1.0, 0.5], yaw(20.0));
    world.spawn((wall_transform, wall_body));
    step_times(&mut world, 90);
    let transform = world.get::<Transform>(character).unwrap();
    assert!(
        transform.position[2].abs() > 0.3,
        "角色没有沿斜墙滑开：z={}",
        transform.position[2]
    );
}

#[test]
fn velocity_system_only_integrates_the_character_vertically() {
    let mut world = World::new();
    let character = world.spawn((
        Transform::default(),
        Velocity {
            linear: [2.0, 3.0, 4.0],
        },
        CharacterController::default(),
    ));
    let mut schedule = Schedule::new();
    schedule.add(VelocitySystem::default());
    schedule.run(&mut world, 0.5);
    let transform = world.get::<Transform>(character).unwrap();
    assert_eq!(transform.position[0], 0.0);
    assert_eq!(transform.position[2], 0.0);
    assert!(
        (transform.position[1] - 1.5).abs() < 1e-6,
        "y={}",
        transform.position[1]
    );
}

/// 同一实体既有 `physics.body` 又有 `physics.character` 时只给一套托管：刚体优先。
#[test]
fn a_character_with_a_rigid_body_is_owned_by_the_body() {
    let mut world = World::new();
    let character = world.spawn((
        Transform::default(),
        Velocity {
            linear: [3.0, 0.0, 0.0],
        },
        CharacterController::default(),
        PhysicsBody {
            kind: "static".to_string(),
            shape: "cuboid".to_string(),
            half_extents: [0.5, 0.5, 0.5],
            ..PhysicsBody::default()
        },
    ));
    step_times(&mut world, 30);
    let transform = world.get::<Transform>(character).unwrap();
    assert_eq!(transform.position[0], 0.0);
    assert_eq!(transform.position[2], 0.0);
}

/// 同时挂 `physics.body` 与 `physics.character` 的实体由刚体表全权托管：
/// 角色 ECS 系统（`CharacterSystem` / `CharacterStepSystem`）不许再写它的 `Transform`。
///
/// 新调度里 rapier 跑在这两个系统**之前**：如果它们还写这个实体，写会落在刚体回写之后、
/// 下一帧又被覆盖 —— 脚下每帧抖一次（旧调度 rapier 最后跑，这些写当帧就被覆盖，问题只在
/// 调序后显形，必须用默认调度跑才抓得到）。
#[test]
fn a_character_with_a_rigid_body_is_left_alone_by_the_character_systems() {
    let mut world = World::new();
    // ① 站在 0.2 m 低箱上的"刚体 + 角色"实体：台阶系统会把它抬到箱顶。
    world.spawn(static_box([0.0, -0.3, 0.0], [0.5, 0.5, 0.5], yaw(0.0)));
    let stepped = world.spawn((
        Transform::default(),
        Velocity::default(),
        CharacterController::default(),
        static_body([0.5, 0.5, 0.5]),
    ));
    // ② 陷在 60° 陡坡里的"刚体 + 角色"实体：坡面系统会把它推到坡外。
    world.spawn((Slope {
        origin: [10.0, 0.0, 0.0],
        size: [10.0, 10.0],
        rise: 17.32,
    },));
    let pushed = world.spawn((
        Transform {
            position: [12.0, 0.0, 0.0],
            ..Transform::default()
        },
        Velocity::default(),
        CharacterController::default(),
        static_body([0.5, 0.5, 0.5]),
    ));

    let mut schedule = Schedule::default_schedule(1);
    for _ in 0..30 {
        schedule.run(&mut world, FIXED_DT);
    }
    let stepped = world.get::<Transform>(stepped).unwrap();
    assert!(
        stepped.position[1].abs() < 1e-5,
        "带刚体的角色不该被台阶系统抬起来：y={}",
        stepped.position[1]
    );
    let pushed = world.get::<Transform>(pushed).unwrap();
    assert!(
        (pushed.position[0] - 12.0).abs() < 1e-4,
        "带刚体的角色不该被坡面系统推走：x={}",
        pushed.position[0]
    );
}

#[test]
fn a_character_at_rest_does_not_drift() {
    let mut world = World::new();
    let character = world.spawn((
        Transform::default(),
        Velocity::default(),
        CharacterController::default(),
    ));
    let (wall_transform, wall_body) = static_box([1.0, 1.0, 0.0], [0.5, 1.0, 0.5], yaw(0.0));
    world.spawn((wall_transform, wall_body));
    step_times(&mut world, 30);
    let transform = world.get::<Transform>(character).unwrap();
    assert_eq!(transform.position, [0.0, 0.0, 0.0]);
}

#[test]
fn character_motion_is_deterministic() {
    fn run() -> Vec<u32> {
        let mut world = World::new();
        let character = world.spawn((
            Transform::default(),
            Velocity {
                linear: [3.0, 0.0, 0.0],
            },
            CharacterController::default(),
        ));
        let (wall_transform, wall_body) = static_box([1.5, 1.0, 0.0], [0.5, 1.0, 0.5], yaw(45.0));
        world.spawn((wall_transform, wall_body));
        step_times(&mut world, 60);
        world
            .get::<Transform>(character)
            .unwrap()
            .position
            .iter()
            .map(|value| value.to_bits())
            .collect()
    }
    assert_eq!(run(), run());
}

/// 球体静态碰撞体也是 KCC 的阻挡面（旧 `CharacterBlockSystem` 有自己的球体分支；
/// 本用例钉的是删掉那套实现之后，球面仍然挡人）。
#[test]
fn character_is_blocked_by_a_ball_collider() {
    let mut world = World::new();
    let character = world.spawn((
        Transform::default(),
        Velocity {
            linear: [6.0, 0.0, 0.0],
        },
        CharacterController::default(),
    ));
    world.spawn((
        Transform {
            position: [2.0, 1.0, 0.0],
            ..Transform::default()
        },
        PhysicsBody {
            kind: "static".to_string(),
            shape: "ball".to_string(),
            radius: 0.5,
            ..PhysicsBody::default()
        },
    ));
    step_times(&mut world, 60);
    let transform = world.get::<Transform>(character).unwrap();
    // 球心 x=2 / 半径 0.5 / 角色半径 0.3 → 胶囊中心停在 1.2 之前。
    assert!(
        transform.position[0] < 1.25,
        "角色穿过球体了：x={}",
        transform.position[0]
    );
    assert!(
        transform.position[0] > 0.8,
        "角色根本没走到球边：x={}",
        transform.position[0]
    );
    assert_eq!(world.get::<Velocity>(character).unwrap().linear[0], 0.0);
}

/// 默认调度下角色也不能一点点挤进陡坡：`CharacterSystem` 必须排在水平位移**之后**。
///
/// 坡面没有 rapier 碰撞体，KCC 只会按 `v·dt` 一直往前推；如果陡坡推出跑在 KCC 之前，
/// 每帧终点就是 `front + v·dt`，下一帧再被拉回 `front` —— 脚本若每帧重设速度（旧
/// `set_velocity` 就是这么写的），脚下就是 30 Hz 的 5 cm 来回跳，且 x 会周期性越界。
/// 这条用例每帧重新写一次速度来模拟那种脚本，并逐帧断言脚底没有越过立面。
#[test]
fn a_character_does_not_creep_into_a_steep_slope_under_the_default_schedule() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [-2.0, 0.0, 0.0],
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
    let mut schedule = Schedule::default_schedule(1);
    for frame in 0..60 {
        // 模拟脚本每帧写一次速度（`legacy::set_velocity` 的行为）。
        if let Some(velocity) = world.get_mut::<Velocity>(character) {
            velocity.linear[0] = 3.0;
        }
        schedule.run(&mut world, FIXED_DT);
        let x = world.get::<Transform>(character).unwrap().position[0];
        assert!(x <= -0.299, "第 {frame} 帧脚底越过了陡坡立面：x={x}");
    }
    let transform = world.get::<Transform>(character).unwrap();
    assert!(
        (transform.position[0] + 0.3).abs() < 1e-3,
        "角色没走到陡坡立面：x={}",
        transform.position[0]
    );
    // 竖直仍由 ECS 负责：站在 y=0 的坡底，没被抬上坡面。
    assert!(transform.position[1].abs() < 1e-5);
}

/// 实体键换代（`Replace` / 组件增删 / 快照恢复）后角色必须继续被 KCC 驱动。
///
/// 注意：这条在切片**之前**的旧实现下也是绿的（那时角色根本没有胶囊，也就没有签名问题）；
/// 它的红灯来自把签名改回"只看实体 ID"这一种变异，钉的是本切片第一版的 bug，不是旧行为回归。
///
/// `xgc-world` 的 `replace_entity_raw` 会 `despawn + restore`，**实体 ID 不变、`EntityKey`
/// 换代**。角色托管表若只按 ID 做签名就会提前返回，`CharacterLink.key` 停在旧代际：
/// `World::get` 拒绝过期代际 → 水平位移静默冻结，旧胶囊还作为幽灵碰撞体留在 rapier 里。
#[test]
fn a_character_keeps_moving_after_its_entity_key_changes() {
    let mut world = World::new();
    let key = world.spawn((
        Transform::default(),
        Velocity {
            linear: [3.0, 0.0, 0.0],
        },
        CharacterController::default(),
    ));
    let id = world.entity(key).unwrap();
    // 全程同一个 `RapierPhysicsSystem`：换一个新的会把角色表整份重建，掩盖"签名没变"
    // 导致的过期句柄（这正是本用例要抓的东西）。
    let mut schedule = Schedule::new();
    schedule.add(VelocitySystem::default());
    schedule.add(RapierPhysicsSystem::default());
    for _ in 0..10 {
        schedule.run(&mut world, FIXED_DT);
    }
    let before = world.get::<Transform>(key).unwrap().position[0];
    assert!((before - 0.5).abs() < 1e-5, "x={before}");

    let data = world.entity_data(id).unwrap();
    let plan = world.plan(vec![Command::Replace { data }]);
    assert!(plan.is_valid(), "{:?}", plan.diagnostics);
    world.apply(&plan).unwrap();
    let replaced = world.key(id).unwrap();
    assert_ne!(replaced, key, "`Replace` 必须换代");

    for _ in 0..10 {
        schedule.run(&mut world, FIXED_DT);
    }
    let after = world.get::<Transform>(replaced).unwrap().position[0];
    assert!(
        (after - 1.0).abs() < 1e-5,
        "换代后角色水平位移被冻结：x={after}"
    );
}

/// 默认调度下角色必须能踏上 `0 < 台阶 <= snap_to_ground` 的低台阶。
///
/// `CharacterSystem` 的贴地吸附会把 `y - ground <= snap_to_ground` 的离地高度吸回地面
/// （它的 `ground` 只含坡面与 y=0，不含箱子顶面），而 0.05~0.2 m 的台阶恰好落在吸附
/// 窗口里：`CharacterStepSystem` 排在吸附**之前**的话每帧抬起又被吸回 0，默认调度下
/// 角色再也上不去（旧顺序是"吸附 → 抬台阶"，抬起的 y 留到帧末）。
/// 速度每帧重写，模拟旧脚本 `character_simple_move` 每帧写一次速度：KCC 撞到箱侧时会把
/// 水平速度回写成实际位移 / dt（撞住即 0），只写一次速度的话台阶系统的预测位移也会归零。
///
/// 注意：旧实现（切片前）下这条也是绿的 —— 旧 `CharacterBlockSystem` 对 `rise <= step` 的
/// 低矮实体直接放行，角色穿过箱子后照样被台阶系统抬到 0.2。红灯来自把两个系统的顺序
/// 换回去，钉的是本切片第一版的调序 bug。
#[test]
fn character_steps_onto_a_low_box_under_the_default_schedule() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [-1.6, 0.0, 0.0],
            ..Transform::default()
        },
        Velocity {
            linear: [2.0, 0.0, 0.0],
        },
        CharacterController::default(),
    ));
    // 顶面 0.2 = 默认 `snap_to_ground`，正是会被吸附吃掉的那一档。
    let (box_transform, box_body) = static_box([0.0, 0.1, 0.0], [1.0, 0.1, 1.0], yaw(0.0));
    world.spawn((box_transform, box_body));
    let mut schedule = Schedule::default_schedule(1);
    for _ in 0..45 {
        if let Some(velocity) = world.get_mut::<Velocity>(character) {
            velocity.linear = [2.0, 0.0, 0.0];
        }
        schedule.run(&mut world, FIXED_DT);
    }
    let transform = *world.get::<Transform>(character).unwrap();
    assert!(
        transform.position[0] > -1.0,
        "角色没走上台阶：x={}",
        transform.position[0]
    );
    assert!(
        (transform.position[1] - 0.2).abs() < 1e-3,
        "默认调度下角色必须站在 0.2 m 台阶上：y={}",
        transform.position[1]
    );
}

/// 小半径胶囊也要能上台阶：KCC 会停在距立面 `offset = skin_width` 处
/// （`check_and_fix_penetrations` 是空实现，必须留间隙），台阶判据要按这份间隙外扩。
/// 不外扩时 0.3 半径能靠"胶囊肚皮比台阶顶宽"的巧合多吃进一点，0.05 半径就永远够不到
/// 台阶面：撞住后 KCC 把速度回写成 0（净位移被 nudge 抵消），预测位移也归零 → 一次都抬不起来。
#[test]
fn a_small_capsule_still_steps_onto_a_low_box() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [-1.6, 0.0, 0.0],
            ..Transform::default()
        },
        Velocity {
            linear: [1.0, 0.0, 0.0],
        },
        CharacterController {
            radius: 0.05,
            ..CharacterController::default()
        },
    ));
    let (box_transform, box_body) = static_box([0.0, 0.1, 0.0], [1.0, 0.1, 1.0], yaw(0.0));
    world.spawn((box_transform, box_body));
    let mut schedule = Schedule::default_schedule(1);
    for _ in 0..90 {
        if let Some(velocity) = world.get_mut::<Velocity>(character) {
            velocity.linear = [1.0, 0.0, 0.0];
        }
        schedule.run(&mut world, FIXED_DT);
    }
    let transform = *world.get::<Transform>(character).unwrap();
    assert!(
        transform.position[0] > -1.0,
        "小半径角色没走上台阶：x={}",
        transform.position[0]
    );
    assert!(
        (transform.position[1] - 0.2).abs() < 1e-3,
        "小半径角色必须站在 0.2 m 台阶上：y={}",
        transform.position[1]
    );
}
