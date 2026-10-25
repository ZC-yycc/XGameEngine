//! 角色控制器 —— 把 [`xgame_core::CharacterController`] 的配置翻译成 rapier 的
//! [`KinematicCharacterController`]，并提供胶囊形状构造。
//!
//! # 为什么用 rapier 内置的而不是自己扫掠
//!
//! "胶囊沿墙滑行、自动上台阶、坡度限制、贴地吸附、陡坡滑落"这一整套行为，
//! 手写的话每一步都是坑（滑行时的抖动、台阶边缘的吸附失败、斜坡上的
//! 穿透与抖动……）。rapier 0.22 的 `KinematicCharacterController` 已经把这些
//! 都实现了，且经过上游多年验证 —— 直接封装它是"参考开源实现"最省事、
//! 最不容易出幻觉的一条路。
//!
//! # 上游缺陷与我们的绕过（重要，改动前必读）
//!
//! **rapier 0.22 + parry 0.17.6 下，胶囊角色站在盒子/三角网/高度场上永远
//! `grounded = false`。** 链路：
//!
//! 1. rapier 的 `detect_grounded_status_and_apply_friction` 用
//!    `DefaultQueryDispatcher::contact_manifolds(角色形状, 碰撞体形状)` 找接触点；
//! 2. parry 0.17.6 的 dispatcher 把 `(Capsule, Cuboid)` 这一组合
//!    **注释掉了**（`contact_manifold_cuboid_capsule_shapes` 未启用），
//!    落到通配分支后 Capsule 没有 polygonal feature map，返回
//!    `Err(Unsupported)`；
//! 3. rapier 用 `let _ =` 吞掉错误 → manifold 为空 → 判定失败。
//!
//! 连带伤害：`snap_to_ground` 以"起始着地"为前提（`grounded_at_starting_pos`），
//! 同样整体失效；移动平台摩擦（kinematic friction）也依赖同一 manifold。
//!
//! **为什么不去升级**：parry3d 0.17.x 最后一个发布版就是 0.17.6（无修复版），
//! 0.18+ 需要 rapier ≥ 0.23，是本项目承担不起的破坏性升级。
//!
//! ## 缺陷二：`check_and_fix_penetrations()` 是空函数
//!
//! rapier 0.22 里整段实现被 `/* */` 注释掉了。胶囊一旦与地面有丝毫重叠，
//! `move_shape` 内部的 `cast_shape(stop_at_penetration: false)` 对"继续深入"
//! 的扫掠返回 `None`，于是**整段期望位移被当成"无阻挡"放行**。实测从一个
//! 2.6mm 的微小重叠起步，两帧内就退化成"每帧沉 0.83m 并高速穿墙"
//! （60 帧后 y = -41.6）。所以：
//! - 陷地时先用射线量出负间隙，把向下的期望抬到"刚好退回皮肤层"（自救）；
//! - `snap` 的贴合目标是 `skin_width` 而**不是 0**：贴到零间隙正好落进上面
//!   那个陷阱，是这条链式崩溃的实际起点（先证红过：`snap` 贴 0 → 三帧后
//!   每帧沉 0.83m）。
//!
//! ## 缺陷二点五：静止在 offset 上 = 站在悬崖边
//!
//! `offset`（皮肤宽度）是扫掠的 `target_distance`。角色的静止间隙如果**正好**
//! 等于 offset，下一帧任何移动都会让 `cast_shape` 判成"已经贴着"，返回 `None`
//! → 全放行。实测角色每帧沉 1.4cm 再被顶回来，或者干脆走穿台阶。所以静止
//! 间隙必须是 `skin_width × 1.1`（见 [`rest_gap`]），且着地判定窗口
//! （[`ground_prediction`]）要比它再宽一点，否则判定会踩在浮点边界上。
//!
//! ## 缺陷四：rapier 的 autostep 对胶囊角色不可用
//!
//! 两条实测结论：
//! 1. `handle_stairs` 只在**第一次扫掠撞到墙**时尝试。走路时只要带一点向下
//!    期望，第一次扫掠撞的永远是地面，剩余位移被地面吃掉，autostep 不触发；
//! 2. 即便触发，`min_width`（默认 0.25 × 半径 = 7.5cm）不足以把胶囊的
//!    **底面接触点**推上台阶，随后的向下探测打到的还是下一层地面，算出的
//!    台阶高度恒为 0。
//!
//! 所以 rapier 的 autostep 被**关掉**（[`build_rapier_controller`] 里设为
//! `None`），台阶改由 [`crate::world::PhysicsWorld::move_character`] 自己实现：
//! 抬到台阶高度 → 水平前进（多推一个半径，让底面中心越过台阶边缘）→ 向下
//! 贴回。三次试探全部复用 `move_shape`，与常规移动共用同一套皮肤/坡面语义。
//!
//! ## 缺陷三：60° 薄板上胶囊扫掠直接返回 `None`
//!
//! 实测同一块板：0° 的 `cast_down` 在 toi 0.01 命中、30° 在 0.104 命中，
//! **60° 时 `cast_down`/`cast_x` 全是 `None`**（parry 的 GJK 扫掠在这类
//! 接近竖直的薄板上退化；射线探测却一切正常）。后果是：纯竖直输入在陡坡上
//! 既扫不到坡面、也就永远拿不到 `is_sliding_down_slope`，角色会被自救逻辑
//! 顶在原地锯齿抖动而**不滑落**。所以陡坡滑落由我们自己实现：着地且法线
//! 超过 `slope_limit` 时，把向下的期望**投影到坡面**（法向分量清零），
//! 剩下的切向天然带水平分量，角色就顺坡下滑，且间隙保持不变。
//!
//! **绕过方案**（都在 [`crate::world::PhysicsWorld::move_character`]）：
//! - `grounded` 改为**自己的向下射线**判定，语义逐条对齐 rapier：
//!   法线点乘 `up >= 1e-3`（≈89.94°，即 rapier 的 `is_grounded_at_contact_manifold`
//!   阈值）且脚底间隙 <= `predict_ground`（`skin_width * 1.1`）；
//! - 射线从**胶囊中心**出发、间隙按 `落距 × n.y − (half_height + radius)` 算
//!   法向距离：斜坡上竖直落距长于真实间隙，直接比阈值会把"稳站坡上"判成悬空；
//!   而脚底球心一旦陷地就在地表以下，向下只会打到地板底面；
//! - `snap_to_ground` 自实现：起始着地 + **不是在向上动** + 落点在 snap 距离
//!   内，贴合到 [`rest_gap`]（皮肤的静悬距）；
//! - 自动上台阶自实现（见"缺陷四"）；
//! - 陡坡下滑自实现（见"缺陷三"）；
//! - 已知缺失：**移动平台不再带动角色**（上游该特性走同一条坏掉的 manifold）。
//!   需要时可在运行时层补：着地时把平台速度 `× dt` 加进期望位移。
//!
//! # 分层
//!
//! - 本文件只做**配置翻译 + 形状构造**，不碰 [`crate::PhysicsWorld`] 的状态；
//! - 真正的 `move_shape` 调用与位置写回在 [`crate::world::PhysicsWorld::move_character`]
//!   （它才能同时借到 `rigid_body_set` / `collider_set` / `query_pipeline`）。

use glam::Vec3;
use rapier3d::control::{CharacterLength, KinematicCharacterController};
use rapier3d::parry::shape::Capsule;
use xgame_core::CharacterController;

/// 一次角色移动的结果快照。
///
/// 拷贝而非引用：调用方（运行时）要把它并进自己的状态表，生命周期必须解耦。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterMoveResult {
    /// 碰撞修正后的**实际位移**（米）—— 不等于输入位移。
    ///
    /// 脚本层靠它反推"真实速度"：`effective_translation / dt` 就是投影到
    /// 可行方向上的速度（撞墙后水平分量归零、上坡时被压短，都体现在这里）。
    /// 贴地吸附（snap）产生的下移也计入其中，与 rapier 的报告口径一致。
    pub effective_translation: Vec3,
    /// 移动结束后角色是否着地。
    ///
    /// **由我们自己的向下射线判定，不是 rapier 的 `grounded`** —— 后者在
    /// parry 0.17.6 上对胶囊形状整体失效（见模块文档"上游缺陷与我们的绕过"）。
    /// 判定口径与 rapier 对齐：法线·up >= [`GROUND_NORMAL_MIN_DOT`] 且脚底
    /// 间隙 <= [`ground_prediction`]。
    ///
    /// 注意这是**位移之后**判定的：起跳那一帧如果输入位移还带着向下的分量，
    /// 仍可能为 `true`。运行时层处理这个时序（见 `xgame-runtime` 的
    /// `character_simple_move`：落地钳制用 `vy.min(0)` 而不是置零，就是为了
    /// 让起跳当帧的速度不被吃掉）。
    pub grounded: bool,
    /// 是否因为坡度超过 `slope_limit` 而正在自动下滑。
    pub sliding_down_slope: bool,
    /// 地面法线（世界坐标，已归一化）。
    ///
    /// `move_shape` 只给 `grounded` 布尔不给法线，这里用从脚底向下的射线补。
    /// 探测距离宽容到 snap 距离：下落贴着地面滑行时也能拿到法线（做坡面
    /// 朝向对齐用）。腾空时归还 `Vec3::Y`（而不是零向量 —— 零向量归一化
    /// 会出 NaN，脚本拿到会很难看）。
    pub ground_normal: Vec3,
    /// 本帧撞到的碰撞体数量。
    ///
    /// 纯调试/统计用途：脚本想知道"我这一下是撞到了东西还是空跑了"时，
    /// 不用再自己去射线扫一遍。
    pub collisions: usize,
    /// 本帧扫掠实际碰到的刚体集合（固定容量，零分配）。
    ///
    /// `collisions` 是次数（同一次移动里可能撞同一个物体两次），这份是
    /// **去重后的刚体集合** —— 运行时拿它和上一帧做差分，才能说出
    /// "刚撞上谁 / 刚离开谁"。角色的运动学胶囊永远停在 skin 距离外，
    /// 物理引擎不会给它发接触事件，所以这条路是角色碰撞事件的唯一来源。
    pub contacts: xgame_core::CharacterContacts,
}

/// 着地判定用的最小法线点乘（≈89.94°）。
///
/// 与 rapier `is_grounded_at_contact_manifold` 的阈值逐字相同（上游用
/// `1.0e-3`）。比 90° 低一丁点是刻意的：f32 里 90° 的余弦约 -6e-8，
/// 边界值会把垂直墙判成地板。
pub const GROUND_NORMAL_MIN_DOT: f32 = 1.0e-3;

/// 角色静止时脚底与地面的间隙（米）。
///
/// **必须严格大于 `skin_width`**：rapier 的扫掠用
/// `target_distance = offset` + `stop_at_penetration: false`，起始时间隙一旦
/// ≤ offset，扫掠就返回 `None` 而把整段位移放行 —— 自动上台阶、贴墙滑行
/// 全部失效，只剩"沉下去再被顶回来"的锯齿。留 10% 余量（默认 2mm）让扫掠
/// 每帧都还有一点点可以走的空间。
pub fn rest_gap(def: &CharacterController) -> f32 {
    def.sanitized().skin_width * 1.1
}

/// 着地判定的最大间隙（米）—— 比静止间隙再宽 20%。
///
/// rapier 的 `predict_ground` 是 `offset * 1.1`；我们的静止间隙已经是
/// `offset * 1.1`，判定窗口必须再宽一点，否则"站在静止间隙上"会踩在
/// 浮点边界上（`0.02200 <= 0.02200` 靠运气）。
pub fn ground_prediction(def: &CharacterController) -> f32 {
    rest_gap(def) * 1.2
}

/// 把引擎的配置翻译成 rapier 的角色控制器配置。
///
/// # 关键决策（都有"为什么"）
///
/// - **`offset` 用绝对米而不是 rapier 默认的 `Relative(0.01)`**：相对值按角色总高度算，换一具身高不同的角色皮肤宽度就跟着变，排查穿透问题时根本对不上；绝对皮肤宽度在编辑器里是一个可理解的数字。
/// - **`autostep` 恒为 `None`**：rapier 的实现对胶囊角色不可用（见模块文档"缺陷四"），台阶由 `PhysicsWorld::move_character` 自己实现，`step_offset` 只作为那次试探的最大高度。
/// - **`min_slope_slide_angle = slope_limit + 10°`**：两者相等时站在临界坡上会"这帧爬、下帧滑"反复横跳；留一条缓冲带，临界坡属于"能站稳"。89° 处截断：再往上是法线点乘变负、把墙判成地板。
/// - **`snap_to_ground` 跟随配置而不是 rapier 默认的 `Relative(0.2)`**：理由同 `offset`，要的是编辑器里可预期的米数。
pub fn build_rapier_controller(def: &CharacterController) -> KinematicCharacterController {
    let c = def.sanitized();
    let climb = c.slope_limit.to_radians();
    let slide = (c.slope_limit + 10.0).min(89.0).to_radians();
    KinematicCharacterController {
        up: rapier3d::math::Vector::y_axis(),
        offset: CharacterLength::Absolute(c.skin_width),
        slide: true,
        // **刻意关掉 rapier 的 autostep**，台阶由我们自己在
        // `PhysicsWorld::move_character` 里实现（"抬升 → 前进 → 贴回"）。
        //
        // 上游实现对胶囊角色不可用，两条实测结论：
        // 1. `handle_stairs` 只在**第一次扫掠撞到墙**时尝试。角色走路时只要
        //    带着一点向下的期望，第一次扫掠撞的永远是地面，剩余位移被地面
        //    吃掉，自动上台阶干脆不触发；
        // 2. 即便触发，`min_width`（默认 0.25 × 半径 = 7.5cm）不足以把胶囊的
        //    **底面接触点**推上台阶，随后的向下探测打到的还是下一层地面，
        //    算出的台阶高度恒为 0。
        // 留着它只会和我们的实现抢同一段位移，收益为零。
        autostep: None,
        max_slope_climb_angle: climb,
        min_slope_slide_angle: slide,
        snap_to_ground: Some(CharacterLength::Absolute(c.snap_to_ground)),
        normal_nudge_factor: 1.0e-4,
    }
}

/// 角色控制器的胶囊形状（与刚体上挂的碰撞体**必须同款**）。
///
/// `move_shape` 用这个形状做扫掠，所以它和实际 collider 哪怕差一根头发，
/// 表现就是"看起来贴着墙却一直渗进去"。两边都从同一个 `def` 推导 ——
/// collider 侧走 `Collider::capsule(def.half_height(), def.radius)`（其内部
/// 即 `SharedShape::capsule` = `Arc<Capsule::new>`，与本函数参数逐字相同），
/// 从根上消除这个不一致的可能。
///
/// 返回**具体类型**而不是 `SharedShape`：`move_shape` 要 `&dyn Shape`，
/// 而 `SharedShape` 是枚举包装、不实现 `Shape`。
pub fn character_capsule(def: &CharacterController) -> Capsule {
    let c = def.sanitized();
    Capsule::new(
        rapier3d::na::Point3::new(0.0, -c.half_height(), 0.0),
        rapier3d::na::Point3::new(0.0, c.half_height(), 0.0),
        c.radius,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{PhysicsConfig, PhysicsWorld};
    use crate::{Collider, RigidBody};
    use glam::{Quat, Vec3};
    use rapier3d::parry::shape::Shape;
    use xgame_math::Transform;

    const DT: f32 = 1.0 / 60.0;

    fn test_world() -> PhysicsWorld {
        PhysicsWorld::new(PhysicsConfig::default())
    }

    /// 静态盒（用于地面/墙/台阶/坡道）。
    fn add_box(w: &mut PhysicsWorld, center: Vec3, half: Vec3, rotation: Quat) -> u64 {
        let body = w.add_body(RigidBody::static_at(Transform {
            position: center,
            rotation,
            ..Transform::IDENTITY
        }));
        w.add_collider(body, Collider::cuboid(half)).unwrap();
        body
    }

    /// 角色刚体：KinematicPosition + 同款胶囊 collider。
    fn add_character(w: &mut PhysicsWorld, pos: Vec3, def: &CharacterController) -> u64 {
        let body = w.add_body(RigidBody::kinematic_position_at(Transform {
            position: pos,
            ..Transform::IDENTITY
        }));
        w.add_collider(body, Collider::capsule(def.half_height(), def.radius))
            .unwrap();
        body
    }

    /// 跑 `frames` 帧，每帧 `move_character(velocity * DT)`。
    ///
    /// `collisions` 汇总整段帧数，不是最后一帧的值：撞墙后角色可能沿墙滑走或
    /// 翻过墙顶，最后一帧可能已经没有接触了，但"这一路撞过"必须能被断言。
    fn drive(
        w: &mut PhysicsWorld,
        body: u64,
        def: &CharacterController,
        velocity: Vec3,
        frames: usize,
    ) -> CharacterMoveResult {
        w.sync_queries();
        let mut last = CharacterMoveResult {
            effective_translation: Vec3::ZERO,
            grounded: false,
            sliding_down_slope: false,
            ground_normal: Vec3::Y,
            collisions: 0,
            contacts: xgame_core::CharacterContacts::new(),
        };
        let mut total_collisions = 0;
        for _ in 0..frames {
            last = w
                .move_character(body, def, velocity * DT, DT)
                .expect("角色刚体应存在");
            total_collisions += last.collisions;
        }
        last.collisions = total_collisions;
        last
    }

    /// 角色刚体原点站在"顶面为 `floor_y` 的地面"上时的高度。
    ///
    /// 胶囊中心在刚体原点，脚底 = 原点 − (half_height + radius)，
    /// 所以站在 y=0 的地面上原点必须在 0.9，不是 0.3。
    fn standing_y(def: &CharacterController, floor_y: f32) -> f32 {
        floor_y + def.radius + def.half_height()
    }

    fn pos_of(w: &PhysicsWorld, body: u64) -> Vec3 {
        w.body_transform(body).unwrap().position
    }

    /// 一块绕 Z 轴倾斜的板（坡道）。`deg > 0` 时 +X 端抬高（朝 +X 为上坡）。
    fn add_slope(w: &mut PhysicsWorld, deg: f32) {
        let rot = Quat::from_rotation_z(deg.to_radians());
        add_box(w, Vec3::ZERO, Vec3::new(3.0, 0.1, 3.0), rot);
    }

    /// 板中段（局部 (0, 0.1, 0)）的世界坐标。
    fn slope_surface_point(deg: f32) -> Vec3 {
        let rot = Quat::from_rotation_z(deg.to_radians());
        rot * Vec3::new(0.0, 0.1, 0.0)
    }

    /// 坡面法线（朝上偏 -X）：绕 Z 转 deg 后 (0,1,0) 的像。
    fn slope_normal(deg: f32) -> Vec3 {
        Quat::from_rotation_z(deg.to_radians()) * Vec3::Y
    }

    /// 站在坡面上时胶囊原点该在的位置：从坡面点沿**法线**抬高，不是沿竖直方向。
    ///
    /// 沿竖直抬高会让胶囊扎进坡面里（30° 时圆心到坡面的法向距离只剩
    /// `0.91 × cos30 = 0.79`，减去 0.9 后是负间隙），探出来的"间隙"全是穿透量，
    /// 斜坡相关行为就全被污染了。
    fn standing_on_slope(def: &CharacterController, deg: f32, gap: f32) -> Vec3 {
        slope_surface_point(deg) + slope_normal(deg) * (def.radius + def.half_height() + gap)
    }

    #[test]
    fn controller_translates_config_field_by_field() {
        let def = CharacterController {
            radius: 0.3,
            height: 1.8,
            step_offset: 0.35,
            slope_limit: 45.0,
            skin_width: 0.02,
            snap_to_ground: 0.3,
        };
        let rc = build_rapier_controller(&def);
        assert!((rc.max_slope_climb_angle - 45f32.to_radians()).abs() < 1e-6);
        // 滑落角必须比爬升角大（缓冲带），否则临界坡上会反复横跳。
        assert!(rc.min_slope_slide_angle > rc.max_slope_climb_angle);
        // autostep 刻意关掉：rapier 的实现对胶囊角色不可用，台阶由
        // `move_character` 自己实现（见 `build_rapier_controller` 的注释）。
        assert!(
            rc.autostep.is_none(),
            "rapier 的 autostep 必须关掉，否则会和自研的台阶逻辑抢位移"
        );
        assert!(rc.slide);
    }

    /// 台阶高度配 0 = 明确要求"不许自动上台阶"：`move_character` 里的
    /// 台阶试探必须完全跳过（`step_offset > 1e-4` 是唯一的开关）。
    #[test]
    fn zero_step_offset_disables_autostep_entirely() {
        let def = CharacterController {
            step_offset: 0.0,
            ..Default::default()
        };
        let mut w = test_world();
        add_box(
            &mut w,
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(10.0, 0.5, 10.0),
            Quat::IDENTITY,
        );
        add_box(
            &mut w,
            Vec3::new(1.5, 0.1, 0.0),
            Vec3::new(0.5, 0.1, 1.0),
            Quat::IDENTITY,
        );
        let body = add_character(
            &mut w,
            Vec3::new(0.0, standing_y(&def, rest_gap(&def)), 0.0),
            &def,
        );
        drive(&mut w, body, &def, Vec3::new(1.5, 0.0, 0.0), 60);
        let p = pos_of(&w, body);
        assert!(
            p.x < 0.95 && p.y < standing_y(&def, 0.05),
            "step_offset=0 时不该上台阶（应停在阶前且保持地面高度），实际 {p:?}"
        );
    }

    /// 角色移动必须遵守**自己那一层的碰撞矩阵**。
    ///
    /// 这条同时钉住两件事：矩阵关掉的一对，角色能穿过去（不是"撞上了再把
    /// 位移退回去"）；而矩阵没动的默认情况下角色照常被挡住 —— 否则这个
    /// 测试在"过滤根本没生效"的实现下也会绿。
    #[test]
    fn character_respects_its_own_layer() {
        let def = CharacterController::default();
        let layer_geom = 1u32 << 1;
        let layer_char = 1u32 << 2;

        let mut w = test_world();
        let floor = add_box(
            &mut w,
            Vec3::new(0.0, -0.1, 0.0),
            Vec3::new(10.0, 0.1, 10.0),
            Quat::IDENTITY,
        );
        let wall = add_box(
            &mut w,
            Vec3::new(0.0, 0.9, -2.0),
            Vec3::new(0.5, 0.9, 0.1),
            Quat::IDENTITY,
        );
        let body = add_character(
            &mut w,
            Vec3::new(0.0, standing_y(&def, rest_gap(&def)), 0.0),
            &def,
        );

        // 默认全通：挡住。
        let blocked = drive(&mut w, body, &def, Vec3::new(0.0, 0.0, -3.0), 40);
        assert!(blocked.collisions > 0, "默认矩阵下角色应当撞到墙");
        assert!(
            pos_of(&w, body).z > -1.9,
            "默认矩阵下角色不该穿墙，实际 {:?}",
            pos_of(&w, body)
        );

        // 把地板留在 0 层（全通），墙 → 1 层，角色 → 2 层，并关掉 1×2。
        w.set_body_groups(floor, 1 << 0, u32::MAX);
        w.set_body_groups(wall, layer_geom, !layer_char);
        w.set_body_groups(body, layer_char, !layer_geom);

        let through = drive(&mut w, body, &def, Vec3::new(0.0, 0.0, -3.0), 40);
        assert_eq!(through.collisions, 0, "关掉 1×2 之后角色不该再撞到墙");
        assert!(
            pos_of(&w, body).z < -2.5,
            "关掉 1×2 之后角色应当穿过墙，实际 {:?}",
            pos_of(&w, body)
        );
    }

    /// 极端坡度不能产生 >90° 的角度（那会把墙判成地板）。
    #[test]
    fn extreme_slope_limit_stays_below_90_degrees() {
        let def = CharacterController {
            slope_limit: 89.0,
            ..Default::default()
        };
        let rc = build_rapier_controller(&def);
        assert!(rc.max_slope_climb_angle < std::f32::consts::FRAC_PI_2);
        assert!(rc.min_slope_slide_angle < std::f32::consts::FRAC_PI_2);
    }

    #[test]
    fn capsule_matches_collider_shape_construction() {
        let def = CharacterController::default();
        let shape = character_capsule(&def);
        // 同一个 def 走 collider 路径（`Collider::capsule(def.half_height(), def.radius)`）
        // 必须得到同一个形状—— 这里用 AABB 比对：高 = 2·half_height + 2·radius。
        let aabb = shape.compute_aabb(&rapier3d::math::Isometry::identity());
        let h = aabb.maxs.y - aabb.mins.y;
        let expected = def.height;
        assert!(
            (h - expected).abs() < 1e-4,
            "胶囊总高应等于 height（{expected}），实际 {h}"
        );
        let w = aabb.maxs.x - aabb.mins.x;
        assert!(
            (w - 2.0 * def.radius).abs() < 1e-4,
            "胶囊直径应等于 2·radius"
        );
    }

    // -----------------------------------------------------------------------
    // 行为测试（通过 PhysicsWorld::move_character 驱动）
    //
    // 所有期望值都先用诊断测试跑过真实数据再定稿，没有一条是纸上谈兵。
    // 站位约定：`standing_y(def, gap)` = 顶面 gap 之上的刚体原点高度
    //（胶囊中心在刚体原点，脚底 = 原点 − (half_height + radius)）。
    // -----------------------------------------------------------------------

    /// 站在地面上：着地判定稳定，零速度时不被无故吸走。
    ///
    /// 这条同时是**上游缺陷的回归测试**：parry 0.17.6 下 rapier 自带的
    /// grounded 对"胶囊 vs 盒子"整体失效（见模块文档），我们改用自己的
    /// 射线判定后它才为真。
    #[test]
    fn standing_on_ground_stays_grounded() {
        let mut w = test_world();
        add_box(
            &mut w,
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(10.0, 0.5, 10.0),
            Quat::IDENTITY,
        );
        let def = CharacterController::default();
        let rest = rest_gap(&def);
        let ground_y = standing_y(&def, rest);
        // 正好站在静止间隙上：这一帧不该有任何位移（不动、不沉、不吸附）。
        let body = add_character(&mut w, Vec3::new(0.0, ground_y, 0.0), &def);

        let r = drive(&mut w, body, &def, Vec3::ZERO, 30);
        assert!(r.grounded, "站在地面上应当着地");
        assert!(
            (pos_of(&w, body).y - ground_y).abs() < 1e-4,
            "零速度时角色应纹丝不动（实测 y={}）",
            pos_of(&w, body).y
        );
    }

    /// snap 窗口：起始着地、本帧净位移向下、落点在 snap 距离内 → 一帧内贴回
    /// 皮肤的静悬距（gap → skin_width）。
    ///
    /// 真实触发场景是"走下矮台阶"：脚下地面塌了 0.18m，但落差小于
    /// `snap_to_ground`（0.3），角色应该被吸下去继续走，而不是腾空。
    #[test]
    fn snap_pulls_character_down_a_low_step() {
        let mut w = test_world();
        // 上层台面 y=0（x ≤ 0），下层地面 y=-0.18（x ≥ 0）。
        add_box(
            &mut w,
            Vec3::new(-3.0, -0.5, 0.0),
            Vec3::new(3.0, 0.5, 6.0),
            Quat::IDENTITY,
        );
        add_box(
            &mut w,
            Vec3::new(3.0, -0.68, 0.0),
            Vec3::new(3.0, 0.5, 6.0),
            Quat::IDENTITY,
        );
        let def = CharacterController::default();
        let body = add_character(&mut w, Vec3::new(-0.6, standing_y(&def, 0.0), 0.0), &def);

        // 先原地站几帧确认站得住，再朝 +X 走下台阶。
        let start = drive(&mut w, body, &def, Vec3::ZERO, 5);
        assert!(start.grounded, "应站在上层台面上");
        let r = drive(&mut w, body, &def, Vec3::new(1.2, -0.5, 0.0), 70);
        let p = pos_of(&w, body);
        assert!(
            r.grounded,
            "落下 0.18m 后仍应着地（snap 应把角色吸到下层地面）"
        );
        assert!(
            (p.y - (def.radius + def.half_height() - 0.18)).abs() < 0.03,
            "应站在下层地面上（期望 y≈{}），实际 y={}（0.18m 的落差没被 snap 吃掉？）",
            def.radius + def.half_height() - 0.18,
            p.y
        );
    }

    /// 悬空超过预测距离：不着地，法线兜底为世界上。
    #[test]
    fn hovering_beyond_prediction_is_not_grounded() {
        let mut w = test_world();
        add_box(
            &mut w,
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(10.0, 0.5, 10.0),
            Quat::IDENTITY,
        );
        let def = CharacterController::default();
        let body = add_character(&mut w, Vec3::new(0.0, standing_y(&def, 0.05), 0.0), &def);

        let r = drive(&mut w, body, &def, Vec3::ZERO, 5);
        assert!(
            !r.grounded,
            "5cm 悬空（> {} cm 预测）不该算着地",
            ground_prediction(&def) * 100.0
        );
        assert_eq!(r.ground_normal, Vec3::Y, "探不到地面时法线应兜底为世界上");
    }

    /// 高墙挡停：朝墙跑，x 停在墙面 - radius - skin 附近，y 不变。
    #[test]
    fn tall_wall_blocks_horizontal_movement() {
        let mut w = test_world();
        add_box(
            &mut w,
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(10.0, 0.5, 10.0),
            Quat::IDENTITY,
        );
        // 墙面在 x = 2.0。
        add_box(
            &mut w,
            Vec3::new(2.5, 1.5, 0.0),
            Vec3::new(0.5, 1.5, 3.0),
            Quat::IDENTITY,
        );
        let def = CharacterController::default();
        let body = add_character(&mut w, Vec3::new(0.0, standing_y(&def, 0.0), 0.0), &def);

        let r = drive(&mut w, body, &def, Vec3::new(5.0, 0.0, 0.0), 60);
        let p = pos_of(&w, body);
        assert!(r.collisions > 0, "应该撞到墙");
        assert!(
            p.x < 1.71 && p.x > 1.4,
            "应停在墙前（2.0 - 0.3 - skin ≈ 1.68），实际 x={}",
            p.x
        );
        assert!(
            (p.y - standing_y(&def, 0.0)).abs() < 0.05,
            "平地跑不该被抬高，实际 y={}",
            p.y
        );
    }

    /// 沿墙滑行：斜着撞墙时，切向（Y）分量必须保留。
    ///
    /// 这是 `slide: true` 的核心语义 —— 旧式"整个位移被吃掉"的角色控制器
    /// 会让斜撞墙的角色同时失去上升分量，看起来像被墙"吸住"。
    #[test]
    fn slide_along_wall_preserves_tangential_motion() {
        let mut w = test_world();
        add_box(
            &mut w,
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(10.0, 0.5, 10.0),
            Quat::IDENTITY,
        );
        add_box(
            &mut w,
            Vec3::new(2.5, 1.5, 0.0),
            Vec3::new(0.5, 1.5, 3.0),
            Quat::IDENTITY,
        );
        let def = CharacterController::default();
        let body = add_character(&mut w, Vec3::new(0.0, standing_y(&def, 0.0), 0.0), &def);

        // 同时朝 +X（撞墙）与 +Y（上升）。墙顶 y=3.0，角色从 0.9 起跳，
        // 40 帧 ≈ 2.0m 的上升仍在墙高范围内；跑满 60 帧会翻过墙顶、最后几帧
        // 已无接触（碰撞计数为 0），断言就失去意义了。
        let r = drive(&mut w, body, &def, Vec3::new(5.0, 3.0, 0.0), 40);
        let p = pos_of(&w, body);
        assert!(r.collisions > 0, "应该撞到墙");
        assert!(p.x < 1.71, "x 应被墙挡停，实际 x={}（说明穿墙了）", p.x);
        assert!(
            p.y > standing_y(&def, 0.0) + 1.5,
            "切向上升分量应被保留（3 m/s × 0.67 s ≈ 2 m），实际 y={}",
            p.y
        );
    }

    /// 自动上台阶：台阶高 0.2（< step_offset 0.35），水平走向它应能踏上去。
    #[test]
    fn autosteps_over_low_step() {
        let mut w = test_world();
        add_box(
            &mut w,
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(10.0, 0.5, 10.0),
            Quat::IDENTITY,
        );
        // 台阶：顶面 y = 0.2，从 x = 1.0 延伸到 x = 2.0。
        add_box(
            &mut w,
            Vec3::new(1.5, 0.1, 0.0),
            Vec3::new(0.5, 0.1, 1.0),
            Quat::IDENTITY,
        );
        let def = CharacterController::default();
        let body = add_character(&mut w, Vec3::new(0.0, standing_y(&def, 0.0), 0.0), &def);

        // 50 帧 ≈ 1.25 m：够走到台阶并踏上去，又不会走过头（台阶只到 x=2.0）。
        let r = drive(&mut w, body, &def, Vec3::new(1.5, 0.0, 0.0), 50);
        let p = pos_of(&w, body);
        assert!(r.grounded, "踏上台阶后应着地");
        assert!(
            p.x > 1.05,
            "应已踏上台阶（x>1.05），实际 x={}（被 0.2m 的矮阶挡死了？）",
            p.x
        );
        assert!(
            (p.y - standing_y(&def, 0.2)).abs() < 0.1,
            "应站在台阶顶面（y≈{}），实际 y={}",
            standing_y(&def, 0.2),
            p.y
        );
    }

    /// 高台阶挡停：台阶高 1.0（> step_offset），水平移动应被完全挡在阶前。
    #[test]
    fn tall_step_blocks_completely() {
        let mut w = test_world();
        add_box(
            &mut w,
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(10.0, 0.5, 10.0),
            Quat::IDENTITY,
        );
        // 台阶：顶面 y = 1.0，x 从 1.0 起。
        add_box(
            &mut w,
            Vec3::new(1.5, 0.5, 0.0),
            Vec3::new(0.5, 0.5, 1.0),
            Quat::IDENTITY,
        );
        let def = CharacterController::default();
        let body = add_character(&mut w, Vec3::new(0.0, standing_y(&def, 0.0), 0.0), &def);

        let r = drive(&mut w, body, &def, Vec3::new(1.5, 0.0, 0.0), 120);
        let p = pos_of(&w, body);
        assert!(r.collisions > 0, "应该撞到台阶");
        assert!(
            p.x < 0.78 && p.x > 0.5,
            "应停在 1m 高台阶前（阶面 x=1.0 − radius − skin ≈ 0.68），实际 x={}",
            p.x
        );
        assert!(
            (p.y - standing_y(&def, 0.0)).abs() < 0.05,
            "不该被抬上高台阶，实际 y={}",
            p.y
        );
    }

    /// 30° 坡（< 45° limit）：朝坡上推，应能爬上去。
    ///
    /// 带 -0.5 m/s 的下落分量模拟重力：爬坡时靠每帧的向下净位移触发
    /// snap，把角色黏在坡面上（不会"爬两步飘一步"）。
    #[test]
    fn climbs_slope_within_limit() {
        let mut w = test_world();
        add_slope(&mut w, 30.0);
        let def = CharacterController::default();
        let body = add_character(&mut w, standing_on_slope(&def, 30.0, 0.021), &def);

        // 先原地站几帧，确认站位正确（着地、贴着坡面）。
        let start = drive(&mut w, body, &def, Vec3::ZERO, 10);
        assert!(start.grounded, "应站在坡上");
        let y0 = pos_of(&w, body).y;

        let r = drive(&mut w, body, &def, Vec3::new(2.0, -0.5, 0.0), 60);
        let p = pos_of(&w, body);
        assert!(r.grounded, "爬坡过程中应保持接触");
        assert!(p.y > y0 + 0.3, "30° 坡应爬得上去：y0={y0}，实际 y={}", p.y);
    }

    /// 60° 坡（> 45° limit）+ 纯下落：应顺着坡面滑下来。
    ///
    /// 这是"陡坡滑落"的 designed 行为。注意 rapier 0.22 的语义：
    /// **水平推力可以爬上任何角度的坡**（handle_slopes 的 else 分支保留
    /// 垂直切向），`max_slope_climb_angle` 实际只 gate 两件事——
    /// 自动上台阶、以及"被诱导的"爬升。运行时层的
    /// `character_simple_move` 在此之上补 Unity 式的陡坡禁行。
    #[test]
    fn steep_slope_slides_down_under_gravity() {
        let mut w = test_world();
        add_slope(&mut w, 60.0);
        let def = CharacterController::default();
        let body = add_character(&mut w, standing_on_slope(&def, 60.0, 0.021), &def);

        let start = drive(&mut w, body, &def, Vec3::ZERO, 10);
        assert!(start.grounded, "应站在坡上");
        let y0 = pos_of(&w, body).y;

        // 每帧下落 3.3cm（间隙只有 1cm），保证每帧的扫掠都真正压在坡面上，
        // `is_sliding_down_slope` 才不会因为"这一帧没接触"而漏报。
        let r = drive(&mut w, body, &def, Vec3::new(0.0, -2.0, 0.0), 30);
        let p = pos_of(&w, body);
        assert!(r.grounded, "滑落过程中应仍贴着坡面");
        assert!(
            r.sliding_down_slope,
            "超过滑落角的坡应标记 is_sliding_down_slope"
        );
        assert!(p.y < y0 - 0.2, "60° 坡应滑落下去：y0={y0}，实际 y={}", p.y);
    }

    /// 坡面法线：站在 30° 坡上，`ground_normal` 应≈坡面法线。
    #[test]
    fn ground_normal_matches_slope() {
        let mut w = test_world();
        add_slope(&mut w, 30.0);
        let def = CharacterController::default();
        let body = add_character(&mut w, standing_on_slope(&def, 30.0, 0.021), &def);

        let r = drive(&mut w, body, &def, Vec3::ZERO, 10);
        assert!(r.grounded);
        let expected = slope_normal(30.0);
        let dot = r.ground_normal.dot(expected);
        assert!(
            dot > 0.95,
            "地面法线应≈坡面法线 {expected:?}，实际 {:?}（dot={dot}）",
            r.ground_normal
        );
    }

    /// 起跳离地、再落地：vy>0 时 grounded 变 false，落回后 grounded 恢复。
    ///
    /// 这条同时钉住起跳当帧的时序：起跳帧位移向上，snap 只在**向下**净位移
    /// 时触发，所以起跳当帧 grounded 应当就是 false。
    #[test]
    fn jump_leaves_ground_and_lands_back() {
        let mut w = test_world();
        add_box(
            &mut w,
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(10.0, 0.5, 10.0),
            Quat::IDENTITY,
        );
        let def = CharacterController::default();
        let body = add_character(&mut w, Vec3::new(0.0, standing_y(&def, 0.0), 0.0), &def);
        drive(&mut w, body, &def, Vec3::ZERO, 5);

        // 起跳：5 m/s 向上。
        let up = drive(&mut w, body, &def, Vec3::new(0.0, 5.0, 0.0), 3);
        assert!(!up.grounded, "起跳后应离地");
        let apex_area = pos_of(&w, body).y;
        assert!(
            apex_area > standing_y(&def, 0.0) + 0.1,
            "应明显离地，实际 y={apex_area}"
        );

        // 重力下落：-9.81 m/s。
        let down = drive(&mut w, body, &def, Vec3::new(0.0, -9.81, 0.0), 90);
        assert!(down.grounded, "落回地面后应恢复 grounded");
        let p = pos_of(&w, body);
        assert!(
            (p.y - standing_y(&def, 0.0)).abs() < 0.05,
            "应落回地面高度，实际 y={}",
            p.y
        );
    }

    /// 传感器不阻挡角色（Unity 语义：trigger 只发事件、不挡移动）。
    #[test]
    fn sensor_does_not_block_character() {
        let mut w = test_world();
        let floor = w.add_body(RigidBody::static_at(Transform {
            position: Vec3::new(0.0, -0.5, 0.0),
            ..Transform::IDENTITY
        }));
        let mut sensor = Collider::cuboid(Vec3::new(10.0, 0.5, 10.0));
        sensor.is_sensor = true;
        w.add_collider(floor, sensor).unwrap();

        let def = CharacterController::default();
        let body = add_character(&mut w, Vec3::new(0.0, 1.0, 0.0), &def);

        let r = drive(&mut w, body, &def, Vec3::ZERO, 10);
        assert!(!r.grounded, "传感器不该承重（角色应悬空）");
        assert!(pos_of(&w, body).y > 0.9, "角色不该被传感器托住");
    }

    /// 高速下冲不穿地：sweep 语义保证任意速度都不会 tunneling。
    #[test]
    fn fast_downward_move_does_not_tunnel() {
        let mut w = test_world();
        add_box(
            &mut w,
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(10.0, 0.5, 10.0),
            Quat::IDENTITY,
        );
        let def = CharacterController::default();
        let body = add_character(&mut w, Vec3::new(0.0, 5.0, 0.0), &def);

        let r = drive(&mut w, body, &def, Vec3::new(0.0, -50.0, 0.0), 60);
        let p = pos_of(&w, body);
        assert!(
            p.y > standing_y(&def, 0.0) - 0.05,
            "不该穿进地面，实际 y={}",
            p.y
        );
        assert!(r.grounded, "落到底后应着地");
    }

    /// 高速横冲不穿墙：这条挡的是"角色在贴地状态下 cast_shape 返回 None、
    /// 于是整段位移被放行"的退化路径（rapier 0.22 的
    /// `check_and_fix_penetrations` 是空函数，没人兜底）。
    #[test]
    fn fast_horizontal_move_does_not_tunnel() {
        let mut w = test_world();
        add_box(
            &mut w,
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(10.0, 0.5, 10.0),
            Quat::IDENTITY,
        );
        add_box(
            &mut w,
            Vec3::new(2.5, 1.5, 0.0),
            Vec3::new(0.5, 1.5, 3.0),
            Quat::IDENTITY,
        );
        let def = CharacterController::default();
        let body = add_character(&mut w, Vec3::new(0.0, standing_y(&def, 0.0), 0.0), &def);

        // 200 m/s：每帧 3.33m，一帧就能跨过整堵墙。
        let r = drive(&mut w, body, &def, Vec3::new(200.0, 0.0, 0.0), 10);
        let p = pos_of(&w, body);
        assert!(r.collisions > 0, "应该撞到墙");
        assert!(
            p.x < 1.71 && p.x > 1.4,
            "高速横冲应被墙挡停（≈1.68），实际 x={}（穿墙了）",
            p.x
        );
    }

    /// 已经陷进地面的角色要能自己顶出来，而不是越陷越深。
    ///
    /// 对应 `check_and_fix_penetrations()` 为空函数那条上游缺陷：没有这层自救，
    /// 5cm 的重叠会在几帧内变成"每帧沉 0.8m"。
    #[test]
    fn penetrating_character_is_pushed_back_out() {
        let mut w = test_world();
        add_box(
            &mut w,
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(10.0, 0.5, 10.0),
            Quat::IDENTITY,
        );
        let def = CharacterController::default();
        // 站姿再往下 5cm = 脚底陷进地面 5cm。
        let body = add_character(&mut w, Vec3::new(0.0, standing_y(&def, -0.05), 0.0), &def);

        let r = drive(&mut w, body, &def, Vec3::new(0.0, -1.0, 0.0), 30);
        let y = pos_of(&w, body).y;
        let rest = standing_y(&def, 0.0) + rest_gap(&def);
        assert!(r.grounded, "被顶出后应着地");
        assert!(
            (y - rest).abs() < 0.01,
            "应被顶回皮肤的静悬距（期望 y≈{rest}），实际 y={y}"
        );
    }

    /// 零位移也要检测 grounded（站立不动时游戏逻辑照样要问着地状态）。
    #[test]
    fn zero_movement_still_detects_ground() {
        let mut w = test_world();
        add_box(
            &mut w,
            Vec3::new(0.0, -0.5, 0.0),
            Vec3::new(10.0, 0.5, 10.0),
            Quat::IDENTITY,
        );
        let def = CharacterController::default();
        let body = add_character(&mut w, Vec3::new(0.0, standing_y(&def, 0.0), 0.0), &def);
        w.sync_queries();
        let r = w.move_character(body, &def, Vec3::ZERO, DT).unwrap();
        assert!(r.grounded, "零位移也应报告着地状态");
    }

    /// 不存在的刚体返回 None 而不是 panic。
    #[test]
    fn move_character_on_missing_body_returns_none() {
        let mut w = test_world();
        assert!(w
            .move_character(999, &CharacterController::default(), Vec3::ONE, DT)
            .is_none());
    }
}
