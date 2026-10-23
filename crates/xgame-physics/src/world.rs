//! 物理世界——拥有 rapier3d 内部状态、驱动模拟步骤，为游戏代码提供简洁接口。

use std::collections::HashMap;
use std::sync::Mutex;

use glam::{Quat, Vec3};
use xgame_core::{CharacterController, Color, Transform};
use xgame_math::glam;

use rapier3d::dynamics::{
    CCDSolver, ImpulseJointSet, IntegrationParameters, IslandManager,
    MultibodyJointSet, RigidBodyBuilder, RigidBodyHandle, RigidBodySet,
};
use rapier3d::geometry::{
    BroadPhaseMultiSap as DefaultBroadPhase, ColliderBuilder, ColliderHandle,
    ColliderSet, Group, InteractionGroups, NarrowPhase,
};
use rapier3d::control::{EffectiveCharacterMovement, KinematicCharacterController};
use rapier3d::math::Isometry;
use rapier3d::parry::shape::Capsule;
use rapier3d::prelude::{ActiveEvents, Ray};
use rapier3d::pipeline::{PhysicsPipeline, QueryFilter, QueryFilterFlags, QueryPipeline};

use crate::character::{
    build_rapier_controller, character_capsule, ground_prediction, CharacterMoveResult,
    GROUND_NORMAL_MIN_DOT,
};
use crate::collider::{Collider, ColliderShape};
use crate::debug_draw::DebugLine;
use crate::rigid_body::{RigidBody, RigidBodyType};

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

/// 全局物理设置。
#[derive(Debug, Clone)]
pub struct PhysicsConfig {
    /// 重力向量（默认：-Y，9.81 m/s²）。
    pub gravity: Vec3,
    /// 每次 `step()` 调用的子步数。越高越稳定但越慢。
    ///
    /// `step(dt)` 会把 `dt` **均分**成这么多子步，所以总推进时长始终是 `dt`，
    /// 子步数只影响精度、不影响速度。
    pub num_sub_steps: u32,
    /// 每个子步的最大时间步长（秒）。
    ///
    /// `step(dt)` 只在传入的 `dt` 非法（0 / 负数 / NaN）时才退回这个值；
    /// 正常调用请以你已经传给 [`PhysicsWorld::step`] 的值为准。
    pub timestep: f32,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            gravity: Vec3::new(0.0, -9.81, 0.0),
            num_sub_steps: 1,
            timestep: 1.0 / 60.0,
        }
    }
}

// ---------------------------------------------------------------------------
// 碰撞事件（简化视图）
// ---------------------------------------------------------------------------

/// rapier3d 发出的接触事件的简化视图。
#[derive(Debug, Clone)]
pub struct CollisionEvent {
    pub body_a: u64,
    pub body_b: u64,
    pub started: bool,
}

// ---------------------------------------------------------------------------
// 射线检测
// ---------------------------------------------------------------------------

/// 射线检测的可选过滤条件。
///
/// 刻意做成一个独立的、`Copy` 的普通结构，而不是把 rapier 的 `QueryFilter`
/// 直接暴露出去：后者带生命周期与 parry 的回调闭包，泄漏到游戏 / 脚本层
/// 会把上层和物理后端的细节绑死。
#[derive(Debug, Clone, Copy, Default)]
pub struct RaycastFilter {
    /// 忽略传感器（触发器）。
    ///
    /// 角色移动、地板探测几乎总是要开这个 —— 否则一个"边缘触发区"会把
    /// 角色顶住，看起来像卡在了空气墙上。
    pub ignore_sensors: bool,
    /// 只命中动态刚体（例如"这一枪有没有打中可击落的东西"）。
    pub only_dynamic: bool,
    /// 排除某个 body（通常是自己，避免射线从自身内部出发时打到自己）。
    pub exclude_body: Option<u64>,
    /// 只命中这些层（第 N 位 = 第 N 层）；`None` = 不限层。
    ///
    /// **只看成员位，不看碰撞矩阵**（Unity `Physics.Raycast(layerMask)` 同语义）：
    /// "摄像机射线只打几何体层"不该因为几何体层和摄像机层被设成不碰撞就失效 ——
    /// 矩阵管的是**接触**，查询掩码管的是**我想看什么**，两件事。
    pub layer_mask: Option<u32>,
}

/// 一次射线检测的命中结果。
#[derive(Debug, Clone, Copy)]
pub struct PhysicsRayHit {
    /// 被命中的 body。理论上可能为 `None`（孤立碰撞体），引擎里不会出现。
    pub body_id: Option<u64>,
    /// 被命中的碰撞体。
    pub collider_id: u64,
    /// 沿射线方向的距离。
    pub distance: f32,
    /// 命中点（世界坐标）。
    pub point: Vec3,
    /// 命中面的世界法线。
    pub normal: Vec3,
    /// 命中的是不是传感器。
    pub is_sensor: bool,
}

// ---------------------------------------------------------------------------
// Internal handle record
// ---------------------------------------------------------------------------

struct BodyRecord {
    rapier_handle: RigidBodyHandle,
}

struct ColliderRecord {
    rapier_handle: ColliderHandle,
    body_id: u64,
}

// ---------------------------------------------------------------------------
// Event collector
// ---------------------------------------------------------------------------

struct RapierEventCollector {
    events: Mutex<Vec<(ColliderHandle, ColliderHandle, bool)>>,
}

impl RapierEventCollector {
    fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    fn drain(self) -> Vec<(ColliderHandle, ColliderHandle, bool)> {
        self.events.into_inner().unwrap()
    }
}

impl rapier3d::pipeline::EventHandler for RapierEventCollector {
    fn handle_collision_event(
        &self,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        event: rapier3d::geometry::CollisionEvent,
        _contact_pair: Option<&rapier3d::geometry::ContactPair>,
    ) {
        self.events
            .lock()
            .unwrap()
            .push((event.collider1(), event.collider2(), event.started()));
    }

    fn handle_contact_force_event(
        &self,
        _dt: f32,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        _contact_pair: &rapier3d::geometry::ContactPair,
        _total_impulse: f32,
    ) {
    }
}

// ---------------------------------------------------------------------------
// 物理世界
// ---------------------------------------------------------------------------

pub struct PhysicsWorld {
    rigid_body_set: RigidBodySet,
    collider_set: ColliderSet,
    integration_params: IntegrationParameters,
    physics_pipeline: PhysicsPipeline,
    island_manager: IslandManager,
    broad_phase: DefaultBroadPhase,
    narrow_phase: NarrowPhase,
    impulse_joint_set: ImpulseJointSet,
    multibody_joint_set: MultibodyJointSet,
    ccd_solver: CCDSolver,
    query_pipeline: QueryPipeline,

    pending_events: Vec<CollisionEvent>,

    config: PhysicsConfig,

    bodies: HashMap<u64, BodyRecord>,
    colliders: HashMap<u64, ColliderRecord>,
    /// `body_id → 它名下的 collider_id`。
    ///
    /// 换层（`set_body_groups`）与角色扫掠（`body_groups`）都要"由刚体找到
    /// 碰撞体"。线性扫 `colliders` 也能做，但角色每帧都要查一次，刚体一多
    /// 就变成 O(角色数 × 碰撞体数)。与 `bodies` / `colliders` 同增同减。
    body_colliders: HashMap<u64, Vec<u64>>,
    /// `rapier 刚体句柄 → 我们的 body_id` 反查表。
    ///
    /// 碰撞事件只给 collider 句柄：要先查 collider 的 parent 刚体句柄，再换成
    /// 我们的 body_id。早先这里是线性扫 `bodies`，每条碰撞事件 O(刚体数)，
    /// 事件一多就退化成 O(事件数 × 刚体数)。改成哈希表后是 O(1)。
    ///
    /// 必须与 `bodies` 同增同减：rapier 的 `RigidBodySet::remove` 会把句柄放回
    /// 空闲表供后续 `insert` 复用，漏删一条旧映射就会命中一个**已被别人占用**的
    /// 句柄，把碰撞事件记到错误的实体上。
    body_handles: HashMap<RigidBodyHandle, u64>,
    /// `rapier 碰撞体句柄 → 我们的 collider_id` 反查表。
    ///
    /// 射线只看得到 `ColliderHandle`，要交给上层就必须换成自己的 ID。
    /// 与 `body_handles` 同理：必须与 `colliders` 同增同减。
    collider_handles: HashMap<ColliderHandle, u64>,
    next_id: u64,
}

impl PhysicsWorld {
    pub fn new(config: PhysicsConfig) -> Self {
        Self {
            rigid_body_set: RigidBodySet::new(),
            collider_set: ColliderSet::new(),
            integration_params: IntegrationParameters::default(),
            physics_pipeline: PhysicsPipeline::new(),
            island_manager: IslandManager::new(),
            broad_phase: DefaultBroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            impulse_joint_set: ImpulseJointSet::new(),
            multibody_joint_set: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            query_pipeline: QueryPipeline::new(),
            pending_events: Vec::new(),
            config,
            bodies: HashMap::new(),
            colliders: HashMap::new(),
            body_colliders: HashMap::new(),
            body_handles: HashMap::new(),
            collider_handles: HashMap::new(),
            next_id: 1,
        }
    }

    // -----------------------------------------------------------------------
    // Body / collider management
    // -----------------------------------------------------------------------

    pub fn add_body(&mut self, body: RigidBody) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        let rb_type = match body.body_type {
            RigidBodyType::Static => rapier3d::dynamics::RigidBodyType::Fixed,
            RigidBodyType::Dynamic => rapier3d::dynamics::RigidBodyType::Dynamic,
            RigidBodyType::KinematicVelocity => {
                rapier3d::dynamics::RigidBodyType::KinematicVelocityBased
            }
            RigidBodyType::KinematicPosition => {
                rapier3d::dynamics::RigidBodyType::KinematicPositionBased
            }
        };

        let translation = glam_to_na_vec3(body.transform.position);
        let rotation = glam_quat_to_na_rotation(body.transform.rotation);

        let mut rb = RigidBodyBuilder::new(rb_type)
            .translation(translation)
            .linvel(glam_to_na_vec3(body.linear_velocity))
            .angvel(glam_to_na_vec3(body.angular_velocity))
            .gravity_scale(if body.gravity_enabled { body.gravity_scale } else { 0.0 })
            .linear_damping(body.linear_damping)
            .angular_damping(body.angular_damping)
            .can_sleep(body.can_sleep)
            .build();

        let mut pos = *rb.position();
        pos.rotation = rotation;
        rb.set_position(pos, true);

        if body.body_type == RigidBodyType::Dynamic && body.mass > 0.0 {
            rb.set_additional_mass(body.mass, true);
        }

        let rapier_handle = self.rigid_body_set.insert(rb);
        self.bodies.insert(id, BodyRecord { rapier_handle });
        self.body_handles.insert(rapier_handle, id);
        id
    }

    pub fn remove_body(&mut self, body_id: u64) {
        if let Some(record) = self.bodies.remove(&body_id) {
            // 反查表要同步清掉，否则 rapier 复用该句柄后会把碰撞事件记到这个
            // 已经删掉的 body 上。
            self.body_handles.remove(&record.rapier_handle);
            self.body_colliders.remove(&body_id);
            // 先收集要删的句柄，再统一清 —— 直接在 `retain` 闭包里借 `self.collider_handles`
            // 会和 `self.colliders` 的可变借用打架。
            let doomed: Vec<ColliderHandle> = self
                .colliders
                .values()
                .filter(|cr| cr.body_id == body_id)
                .map(|cr| cr.rapier_handle)
                .collect();
            for handle in doomed {
                self.collider_handles.remove(&handle);
            }
            self.colliders.retain(|_, cr| {
                if cr.body_id == body_id {
                    self.collider_set.remove(
                        cr.rapier_handle,
                        &mut self.island_manager,
                        &mut self.rigid_body_set,
                        false,
                    );
                    false
                } else {
                    true
                }
            });
            self.rigid_body_set.remove(
                record.rapier_handle,
                &mut self.island_manager,
                &mut self.collider_set,
                &mut self.impulse_joint_set,
                &mut self.multibody_joint_set,
                true,
            );
        }
    }

    /// 给刚体挂一个碰撞体，返回碰撞体 ID。
    ///
    /// `body_id` 不存在时返回 `None` 并记一条警告 —— **不 panic**。
    /// 早先这里是 `.expect("unknown body")`：一次笔误（脚本在刚体被删之后仍然
    /// 挂碰撞体、或宿主把 body_id 传串了）就会把整个进程带走。引擎里没有任何
    /// 一条路径值得为这种输入付出崩溃的代价，退化成"少一个碰撞体"就够了。
    pub fn add_collider(&mut self, body_id: u64, collider: Collider) -> Option<u64> {
        let cid = self.next_id;
        self.next_id += 1;

        let Some(body_record) = self.bodies.get(&body_id) else {
            log::warn!("add_collider: 未知的 body_id={body_id}，已忽略（不挂碰撞体）");
            return None;
        };

        let shape = collider_to_shared_shape(&collider.shape);
        let cb = ColliderBuilder::new(shape)
            .friction(collider.friction)
            .restitution(collider.restitution)
            .sensor(collider.is_sensor)
            .density(collider.density)
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .collision_groups(interaction_groups(collider.memberships, collider.filter))
            .translation(glam_to_na_vec3(collider.offset));

        let rapier_handle = self.collider_set.insert_with_parent(
            cb, body_record.rapier_handle, &mut self.rigid_body_set,
        );

        self.colliders.insert(cid, ColliderRecord { rapier_handle, body_id });
        self.collider_handles.insert(rapier_handle, cid);
        self.body_colliders.entry(body_id).or_default().push(cid);
        Some(cid)
    }

    /// 改某个刚体（它名下所有碰撞体）的碰撞组。
    ///
    /// 用于"实体换层"这种**运行中**的改动：`sync_bodies` 只在结构性变化时补建，
    /// 层变了不需要重建刚体，但必须让 rapier 立刻看到新掩码 —— 否则要等到
    /// 下一次有实体增减才会生效，表现为"脚本里 set_layer 之后要等几秒才不碰"。
    ///
    /// 返回是否真的改了（刚体不存在 / 没有碰撞体时为 `false`）。
    pub fn set_body_groups(&mut self, body_id: u64, memberships: u32, filter: u32) -> bool {
        let groups = interaction_groups(memberships, filter);
        let Some(collider_ids) = self.body_colliders.get(&body_id).cloned() else {
            return false;
        };
        let mut changed = false;
        for cid in collider_ids {
            let Some(record) = self.colliders.get(&cid) else { continue };
            if let Some(collider) = self.collider_set.get_mut(record.rapier_handle) {
                collider.set_collision_groups(groups);
                changed = true;
            }
        }
        if changed {
            // **刻意不重建 query pipeline**：碰撞组存在 collider 的 flags 里，
            // rapier 的 `QueryFilter::test` 是查询时现读的（不是烘进 QBVH 的）。
            // 每帧 set_layer 的脚本不该为此付一次全量 AABB 重建。
        }
        changed
    }

    /// 某个刚体的碰撞组（多个碰撞体时取第一个；引擎目前一刚体一碰撞体）。
    ///
    /// 只在移动角色时用：角色的扫掠必须遵守它自己那一层的矩阵，
    /// 否则"关掉的两层"对角色移动不起作用（角色照样被不该碰的东西挡住）。
    fn body_groups(&self, body_id: u64) -> InteractionGroups {
        self.body_colliders
            .get(&body_id)
            .and_then(|ids| ids.first())
            .and_then(|cid| self.colliders.get(cid))
            .and_then(|record| self.collider_set.get(record.rapier_handle))
            .map(|collider| collider.collision_groups())
            .unwrap_or_else(InteractionGroups::all)
    }

    pub fn remove_collider(&mut self, collider_id: u64) {
        if let Some(record) = self.colliders.remove(&collider_id) {
            self.collider_handles.remove(&record.rapier_handle);
            if let Some(ids) = self.body_colliders.get_mut(&record.body_id) {
                ids.retain(|id| *id != collider_id);
                if ids.is_empty() {
                    self.body_colliders.remove(&record.body_id);
                }
            }
            self.collider_set.remove(
                record.rapier_handle,
                &mut self.island_manager,
                &mut self.rigid_body_set,
                false,
            );
        }
    }

    // -----------------------------------------------------------------------
    // Simulation
    // -----------------------------------------------------------------------

    /// 推进物理模拟 `dt` 秒。
    ///
    /// # 两个曾经踩过的坑
    ///
    /// 1. **参数曾被静默忽略**：旧实现固定写 `integration_params.dt = config.timestep`，
    ///    传进来的 `dt` 直接丢掉。但 `time.rs` 的用法明明是
    ///    `physics_world.step(clock.fixed_dt())` —— 时钟才是固定步长的唯一真相。
    ///    改了时钟的步长、物理却纹丝不动，调用方还看不出自己的值被吃了。
    ///    现在以传入值为准，只在传入值非法时退回配置值。
    /// 2. **子步数曾等于加速**：每个子步都用完整的 `dt`，`num_sub_steps = 4` 就等于
    ///    一次调用推进 `4 × dt`，物理直接"快进"。而 `num_sub_steps` 的文档说
    ///    "越高越稳定"——稳定只能来自**更小**的步长，所以 dt 必须按子步数均分。
    pub fn step(&mut self, dt: f32) {
        // 非法值（0 / 负数 / NaN / inf）退回配置值：一次笔误不该让模拟发散到全 NaN。
        let dt = if dt.is_finite() && dt > 0.0 {
            dt
        } else {
            self.config.timestep
        };

        let gravity = glam_to_na_vec3(self.config.gravity);
        let event_collector = RapierEventCollector::new();

        let num_steps = self.config.num_sub_steps.max(1);
        // 总推进时长恒为 `dt`，子步数只影响精度，不影响速度。
        self.integration_params.dt = dt / num_steps as f32;

        for _ in 0..num_steps {
            self.physics_pipeline.step(
                &gravity,
                &self.integration_params,
                &mut self.island_manager,
                &mut self.broad_phase,
                &mut self.narrow_phase,
                &mut self.rigid_body_set,
                &mut self.collider_set,
                &mut self.impulse_joint_set,
                &mut self.multibody_joint_set,
                &mut self.ccd_solver,
                Some(&mut self.query_pipeline),
                &(),
                &event_collector,
            );
        }

        for (c1, c2, started) in event_collector.drain() {
            let body_a = self.resolve_collider_body(c1);
            let body_b = self.resolve_collider_body(c2);
            if let (Some(a), Some(b)) = (body_a, body_b) {
                self.pending_events.push(CollisionEvent {
                    body_a: a, body_b: b, started,
                });
            }
        }
    }

    // -----------------------------------------------------------------------
    // Character controller
    // -----------------------------------------------------------------------

    /// 用 rapier 的角色控制器移动一个刚体，返回实际位移与着地信息。
    ///
    /// # 前置条件
    ///
    /// - `body_id` 必须存在，且它上面挂的碰撞体与 [`character_capsule`] 同款
    ///   （运行时 `sync_bodies` 保证这一点）；
    /// - **调用前应已 `sync_queries()`**：角色移动发生在 `step()` 之前，用的是
    ///   上一帧留下的 `query_pipeline`。若刚体位置在别处被改过（编辑器拖动物体、
    ///   上一帧的 `sync_bodies`），broad-phase 会滞后于实际位置，`move_shape`
    ///   可能漏掉本该挡路的碰撞体。批量移动多个角色时，循环外刷新一次即可。
    ///
    /// # 为什么传感器被排除
    ///
    /// Unity 的 `CharacterController.Move` 不会被 trigger 阻挡（trigger 只发
    /// 事件）。这里保持同样语义：否则角色一走进对话触发区就走不动了。
    /// 代价是「拿传感器当隐形平台」不再可行 —— 那请用 `is_sensor: false` 的
    /// 运动学刚体。
    pub fn move_character(
        &mut self,
        body_id: u64,
        def: &CharacterController,
        desired_translation: Vec3,
        dt: f32,
    ) -> Option<CharacterMoveResult> {
        let handle = self.bodies.get(&body_id)?.rapier_handle;
        let rb = self.rigid_body_set.get(handle)?;
        let char_pos: Isometry<f32> = *rb.position();
        let c = def.sanitized();

        let controller = build_rapier_controller(def);
        let shape = character_capsule(def);
        let filter = QueryFilter {
            flags: QueryFilterFlags::EXCLUDE_SENSORS,
            // 角色移动必须遵守**它自己那一层**的碰撞矩阵：否则"关掉的两层"
            // 对角色不起作用，敌人从玩家身上径直穿过 —— 而同一对刚体之间的
            // 接触却被物理挡下了，两边行为不一致。
            groups: Some(self.body_groups(body_id)),
            exclude_collider: None,
            exclude_rigid_body: Some(handle),
            predicate: None,
        };

        // ---- 脚下探测（自实现，绕开上游 parry 缺陷，见 character.rs 模块文档）----
        // 一次探测同时服务三件事：起始着地判定、陷地自救、snap 的落点测量。
        // 探测距离取 snap + predict：既要覆盖"贴地但略微悬空"，也要覆盖
        // "脚下掉了个小台阶"（snap 窗口）。
        let prediction = ground_prediction(&c);
        let lookahead = c.snap_to_ground + prediction;
        let below = self
            .probe_ground(handle, &c, &char_pos, lookahead)
            .filter(|(normal, _)| normal.dot(Vec3::Y) >= GROUND_NORMAL_MIN_DOT);
        let gap_before = below.map(|(_, gap)| gap);
        let grounded_at_start = gap_before.is_some_and(|gap| gap <= prediction + 1e-5);

        // ---- 陡坡不承重：自己生成下滑切向 ----
        // rapier 0.22 的 `is_sliding_down_slope` 只在扫掠**命中**时才由
        // `handle_slopes` 置位。实测胶囊在 60° 薄板上做竖直扫掠时，parry 的
        // shape cast 直接返回 None（0°/30° 都正常，60° 退化），于是角色要么
        // 被陷地自救顶住原地抖、要么沉进坡里，永远不会滑。
        //
        // 这里在移动前把"向下的期望"投影到坡面：法向分量清零，剩下的切向
        // 天然带一个水平分量，角色就会顺着坡往下滑，且间隙保持不变（不会再
        // 触发上面那个退化扫掠）。
        let is_steep = below.is_some_and(|(n, _)| n.dot(Vec3::Y) < c.slope_limit_cos());
        let steep_slide = grounded_at_start && is_steep && desired_translation.y < 0.0;
        let mut desired = desired_translation;
        if steep_slide {
            if let Some((normal, _)) = below {
                let into_plane = desired.dot(normal);
                desired -= normal * into_plane;
            }
        }

        // ---- 皮肤层硬约束（陷地自救）----
        // rapier 0.22 的 `check_and_fix_penetrations()` 整段被注释成了空函数，
        // 而 `move_shape` 的扫掠是 `target_distance = offset` +
        // `stop_at_penetration: false`：**只要起始时间隙 ≤ 皮肤宽度，扫掠就
        // 直接返回 None，整段期望位移被放行**。于是"停在 skin 上"这个静止
        // 状态本身就是个陷阱 —— 下一帧往下一压就沉进去，再被顶回来，角色
        // 变成锯齿状抖动（实测每帧沉 1.4cm）。
        //
        // 所以约束定在**移动前**：间隙不足皮肤宽度时，向下的期望一律抬到
        // "刚好补回皮肤层"（gap < 0 时 desired.y 由负变正，把角色顶出来）。
        // 陡坡滑落例外：那时 desired 已经投影成坡面切向、法向分量为零，
        // 再钳一次会把下滑分量清零，角色就挂在陡坡上了。
        if !steep_slide {
            if let Some(gap) = gap_before {
                // 迟滞带：间隙落在 [skin, rest] 之间时**不干预** —— 那段是
                // rapier 自己收敛出来的正常工作区间，硬抬会把扫掠的接触几何
                // 每帧改动 2mm，自动上台阶直接失效（先证红过）。只有真正
                // 掉进 offset 以内（那之后所有扫掠都会返回 None）才补回来。
                if gap < c.skin_width {
                    let dy_min = crate::character::rest_gap(&c) - gap;
                    if desired.y < dy_min {
                        desired.y = dy_min;
                    }
                }
            }
        }

        // 撞到的碰撞体句柄先攒在栈上（固定容量，热路径零分配），
        // `move_shape` 借走了 `&self`，回调里没法查反查表。
        let mut hit_handles = [None; xgame_core::MAX_CHARACTER_CONTACTS];
        let mut collisions = 0usize;
        let movement = controller.move_shape(
            dt,
            &self.rigid_body_set,
            &self.collider_set,
            &self.query_pipeline,
            &shape,
            &char_pos,
            glam_to_na_vec3(desired),
            filter,
            |hit| {
                collisions += 1;
                if collisions <= xgame_core::MAX_CHARACTER_CONTACTS {
                    hit_handles[collisions - 1] = Some(hit.handle);
                }
            },
        );

        // 句柄 → 刚体 id（去重）。角色停在 skin 距离外、物理层不会给它发
        // 接触事件，这份集合就是运行时判定"撞上谁 / 离开谁"的唯一依据。
        let mut contacts = xgame_core::CharacterContacts::new();
        for handle in hit_handles.into_iter().flatten() {
            if let Some(body) = self.resolve_collider_body(handle) {
                contacts.push(body);
            }
        }

        let mut new_pos = char_pos;
        new_pos.translation.vector += movement.translation;

        // ---- 自动上台阶（自实现，rapier 的 autostep 对胶囊不可用）----
        // 只在"想走却几乎走不动"时试探：抬到台阶高度 → 水平前进 → 贴回地面。
        // 三次试探全部复用 `move_shape`，所以皮肤间隙、坡面、碰撞过滤与常规
        // 移动完全同一套语义。
        if grounded_at_start && c.step_offset > 1e-4 && desired.y <= 1.0e-5 {
            let flat = Vec3::new(desired.x, 0.0, desired.z);
            let want_h = flat.length();
            // 用**沿期望方向**的投影而不是位移长度：贴墙时 `handle_slopes`
            // 的推离分量是反方向的一小段，按长度算会误判成"已经走了"，
            // 自动上台阶永远等不到机会（实测在原地来回抖 1.5cm）。
            let dir = if want_h > 1e-4 { flat / want_h } else { Vec3::ZERO };
            let got_h = Vec3::new(movement.translation.x, 0.0, movement.translation.z)
                .dot(dir)
                .max(0.0);
            if want_h > 1e-4 && got_h < want_h * 0.5 {
                if let Some(stepped) = self.try_step_up(
                    &controller,
                    &shape,
                    &char_pos,
                    desired,
                    c.step_offset + crate::character::rest_gap(&c),
                    c.radius,
                    crate::character::rest_gap(&c),
                    filter,
                    dt,
                ) {
                    let step_h = Vec3::new(
                        stepped.translation.vector.x - char_pos.translation.vector.x,
                        0.0,
                        stepped.translation.vector.z - char_pos.translation.vector.z,
                    )
                    .dot(dir);
                    // 只有真的比原来走得更远才采纳：否则"抬起来又落回原地"
                    // 会变成一次白嫖的高度抖动。
                    if step_h > got_h + 1e-4 {
                        new_pos = stepped;
                    }
                }
            }
        }

        // ---- 自实现 snap_to_ground ----
        // 上游的 snap 挂在 `grounded_at_starting_pos` 上，而它对胶囊整体失效
        // （见 character.rs 模块文档）。这里按同款条件重写：
        //   起始着地 + 本帧净位移向下 + 落点在 snap 距离内 → 贴合下去。
        // 逐条对齐 rapier：`result.translation.dot(up) < -1e-5` 才 snap，
        // 所以起跳/上升帧绝不被吸附（否则跳跃会被 snap 吃掉）。
        //
        // 贴合目标是**皮肤的静悬距**，不是零间隙。这条是实测换来的：贴到 0 会
        // 正好踩进上面那个空函数的坑，下一帧扫掠返回 None，三帧内沉进地面。
        // 门槛从 rapier 的"净位移向下"放宽成"不是在向上动"：皮肤层约束会把
        // 平地/台阶边缘的竖直期望压到 0，若还要求"净位移向下"，走下矮台阶时
        // 就永远不会被吸附（角色会先腾空一帧）。跳跃（desired.y > 0）仍然排除。
        if grounded_at_start && c.snap_to_ground > 1e-4 && desired.y <= 1.0e-5 {
            if let Some((_, gap)) = self
                .probe_ground(handle, &c, &new_pos, lookahead + c.skin_width)
                .filter(|(normal, _)| normal.dot(Vec3::Y) >= GROUND_NORMAL_MIN_DOT)
            {
                let rest = crate::character::rest_gap(&c);
                if gap > rest + 1e-4 && gap <= c.snap_to_ground {
                    new_pos.translation.vector -= rapier3d::math::Vector::y() * (gap - rest);
                }
            }
        }

        // ---- 传感器（trigger）重叠 ----
        // 传感器不挡路（Unity 语义），但"走进触发区"必须能发事件。角色与传感器
        // 永远不会产生物理接触（扫掠排除了它们，它们本来也不挡），所以这里单独
        // 用一次重叠查询把传感器并进接触集合 —— 运行时那边不需要知道区别，
        // 差分出来的 enter/exit 与实体碰撞完全同款。
        let sensor_filter = QueryFilter {
            flags: QueryFilterFlags::EXCLUDE_SOLIDS,
            ..filter
        };
        self.query_pipeline.intersections_with_shape(
            &self.rigid_body_set,
            &self.collider_set,
            &new_pos,
            &shape,
            sensor_filter,
            |handle| {
                if let Some(body) = self.resolve_collider_body(handle) {
                    contacts.push(body);
                }
                true
            },
        );

        // 位置写回。只动平移、保留朝向：角色朝向是渲染/脚本层的事，
        // 物理层不该因为它撞了个墙角就悄悄转个向。
        let rb = self.rigid_body_set.get_mut(handle)?;
        rb.set_position(new_pos, true);

        // 实际位移从最终位置反算：snap 与陷地自救的修正都要如实报给调用方，
        // 否则运行时的速度积分会和真实位置悄悄脱节。
        let effective = Vec3::new(
            new_pos.translation.vector.x - char_pos.translation.vector.x,
            new_pos.translation.vector.y - char_pos.translation.vector.y,
            new_pos.translation.vector.z - char_pos.translation.vector.z,
        );

        // ---- 最终着地判定与地面法线 ----
        // 法线探测距离宽容到 snap 距离（下滑时也能拿到坡面法线）；
        // grounded 判定则严格用 predict_ground 的间隙窗口。
        let (ground_normal, grounded) = match self
            .probe_ground(handle, &c, &new_pos, lookahead)
        {
            Some((normal, gap)) => (
                normal,
                normal.dot(Vec3::Y) >= GROUND_NORMAL_MIN_DOT && gap <= prediction + 1e-5,
            ),
            None => (Vec3::Y, false),
        };

        Some(CharacterMoveResult {
            effective_translation: effective,
            grounded,
            sliding_down_slope: movement.is_sliding_down_slope || steep_slide,
            ground_normal,
            collisions,
            contacts,
        })
    }

    /// 直接跑一次 `move_shape` 试探（不写回任何状态）。
    fn probe_move(
        &self,
        controller: &KinematicCharacterController,
        shape: &Capsule,
        pos: &Isometry<f32>,
        translation: Vec3,
        filter: QueryFilter,
        dt: f32,
    ) -> EffectiveCharacterMovement {
        controller.move_shape(
            dt,
            &self.rigid_body_set,
            &self.collider_set,
            &self.query_pipeline,
            shape,
            pos,
            glam_to_na_vec3(translation),
            filter,
            |_| {},
        )
    }

    /// 自动上台阶：抬到台阶高度 → 水平前进 → 贴回地面，返回落点。
    ///
    /// # 为什么自己写而不是用 rapier 的 `autostep`
    ///
    /// 上游实现对胶囊角色不可用（两条实测结论见
    /// [`build_rapier_controller`](crate::character::build_rapier_controller)
    /// 里关掉 autostep 的注释）。这里只用 `move_shape` 做三次试探，台阶高度
    /// 判定、皮肤间隙、坡面投影全部与常规移动共用同一套代码 —— 没有第二套
    /// 穿透规则，也就没有第二类"只在台阶上才出现"的 bug。
    ///
    /// # 为什么要往前多推一个半径
    ///
    /// 胶囊的最低接触点在**底面中心**。只前进原来那点距离的话，中心还在台
    /// 阶边缘之外，向下探测打到的是下一层地面，角色会被"抬起来又放回原地"，
    /// 却已经横向挤进了台阶侧面（实测：直接穿过去）。所以这里把前进距离
    /// 补到"中心至少跨过一个半径"，落地才算真的踩上了台阶。
    // 参数多是因为它是一段纯几何试探：控制器、形状、起点、期望、高度、半径、
    // 间隙、过滤器、dt 全都是输入。打包成结构体只会把这层直白的调用藏起来。
    #[allow(clippy::too_many_arguments)]
    fn try_step_up(
        &self,
        controller: &KinematicCharacterController,
        shape: &Capsule,
        char_pos: &Isometry<f32>,
        desired: Vec3,
        step_height: f32,
        radius: f32,
        rest: f32,
        filter: QueryFilter,
        dt: f32,
    ) -> Option<Isometry<f32>> {
        let horiz = Vec3::new(desired.x, 0.0, desired.z);
        let horiz_len = horiz.length();
        if horiz_len < 1e-4 {
            return None;
        }
        let dir = horiz / horiz_len;
        // 中心必须越过台阶边缘，否则向下探测踩不到台面。
        let advance = dir * (horiz_len + radius + 0.02);

        // 1. 头顶得先有空间，否则就是"钻进天花板"。
        let lift = self.probe_move(controller, shape, char_pos, Vec3::Y * step_height, filter, dt);
        if lift.translation.y < step_height - 1e-3 {
            return None;
        }
        let mut raised = *char_pos;
        raised.translation.vector += lift.translation;

        // 2. 抬起来之后能不能真的走过去。走不过去说明前面是一堵比自己高的
        //    墙，不是台阶 —— 这一条把"贴墙起跳"挡在门外。
        let forward = self.probe_move(controller, shape, &raised, advance, filter, dt);
        let advanced_h = Vec3::new(forward.translation.x, 0.0, forward.translation.z).dot(dir);
        if advanced_h < advance.length() - 1e-3 {
            return None;
        }
        let mut advanced = raised;
        advanced.translation.vector += forward.translation;

        // 3. 贴回地面，且必须落在**高于原地**的表面上。
        //    落了整整一个 `step_height`（只剩皮肤的余量）就说明前面是平地，
        //    不是台阶 —— 那叫"抬起来又放回原地"，采纳它只会把角色塞进墙里。
        let down_dist = step_height + 0.05;
        let down = self.probe_move(controller, shape, &advanced, -Vec3::Y * down_dist, filter, dt);
        if down.translation.y <= -(step_height - rest - 1e-3) {
            return None;
        }
        advanced.translation.vector += down.translation;
        Some(advanced)
    }

    /// 从胶囊中心竖直向下探测脚下表面，返回 `(法线, 法向间隙)`。
    ///
    /// 间隙 = `竖直落距 × n.y − (half_height + radius)`，也就是**沿地面法线**量的
    /// 脚底高度差；平地上退化成"脚底离地多少米"。
    ///
    /// 三个设计点都是踩过坑才定下来的：
    ///
    /// 1. **原点取胶囊中心，不取脚底球心**。角色一旦陷进地面，脚底球心已经在
    ///    地表以下，向下射线只会打到地板的**底面**（法线朝下被过滤），于是
    ///    "陷得越深越探不到地面"，自救逻辑彻底失效。
    /// 2. **乘 `n.y` 换算成法向距离**。斜坡上竖直落距会长于真实间隙（45° 时
    ///    长 41%），直接拿来比 `predict_ground` 会把"稳站坡上"误判成悬空。
    /// 3. `solid = true`，但 `toi == 0`（中心已在碰撞体内部）视为探不到：那时
    ///    法线不可信，宁可报"没有地面"也不要把角色往某个随机方向弹。
    ///
    /// `extra` 是"间隙之外还允许多探多远"：着地判定传 `predict_ground`（严），
    /// 法线与 snap 传 `snap_to_ground`（宽）。射线长度按 2 倍预留，覆盖到 60°
    /// 的坡（竖直落距 = 法向距离 / n.y）。
    ///
    /// 传感器被排除：trigger 不该承重（Unity 语义，与 `move_shape` 一致）。
    fn probe_ground(
        &self,
        body_handle: RigidBodyHandle,
        def: &CharacterController,
        pos: &Isometry<f32>,
        extra: f32,
    ) -> Option<(Vec3, f32)> {
        let c = def.sanitized();
        let core = c.half_height() + c.radius;
        let max_dist = ((core + extra).max(1e-3)) * 2.0;
        let origin = pos.translation.vector;
        let filter = QueryFilter {
            flags: QueryFilterFlags::EXCLUDE_SENSORS,
            groups: None,
            exclude_collider: None,
            exclude_rigid_body: Some(body_handle),
            predicate: None,
        };
        let ray = Ray::new(
            rapier3d::na::Point3::new(origin.x, origin.y, origin.z),
            -rapier3d::math::Vector::y(),
        );
        self.query_pipeline
            .cast_ray_and_get_normal(
                &self.rigid_body_set,
                &self.collider_set,
                &ray,
                max_dist,
                true,
                filter,
            )
            .and_then(|(_, inter)| {
                if inter.time_of_impact <= 0.0 {
                    return None;
                }
                let n = inter.normal;
                let normal = Vec3::new(n.x, n.y, n.z);
                Some((normal, inter.time_of_impact * n.y - core))
            })
    }

    // -----------------------------------------------------------------------
    // Query
    // -----------------------------------------------------------------------

    pub fn body_transform(&self, body_id: u64) -> Option<Transform> {
        let record = self.bodies.get(&body_id)?;
        let rb = self.rigid_body_set.get(record.rapier_handle)?;
        let pos = rb.position();
        let t = pos.translation.vector;
        let rot = pos.rotation;
        Some(Transform {
            position: Vec3::new(t.x, t.y, t.z),
            rotation: Quat::from_xyzw(rot.coords.x, rot.coords.y, rot.coords.z, rot.coords.w),
            scale: Vec3::ONE,
        })
    }

    pub fn body_velocity(&self, body_id: u64) -> Option<Vec3> {
        let record = self.bodies.get(&body_id)?;
        let rb = self.rigid_body_set.get(record.rapier_handle)?;
        let v = *rb.linvel();
        Some(Vec3::new(v.x, v.y, v.z))
    }

    /// 直接设置线速度（运动学体与动态体都适用）。
    pub fn set_velocity(&mut self, body_id: u64, velocity: Vec3) {
        if let Some(record) = self.bodies.get(&body_id) {
            if let Some(rb) = self.rigid_body_set.get_mut(record.rapier_handle) {
                rb.set_linvel(na_vector(velocity.x, velocity.y, velocity.z), true);
            }
        }
    }

    /// 直接设置角速度。
    pub fn set_angular_velocity(&mut self, body_id: u64, velocity: Vec3) {
        if let Some(record) = self.bodies.get(&body_id) {
            if let Some(rb) = self.rigid_body_set.get_mut(record.rapier_handle) {
                rb.set_angvel(na_vector(velocity.x, velocity.y, velocity.z), true);
            }
        }
    }

    /// 把刚体瞬移到指定位置（世界坐标），保留当前朝向。
    ///
    /// 传送用 `set_position` 而不是设速度：设速度会穿过路上的所有东西，
    /// 而"传送"的语义就是跳过路径。
    pub fn teleport(&mut self, body_id: u64, position: Vec3) {
        if let Some(record) = self.bodies.get(&body_id) {
            if let Some(rb) = self.rigid_body_set.get_mut(record.rapier_handle) {
                let mut pos = *rb.position();
                pos.translation = glam_to_na_vec3(position).into();
                rb.set_position(pos, true);
            }
        }
    }

    /// 直接设置刚体的世界朝向。
    pub fn set_rotation(&mut self, body_id: u64, rotation: glam::Quat) {
        if let Some(record) = self.bodies.get(&body_id) {
            if let Some(rb) = self.rigid_body_set.get_mut(record.rapier_handle) {
                let mut pos = *rb.position();
                pos.rotation = glam_quat_to_na_rotation(rotation);
                rb.set_position(pos, true);
            }
        }
    }

    pub fn apply_force(&mut self, body_id: u64, force: Vec3) {
        if let Some(record) = self.bodies.get(&body_id) {
            if let Some(rb) = self.rigid_body_set.get_mut(record.rapier_handle) {
                rb.add_force(na_vector(force.x, force.y, force.z), true);
            }
        }
    }

    pub fn apply_impulse(&mut self, body_id: u64, impulse: Vec3) {
        if let Some(record) = self.bodies.get(&body_id) {
            if let Some(rb) = self.rigid_body_set.get_mut(record.rapier_handle) {
                rb.apply_impulse(na_vector(impulse.x, impulse.y, impulse.z), true);
            }
        }
    }

    pub fn apply_torque(&mut self, body_id: u64, torque: Vec3) {
        if let Some(record) = self.bodies.get(&body_id) {
            if let Some(rb) = self.rigid_body_set.get_mut(record.rapier_handle) {
                rb.add_torque(na_vector(torque.x, torque.y, torque.z), true);
            }
        }
    }

    pub fn set_gravity(&mut self, gravity: Vec3) {
        self.config.gravity = gravity;
    }

    pub fn gravity(&self) -> Vec3 {
        self.config.gravity
    }

    /// 重建射线用的加速结构。
    ///
    /// `step()` 会顺带更新它，所以游戏循环里通常**不需要**手动调。但编辑器
    /// 在**暂停物理**（编辑态）时也要能拾取物体，那时没有 `step()`，
    /// 就必须在建完刚体 / 改过位置之后显式调一次，否则射线会打到旧的包围体上。
    pub fn sync_queries(&mut self) {
        self.query_pipeline.update(&self.collider_set);
    }

    /// 从世界坐标 `origin` 沿 `direction` 发一条射线，返回最近的实心命中。
    ///
    /// `distance` 是沿**归一化方向**的长度；`direction` 传零向量时返回 `None`
    /// （而不是抛错）—— 调用方多半是脚本，不该因为一次笔误打断整帧。
    pub fn raycast(&self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<PhysicsRayHit> {
        self.raycast_with_filter(origin, direction, max_distance, true, RaycastFilter::default())
    }

    /// 带过滤条件的射线检测。
    ///
    /// `solid` 为 `true` 时，射线起点落在某个形状内部会立即命中它（返回距离 0）；
    /// 为 `false` 时则只认从外面穿进去的那一下 —— 角色从物体里走出来时，
    /// 后者不会把脚下的地面误判成"就在原点"。
    pub fn raycast_with_filter(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
        solid: bool,
        filter: RaycastFilter,
    ) -> Option<PhysicsRayHit> {
        if direction.length_squared() <= 1e-12 {
            return None;
        }
        let dir = direction.normalize();
        let ray = Ray::new(glam_to_na_point(origin), glam_to_na_vec3(dir));

        // 层过滤走 `predicate` 而不是 `QueryFilter::groups`：后者用的是
        // `InteractionGroups::test`，要求**双侧相与**（对方掩码也得同意），
        // 于是"射线想看第 5 层"会被第 5 层自己的碰撞矩阵挡掉 —— 与 Unity
        // 的 layerMask 语义不符。这里只查成员位。
        let mask = filter.layer_mask;
        let layer_predicate = |_handle: ColliderHandle, collider: &rapier3d::geometry::Collider| {
            match mask {
                Some(mask) => (collider.collision_groups().memberships.bits() & mask) != 0,
                None => true,
            }
        };
        let mut query_filter = self.build_query_filter(filter);
        if mask.is_some() {
            query_filter.predicate = Some(&layer_predicate);
        }
        let (handle, inter) = self.query_pipeline.cast_ray_and_get_normal(
            &self.rigid_body_set,
            &self.collider_set,
            &ray,
            max_distance,
            solid,
            query_filter,
        )?;

        let collider_id = self.collider_handles.get(&handle).copied()?;
        let is_sensor = self
            .collider_set
            .get(handle)
            .map(|c| c.is_sensor())
            .unwrap_or(false);
        let n = inter.normal;

        Some(PhysicsRayHit {
            body_id: self.resolve_collider_body(handle),
            collider_id,
            distance: inter.time_of_impact,
            point: origin + dir * inter.time_of_impact,
            normal: Vec3::new(n.x, n.y, n.z),
            is_sensor,
        })
    }

    /// 把本 crate 的过滤器翻译成 rapier 的。
    fn build_query_filter(&self, filter: RaycastFilter) -> QueryFilter<'_> {
        let mut flags = QueryFilterFlags::empty();
        if filter.ignore_sensors {
            flags |= QueryFilterFlags::EXCLUDE_SENSORS;
        }
        if filter.only_dynamic {
            flags |= QueryFilterFlags::ONLY_DYNAMIC;
        }
        let exclude_rigid_body = filter
            .exclude_body
            .and_then(|id| self.bodies.get(&id))
            .map(|record| record.rapier_handle);
        QueryFilter {
            flags,
            groups: None,
            exclude_collider: None,
            exclude_rigid_body,
            predicate: None,
        }
    }

    // -----------------------------------------------------------------------
    // Debug draw
    // -----------------------------------------------------------------------

    /// 使用 rapier 的 `compute_aabb` 生成碰撞体线框调试线条。
    pub fn debug_lines(&self) -> Vec<DebugLine> {
        let mut lines = Vec::new();
        let color = Color::WHITE;
        let green = Color::new(0.0, 1.0, 0.0, 0.7);
        let red = Color::new(1.0, 0.2, 0.2, 0.7);

        for record in self.colliders.values() {
            // 获取碰撞体及其形状
            let Some(collider) = self.collider_set.get(record.rapier_handle) else { continue };

            // 用形状 + 位置计算世界空间 AABB
            let shape = collider.shape();
            let pos = collider.position();
            let aabb = shape.compute_aabb(pos);

            let min = Vec3::new(aabb.mins.x, aabb.mins.y, aabb.mins.z);
            let max = Vec3::new(aabb.maxs.x, aabb.maxs.y, aabb.maxs.z);

            let corners = [
                Vec3::new(min.x, min.y, min.z),
                Vec3::new(max.x, min.y, min.z),
                Vec3::new(max.x, max.y, min.z),
                Vec3::new(min.x, max.y, min.z),
                Vec3::new(min.x, min.y, max.z),
                Vec3::new(max.x, min.y, max.z),
                Vec3::new(max.x, max.y, max.z),
                Vec3::new(min.x, max.y, max.z),
            ];

            const EDGES: [(usize, usize); 12] = [
                (0, 1), (1, 2), (2, 3), (3, 0),
                (4, 5), (5, 6), (6, 7), (7, 4),
                (0, 4), (1, 5), (2, 6), (3, 7),
            ];

            // 判断是否为动态体（绿色）或静态体（红色）
            let c = if let Some(body_record) = self.bodies.get(&record.body_id) {
                if let Some(rb) = self.rigid_body_set.get(body_record.rapier_handle) {
                    if rb.is_dynamic() { green } else { red }
                } else { color }
            } else { color };

            for (a, b) in EDGES {
                lines.push(DebugLine::new(corners[a], corners[b], c));
            }
        }

        lines
    }

    // -----------------------------------------------------------------------
    // Events
    // -----------------------------------------------------------------------

    pub fn drain_events(&mut self) -> impl Iterator<Item = CollisionEvent> + '_ {
        self.pending_events.drain(..)
    }

    pub fn stats(&self) -> (usize, usize) {
        (self.bodies.len(), self.colliders.len())
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn resolve_collider_body(&self, collider: ColliderHandle) -> Option<u64> {
        let c = self.collider_set.get(collider)?;
        let body_handle = c.parent()?;
        self.body_handles.get(&body_handle).copied()
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn na_vector(x: f32, y: f32, z: f32) -> rapier3d::na::Vector3<f32> {
    rapier3d::na::Vector3::new(x, y, z)
}

fn glam_to_na_vec3(v: glam::Vec3) -> rapier3d::na::Vector3<f32> {
    na_vector(v.x, v.y, v.z)
}

fn glam_to_na_point(v: glam::Vec3) -> rapier3d::na::Point3<f32> {
    rapier3d::na::Point3::new(v.x, v.y, v.z)
}

fn glam_quat_to_na_rotation(q: glam::Quat) -> rapier3d::na::UnitQuaternion<f32> {
    rapier3d::na::UnitQuaternion::from_quaternion(rapier3d::na::Quaternion::new(
        q.w, q.x, q.y, q.z,
    ))
}

/// 裸位掩码 → rapier 的碰撞组。
///
/// 位掩码（两个 `u32`）是本 crate 对外的表示：`xgame-core` 的层配置、
/// 场景文件、脚本 API 全是位掩码，rapier 的 `Group` 只在物理层内部出现。
fn interaction_groups(memberships: u32, filter: u32) -> InteractionGroups {
    InteractionGroups::new(
        Group::from_bits_truncate(memberships),
        Group::from_bits_truncate(filter),
    )
}

fn collider_to_shared_shape(shape: &ColliderShape) -> rapier3d::geometry::SharedShape {
    match shape {
        ColliderShape::Cuboid { half_extents } => {
            rapier3d::geometry::SharedShape::cuboid(half_extents.x, half_extents.y, half_extents.z)
        }
        ColliderShape::Ball { radius } => rapier3d::geometry::SharedShape::ball(*radius),
        ColliderShape::Capsule { half_height, radius } => {
            rapier3d::geometry::SharedShape::capsule(
                rapier3d::na::Point3::new(0.0, -*half_height, 0.0),
                rapier3d::na::Point3::new(0.0, *half_height, 0.0),
                *radius,
            )
        }
        ColliderShape::Cylinder { half_height, radius } => {
            rapier3d::geometry::SharedShape::cylinder(*half_height, *radius)
        }
        ColliderShape::Trimesh { vertices, indices } => {
            rapier3d::geometry::SharedShape::trimesh(
                vertices.iter().map(|v| rapier3d::na::Point3::new(v[0], v[1], v[2])).collect(),
                indices.iter().map(|t| [t[0], t[1], t[2]]).collect(),
            )
        }
        ColliderShape::HeightField { heights, nx, nz, scale } => {
            let data = rapier3d::na::DMatrix::from_row_slice(*nx as usize, *nz as usize, heights);
            rapier3d::geometry::SharedShape::heightfield(data, na_vector(scale.x, scale.y, scale.z))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn drop_body_frees_colliders() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        let body = world.add_body(RigidBody::dynamic_at(Transform::IDENTITY));
        let _ = world.add_collider(body, Collider::default());
        assert_eq!(world.stats(), (1, 1));
        world.remove_body(body);
        assert_eq!(world.stats(), (0, 0));
    }

    /// 给一个不存在的刚体挂碰撞体**不该** panic —— 早先这里是
    /// `.expect("unknown body")`，一次传错 id 就能带走整个进程。
    #[test]
    fn add_collider_on_unknown_body_is_rejected_without_panicking() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        assert_eq!(world.add_collider(999, Collider::default()), None);
        assert_eq!(world.stats(), (0, 0), "被拒绝的挂载不该留下任何记录");

        // 删掉刚体之后再挂，同样只应被拒绝。
        let body = world.add_body(RigidBody::dynamic_at(Transform::IDENTITY));
        world.remove_body(body);
        assert_eq!(world.add_collider(body, Collider::default()), None);
        assert_eq!(world.stats(), (0, 0));
    }

    #[test]
    fn gravity_pulls_downward() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        let body = world.add_body(RigidBody::dynamic_at(Transform {
            position: Vec3::new(0.0, 5.0, 0.0),
            ..Transform::IDENTITY
        }));
        let _ = world.add_collider(body, Collider::default());
        for _ in 0..10 {
            world.step(1.0 / 60.0);
        }
        let t = world.body_transform(body).unwrap();
        assert!(t.position.y < 5.0, "body should fall due to gravity");
    }

    #[test]
    fn static_body_does_not_move() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        let body = world.add_body(RigidBody::static_at(Transform {
            position: Vec3::new(1.0, 2.0, 3.0),
            ..Transform::IDENTITY
        }));
        let _ = world.add_collider(body, Collider::default());
        for _ in 0..10 {
            world.step(1.0 / 60.0);
        }
        let t = world.body_transform(body).unwrap();
        assert_eq!(t.position, Vec3::new(1.0, 2.0, 3.0));
    }

    /// `step(dt)` 必须按**传入的** `dt` 积分。
    ///
    /// 关键是把 `config.timestep` 设成一个和传入值**不同**的数：旧实现固定用
    /// `config.timestep`、把参数丢掉，只有这种情况下才会露馅。重力下速度就是
    /// `g × 实际步长`，一比就知道用的是哪个。
    #[test]
    fn step_honors_passed_dt() {
        let mut world = PhysicsWorld::new(PhysicsConfig {
            timestep: 1.0 / 240.0,
            ..Default::default()
        });
        let body = world.add_body(RigidBody::dynamic_at(Transform::IDENTITY));
        let _ = world.add_collider(body, Collider::default());

        let dt = 1.0 / 30.0;
        world.step(dt);

        let v = world.body_velocity(body).unwrap().y.abs();
        let expected = 9.81 * dt;
        assert!(
            (v - expected).abs() < 1e-2,
            "step 应当按传入的 dt={dt} 积分（期望速度 {expected}），实际 {v}"
        );
    }

    /// 子步只是把 `dt` 均分，总推进时长不变。
    ///
    /// 旧实现每个子步都用完整的 `dt`，`num_sub_steps = 8` 就等于一次调用推进 8 倍
    /// 的时间 —— 物理直接"快进"。子步数不改变速度，只改变精度。
    #[test]
    fn sub_steps_do_not_change_simulated_time() {
        let dt = 1.0 / 60.0;

        let mut single = PhysicsWorld::new(PhysicsConfig {
            num_sub_steps: 1,
            ..Default::default()
        });
        let a = single.add_body(RigidBody::dynamic_at(Transform::IDENTITY));
        let _ = single.add_collider(a, Collider::default());
        single.step(dt);

        let mut subbed = PhysicsWorld::new(PhysicsConfig {
            num_sub_steps: 8,
            ..Default::default()
        });
        let b = subbed.add_body(RigidBody::dynamic_at(Transform::IDENTITY));
        let _ = subbed.add_collider(b, Collider::default());
        subbed.step(dt);

        let va = single.body_velocity(a).unwrap();
        let vb = subbed.body_velocity(b).unwrap();
        assert!(
            (va - vb).length() < 1e-3,
            "子步数不该改变一次 step 推进的总时长: {va} vs {vb}"
        );
    }

    /// 非法 `dt` 退回配置值，而不是把模拟推进到 NaN。
    #[test]
    fn step_falls_back_on_invalid_dt() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        let body = world.add_body(RigidBody::dynamic_at(Transform {
            position: Vec3::new(0.0, 5.0, 0.0),
            ..Transform::IDENTITY
        }));
        let _ = world.add_collider(body, Collider::default());

        world.step(0.0);
        world.step(-1.0);
        world.step(f32::NAN);

        let t = world.body_transform(body).unwrap();
        assert!(t.position.is_finite(), "非法 dt 不该让位置变成 NaN: {t:?}");
        assert!(t.position.y < 5.0, "退回默认步长后仍应正常下落: {t:?}");
    }

    #[test]
    fn ball_collides_with_floor() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        let floor = world.add_body(RigidBody::static_at(Transform {
            position: Vec3::new(0.0, -1.0, 0.0),
            ..Transform::IDENTITY
        }));
        let _ = world.add_collider(floor, Collider::cuboid(Vec3::new(10.0, 0.5, 10.0)));

        let ball = world.add_body(RigidBody::dynamic_at(Transform {
            position: Vec3::new(0.0, 2.0, 0.0),
            ..Transform::IDENTITY
        }));
        let _ = world.add_collider(ball, Collider::ball(1.0));

        for _ in 0..120 {
            world.step(1.0 / 60.0);
        }

        let ball_pos = world.body_transform(ball).unwrap();
        assert!(ball_pos.position.y > -0.5);

        let events: Vec<_> = world.drain_events().collect();
        assert!(!events.is_empty(), "collision events should be non-empty");
    }

    // -----------------------------------------------------------------------
    // 层 / 碰撞矩阵
    // -----------------------------------------------------------------------

    /// 造一个"地板 + 落球"的场景，两侧各给一对碰撞组。
    fn drop_onto_floor(floor_groups: (u32, u32), ball_groups: (u32, u32)) -> f32 {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        let floor = world.add_body(RigidBody::static_at(Transform {
            position: Vec3::new(0.0, -1.0, 0.0),
            ..Transform::IDENTITY
        }));
        let _ = world.add_collider(
            floor,
            Collider {
                memberships: floor_groups.0,
                filter: floor_groups.1,
                ..Collider::cuboid(Vec3::new(10.0, 0.5, 10.0))
            },
        );

        let ball = world.add_body(RigidBody::dynamic_at(Transform {
            position: Vec3::new(0.0, 3.0, 0.0),
            ..Transform::IDENTITY
        }));
        let _ = world.add_collider(
            ball,
            Collider {
                memberships: ball_groups.0,
                filter: ball_groups.1,
                ..Collider::ball(0.5)
            },
        );

        for _ in 0..120 {
            world.step(1.0 / 60.0);
        }
        world.body_transform(ball).unwrap().position.y
    }

    /// 矩阵里关掉的一对**根本不产生接触**：球直接穿过地板。
    ///
    /// 这是层功能的核心断言 —— 如果它只是"事件被过滤掉"，球仍会停在地板上。
    #[test]
    fn non_colliding_layers_pass_through_each_other() {
        let (layer1, layer2) = (1u32 << 1, 1u32 << 2);
        // 两侧都同意"互不碰撞"（rapier 的判定是双侧相与）。
        let floor = (layer1, !layer2);
        let ball = (layer2, !layer1);

        let y_blocked = drop_onto_floor((layer1, u32::MAX), (layer2, u32::MAX));
        assert!(
            y_blocked > -0.5,
            "默认全通时球应当停在地板上（y≈0），实际 y={y_blocked}"
        );

        let y_through = drop_onto_floor(floor, ball);
        assert!(
            y_through < -5.0,
            "关掉 1×2 之后球必须穿过去（实际 y={y_through}）"
        );
    }

    /// 换层立刻生效：不用等下一次 `sync_bodies` / 重建刚体。
    #[test]
    fn set_body_groups_takes_effect_immediately() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        let floor = world.add_body(RigidBody::static_at(Transform {
            position: Vec3::new(0.0, -0.5, 0.0),
            ..Transform::IDENTITY
        }));
        let _ = world.add_collider(floor, Collider::cuboid(Vec3::new(10.0, 0.5, 10.0)));

        let ball = world.add_body(RigidBody::dynamic_at(Transform {
            position: Vec3::new(0.0, 1.0, 0.0),
            ..Transform::IDENTITY
        }));
        let ball_collider = world
            .add_collider(ball, Collider::ball(0.4))
            .expect("碰撞体挂载失败");

        // 让球不再搭理地板（0 层）：两侧都要同意，所以地板那一侧也得改。
        assert!(world.set_body_groups(floor, 1 << 0, !(1 << 2)));
        assert!(world.set_body_groups(ball, 1 << 2, !(1 << 0)));

        for _ in 0..120 {
            world.step(1.0 / 60.0);
        }
        let y = world.body_transform(ball).unwrap().position.y;
        assert!(y < -5.0, "换层后球应当穿地板落下，实际 y={y}");

        // 未知刚体不该 panic，只返回 false。
        assert!(!world.set_body_groups(9999, 1, u32::MAX));
        assert!(world.colliders.contains_key(&ball_collider));
    }

    /// 射线层掩码：只看成员位，**不看碰撞矩阵**。
    #[test]
    fn raycast_layer_mask_ignores_collision_matrix() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());

        // 目标在 3 层，但它的掩码是 0（与任何东西都不碰）。
        let target = world.add_body(RigidBody::static_at(Transform {
            position: Vec3::new(0.0, 0.0, -5.0),
            ..Transform::IDENTITY
        }));
        let _ = world.add_collider(
            target,
            Collider {
                memberships: 1 << 3,
                filter: 0,
                ..Collider::cuboid(Vec3::splat(1.0))
            },
        );

        // 查询结构只在 `step()` / `sync_queries()` 里刷新 —— 建完刚体要显式同步一次。
        world.sync_queries();

        // 3 层掩码：打得到。
        let hit = world.raycast_with_filter(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -1.0),
            100.0,
            true,
            RaycastFilter {
                layer_mask: Some(1 << 3),
                ..RaycastFilter::default()
            },
        );
        assert!(hit.is_some(), "掩码包含目标层时必须命中（矩阵不参与查询过滤）");
        assert_eq!(hit.unwrap().body_id, Some(target));

        // 别的层掩码：打不到。
        let miss = world.raycast_with_filter(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -1.0),
            100.0,
            true,
            RaycastFilter {
                layer_mask: Some(1 << 4),
                ..RaycastFilter::default()
            },
        );
        assert!(miss.is_none(), "掩码不含目标层时必须打不到");

        // 不给掩码 = 和以前一样，全都算。
        assert!(world.raycast(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), 100.0).is_some());
    }
}
