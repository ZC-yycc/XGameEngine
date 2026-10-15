//! 游戏上下文方法层：脚本 API 的真相源。
//!
//! [`GameCtx`] 把"一次派发所需的全部状态"捆成一束：场景、物理、输入、
//! 日志与命令缓冲、触发本次调用的实体、时间。原先散在 `api.rs`（Rhai 转发壳，
//! 已随 Rhai 删除）里的 100+ 个闭包**函数体**全部平移为这里的 `impl GameCtx`
//! 方法；`lua_api.rs` 退化成一行一个的转发壳。这条分界是刻意的：
//!
//! - **脚本层是公共 API 的第一个用户**。Rust 组件（`Lifecycle`）与 Lua 绑定
//!   调的是同一批 `GameCtx` 方法，两个轨道的能力边界与行为天然一致；
//! - 引擎侧调试 / 测试可以直接 `GameCtx { .. }` 构造一束状态调方法，
//!   不需要经过 Lua 解释器。
//!
//! # 为什么这个类型住在 `xgame-script` 而不是 `xgame-runtime`
//!
//! 双轨计划文档最初把 `ctx.rs` 定位在 `xgame-runtime`，但那会撞上 crate 依赖
//! 方向的硬约束：`xgame-script` 的转发壳要调用 `GameCtx` 方法，若
//! `GameCtx` 在 runtime 里，就变成 script → runtime，而 runtime 本来就依赖
//! script（`World` 持有 `LuaHost`）—— 循环依赖编译不过。既定方向
//! `core ← math ← {render, physics}`、`script` 中层、`runtime` 在上不许反向，
//! 所以方法层必须放在 script（或更低层）这一侧；它需要的全部类型
//! （`PhysicsWorld` / `Input` / glam）script 本来就依赖着，零新增依赖。
//!
//! # 坐标约定（与旧脚本 API 完全一致，别改）
//!
//! | 方法 | 空间 |
//! |---|---|
//! | `pos` / `set_pos` / `translate` | **世界**（脚本作者最常想要的） |
//! | `local_pos` / `set_local_pos` | 局部（相对父节点） |
//! | `rot` / `set_rot` | **局部**欧拉角，单位**度**，顺序 Y-X-Z |
//! | `scale` / `set_scale` | 局部 |
//!
//! 位置用世界空间、旋转用局部角度，是因为位置几乎总在"摆到哪里"的语境里
//! （父节点已经摆好了），而旋转几乎总在"这个物体自己转多少"的语境里。
//!
//! # 生命周期约定
//!
//! `GameCtx` 内全是借用：由派发方（`LuaHost::run_hook`，或组件分发）
//! 在栈上构造，活到钩子调用返回为止。交给 Lua 的 [`crate::ScriptCtx`] 只是
//! 它的**地址束**（见 `context.rs` 的安全论证），不延长任何借用。

use xgame_core::{Entity, EntityId, PhysicsBody, PhysicsHandle, Scene};
use xgame_input::{Input, KeyCode, MouseButton};
use xgame_math::glam::{EulerRot, Quat, Vec2, Vec3};
use xgame_physics::{PhysicsRayHit, PhysicsWorld, RaycastFilter};

use crate::context::{LogLevel, ScriptCommand, ScriptLog, SpawnRequest};

// ---------------------------------------------------------------------------
// 上下文
// ---------------------------------------------------------------------------

/// 一次派发所需的全部状态束 —— 脚本 API 的真相源。
///
/// 所有方法收 `&mut self`：即便个别方法只读，也统一成可变访问，免得每个方法
/// 都要为"从 `&self` 里借出 `&mut Scene`"单独写 unsafe —— 字段是 `&mut` 借用时，
/// 从不可变的 `&self` 交不出可变别名。
pub struct GameCtx<'a> {
    /// 当前场景（增删实体、改变换都落在这上面）。
    pub scene: &'a mut Scene,
    /// 物理世界。没有物理时（无头测试）为 `None`。
    pub physics: Option<&'a mut PhysicsWorld>,
    /// 输入状态。没有输入系统时为 `None`。
    pub input: Option<&'a Input>,
    /// 日志缓冲（派发方持有，帧末统一回收）。
    pub logs: &'a mut Vec<ScriptLog>,
    /// 结构性改动通知缓冲：场景是上下文改的，刚体是运行时建的，
    /// 靠这份通知让运行时把物理体补上 / 清掉（见 [`ScriptCommand`]）。
    pub commands: &'a mut Vec<ScriptCommand>,
    /// 日志归属名：脚本轨传脚本的项目相对路径，组件轨传行为名。
    pub source: &'a str,
    /// 触发本次调用的实体（"self"）。[`EntityId::raw`] 为 `0` 表示没有实体上下文。
    pub entity: EntityId,
    /// 本帧真实帧间隔（秒）。
    pub delta: f32,
    /// 游戏已运行的总时长（秒）。
    pub time: f32,
}

/// 射线命中结果。
///
/// 用这个普通结构而不是直接暴露 rapier 的类型：脚本侧把它摊成 Map、组件侧
/// 直接读字段，两轨看到同一份语义。未命中时所有字段填哨兵值（`hit = false`），
/// 调用方不需要先判空就能读字段。
#[derive(Debug, Clone, Copy)]
pub struct RayHit {
    pub hit: bool,
    pub point: Vec3,
    pub normal: Vec3,
    pub distance: f32,
    /// 命中的实体（由刚体句柄反查场景得到）。
    pub entity: Option<EntityId>,
    pub is_sensor: bool,
}

impl RayHit {
    /// 未命中的哨兵值。
    pub fn miss() -> Self {
        Self {
            hit: false,
            point: Vec3::ZERO,
            normal: Vec3::Y,
            distance: 0.0,
            entity: None,
            is_sensor: false,
        }
    }

    /// 从物理侧的命中结果构造（`entity` 是反查出来的实体）。
    pub fn from_hit(hit: Option<PhysicsRayHit>, entity: Option<EntityId>) -> Self {
        match hit {
            Some(h) => Self {
                hit: true,
                point: h.point,
                normal: h.normal,
                distance: h.distance,
                entity,
                is_sensor: h.is_sensor,
            },
            None => Self::miss(),
        }
    }
}

impl GameCtx<'_> {
    // -----------------------------------------------------------------------
    // 基础访问
    // -----------------------------------------------------------------------

    /// 触发本次调用的实体 ID（`raw() == 0` 表示没有实体上下文）。
    pub fn self_id(&self) -> EntityId {
        self.entity
    }

    /// 同上，但没有上下文时返回 `None`（内部逻辑用这个）。
    pub fn entity_opt(&self) -> Option<EntityId> {
        (self.entity.raw() != 0).then_some(self.entity)
    }

    /// 当前实体在场景里的索引。
    pub fn entity_index(&mut self) -> Option<usize> {
        self.scene.find_entity(self.entity)
    }

    /// 本帧帧间隔（秒）。
    pub fn delta(&self) -> f32 {
        self.delta
    }

    /// 游戏已运行总时长（秒）。
    pub fn time(&self) -> f32 {
        self.time
    }

    /// 记一条 Info 日志（编辑器控制台 / 游戏日志）。
    pub fn log(&mut self, message: impl Into<String>) {
        self.push_log(LogLevel::Info, message.into());
    }

    /// 记一条 Warn 日志。
    pub fn warn(&mut self, message: impl Into<String>) {
        self.push_log(LogLevel::Warn, message.into());
    }

    /// 记一条 Error 日志。
    pub fn error(&mut self, message: impl Into<String>) {
        self.push_log(LogLevel::Error, message.into());
    }

    fn push_log(&mut self, level: LogLevel, message: String) {
        let entry = ScriptLog {
            level,
            script: self.source.to_owned(),
            entity: self.entity_opt(),
            message,
        };
        self.logs.push(entry);
    }

    fn push_command(&mut self, command: ScriptCommand) {
        self.commands.push(command);
    }

    // -----------------------------------------------------------------------
    // 自身变换
    // -----------------------------------------------------------------------

    /// 实体的**世界**位置。
    pub fn pos(&mut self) -> Vec3 {
        self.entity_index()
            .map(|i| world_position(self.scene, i))
            .unwrap_or(Vec3::ZERO)
    }

    /// 实体的**局部**位置（相对父节点）。
    pub fn local_pos(&mut self) -> Vec3 {
        self.entity_index()
            .and_then(|i| self.scene.entities.get(i).map(|e| e.transform.position))
            .unwrap_or(Vec3::ZERO)
    }

    /// 设置**世界**位置（有父节点时自动换算回局部）。
    pub fn set_pos(&mut self, position: Vec3) {
        if let Some(i) = self.entity_index() {
            set_world_position(self.scene, i, position);
        }
    }

    /// 设置**局部**位置。
    pub fn set_local_pos(&mut self, position: Vec3) {
        if let Some(i) = self.entity_index() {
            if let Some(e) = self.scene.entities.get_mut(i) {
                e.transform.position = position;
            }
        }
    }

    /// 沿世界坐标平移。
    pub fn translate(&mut self, delta: Vec3) {
        if let Some(i) = self.entity_index() {
            let p = world_position(self.scene, i) + delta;
            set_world_position(self.scene, i, p);
        }
    }

    /// 局部欧拉角（度，Y-X-Z）。
    pub fn rot(&mut self) -> Vec3 {
        self.entity_index()
            .and_then(|i| self.scene.entities.get(i).map(|e| euler_degrees(e.transform.rotation)))
            .unwrap_or(Vec3::ZERO)
    }

    /// 设置局部欧拉角（度）。
    pub fn set_rot(&mut self, degrees: Vec3) {
        if let Some(i) = self.entity_index() {
            if let Some(e) = self.scene.entities.get_mut(i) {
                e.transform.rotation = quat_from_degrees(degrees);
            }
        }
    }

    /// 在当前旋转上叠加一个增量旋转（度）。
    pub fn rotate(&mut self, degrees: Vec3) {
        if let Some(i) = self.entity_index() {
            let delta = quat_from_degrees(degrees);
            if let Some(e) = self.scene.entities.get_mut(i) {
                e.transform.rotation = (delta * e.transform.rotation).normalize();
            }
        }
    }

    /// 局部缩放。
    pub fn scale(&mut self) -> Vec3 {
        self.entity_index()
            .and_then(|i| self.scene.entities.get(i).map(|e| e.transform.scale))
            .unwrap_or(Vec3::ONE)
    }

    /// 设置局部缩放。
    pub fn set_scale(&mut self, scale: Vec3) {
        if let Some(i) = self.entity_index() {
            if let Some(e) = self.scene.entities.get_mut(i) {
                e.transform.scale = scale;
            }
        }
    }

    /// 世界朝向的 +Z 轴（算"往前飞"最常用的三个向量，省得自己拿四元数去乘）。
    pub fn forward(&mut self) -> Vec3 {
        self.entity_index()
            .map(|i| world_rotation(self.scene, i).mul_vec3(Vec3::Z))
            .unwrap_or(Vec3::Z)
    }

    /// 世界朝向的 +X 轴。
    pub fn right(&mut self) -> Vec3 {
        self.entity_index()
            .map(|i| world_rotation(self.scene, i).mul_vec3(Vec3::X))
            .unwrap_or(Vec3::X)
    }

    /// 世界朝向的 +Y 轴。
    pub fn up(&mut self) -> Vec3 {
        self.entity_index()
            .map(|i| world_rotation(self.scene, i).mul_vec3(Vec3::Y))
            .unwrap_or(Vec3::Y)
    }

    // -----------------------------------------------------------------------
    // 实体查询与操作
    // -----------------------------------------------------------------------

    /// 自己的显示名称。
    pub fn self_name(&mut self) -> String {
        self.entity_index()
            .and_then(|i| self.scene.entities.get(i).map(|e| e.name.clone()))
            .unwrap_or_default()
    }

    /// 场景里的实体数。
    pub fn entity_count(&self) -> usize {
        self.scene.entities.len()
    }

    /// 按索引取实体 ID（越界返回 `None`）。
    pub fn entity_at(&mut self, index: usize) -> Option<EntityId> {
        self.scene.entities.get(index).map(|e| e.id)
    }

    /// 按名字查实体 ID（找不到返回 `None`）。
    pub fn find_entity_id(&mut self, name: &str) -> Option<EntityId> {
        self.scene.entities.iter().find(|e| e.name == name).map(|e| e.id)
    }

    /// 实体的显示名称。
    pub fn entity_name(&mut self, id: EntityId) -> String {
        self.scene
            .find_entity(id)
            .and_then(|i| self.scene.entities.get(i).map(|e| e.name.clone()))
            .unwrap_or_default()
    }

    /// 实体的模型路径。
    pub fn entity_model(&mut self, id: EntityId) -> String {
        self.scene
            .find_entity(id)
            .and_then(|i| self.scene.entities.get(i).map(|e| e.model.clone()))
            .unwrap_or_default()
    }

    /// 改实体的模型路径。
    pub fn set_entity_model(&mut self, id: EntityId, model: &str) {
        if let Some(i) = self.scene.find_entity(id) {
            if let Some(e) = self.scene.entities.get_mut(i) {
                e.model = model.to_owned();
            }
        }
    }

    /// 实体的**世界**位置。
    pub fn entity_pos(&mut self, id: EntityId) -> Vec3 {
        self.scene
            .find_entity(id)
            .map(|i| world_position(self.scene, i))
            .unwrap_or(Vec3::ZERO)
    }

    /// 设置实体的**世界**位置（有父节点时自动换算回局部）。
    pub fn set_entity_pos(&mut self, id: EntityId, position: Vec3) {
        if let Some(i) = self.scene.find_entity(id) {
            set_world_position(self.scene, i, position);
        }
    }

    /// 实体的局部缩放。
    pub fn entity_scale(&mut self, id: EntityId) -> Vec3 {
        self.scene
            .find_entity(id)
            .and_then(|i| self.scene.entities.get(i).map(|e| e.transform.scale))
            .unwrap_or(Vec3::ONE)
    }

    /// 设置实体的局部缩放。
    pub fn set_entity_scale(&mut self, id: EntityId, scale: Vec3) {
        if let Some(i) = self.scene.find_entity(id) {
            if let Some(e) = self.scene.entities.get_mut(i) {
                e.transform.scale = scale;
            }
        }
    }

    /// 实体的局部欧拉角（度）。
    pub fn entity_rot(&mut self, id: EntityId) -> Vec3 {
        self.scene
            .find_entity(id)
            .and_then(|i| self.scene.entities.get(i).map(|e| euler_degrees(e.transform.rotation)))
            .unwrap_or(Vec3::ZERO)
    }

    /// 设置实体的局部欧拉角（度）。
    pub fn set_entity_rot(&mut self, id: EntityId, degrees: Vec3) {
        if let Some(i) = self.scene.find_entity(id) {
            if let Some(e) = self.scene.entities.get_mut(i) {
                e.transform.rotation = quat_from_degrees(degrees);
            }
        }
    }

    /// 自己到目标实体的世界距离（目标不存在返回 0）。
    pub fn distance_to(&mut self, other: EntityId) -> f32 {
        let Some(self_i) = self.entity_index() else { return 0.0 };
        let Some(other_i) = self.scene.find_entity(other) else { return 0.0 };
        world_position(self.scene, self_i).distance(world_position(self.scene, other_i))
    }

    /// 销毁实体（整棵子树），并把被删掉的 ID 报给运行时清刚体。
    pub fn destroy(&mut self, id: EntityId) {
        let Some(index) = self.scene.find_entity(id) else {
            return;
        };
        let removed = self.scene.remove_subtree(index);
        if removed.is_empty() {
            return;
        }
        let ids: Vec<EntityId> = removed.iter().map(|e| e.id).collect();
        self.push_command(ScriptCommand::EntitiesRemoved { ids });
    }

    /// 改实体的父节点（`None` = 挂到场景根）。成环时拒绝（`Scene::set_parent`）。
    pub fn set_parent(&mut self, child: EntityId, parent: Option<EntityId>) {
        let Some(child_index) = self.scene.find_entity(child) else {
            return;
        };
        let parent_index = match parent {
            Some(p) => match self.scene.find_entity(p) {
                Some(index) => Some(index),
                None => return,
            },
            None => None,
        };
        // `set_parent` 自己会拒绝成环，返回 false 时不要假装成功。
        if self.scene.set_parent(child_index, parent_index) {
            self.push_command(ScriptCommand::Reparented { child });
        }
    }

    // -----------------------------------------------------------------------
    // 生成实体
    // -----------------------------------------------------------------------

    /// 生成一个实体，返回它的 ID。
    pub fn spawn_entity(&mut self, name: &str, model: &str) -> EntityId {
        self.spawn_request(SpawnRequest {
            name: name.to_owned(),
            model: model.to_owned(),
            ..SpawnRequest::new(name)
        })
    }

    /// 在指定**世界位置**生成一个实体。
    pub fn spawn_entity_at(&mut self, name: &str, model: &str, position: Vec3) -> EntityId {
        self.spawn_request(SpawnRequest {
            name: name.to_owned(),
            model: model.to_owned(),
            position,
            ..SpawnRequest::new(name)
        })
    }

    /// 生成一个动态球体（自带球形碰撞体）。
    pub fn spawn_ball(&mut self, name: &str, radius: f32, center: Vec3) -> EntityId {
        self.spawn_request(SpawnRequest {
            name: name.to_owned(),
            model: "builtin://sphere".to_owned(),
            position: center,
            scale: Vec3::splat(radius * 2.0),
            physics: Some(PhysicsBody::dynamic_ball(radius)),
            ..SpawnRequest::new(name)
        })
    }

    /// 生成一个静态方盒（自带盒形碰撞体）。
    pub fn spawn_static_box(&mut self, name: &str, half_extents: Vec3, center: Vec3) -> EntityId {
        self.spawn_request(SpawnRequest {
            name: name.to_owned(),
            model: "builtin://cube".to_owned(),
            position: center,
            scale: half_extents * 2.0,
            physics: Some(PhysicsBody::static_box(half_extents.to_array())),
            ..SpawnRequest::new(name)
        })
    }

    /// 按请求生成实体，**立即**放进场景，返回它的 ID。
    ///
    /// 立即（而不是攒成命令延后执行）是刻意的：脚本里
    /// `let id = spawn_entity(...); set_entity_pos(id, ...)` 是最自然的写法，
    /// 延后执行会让第二句对着一个还不存在的实体静默失效。
    /// 安全性依据见 [`ScriptCommand`] 的说明。
    fn spawn_request(&mut self, request: SpawnRequest) -> EntityId {
        let id = EntityId::new();
        let mut entity = Entity::new(&request.name);
        entity.id = id;
        entity.model = request.model;
        entity.transform.position = request.position;
        entity.transform.rotation = request.rotation;
        entity.transform.scale = request.scale;
        entity.physics = request.physics;

        self.scene.entities.push(entity);
        self.push_command(ScriptCommand::EntitySpawned {
            id,
            physics: request.physics,
        });
        id
    }

    // -----------------------------------------------------------------------
    // 物理
    // -----------------------------------------------------------------------

    /// 目标实体有没有刚体。
    pub fn has_physics(&mut self, id: EntityId) -> bool {
        body_of(self.scene, id).is_some()
    }

    /// 沿方向发射线（默认忽略传感器、排除自己），返回命中结果。
    pub fn raycast(&mut self, origin: Vec3, dir: Vec3, max_dist: f32) -> RayHit {
        let Some(physics) = self.physics.as_deref() else {
            return RayHit::miss();
        };
        let filter = RaycastFilter {
            ignore_sensors: true,
            exclude_body: self.entity_opt().and_then(|id| body_of(self.scene, id)),
            ..RaycastFilter::default()
        };
        let hit = physics.raycast_with_filter(origin, dir, max_dist, true, filter);
        // 命中结果里的 body 句柄对脚本 / 组件没意义，换成实体 ID 才可用。
        let entity = hit
            .and_then(|h| h.body_id)
            .and_then(|body| entity_of_body(self.scene, body));
        RayHit::from_hit(hit, entity)
    }

    /// 给目标刚体加持续力。
    pub fn apply_force(&mut self, id: EntityId, force: Vec3) {
        let Some(body) = body_of(self.scene, id) else { return };
        if let Some(physics) = self.physics.as_deref_mut() {
            physics.apply_force(body, force);
        }
    }

    /// 给目标刚体加瞬时冲量。
    pub fn apply_impulse(&mut self, id: EntityId, impulse: Vec3) {
        let Some(body) = body_of(self.scene, id) else { return };
        if let Some(physics) = self.physics.as_deref_mut() {
            physics.apply_impulse(body, impulse);
        }
    }

    /// 给目标刚体加扭矩。
    pub fn add_torque(&mut self, id: EntityId, torque: Vec3) {
        let Some(body) = body_of(self.scene, id) else { return };
        if let Some(physics) = self.physics.as_deref_mut() {
            physics.apply_torque(body, torque);
        }
    }

    /// 目标刚体的线速度（无刚体返回零向量）。
    pub fn velocity(&mut self, id: EntityId) -> Vec3 {
        let Some(body) = body_of(self.scene, id) else { return Vec3::ZERO };
        self.physics
            .as_deref()
            .and_then(|p| p.body_velocity(body))
            .unwrap_or(Vec3::ZERO)
    }

    /// 设置目标刚体的线速度。
    pub fn set_velocity(&mut self, id: EntityId, velocity: Vec3) {
        let Some(body) = body_of(self.scene, id) else { return };
        if let Some(physics) = self.physics.as_deref_mut() {
            physics.set_velocity(body, velocity);
        }
    }

    /// 当前重力。
    pub fn gravity(&mut self) -> Vec3 {
        self.physics.as_deref().map(|p| p.gravity()).unwrap_or(Vec3::ZERO)
    }

    /// 设置重力。
    pub fn set_gravity(&mut self, gravity: Vec3) {
        if let Some(physics) = self.physics.as_deref_mut() {
            physics.set_gravity(gravity);
        }
    }

    // -----------------------------------------------------------------------
    // 输入
    // -----------------------------------------------------------------------

    /// 动作是否按住。
    pub fn action_down(&mut self, action: &str) -> bool {
        self.input.map(|i| i.action_down(action)).unwrap_or(false)
    }

    /// 动作是否**本帧**按下。
    pub fn action_pressed(&mut self, action: &str) -> bool {
        self.input.map(|i| i.action_pressed(action)).unwrap_or(false)
    }

    /// 动作是否**本帧**松开。
    pub fn action_released(&mut self, action: &str) -> bool {
        self.input.map(|i| i.action_released(action)).unwrap_or(false)
    }

    /// 轴的当前值（-1..=1，双向轴）。
    pub fn axis(&mut self, axis: &str) -> f32 {
        self.input.map(|i| i.axis(axis)).unwrap_or(0.0)
    }

    /// 按键是否按住。名字未知时记一条警告 —— 打错键名很难自己发现。
    pub fn key_down(&mut self, key: &str) -> bool {
        let Some(code) = KeyCode::from_name(key) else {
            self.warn(format!("未知按键 '{key}'"));
            return false;
        };
        self.input.map(|i| i.key_down(code)).unwrap_or(false)
    }

    /// 按键是否**本帧**按下（未知名字静默返回 false，避免每帧刷警告）。
    pub fn key_pressed(&mut self, key: &str) -> bool {
        let Some(code) = KeyCode::from_name(key) else {
            return false;
        };
        self.input.map(|i| i.key_pressed(code)).unwrap_or(false)
    }

    /// 鼠标键是否按住。
    pub fn mouse_down(&mut self, button: &str) -> bool {
        let Some(b) = MouseButton::from_name(button) else {
            return false;
        };
        self.input.map(|i| i.mouse_down(b)).unwrap_or(false)
    }

    /// 鼠标键是否**本帧**按下。
    pub fn mouse_pressed(&mut self, button: &str) -> bool {
        let Some(b) = MouseButton::from_name(button) else {
            return false;
        };
        self.input.map(|i| i.mouse_pressed(b)).unwrap_or(false)
    }

    /// 鼠标指针位置（视口像素坐标）。
    pub fn mouse_pos(&mut self) -> Vec2 {
        self.input.map(|i| i.mouse_pos()).unwrap_or(Vec2::ZERO)
    }

    /// 本帧鼠标位移。
    pub fn mouse_delta(&mut self) -> Vec2 {
        self.input.map(|i| i.mouse_delta()).unwrap_or(Vec2::ZERO)
    }

    /// 本帧滚轮位移。
    pub fn scroll(&mut self) -> Vec2 {
        self.input.map(|i| i.scroll()).unwrap_or(Vec2::ZERO)
    }

    // -----------------------------------------------------------------------
    // 动画状态机
    // -----------------------------------------------------------------------

    /// 写本实体状态机的参数（键不存在就创建）。
    ///
    /// 没挂 animator / 没配机器 → **静默 no-op**：脚本照常跑，只是机器不动。
    /// 这让"通用移动脚本挂到任意实体上"成为可能，不需要调用方先探测组件。
    pub fn set_anim_param(&mut self, name: &str, value: f32) {
        if let Some(i) = self.entity_index() {
            if let Some(animator) = self.scene.entities.get_mut(i).and_then(|e| e.animator.as_mut())
            {
                if let Some(machine) = animator.machine.as_mut() {
                    machine.parameters.insert(name.to_owned(), value);
                }
            }
        }
    }

    /// 写本实体叠加层的权重 ∈ [0,1]（引擎侧钳位）。
    ///
    /// 经典用法：受击晃动"淡入再淡出"——平时 weight=0（零成本 no-op），
    /// 挨打瞬间拉到 1 再随时间衰减回去。没配叠加层 → 静默 no-op。
    pub fn set_anim_additive_weight(&mut self, value: f32) {
        if let Some(i) = self.entity_index() {
            if let Some(add) = self
                .scene
                .entities
                .get_mut(i)
                .and_then(|e| e.animator.as_mut())
                .and_then(|a| a.additive.as_mut())
            {
                add.weight = value.clamp(0.0, 1.0);
            }
        }
    }

    /// 写本实体层栈中**按名匹配的层**的权重 ∈ [0,1]（引擎侧钳位）。
    ///
    /// 重名按栈内顺序取第一个命中；层名不存在 / 没挂 animator → 静默 no-op。
    pub fn set_anim_layer_weight(&mut self, layer: &str, value: f32) {
        if let Some(i) = self.entity_index() {
            if let Some(layers) = self
                .scene
                .entities
                .get_mut(i)
                .and_then(|e| e.animator.as_mut())
                .map(|a| &mut a.layers)
            {
                if let Some(l) = layers.iter_mut().find(|l| l.name == layer) {
                    l.weight = value.clamp(0.0, 1.0);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 场景辅助（原 api.rs 的自由函数，随方法层一并平移）
// ---------------------------------------------------------------------------

/// 实体的世界坐标（取世界矩阵的平移分量）。
fn world_position(scene: &Scene, index: usize) -> Vec3 {
    scene.world_matrix(index).w_axis.truncate()
}

/// 实体的世界旋转。
fn world_rotation(scene: &Scene, index: usize) -> Quat {
    let (_, rotation, _) = scene.world_matrix(index).to_scale_rotation_translation();
    rotation
}

/// 设置实体的**世界**位置：有父节点时换算回局部。
///
/// 直接写 `transform.position` 会把世界坐标当成局部坐标用 —— 子物体一动就飞。
fn set_world_position(scene: &mut Scene, index: usize, world_pos: Vec3) {
    let local = match scene.parent_index(index) {
        Some(parent) => {
            let parent_world = scene.world_matrix(parent);
            if parent_world.determinant().abs() > 1e-12 {
                (parent_world.inverse() * world_pos.extend(1.0)).truncate()
            } else {
                world_pos
            }
        }
        None => world_pos,
    };
    if let Some(entity) = scene.entities.get_mut(index) {
        entity.transform.position = local;
    }
}

fn euler_degrees(q: Quat) -> Vec3 {
    let (y, x, z) = q.to_euler(EulerRot::YXZ);
    Vec3::new(x.to_degrees(), y.to_degrees(), z.to_degrees())
}

fn quat_from_degrees(v: Vec3) -> Quat {
    Quat::from_euler(
        EulerRot::YXZ,
        v.y.to_radians(),
        v.x.to_radians(),
        v.z.to_radians(),
    )
}

/// 实体 ID → 物理刚体句柄。
fn body_of(scene: &Scene, id: EntityId) -> Option<u64> {
    let index = scene.find_entity(id)?;
    scene.entities
        .get(index)?
        .component::<PhysicsHandle>()
        .map(|h| h.0)
}

/// 刚体句柄 → 实体 ID。
///
/// 线性扫场景：射线不是每帧每实体都发，O(n) 足够了；真要成了热点，
/// 再让运行时在上下文里带一张反查表。
fn entity_of_body(scene: &Scene, body: u64) -> Option<EntityId> {
    scene
        .entities
        .iter()
        .find(|e| e.component::<PhysicsHandle>().map(|h| h.0) == Some(body))
        .map(|e| e.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 原 api.rs 的回归测试，随函数体一并平移（YXZ 分解往返稳定）。
    #[test]
    fn euler_roundtrip_is_stable() {
        let original = Vec3::new(12.0, 45.0, -30.0);
        let back = euler_degrees(quat_from_degrees(original));
        // YXZ 分解在同一象限内应当还原；浮点误差留一点余量。
        assert!((back.x - original.x).abs() < 1e-2, "{back:?}");
        assert!((back.y - original.y).abs() < 1e-2, "{back:?}");
        assert!((back.z - original.z).abs() < 1e-2, "{back:?}");
    }

    /// 原 api.rs 的回归测试：世界坐标写入要经过父矩阵换算。
    #[test]
    fn setting_world_position_respects_parent() {
        let mut scene = Scene::new("t");
        let parent_id = scene.add_entity(xgame_core::Entity::new("parent"));
        let parent = scene.find_entity(parent_id).unwrap();
        scene.entities[parent].transform.position = Vec3::new(10.0, 0.0, 0.0);

        let child_id = scene.add_entity(xgame_core::Entity::new("child"));
        let child = scene.find_entity(child_id).unwrap();
        scene.set_parent(child, Some(parent));

        set_world_position(&mut scene, child, Vec3::new(12.0, 5.0, 0.0));

        assert_eq!(scene.entities[child].transform.position, Vec3::new(2.0, 5.0, 0.0));
        let world = scene.world_matrix(child).w_axis.truncate();
        assert!((world - Vec3::new(12.0, 5.0, 0.0)).length() < 1e-4);
    }

    /// GameCtx 作为一束状态可以直接构造并调方法 —— 不经过脚本解释器。
    /// 这是"脚本层只是 API 的第一个用户"这条架构原则的最小验证。
    #[test]
    fn game_ctx_drives_an_entity_without_a_script_engine() {
        let mut scene = Scene::new("direct");
        scene.add_entity(Entity::new("actor"));
        let id = scene.entities[0].id;

        let mut logs = Vec::new();
        let mut commands = Vec::new();
        let mut ctx = GameCtx {
            scene: &mut scene,
            physics: None,
            input: None,
            logs: &mut logs,
            commands: &mut commands,
            source: "Spinner",
            entity: id,
            delta: 1.0 / 60.0,
            time: 0.0,
        };

        ctx.set_pos(Vec3::new(1.0, 2.0, 3.0));
        ctx.log("hello from a component");
        assert_eq!(ctx.pos(), Vec3::new(1.0, 2.0, 3.0));

        let spawned = ctx.spawn_ball("ball", 0.5, Vec3::ZERO);
        assert_ne!(spawned.raw(), 0);
        assert_eq!(scene.entities.len(), 2);
        assert_eq!(commands.len(), 1, "生成应当上报一条命令");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].script, "Spinner", "日志归属应当是 source");
    }
}
