//! 角色移动接入 rapier 的 `KinematicCharacterController`：本刀只让它负责
//! **水平** collide-and-slide，竖直（坡面 / 贴地 / 台阶）仍由既有 ECS 系统决定。
//!
//! 为什么竖直没有一起搬过来：rapier 0.22 + parry 0.17 的 KCC 在胶囊角色上
//! `grounded` / `snap_to_ground` / `autostep` / `is_sliding_down_slope` 四个能力
//! 全都不可用（旧端 `xgame-physics/src/character.rs` 模块文档逐个记录了这些上游
//! 缺陷），照搬 `move_shape` 的返回值当地面判定会踩同一批坑。旧端实际用法是
//! 只取 `EffectiveCharacterMovement.translation` 做碰撞滑动，地面探测 / 抬离地面 /
//! 贴地吸附 / 台阶抬升全部自实现——这里先对齐"只用碰撞滑动"这一半。
//!
//! 为什么刚体胶囊比脚底高 `rest_gap`：KCC 的 `check_and_fix_penetrations` 在 0.22
//! 是空函数（`stop_at_penetration` 也是 false），当角色与碰撞体的起始间隙 ≤ `offset`
//! 时它会放行整段期望位移——也就是会穿墙。ECS 目前把脚底精确压在地面高度（间隙 0），
//! 正好落在那个区间里；把胶囊整体抬高 `skin_width * 1.1` 就绕开了。等竖直搬进 KCC、
//! 有了"抬离地面"逻辑后，这条虚拟抬升会被取代（与旧端 `move_character` 同款）。

use rapier3d::control::{CharacterLength, KinematicCharacterController};
use rapier3d::na::{Isometry3, Point3, Vector3};
use rapier3d::prelude::*;

use xgc_base::EntityKey;
use xgc_world::{CharacterController, PhysicsBody, Transform, World};

/// 胶囊底面与脚底之间保留的间隙系数：旧端 `REST_GAP_FACTOR`，`rest_gap = skin * 1.1`。
pub const REST_GAP_FACTOR: f32 = 1.1;

/// `skin_width` 的消毒区间与缺省值（旧端 `CharacterController::sanitized` 同口径）。
const SKIN_WIDTH_RANGE: (f32, f32) = (1.0e-3, 0.5);
const SKIN_WIDTH_DEFAULT: f32 = 0.02;
/// 胶囊半径 / 高度的消毒区间与缺省值（同上）。
const RADIUS_RANGE: (f32, f32) = (0.05, 2.0);
const RADIUS_DEFAULT: f32 = 0.3;
const HEIGHT_MAX: f32 = 5.0;
/// 坡限缺省值与上限（旧端 `sanitized` 把越界值夹进 `0..=89`，NaN 落缺省）。
const SLOPE_LIMIT_DEFAULT: f32 = 45.0;
const SLOPE_LIMIT_MAX: f32 = 89.0;

fn sanitize(value: f32, default: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        default
    }
}

/// 角色半径（消毒后）：胶囊与水平扫掠都用它。
pub fn radius(def: &CharacterController) -> f32 {
    sanitize(def.radius, RADIUS_DEFAULT, RADIUS_RANGE.0, RADIUS_RANGE.1)
}

/// 角色总高（消毒后）：保证至少容得下两个半球。
pub fn height(def: &CharacterController) -> f32 {
    let floor = 2.0 * radius(def) + 1.0e-3;
    sanitize(def.height, 1.8_f32.max(floor), floor, HEIGHT_MAX)
}

/// 胶囊轴线半长：`(height - 2r) / 2`，负值夹 0（半径大于半高时退化成球）。
pub fn half_height(def: &CharacterController) -> f32 {
    ((height(def) - 2.0 * radius(def)) * 0.5).max(0.0)
}

/// 实体原点是**脚底**，刚体原点是胶囊中心，相差 `half_height + radius`。
pub fn center_height(def: &CharacterController) -> f32 {
    half_height(def) + radius(def)
}

/// 角色皮肤厚度（消毒后）：既是 KCC 的 `offset`，也是 `rest_gap` 的基准。
pub fn skin_width(def: &CharacterController) -> f32 {
    sanitize(
        def.skin_width,
        SKIN_WIDTH_DEFAULT,
        SKIN_WIDTH_RANGE.0,
        SKIN_WIDTH_RANGE.1,
    )
}

/// 胶囊底面与脚底的间隙：`skin_width * REST_GAP_FACTOR`。
pub fn rest_gap(def: &CharacterController) -> f32 {
    skin_width(def) * REST_GAP_FACTOR
}

/// 实体是否由 KCC 托管：挂了 `physics.character`、没有 `physics.body`、且有 `Transform`。
///
/// `physics.body` 是显式的动力学声明：两者同时挂时由刚体表全权托管 —— KCC 不建胶囊，
/// 角色 ECS 系统（`CharacterSystem` / `CharacterStepSystem`）也不碰它，否则两套移动逻辑
/// 会抢同一个 `Transform`（新调度里 rapier 先跑，ECS 的写落在刚体回写之后、下一帧又被
/// 覆盖，脚下就是每帧抖一次）。`VelocitySystem` 也按 `physics.character` 整份跳过
/// 角色（水平归 KCC、竖直归 `CharacterSystem`），这类组合同样一个分量都不被它积分。
pub(crate) fn owned_by_kcc(world: &World, key: EntityKey) -> bool {
    world.get::<CharacterController>(key).is_some()
        && world.get::<PhysicsBody>(key).is_none()
        && world.get::<Transform>(key).is_some()
}

/// 与 `move_shape` 使用的角色形状同参数（球心线段 + 半径）。
pub fn character_capsule(def: &CharacterController) -> Capsule {
    let half = half_height(def);
    Capsule::new(
        Point3::new(0.0, -half, 0.0),
        Point3::new(0.0, half, 0.0),
        radius(def),
    )
}

/// 可攀爬坡度上限（度）：旧端 `max_slope_climb_angle` 与 `min_slope_slide_angle` 的基准。
pub fn slope_limit(def: &CharacterController) -> f32 {
    sanitize(def.slope_limit, SLOPE_LIMIT_DEFAULT, 0.0, SLOPE_LIMIT_MAX)
}

/// 旧端 `build_rapier_controller` 的参数投影。
///
/// `autostep` / `snap_to_ground` 显式关掉：两者都依赖上面说的坏掉的 grounded 判定，
/// 打开只会让行为随 rapier 版本漂移。台阶与贴地由 ECS（本刀）/ 后续自实现接管。
pub fn build_character_controller(def: &CharacterController) -> KinematicCharacterController {
    let limit = slope_limit(def);
    let skin = skin_width(def);
    KinematicCharacterController {
        up: Vector::y_axis(),
        offset: CharacterLength::Absolute(skin),
        slide: true,
        autostep: None,
        max_slope_climb_angle: limit.to_radians(),
        // 与旧端同式：比可爬角再宽 10° 才判定"往下滑"，防临界坡上爬 / 下滑横跳。
        min_slope_slide_angle: (limit + 10.0).min(SLOPE_LIMIT_MAX).to_radians(),
        snap_to_ground: None,
        normal_nudge_factor: 1.0e-4,
    }
}

/// 一次水平碰撞滑动的结果。
pub(crate) struct HorizontalMove {
    /// 实际发生的位移（撞墙 / 沿墙滑之后）。`move_shape` 即使期望位移的 `y` 为 0
    /// 也可能给出竖直切向分量（rapier 的坡面分支会 `unconstrained_slide_part`），
    /// 所以这里不承诺 `y == 0`；本刀的水平路径只消费 `x` / `z`。
    pub translation: Vector3<f32>,
}

/// 调 `move_shape` 做一次水平 collide-and-slide。
///
/// 过滤器与旧端同义：跳过传感器、跳过角色自己的刚体；XGC 还没有碰撞分组，
/// 等分层落地后这里必须补上"自身层组"（旧端 `QueryFilter.groups`）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn move_horizontal(
    bodies: &RigidBodySet,
    colliders: &ColliderSet,
    query: &QueryPipeline,
    controller: &KinematicCharacterController,
    capsule: &Capsule,
    position: &Isometry3<f32>,
    desired: Vector3<f32>,
    own_body: RigidBodyHandle,
    dt: f32,
) -> HorizontalMove {
    let filter = QueryFilter {
        flags: QueryFilterFlags::EXCLUDE_SENSORS,
        exclude_rigid_body: Some(own_body),
        ..Default::default()
    };
    let movement = controller.move_shape(
        dt,
        bodies,
        colliders,
        query,
        capsule,
        position,
        desired,
        filter,
        |_collision| {},
    );
    HorizontalMove {
        translation: movement.translation,
    }
}
