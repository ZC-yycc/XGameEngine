//! 角色移动接入 rapier 的 `KinematicCharacterController`：水平 collide-and-slide、
//! 竖直重力、地面探测 / 抬离地面 / 贴地吸附 / 台阶抬升都在这一层。
//!
//! 为什么不直接用 KCC 的 `grounded` / `snap_to_ground` / `autostep`：rapier 0.22 +
//! parry 0.17 在胶囊角色上这四个能力全都不可用（旧端 `xgame-physics/src/character.rs`
//! 模块文档逐个记录了上游缺陷：胶囊接触流形被注释掉、`check_and_fix_penetrations` 是
//! 空函数、autostep 对胶囊不生效…）。旧端的做法是只取 `EffectiveCharacterMovement
//! .translation` 做碰撞滑动，其余全部自实现——这里逐条对齐那个口径。
//!
//! 为什么刚体胶囊比脚底高 `rest_gap`：KCC 的 `check_and_fix_penetrations` 在 0.22
//! 是空函数（`stop_at_penetration` 也是 false），当角色与碰撞体的起始间隙 ≤ `offset`
//! 时它会放行整段期望位移——也就是会穿墙。把胶囊整体抬高 `skin_width * 1.1`，
//! 静止时间隙恰好等于 `rest_gap`，既绕开了那个区间，也让"贴地吸附"有个稳定的目标值。
//!
//! `y = 0` 在 XGC 里是隐式地面（旧引擎项目总会摆一块地板，XGC 没有）：探地时把它当
//! 一张无限大的解析平面参与比较，否则竖直搬进 KCC 之后角色会一路掉下去。

use rapier3d::control::{CharacterLength, KinematicCharacterController};
use rapier3d::na::{Isometry3, Point3, Translation3, Vector3};
use rapier3d::prelude::*;

use xgc_base::EntityKey;
use xgc_world::{CharacterController, PhysicsBody, Transform, World};

/// 胶囊底面与脚底之间保留的间隙系数：旧端 `REST_GAP_FACTOR`，`rest_gap = skin * 1.1`。
pub const REST_GAP_FACTOR: f32 = 1.1;

/// 角色重力（缺省调度里只有这一处在给纯角色积分竖直）。
///
/// 与 `rapier_world::GRAVITY`（刚体表的重力）和 `GravitySystem` 的缺省值同为 `-9.81`：
/// 三处目前各自独立，场景级重力一旦变成可配置项要一起改，否则角色会与刚体分家。
pub const GRAVITY_Y: f32 = -9.81;

/// 判"站在地面上"的法线阈值：`normal.y` 低于它就算侧壁接触，不是地面。
pub const GROUND_NORMAL_MIN_DOT: f32 = 1.0e-3;

/// 着地判定的间隙容差系数：`prediction = rest_gap * 1.2`（旧端同式）。
pub const GROUND_PREDICTION_FACTOR: f32 = 1.2;

/// 台阶试探的水平余量：越过台阶立面要再往前 `radius + 2cm` 才落得下去。
pub const STEP_PROBE_MARGIN: f32 = 0.02;

/// 台阶试探向下落的最大距离余量：落得比这更远说明前面是平地、不是台阶。
pub const STEP_DROP_MARGIN: f32 = 0.05;

/// 浮点容差：三段试探里判"真的抬上去了 / 真的被挡住"。
pub const STEP_EPSILON: f32 = 1.0e-3;

/// 角色半径（消毒后）：胶囊与水平扫掠都用它。
///
/// 消毒规则只在 `CharacterController::sanitized` 一处（旧端同款）。这里不再抄一份
/// —— schema 缺省值、KCC 参数、台阶判据一旦各写一份，漂移后就会互相打架。
pub fn radius(def: &CharacterController) -> f32 {
    def.sanitized().radius
}

/// 胶囊轴线半长：`(height - 2r) / 2`，负值夹 0（半径大于半高时退化成球）。
pub fn half_height(def: &CharacterController) -> f32 {
    def.sanitized().half_height()
}

/// 实体原点是**脚底**，刚体原点是胶囊中心，相差 `half_height + radius`。
pub fn center_height(def: &CharacterController) -> f32 {
    def.sanitized().center_height()
}

/// 角色皮肤厚度（消毒后）：既是 KCC 的 `offset`，也是 `rest_gap` 的基准。
pub fn skin_width(def: &CharacterController) -> f32 {
    def.sanitized().skin_width
}

/// 胶囊底面与脚底的间隙：`skin_width * REST_GAP_FACTOR`。
pub fn rest_gap(def: &CharacterController) -> f32 {
    skin_width(def) * REST_GAP_FACTOR
}

/// 实体是否由 KCC 托管：挂了 `physics.character`、没有 `physics.body`、且有 `Transform`。
///
/// `physics.body` 是显式的动力学声明：两者同时挂时由刚体表全权托管 —— KCC 不建胶囊，
/// `RapierPhysicsSystem` 的角色阶段跳过它，`VelocitySystem` 也按 `physics.character`
/// 整份跳过（角色三轴位移一帧只能有一处写者）。否则刚体回写与角色阶段会抢同一个
/// `Transform`，下一帧又被覆盖，脚下就是每帧抖一次。
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
    def.sanitized().slope_limit
}

/// 陡坡下滑的迟滞带（度）：超过 `slope_limit + SLIDE_BAND_DEGREES` 的坡才是"站不住"的。
///
/// 旧端 `build_rapier_controller` 里 `min_slope_slide_angle = (slope_limit + 10).min(89)`：
/// `slope_limit` 以内可以爬，`slope_limit..+10` 是迟滞带（爬不上去但也不主动下滑，
/// 免得在临界角上每帧抖一次），超过 +10 才沿坡面滑下去。
pub const SLIDE_BAND_DEGREES: f32 = 10.0;

/// 自动下滑的临界坡角（度）：`(slope_limit + 10).min(89)`，与旧端 `min_slope_slide_angle` 同式。
///
/// 上限 89° 与 `slope_limit` 相同：90° 的余弦在 f32 是 ~-6e-8，`normal.y < cos(90°)`
/// 会把竖直面也算成"可滑坡"，分不清墙与坡。
pub fn slip_limit(def: &CharacterController) -> f32 {
    (slope_limit(def) + SLIDE_BAND_DEGREES).min(xgc_world::SLOPE_LIMIT_MAX)
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
        // 走 `slip_limit` 而不是再抄一遍式子：迟滞带的定义只留一处。
        min_slope_slide_angle: slip_limit(def).to_radians(),
        snap_to_ground: None,
        normal_nudge_factor: 1.0e-4,
    }
}

/// 一次向下探地的结果。
pub(crate) struct GroundHit {
    /// 地面法线（解析地面恒为 +Y）。
    pub normal: Vector3<f32>,
    /// 胶囊底面到地面的间隙：`rest_gap` 表示正好停在静息高度。
    pub gap: f32,
}

/// 从胶囊中心向下探地，返回法线与间隙。
///
/// 真实碰撞体与解析地面（`y = 0`）一起比较，取更近的那个：静止时两者都不会被穿透，
/// 所以"取更近"等价于"取当前真正站在上面的那个面"。
///
/// 返回 `Option` 只为将来给地面也建碰撞体时留形态：现在解析平面恒是候选，永远不会 `None`，
/// 调用方的 `is_some_and` / `is_none_or` 都是空分支。另注意解析平面**可能赢过**位于 `y < 0`
/// 的碰撞体（它更近），此时角色站在 `y = 0` 而不是那块几何上 —— 刻意如此（`y = 0` 是 XGC
/// 的隐式地面），等地面碰撞体补上再统一。
#[allow(clippy::too_many_arguments)]
pub(crate) fn probe_ground(
    bodies: &RigidBodySet,
    colliders: &ColliderSet,
    query: &QueryPipeline,
    capsule: &Capsule,
    center_height: f32,
    position: &Isometry3<f32>,
    filter: QueryFilter<'_>,
    extra: f32,
) -> Option<GroundHit> {
    let origin = Point3::from(position.translation.vector);
    let ray = Ray::new(origin, Vector3::new(0.0, -1.0, 0.0));
    // 与旧端同式：射线要够长，能探到"再往下 `extra` 还有没有面"。
    let max_toi = (center_height + extra).max(1e-3) * 2.0;
    let mut hit = None;
    if let Some((_, intersection)) =
        query.cast_ray_and_get_normal(bodies, colliders, &ray, max_toi, true, filter)
    {
        // `toi == 0` 表示射线起点已经在某个碰撞体里（或贴面）：既可能是角色真的陷进去了，
        // 也可能是别的刚体压在身上，旧端同样把它当作"这一帧探不到地面"。
        if intersection.time_of_impact > 0.0 {
            // 间隙按**胶囊下球心到表面的法向距离减半径**算，而不是"竖直落距乘法线余弦减中心高"：
            // 后者只有在水平面上才等于真实间隙，斜面上会差 `core · (n.y - 1)`，让站在坡上的
            // 角色误判成陷进地面、每帧被抬一点。
            let half_height = center_height - capsule.radius;
            hit = Some(GroundHit {
                normal: intersection.normal,
                gap: (intersection.time_of_impact - half_height) * intersection.normal.y
                    - capsule.radius,
            });
        }
    }
    let plane = GroundHit {
        normal: Vector3::new(0.0, 1.0, 0.0),
        gap: origin.y - center_height,
    };
    Some(match hit {
        Some(hit) if hit.gap <= plane.gap => hit,
        _ => plane,
    })
}

/// 一次 collide-and-slide 的结果。
pub(crate) struct CharacterMovement {
    /// `move_shape` 给出的实际位移（含竖直分量：碰撞滑动 / 台阶 / 坡面都会改它）。
    pub translation: Vector3<f32>,
}

/// 调 `move_shape` 做一次 collide-and-slide（水平 + 竖直都由调用方放进 `desired`）。
///
/// 与探地和台阶试探共用角色的层过滤条件。
#[allow(clippy::too_many_arguments)]
pub(crate) fn move_character(
    bodies: &RigidBodySet,
    colliders: &ColliderSet,
    query: &QueryPipeline,
    controller: &KinematicCharacterController,
    capsule: &Capsule,
    position: &Isometry3<f32>,
    desired: Vector3<f32>,
    filter: QueryFilter<'_>,
    dt: f32,
    wall_normals: &mut Vec<Vector3<f32>>,
) -> CharacterMovement {
    let movement = controller.move_shape(
        dt,
        bodies,
        colliders,
        query,
        capsule,
        position,
        desired,
        filter,
        |collision| wall_normals.push(*collision.hit.normal1),
    );
    CharacterMovement {
        translation: movement.translation,
    }
}

/// 自实现的三段台阶试探（与旧端 `try_step_up` 同序）：先抬、再前进、后落下。
///
/// 为什么不用 KCC 的 `autostep`：0.22 的 autostep 对胶囊不生效（上游缺陷，旧端模块
/// 文档有记录），只能自己用三次 `move_shape` 拼出来。任何一段没走满就判失败 ——
/// 否则会把角色"抬到半空再掉下来"。
#[allow(clippy::too_many_arguments)]
pub(crate) fn try_step_up(
    bodies: &RigidBodySet,
    colliders: &ColliderSet,
    query: &QueryPipeline,
    controller: &KinematicCharacterController,
    capsule: &Capsule,
    position: &Isometry3<f32>,
    direction: Vector3<f32>,
    horizontal: f32,
    step_height: f32,
    radius: f32,
    rest_gap: f32,
    filter: QueryFilter<'_>,
    dt: f32,
) -> Option<Isometry3<f32>> {
    let advance = direction * (horizontal + radius + STEP_PROBE_MARGIN);
    let lift = controller
        .move_shape(
            dt,
            bodies,
            colliders,
            query,
            capsule,
            position,
            Vector3::new(0.0, step_height, 0.0),
            filter,
            |_| {},
        )
        .translation;
    if lift.y < step_height - STEP_EPSILON {
        return None;
    }
    let lifted = shifted(position, lift);
    let forward = controller
        .move_shape(
            dt,
            bodies,
            colliders,
            query,
            capsule,
            &lifted,
            advance,
            filter,
            |_| {},
        )
        .translation;
    if forward.dot(&direction) < advance.norm() - STEP_EPSILON {
        return None;
    }
    let advanced = shifted(&lifted, forward);
    let down = controller
        .move_shape(
            dt,
            bodies,
            colliders,
            query,
            capsule,
            &advanced,
            Vector3::new(0.0, -(step_height + STEP_DROP_MARGIN), 0.0),
            filter,
            |_| {},
        )
        .translation;
    // 落得比"台阶高度 - 静息间隙"还深，说明前面根本没有台阶，是平地。
    if down.y <= -(step_height - rest_gap - STEP_EPSILON) {
        return None;
    }
    Some(shifted(&advanced, down))
}

/// 平移一个位姿（旋转不动：角色只支持直立）。
fn shifted(position: &Isometry3<f32>, translation: Vector3<f32>) -> Isometry3<f32> {
    Isometry3::from_parts(
        Translation3::from(position.translation.vector + translation),
        position.rotation,
    )
}
