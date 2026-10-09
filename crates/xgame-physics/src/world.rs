//! 物理世界——拥有 rapier3d 内部状态、驱动模拟步骤，为游戏代码提供简洁接口。

use std::collections::HashMap;
use std::sync::Mutex;

use glam::{Quat, Vec3};
use xgame_core::{Color, Transform};
use xgame_math::glam;

use rapier3d::dynamics::{
    CCDSolver, ImpulseJointSet, IntegrationParameters, IslandManager,
    MultibodyJointSet, RigidBodyBuilder, RigidBodyHandle, RigidBodySet,
};
use rapier3d::geometry::{
    BroadPhaseMultiSap as DefaultBroadPhase, ColliderBuilder, ColliderHandle,
    ColliderSet, NarrowPhase,
};
use rapier3d::prelude::{ActiveEvents, Ray};
use rapier3d::pipeline::{PhysicsPipeline, QueryFilter, QueryFilterFlags, QueryPipeline};

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

    pub fn add_collider(&mut self, body_id: u64, collider: Collider) -> u64 {
        let cid = self.next_id;
        self.next_id += 1;

        let body_record = self.bodies.get(&body_id).expect("unknown body");

        let shape = collider_to_shared_shape(&collider.shape);
        let cb = ColliderBuilder::new(shape)
            .friction(collider.friction)
            .restitution(collider.restitution)
            .sensor(collider.is_sensor)
            .density(collider.density)
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .translation(glam_to_na_vec3(collider.offset));

        let rapier_handle = self.collider_set.insert_with_parent(
            cb, body_record.rapier_handle, &mut self.rigid_body_set,
        );

        self.colliders.insert(cid, ColliderRecord { rapier_handle, body_id });
        self.collider_handles.insert(rapier_handle, cid);
        cid
    }

    pub fn remove_collider(&mut self, collider_id: u64) {
        if let Some(record) = self.colliders.remove(&collider_id) {
            self.collider_handles.remove(&record.rapier_handle);
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

        let query_filter = self.build_query_filter(filter);
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
        world.add_collider(body, Collider::default());
        assert_eq!(world.stats(), (1, 1));
        world.remove_body(body);
        assert_eq!(world.stats(), (0, 0));
    }

    #[test]
    fn gravity_pulls_downward() {
        let mut world = PhysicsWorld::new(PhysicsConfig::default());
        let body = world.add_body(RigidBody::dynamic_at(Transform {
            position: Vec3::new(0.0, 5.0, 0.0),
            ..Transform::IDENTITY
        }));
        world.add_collider(body, Collider::default());
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
        world.add_collider(body, Collider::default());
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
        world.add_collider(body, Collider::default());

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
        single.add_collider(a, Collider::default());
        single.step(dt);

        let mut subbed = PhysicsWorld::new(PhysicsConfig {
            num_sub_steps: 8,
            ..Default::default()
        });
        let b = subbed.add_body(RigidBody::dynamic_at(Transform::IDENTITY));
        subbed.add_collider(b, Collider::default());
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
        world.add_collider(body, Collider::default());

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
        world.add_collider(floor, Collider::cuboid(Vec3::new(10.0, 0.5, 10.0)));

        let ball = world.add_body(RigidBody::dynamic_at(Transform {
            position: Vec3::new(0.0, 2.0, 0.0),
            ..Transform::IDENTITY
        }));
        world.add_collider(ball, Collider::ball(1.0));

        for _ in 0..120 {
            world.step(1.0 / 60.0);
        }

        let ball_pos = world.body_transform(ball).unwrap();
        assert!(ball_pos.position.y > -0.5);

        let events: Vec<_> = world.drain_events().collect();
        assert!(!events.is_empty(), "collision events should be non-empty");
    }
}
