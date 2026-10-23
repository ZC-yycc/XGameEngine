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

use xgame_core::{
    same_scene_path, Entity, EntityId, GameClock, PhysicsBody, PhysicsHandle, PlaySound, Scene,
    SceneLoader,
};
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
    /// 音频命令束。宿主没接音频子系统时为 `None` —— 脚本的音频调用
    /// 静默 no-op（与"没接物理时 raycast 返回 miss"同款容错哲学）。
    pub audio: Option<&'a mut xgame_core::AudioPort>,
    /// 日志缓冲（派发方持有，帧末统一回收）。
    pub logs: &'a mut Vec<ScriptLog>,
    /// 结构性改动通知缓冲：场景是上下文改的，刚体是运行时建的，
    /// 靠这份通知让运行时把物理体补上 / 清掉（见 [`ScriptCommand`]）。
    pub commands: &'a mut Vec<ScriptCommand>,
    /// 日志归属名：脚本轨传脚本的项目相对路径，组件轨传行为名。
    pub source: &'a str,
    /// 当前场景的项目相对路径（`""` = 宿主没告诉运行时）。
    pub scene_name: &'a str,
    /// 发布清单里的关卡顺序（`build.scenes`）。空 = 没配 / 宿主没注入。
    pub scene_order: &'a [String],
    /// 资产装载来源（场景 / 预制体）。`None` = 宿主没接装载器：
    /// `load_scene` / `spawn_prefab` 记一条错误日志并返回失败，不静默。
    pub loader: Option<&'a dyn SceneLoader>,
    /// 游戏时钟（时间缩放的唯一真相源）。`None` = 宿主没接：
    /// `set_time_scale` 静默 no-op、`time_scale()` 恒为 1.0。
    pub clock: Option<&'a mut GameClock>,
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

    /// 本帧帧间隔（秒，**已按 `time_scale` 缩放**）。暂停时恒为 0。
    pub fn delta(&self) -> f32 {
        self.delta
    }

    /// 游戏已运行总时长（秒，同样按 `time_scale` 缩放）。
    ///
    /// 与 Unity `Time.time` 一致：暂停时它不增长。暂停菜单里要"照常走"的动画
    /// 得自己用 `delta` 之外的计数（脚本侧 `state_time` 之类的局部状态），
    /// 引擎不另发一份未缩放时钟 —— 那会让"两套时间"在每处调用点都要选一次。
    pub fn time(&self) -> f32 {
        self.time
    }

    /// 当前时间缩放系数。没接时钟时返回 1.0。
    pub fn time_scale(&self) -> f32 {
        self.clock.as_deref().map_or(1.0, GameClock::time_scale)
    }

    /// 设置时间缩放系数（`0` = 暂停模拟）。
    ///
    /// 只有 `GameClock::tick` 一处消费它，所以物理固定步、脚本 `delta` /
    /// `time`、粒子模拟会**同帧同源**地一起变慢；音频与 UI 不受影响
    /// （kira 有独立时钟；暂停菜单必须还能点）——这与 Unity `timeScale`
    /// 的行为一致，见决策记录 §二十八。
    ///
    /// 生效时机是**下一帧**：本帧的固定步循环已经跑完，不可能回头重算。
    pub fn set_time_scale(&mut self, scale: f32) {
        if let Some(clock) = self.clock.as_deref_mut() {
            clock.set_time_scale(scale);
        }
    }

    // -----------------------------------------------------------------------
    // 场景
    // -----------------------------------------------------------------------

    /// 当前场景的项目相对路径（`""` = 宿主没告诉运行时）。
    pub fn current_scene(&self) -> &str {
        self.scene_name
    }

    /// 请求切换到项目内的另一份场景（如 `"Scenes/Level2.ron"`）。
    ///
    /// **不在调用处生效**：本帧剩余的逻辑跑的还是旧场景，切换发生在
    /// `World::update` 的末尾（见 [`ScriptCommand::LoadScene`]）。所以
    /// "请求切换后别再往下写同一个钩子"是脚本作者的责任 —— 新场景的
    /// `start` 在下一帧才跑。
    ///
    /// 路径按**项目相对路径**解释（与场景文件里存资产的写法一致）：
    /// 只写 `"Level2"` 会找不到文件并记一条错误日志 —— 不做"猜扩展名 /
    /// 猜目录"的模糊匹配，那是给智能体和 CI 埋不确定性。
    pub fn load_scene(&mut self, path: &str) {
        self.push_command(ScriptCommand::LoadScene {
            path: path.to_owned(),
        });
    }

    // -- 关卡顺序（`project.ron` 的 `build.scenes`）------------------------

    /// 关卡总数（`0` = 这次运行没有关卡顺序可用）。
    ///
    /// "没配清单"与"清单里没有当前场景"在作者眼里是同一件事 —— 都意味着
    /// 不能靠序号推关卡，只能按路径 `load_scene()`。
    pub fn scene_count(&self) -> usize {
        self.scene_order.len()
    }

    /// 当前场景在关卡顺序里的下标（`None` = 不在清单里）。
    ///
    /// 比较时**两边都做路径规范化**：清单里可能写成 `Scenes\\Level1.ron`，
    /// 而运行时记的是 `Scenes/Level1.ron`。不一致会让"同一关"变成两关，
    /// 症状是 `scene_index()` 突然返回 nil（而不是报错），最难查。
    pub fn scene_index(&self) -> Option<usize> {
        self.scene_order
            .iter()
            .position(|path| same_scene_path(path, self.scene_name))
    }

    /// 第 `index` 关的项目相对路径（越界返回 `None`）。
    pub fn scene_path_at(&self, index: usize) -> Option<&str> {
        self.scene_order.get(index).map(String::as_str)
    }

    /// 切到关卡顺序里的第 `index` 关（0 起）。
    ///
    /// 越界记一条 **Error** 而不是静默 no-op：脚本算错索引（比如最后一关
    /// `index + 1`）是常见 bug，静默会让它表现为"卡在关底不动"，而一行日志
    /// 直接指出来。与 `load_scene` 一样只登记请求，帧末才真的换。
    pub fn load_scene_at(&mut self, index: usize) {
        match self.scene_path_at(index) {
            Some(path) => {
                let path = path.to_owned();
                self.load_scene(&path);
            }
            None => {
                let count = self.scene_count();
                let msg = if count == 0 {
                    format!(
                        "load_scene_at({index}) 失败：这次运行没有关卡顺序（project.ron 的 build.scenes 为空或宿主没注入）"
                    )
                } else {
                    format!("load_scene_at({index}) 越界：清单里只有 {count} 关（下标 0..{count}）")
                };
                self.error(msg);
            }
        }
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
    // 音频
    // -----------------------------------------------------------------------

    /// 播放一次音效（非空间化）。
    ///
    /// `volume` 是线性音量（`0.0..=1.0`），`pitch` 是播放速率（`1.0` = 原速）。
    /// 命令进 `AudioPort`，World 帧末结算 —— 脚本层不碰 kira（见 `context.rs`）。
    pub fn play_sound(&mut self, clip: &str, volume: f32, pitch: f32) {
        if let Some(audio) = &mut self.audio {
            audio.play(PlaySound::new(clip).with_volume(volume).with_pitch(pitch));
        }
    }

    /// 在世界坐标播放一次空间化音效（固定位置，不跟随实体）。
    pub fn play_sound_at(&mut self, clip: &str, x: f32, y: f32, z: f32, volume: f32) {
        if let Some(audio) = &mut self.audio {
            audio.play(PlaySound::new(clip).at([x, y, z]).with_volume(volume));
        }
    }

    /// 停止某 clip 的全部实例。
    pub fn stop_sound(&mut self, clip: &str) {
        if let Some(audio) = &mut self.audio {
            audio.stop_clip(clip);
        }
    }

    /// 停止一切声音。
    pub fn stop_all_sounds(&mut self) {
        if let Some(audio) = &mut self.audio {
            audio.stop_all();
        }
    }

    /// 设置 3D 监听者（玩家的耳朵）：位置 + 朝向，up 恒为世界 +Y。
    pub fn set_listener(&mut self, x: f32, y: f32, z: f32, fx: f32, fy: f32, fz: f32) {
        if let Some(audio) = &mut self.audio {
            audio.set_listener([x, y, z], [fx, fy, fz]);
        }
    }

    /// 设置主音量（线性 `0.0..=1.0`，超界被 clamp）。
    pub fn set_master_volume(&mut self, volume: f32) {
        if let Some(audio) = &mut self.audio {
            audio.set_master_volume(volume);
        }
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
    // 相机
    // -----------------------------------------------------------------------

    /// 让自身的本地方向 **+Z** 指向世界坐标 `target`（"看向哪"）。
    ///
    /// 只改旋转、不动位置：跟随相机是"先摆位置、再看向目标"两步，合成一个
    /// 方法会让每帧都被拽到目标旁边。
    ///
    /// 世界朝向 → 局部旋转的换算带着父节点（肩视角相机通常挂在角色下面，
    /// 父节点自己转了，子节点的局部旋转要抵消掉那部分）。
    ///
    /// 退化情形一律安全：`target` 与自身重合、坐标含 NaN、实体不存在 ——
    /// 都是"什么都不做"，不会写出 NaN 四元数（写进去之后每帧渲染都会坏）。
    pub fn look_at(&mut self, target: Vec3) {
        let Some(i) = self.entity_index() else {
            return;
        };
        if !target.is_finite() {
            return;
        }
        let eye = world_position(self.scene, i);
        let desired = xgame_core::look_rotation(target - eye, Vec3::Y);
        if !desired.is_finite() {
            return;
        }
        let local = match self.scene.entities[i]
            .parent
            .and_then(|p| self.scene.index_of(p))
        {
            Some(pi) => world_rotation(self.scene, pi).inverse() * desired,
            None => desired,
        };
        if let Some(e) = self.scene.entities.get_mut(i) {
            e.transform.rotation = local.normalize();
        }
    }

    /// 让自身看向另一个实体（世界坐标）。
    ///
    /// 目标不存在时静默 no-op —— 与其它"容错优先"的 API 一致（脚本里
    /// `look_at_entity(ctx, entity_id(ctx, "Player"))` 找不到人时不该报错）。
    pub fn look_at_entity(&mut self, target: EntityId) {
        let Some(ti) = self.scene.find_entity(target) else {
            return;
        };
        let position = world_position(self.scene, ti);
        self.look_at(position);
    }

    /// 当前**主相机**的实体 id（没有相机实体 → `None`）。
    ///
    /// 选择规则与宿主完全一致（`Scene::active_camera_index`），所以脚本里
    /// "主相机是谁"和画面上"谁在拍"永远是同一个答案。
    pub fn active_camera(&mut self) -> Option<EntityId> {
        self.scene.active_camera().map(|e| e.id)
    }

    /// 把 `id` 设为**唯一**主相机（其它相机全部置为 inactive）。
    ///
    /// 为什么是"独占式"而不是单纯置 `active = true`：解析规则是"第一个
    /// active 的相机"，只置 true 的话第二台永远轮不到 —— 作者会以为
    /// 切换 API 坏了。想同时留多台候选机位，就在切回来时再调一次。
    pub fn set_active_camera(&mut self, id: EntityId) {
        // 先确认目标真的是相机：把 id 写错成普通实体时不该把全场相机都关掉。
        let Some(target) = self.scene.find_entity(id) else {
            return;
        };
        if self.scene.entities[target].camera.is_none() {
            return;
        }
        for entity in &mut self.scene.entities {
            if let Some(camera) = entity.camera.as_mut() {
                camera.active = entity.id == id;
            }
        }
    }

    /// 相机的垂直视场角（度）。不是相机 / 实体不存在 → `None`。
    pub fn camera_fov(&mut self, id: EntityId) -> Option<f32> {
        let i = self.scene.find_entity(id)?;
        self.scene.entities[i].camera.as_ref().map(|c| c.fov)
    }

    /// 改相机的垂直视场角（度）。不是相机时静默 no-op。
    ///
    /// 只钳到 1..179（写入时的坏值会让投影矩阵退化），不做"变焦动画"——
    /// 那是脚本自己的事（按 `delta` 插值）。
    pub fn set_camera_fov(&mut self, id: EntityId, degrees: f32) {
        if !degrees.is_finite() {
            return;
        }
        if let Some(i) = self.scene.find_entity(id) {
            if let Some(camera) = self.scene.entities[i].camera.as_mut() {
                camera.fov = degrees.clamp(1.0, 179.0);
            }
        }
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

    /// 实体引用的共享材质资产路径（没引用 → 空串）。
    pub fn entity_material_asset(&mut self, id: EntityId) -> String {
        self.scene
            .find_entity(id)
            .and_then(|i| self.scene.entities.get(i))
            .and_then(|e| e.material_asset.clone())
            .unwrap_or_default()
    }

    /// 指向 / 清空实体的共享材质资产（`""` = 恢复模型自带材质）。
    ///
    /// 只接受项目相对路径或 `guid://`；路径不存在不在这里报错 ——
    /// 渲染侧会退回模型材质，`--check` 会点名悬空引用。
    pub fn set_entity_material_asset(&mut self, id: EntityId, material: &str) {
        if let Some(i) = self.scene.find_entity(id) {
            if let Some(e) = self.scene.entities.get_mut(i) {
                e.material_asset = (!material.is_empty()).then(|| material.to_owned());
            }
        }
    }

    /// 一次性设置实体的内联材质覆盖（颜色 + 金属度 + 粗糙度）。
    ///
    /// 这是给脚本的**便捷入口**：`set_material(id, r,g,b,a, metallic, roughness)`。
    /// 三个字段都会被写上（不像 Inspector 那样逐字段勾选）；要把控制权还给
    /// 模型自带材质，用 `clear_material` 或改用 `set_material_asset`。
    /// 与共享资产同时存在时，覆盖在资产结果之上继续生效（本函数不改资产）。
    pub fn set_entity_material(
        &mut self,
        id: EntityId,
        base_color: [f32; 4],
        metallic: f32,
        roughness: f32,
    ) {
        let Some(i) = self.scene.find_entity(id) else {
            return;
        };
        let Some(e) = self.scene.entities.get_mut(i) else {
            return;
        };
        let finite = |v: f32, fallback: f32| if v.is_finite() { v } else { fallback };
        let color = base_color.map(|v| finite(v, 1.0));
        e.material = Some(xgame_core::MaterialOverride {
            base_color: Some(color),
            metallic: Some(finite(metallic, 0.0).clamp(0.0, 1.0)),
            roughness: Some(finite(roughness, 0.5).clamp(0.0, 1.0)),
        });
    }

    /// 清掉内联材质覆盖（共享材质资产与模型材质不受影响）。
    pub fn clear_entity_material(&mut self, id: EntityId) {
        if let Some(i) = self.scene.find_entity(id) {
            if let Some(e) = self.scene.entities.get_mut(i) {
                e.material = None;
            }
        }
    }

    // -----------------------------------------------------------------------
    // 粒子（运行态动作；配置本身在场景文件 / Inspector 里）
    // -----------------------------------------------------------------------

    /// 实体是否挂着粒子发射器且在发射（没挂 = `false`）。
    pub fn entity_particle_emitting(&mut self, id: EntityId) -> bool {
        self.scene
            .find_entity(id)
            .and_then(|i| self.scene.entities.get(i))
            .and_then(|e| e.particle.as_ref())
            .is_some_and(|p| p.emitting)
    }

    /// 开 / 关持续发射。关掉后**已出生的粒子照常走完寿命**（不是清屏）——
    /// 想立刻消失得另加"清空"语义，而那个语义应该由 Kill 计时器之类的
    /// 游戏逻辑自己做，引擎不替作者猜。
    pub fn set_entity_particle_emitting(&mut self, id: EntityId, emitting: bool) {
        if let Some(i) = self.scene.find_entity(id) {
            if let Some(e) = self.scene.entities.get_mut(i) {
                if let Some(p) = e.particle.as_mut() {
                    p.emitting = emitting;
                }
            }
        }
    }

    /// 一次爆发：同一帧多出 `count` 颗粒子（超出上限的部分被丢弃）。
    ///
    /// 走组件上的 `burst` 请求位（`#[serde(skip)]` 的运行态字段）：模拟层
    /// 消费后清零，所以它是"一次性"的 —— 连按两帧就是两次爆发，不需要
    /// 脚本侧自己记状态。没挂发射器时静默 no-op。
    pub fn entity_particle_burst(&mut self, id: EntityId, count: u32) {
        if let Some(i) = self.scene.find_entity(id) {
            if let Some(e) = self.scene.entities.get_mut(i) {
                if let Some(p) = e.particle.as_mut() {
                    p.burst = p.burst.saturating_add(count);
                }
            }
        }
    }

    /// 改发射速率（颗/秒）。`0` = 只靠 burst 出粒子。
    ///
    ///  clamp 到 `[0, 1e5]`：脚本算错（比如把帧数当速率传进来）时不该把
    /// 模拟层打到卡死，钳一下还有救。
    pub fn set_entity_particle_rate(&mut self, id: EntityId, rate: f32) {
        if let Some(i) = self.scene.find_entity(id) {
            if let Some(e) = self.scene.entities.get_mut(i) {
                if let Some(p) = e.particle.as_mut() {
                    p.rate = if rate.is_finite() {
                        rate.clamp(0.0, 1.0e5)
                    } else {
                        0.0
                    };
                }
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

    /// 另一个实体的世界朝向 **+Z**（目标不存在 → `Vec3::Z`）。
    ///
    /// 与自身版 `forward` 同款，只是换成别的实体：跟随相机要"绕到角色背后"、
    /// AI 要判断"它是不是背对我"，都得先拿到别人的朝向。
    pub fn entity_forward(&mut self, id: EntityId) -> Vec3 {
        self.scene
            .find_entity(id)
            .map(|i| world_rotation(self.scene, i).mul_vec3(Vec3::Z))
            .unwrap_or(Vec3::Z)
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

    /// 把预制体资产**实例化**进当前场景，返回实例根的实体 ID（失败 = `None`）。
    ///
    /// `position` 覆盖根的**世界**位置（Unity `Instantiate(prefab, pos, rot)` 的
    /// 语义）：预制体里根节点自己写的位置被入参顶掉，**子节点原样保留** ——
    /// 一棵"炮口在 (0,1,0)、炮身在世界原点"的预制体，实例化到 (10,0,0) 之后
    /// 炮口就在 (10,1,0)。
    ///
    /// 与 `spawn_entity` 的关系：那条生成的是**一个空壳实体**（给个名字 + 模型），
    /// 这条生成的是**一整棵子树**（预制体资产里有什么就有什么：层级 / 变换 /
    /// 物理 / 粒子 / 音频 / 相机 / UI 全带上），并且每个新实体都记着
    /// `PrefabInstanceRef`（编辑器 Play 里的 Prefab 面板仍然认得出它）。
    ///
    /// # 为什么是"立即"而不是延后
    ///
    /// 与 [`spawn_request`](Self::spawn_request) 同一条理由：
    /// `local e = spawn_prefab(...); set_entity_pos(e, ...)` 是最自然的写法，
    /// 延后会让第二句对着一个还不存在的实体静默失效。
    ///
    /// # 失败是三种，日志分开说
    ///
    /// 1. 宿主没注入装载器（`loader == None`）—— 接线问题，测试 / 自建宿主常见；
    /// 2. 文件读不出来 / RON 解析失败 —— 路径写错或资产坏了；
    /// 3. 预制体是空的 —— 不是错误，但脚本通常也不想看到"什么都没发生"。
    pub fn spawn_prefab(&mut self, path: &str, position: Vec3) -> Option<EntityId> {
        self.spawn_prefab_request(path, None, position)
    }

    /// 同 [`spawn_prefab`](Self::spawn_prefab)，但把实例根**挂到** `parent` 底下。
    ///
    /// `offset` 是**局部**位置（相对父节点）：挂在玩家手上的子弹写成
    /// `spawn_prefab_under("Prefabs/Bullet.prefab.ron", player, 0, 1.5, 0)`，
    /// 之后子弹会跟着玩家走（父节点带动子节点），不需要每帧自己追。
    ///
    /// 父节点在当前场景里找不到时**退回世界根**并记一条警告：把一颗子弹的
    /// 父引用写死成"挂在某个可能已被销毁的实体上"，静默丢弃整次生成更糟。
    pub fn spawn_prefab_under(
        &mut self,
        path: &str,
        parent: EntityId,
        offset: Vec3,
    ) -> Option<EntityId> {
        let parent_index = self.scene.find_entity(parent);
        if parent_index.is_none() {
            self.push_log(
                LogLevel::Warn,
                format!(
                    "spawn_prefab_under('{path}') 的父实体 {} 不在当前场景里，实例挂到场景根",
                    parent.raw()
                ),
            );
        }
        self.spawn_prefab_request(path, parent_index, offset)
    }

    fn spawn_prefab_request(
        &mut self,
        path: &str,
        parent: Option<usize>,
        position: Vec3,
    ) -> Option<EntityId> {
        // 1. 装载器：宿主的接线问题，日志要说清"去哪接"。
        let Some(loader) = self.loader else {
            self.push_log(
                LogLevel::Error,
                format!(
                    "spawn_prefab('{path}') 失败：宿主没有接入资产装载器（World 需要 with_scene_loader）"
                ),
            );
            return None;
        };

        // 2. 读盘：路径写错 / 资产坏了都走这里，错误文案由装载器给出（含绝对路径）。
        let prefab = match loader.load_prefab(path) {
            Ok(prefab) => prefab,
            Err(reason) => {
                self.push_log(
                    LogLevel::Error,
                    format!("spawn_prefab('{path}') 失败：{reason}"),
                );
                return None;
            }
        };

        // 3. 实例化：`instantiate_prefab` 自己会分配新 ID、重映射内部父引用、
        //    贴 `PrefabInstanceRef`。空预制体返回 None —— 不是故障，但作者
        //    通常想被告知，所以记 warn。
        // 先记下"追加前有多少实体"：新增块就是这一段，收集新实体命令时
        //    不能反推（实例根在块内的偏移没有保证）。
        let base = self.scene.entities.len();
        let Some(root) = self.scene.instantiate_prefab(&prefab, path) else {
            self.push_log(
                LogLevel::Warn,
                format!("spawn_prefab('{path}')：预制体是空的，什么都没生成"),
            );
            return None;
        };
        let root_id = self.scene.entities[root].id;
        self.scene.entities[root].transform.position = position;
        if let Some(parent_index) = parent {
            // `set_parent` 自己会拒绝成环，返回 false 时不要假装成功
            // （实例根不可能成环 —— 它是刚加进来的，但别依赖这个巧合）。
            if !self.scene.set_parent(root, Some(parent_index)) {
                self.push_log(
                    LogLevel::Warn,
                    format!("spawn_prefab_under('{path}')：挂父节点被拒绝（会成环？），实例留在场景根"),
                );
            }
        }

        // 4. 通知运行时补刚体：每个新实体一条，与 `spawn_request` 走同一条命令
        //    路径。父引用此刻已在场景里，命令被消费时刚体建在世界正确位置。
        let spawned: Vec<(EntityId, Option<PhysicsBody>)> = self.scene.entities[base..]
            .iter()
            .map(|e| (e.id, e.physics))
            .collect();
        for (id, physics) in spawned {
            self.push_command(ScriptCommand::EntitySpawned { id, physics });
        }
        Some(root_id)
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
    // 角色控制器
    // -----------------------------------------------------------------------

    /// 角色着地时的"贴地速度"（米/秒，向下）。
    ///
    /// **必须是 0**，这不是省事而是必需：`move_shape` 的第一次扫掠如果带着
    /// 向下的分量，会先撞到**地面**，而 rapier 的自动上台阶只在"撞到墙"时
    /// 才尝试 —— 剩下的位移被地面吃掉，0.2m 的台阶会把角色焊死在原地
    /// （实测锯齿抖动但永远上不去）。贴地交给 `snap_to_ground`（走下矮台阶）
    /// 与坡面切向投影（走坡）就够了，不需要额外向下推。
    pub const GROUND_STICK_SPEED: f32 = 0.0;

    /// 目标实体的角色控制器参数；不是角色返回 `None`。
    pub fn character_config(&mut self, id: EntityId) -> Option<xgame_core::CharacterController> {
        let index = self.scene.find_entity(id)?;
        self.scene.entities.get(index)?.controller
    }

    /// 当前实体的角色运行时状态（没配控制器 / 还没移动过 → 默认值）。
    ///
    /// 返回**拷贝**：状态表在 `ComponentMap` 里，交出去引用会卡住后续的
    /// `character_*` 调用（它们都要 `&mut self`）。
    pub fn character_state(&mut self) -> xgame_core::CharacterState {
        let Some(index) = self.entity_index() else {
            return xgame_core::CharacterState::default();
        };
        self.scene.entities[index]
            .component::<xgame_core::CharacterState>()
            .copied()
            .unwrap_or_default()
    }

    /// Unity `CharacterController.Move`：按**位移**（米）移动角色，不施加重力。
    ///
    /// 竖直速度完全由调用方负责（想做跳跃就自己积分重力再一起传进来）。
    /// 物理层会解析碰撞、坡面、台阶与贴地吸附，返回的实际位移反推成
    /// [`character_velocity`](Self::character_velocity)。
    ///
    /// 返回 `false` 表示这个实体没配控制器、没有物理刚体，或没有物理世界
    /// —— 与其它 API 同款的静默容错。
    pub fn character_move(&mut self, displacement: Vec3) -> bool {
        self.move_character_internal(displacement, true).is_some()
    }

    /// Unity `CharacterController.SimpleMove`：给**水平速度**（米/秒），
    /// 重力与坡面跟随由引擎负责。
    ///
    /// `planar_velocity` 的 y 分量会被忽略 —— 竖直方向由引擎算（这是
    /// "Simple" 的全部含义）。着地时角色被贴地速度轻轻压住，腾空时按
    /// 物理世界的重力积分；落在坡上时竖直速度按坡面跟随率反推，且
    /// **只取向下分量**（上坡的爬升由物理层沿坡切向完成，这里再叠加一次
    /// 就是双倍爬升）。
    pub fn character_simple_move(&mut self, planar_velocity: Vec3) -> bool {
        let Some(index) = self.entity_index() else {
            return false;
        };
        let Some(def) = self.scene.entities.get(index).and_then(|e| e.controller) else {
            return false;
        };
        let c = def.sanitized();
        let dt = self.delta.max(1e-5);
        let gravity = self
            .physics
            .as_deref()
            .map(|p| p.gravity())
            .unwrap_or(Vec3::ZERO);

        let mut state = self.scene.entities[index]
            .component::<xgame_core::CharacterState>()
            .copied()
            .unwrap_or_default();

        // 竖直速度：着地时给贴地速度，腾空时积分重力。
        if state.grounded {
            state.velocity.y = Self::GROUND_STICK_SPEED;
        } else {
            state.velocity.y += gravity.y * dt;
        }

        // 陡坡禁行（Unity 语义）：站在超过 slope_limit 的坡上时不给水平期望，
        // 只留竖直分量让物理层把角色沿坡面分解成下滑。用**上一帧**的法线判断
        // —— 这一帧的地面还没测出来，而"已经站在陡坡上"本来就该由上一帧得知。
        let n = state.ground_normal;
        let too_steep = state.grounded && n.dot(Vec3::Y) < c.slope_limit_cos();
        let planar = if too_steep {
            Vec3::ZERO
        } else {
            Vec3::new(planar_velocity.x, 0.0, planar_velocity.z)
        };

        let desired = Vec3::new(planar.x, state.velocity.y, planar.z) * dt;
        let Some(r) = self.move_character_internal(desired, false) else {
            return false;
        };

        // 速度从**实际**位移反推：撞墙后水平分量归零、上坡被压短，动画混合
        // 才不会对着墙还在跑步。
        state.velocity.x = r.effective_translation.x / dt;
        state.velocity.z = r.effective_translation.z / dt;
        if r.grounded {
            let gn = r.ground_normal;
            // 坡面跟随率：水平速度在坡面上的竖直分量。只取 ≤0，
            // 上坡交给物理层的切向投影。
            let follow = -(gn.x * state.velocity.x + gn.z * state.velocity.z) / gn.y.max(1e-4);
            state.velocity.y = follow.min(0.0);
        } else {
            state.velocity.y = r.effective_translation.y / dt;
        }

        // 起跳当帧不被吃速度，落地当帧也不该把手里的跳跃初速清零 ——
        // `follow.min(0.0)` 与这里的 `effective/dt` 共同保证这一点。
        let s = self.scene.entities[index]
            .components
            .get_or_insert_with(xgame_core::CharacterState::default);
        s.velocity = state.velocity;
        s.grounded = r.grounded;
        s.ground_normal = r.ground_normal;
        s.sliding_down_slope = r.sliding_down_slope;
        s.contacts = r.contacts;
        true
    }

    /// 起跳：着地时把竖直速度设为 `speed`（米/秒，向上）。
    ///
    /// 返回 `false` 表示没配控制器，或此刻不在地面上（不能二段跳）。
    /// 起跳只是**设速度**，真正离地发生在下一次 `character_simple_move`：
    /// 位移由物理层解析，所以"起跳瞬间撞到天花板"依然会被正确处理。
    pub fn character_jump(&mut self, speed: f32) -> bool {
        let Some(index) = self.entity_index() else {
            return false;
        };
        if self.scene.entities[index].controller.is_none() {
            return false;
        }
        let state = self.scene.entities[index]
            .components
            .get_or_insert_with(xgame_core::CharacterState::default);
        if !state.grounded {
            return false;
        }
        state.velocity.y = speed.max(0.0);
        state.grounded = false;
        true
    }

    /// 角色当前速度（米/秒）。不是角色返回零向量。
    pub fn character_velocity(&mut self) -> Vec3 {
        self.character_state().velocity
    }

    /// 直接改角色速度（被击退、被传送带推动等外部控制用）。
    pub fn character_set_velocity(&mut self, velocity: Vec3) {
        let Some(index) = self.entity_index() else { return };
        let state = self.scene.entities[index]
            .components
            .get_or_insert_with(xgame_core::CharacterState::default);
        state.velocity = velocity;
    }

    /// 角色是否着地（最近一次移动结束后）。
    pub fn character_is_grounded(&mut self) -> bool {
        self.character_state().grounded
    }

    /// 角色脚下的地面法线（腾空时为 `Vec3::Y`）。
    pub fn character_ground_normal(&mut self) -> Vec3 {
        self.character_state().ground_normal
    }

    /// 角色是否正在陡坡上自动下滑。
    pub fn character_is_sliding(&mut self) -> bool {
        self.character_state().sliding_down_slope
    }

    /// `character_move` / `character_simple_move` 的共同主体。
    ///
    /// `update_velocity` 为 `true` 时把实际位移反推成速度；`simple_move`
    /// 传 `false`（它要在落地/坡面语义上再修一次竖直分量）。
    fn move_character_internal(
        &mut self,
        desired: Vec3,
        update_velocity: bool,
    ) -> Option<xgame_physics::CharacterMoveResult> {
        let index = self.entity_index()?;
        let entity = self.scene.entities.get(index)?;
        let def = entity.controller?;
        let body = entity.component::<PhysicsHandle>()?.0;
        let dt = self.delta.max(1e-5);

        // 字段级拆借：`physics` 与 `scene` 是 GameCtx 的两个不同字段。
        let physics = self.physics.as_deref_mut()?;
        let r = physics.move_character(body, &def, desired, dt)?;
        // 刚体原点是**胶囊中心**，实体原点是**脚底**：写回时要减掉中心高度。
        let center = physics.body_transform(body)?.position;
        let feet = center - Vec3::Y * def.sanitized().center_height();

        // 物理在世界空间工作，实体存局部变换：由 Scene 负责换算（根节点零成本）。
        self.scene.set_world_position(index, feet);

        let state = self.scene.entities[index]
            .components
            .get_or_insert_with(xgame_core::CharacterState::default);
        if update_velocity {
            state.velocity = Vec3::new(
                r.effective_translation.x / dt,
                r.effective_translation.y / dt,
                r.effective_translation.z / dt,
            );
        }
        state.grounded = r.grounded;
        state.ground_normal = r.ground_normal;
        state.sliding_down_slope = r.sliding_down_slope;
        state.contacts = r.contacts;
        Some(r)
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
    // 动画：基础层直接控制 + 状态机 + 叠加层 + 层栈
    // -----------------------------------------------------------------------

    /// 本实体的 animator（没挂 = `None`）。
    ///
    /// 新方法共用这一处借用，避免每个 setter 重复三行 `entity_index` +
    /// `get_mut` + `as_mut`。老方法保留原样，不为一处小重构扩大改动面。
    fn animator_mut(&mut self) -> Option<&mut xgame_core::Animator> {
        let index = self.entity_index()?;
        self.scene.entities.get_mut(index)?.animator.as_mut()
    }

    /// 切换**基础层**剪辑（直接控制，不经状态机）。
    ///
    /// 与编辑器里改 `Animator.clip` 走同一条路径：渲染侧 `AnimationDriver`
    /// 下一帧发现剪辑名变了，就从旧剪辑交叉淡入（时长 = `fade_time`）。
    ///
    /// **状态机优先**：配了可用机器（`Animator.machine` 的 entry 指得到状态）时，
    /// 基础剪辑由状态表决定，这里写入的 `clip` 不会生效 —— 与 Inspector 的语义
    /// 一致。要直接控制，先移除机器配置；状态机驱动的切换用 `set_anim_param`。
    ///
    /// 不改变 `autoplay` / `speed` / `looping`：只想换剪辑的调用方不该被悄悄
    /// 改掉播放参数（`autoplay = false` 时切完仍停在第一帧）。
    pub fn set_anim_clip(&mut self, clip: &str) {
        if let Some(animator) = self.animator_mut() {
            animator.clip = clip.to_owned();
        }
    }

    /// 基础层播放速度（`1.0` = 原速，`0` = 冻结）。
    ///
    /// 非有限值忽略；钳在 `[0, 100]` —— 脚本算错时不该把时钟推成 `inf`。
    /// 状态机模式下由状态表覆盖，见 [`Self::set_anim_clip`]。
    pub fn set_anim_speed(&mut self, speed: f32) {
        if !speed.is_finite() {
            return;
        }
        if let Some(animator) = self.animator_mut() {
            animator.speed = speed.clamp(0.0, 100.0);
        }
    }

    /// 基础层是否循环（`false` = 播到末尾停在最后一帧）。
    ///
    /// 状态机模式下由状态表覆盖，见 [`Self::set_anim_clip`]。
    pub fn set_anim_looping(&mut self, looping: bool) {
        if let Some(animator) = self.animator_mut() {
            animator.looping = looping;
        }
    }

    /// 换剪辑的交叉淡入时长（秒，`0` = 硬切）。
    ///
    /// 非有限值忽略；钳在 `[0, 10]`。它同时管状态机换状态的过渡（v1 全局一条），
    /// 与 Inspector / RON 里的 `fade_time` 是同一个字段。
    pub fn set_anim_fade_time(&mut self, seconds: f32) {
        if !seconds.is_finite() {
            return;
        }
        if let Some(animator) = self.animator_mut() {
            animator.fade_time = seconds.clamp(0.0, 10.0);
        }
    }

    /// 切换**叠加层**剪辑（增量层）。
    ///
    /// 没配叠加层 → 静默 no-op（与 `set_anim_additive_weight` 同款容错）。
    /// 叠加层换剪辑是**硬切**（没有交叉淡入）；想平滑就先 `set_anim_additive_weight`
    /// 拉到 0、换好再拉回。权重 / 速度 / 循环不变。
    pub fn set_anim_additive_clip(&mut self, clip: &str) {
        if let Some(add) = self.animator_mut().and_then(|a| a.additive.as_mut()) {
            add.clip = clip.to_owned();
        }
    }

    /// 按层名切换**层栈**里的剪辑。
    ///
    /// 重名按栈内顺序取第一个命中；层名不存在 / 没挂 animator → 静默 no-op。
    /// 与叠加层同款：换层剪辑是硬切，权重 / 速度 / 循环 / 遮罩不变。
    pub fn set_anim_layer_clip(&mut self, layer: &str, clip: &str) {
        if let Some(layers) = self.animator_mut().map(|a| &mut a.layers) {
            if let Some(l) = layers.iter_mut().find(|l| l.name == layer) {
                l.clip = clip.to_owned();
            }
        }
    }

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
pub(crate) mod tests {
    use super::*;
    // `Prefab` / `Transform` 只在测试里用（造预制体模板），不放顶层 import。
    use xgame_core::{Prefab, Transform};

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

    /// 材质 API：资产引用与内联覆盖是两层，互不取消；坏数值被钳到合法区间。
    #[test]
    fn material_api_assigns_asset_and_clamps_override() {
        let mut scene = Scene::new("materials");
        scene.add_entity(Entity::new("crate"));
        let id = scene.entities[0].id;
        let mut logs = Vec::new();
        let mut commands = Vec::new();

        {
            let mut ctx = GameCtx {
                scene: &mut scene,
                physics: None,
                input: None,
                audio: None,
                logs: &mut logs,
                commands: &mut commands,
                source: "MaterialProbe",
                scene_name: "",
                scene_order: &[],
                loader: None,
                clock: None,
                entity: id,
                delta: 1.0 / 60.0,
                time: 0.0,
            };
            assert_eq!(ctx.entity_material_asset(id), "");
            ctx.set_entity_material_asset(id, "Resources/Materials/Wood.mat");
            assert_eq!(
                ctx.entity_material_asset(id),
                "Resources/Materials/Wood.mat"
            );

            ctx.set_entity_material(
                id,
                [0.8, f32::NAN, f32::INFINITY, 1.0],
                5.0,
                -2.0,
            );
        }

        let entity = &scene.entities[0];
        assert_eq!(
            entity.material_asset.as_deref(),
            Some("Resources/Materials/Wood.mat"),
            "设置覆盖不该清掉资产引用"
        );
        let material = entity.material.as_ref().expect("覆盖应写入");
        assert_eq!(
            material.base_color,
            Some([0.8, 1.0, 1.0, 1.0]),
            "非有限颜色分量退回 1.0"
        );
        assert_eq!(material.metallic, Some(1.0), "超界金属度必须钳到 1");
        assert_eq!(material.roughness, Some(0.0), "负粗糙度必须钳到 0");

        let mut logs = Vec::new();
        let mut commands = Vec::new();
        {
            let mut ctx = GameCtx {
                scene: &mut scene,
                physics: None,
                input: None,
                audio: None,
                logs: &mut logs,
                commands: &mut commands,
                source: "MaterialProbe",
                scene_name: "",
                scene_order: &[],
                loader: None,
                clock: None,
                entity: id,
                delta: 1.0 / 60.0,
                time: 0.0,
            };
            ctx.clear_entity_material(id);
            ctx.set_entity_material_asset(id, "");
        }
        assert!(scene.entities[0].material.is_none());
        assert!(scene.entities[0].material_asset.is_none(), "空串 = 清空引用");
    }

    /// 动画 setter：只写自己的 animator；越界 / 非有限值被钳住或忽略；
    /// 没挂 animator / 层名不存在时静默 no-op。
    #[test]
    fn animation_setters_clamp_and_apply_to_the_owning_entity_only() {
        use xgame_core::{AnimAdditive, AnimLayer, Animator};

        let mut scene = Scene::new("anim");
        let mut owner_animator = Animator::new("Walk");
        owner_animator.additive = Some(AnimAdditive {
            clip: "Walk".into(),
            ..Default::default()
        });
        owner_animator.layers.push(AnimLayer {
            name: "upper".into(),
            clip: "Walk".into(),
            ..Default::default()
        });
        let owner = scene.add_entity(Entity::new("owner").with_animator(owner_animator));
        scene.add_entity(Entity::new("other").with_animator(Animator::new("Walk")));
        let plain = scene.add_entity(Entity::new("plain"));

        let mut logs = Vec::new();
        let mut commands = Vec::new();
        {
            let mut ctx = GameCtx {
                scene: &mut scene,
                physics: None,
                input: None,
                audio: None,
                logs: &mut logs,
                commands: &mut commands,
                source: "AnimProbe",
                scene_name: "",
                scene_order: &[],
                loader: None,
                clock: None,
                entity: owner,
                delta: 1.0 / 60.0,
                time: 0.0,
            };

            ctx.set_anim_clip("Run");
            ctx.set_anim_speed(f32::NAN); // 非有限值：忽略，保持原值
            assert_eq!(ctx.scene.entities[0].animator.as_ref().unwrap().speed, 1.0);
            ctx.set_anim_speed(-2.0);
            assert_eq!(ctx.scene.entities[0].animator.as_ref().unwrap().speed, 0.0);
            ctx.set_anim_speed(999.0);
            assert_eq!(ctx.scene.entities[0].animator.as_ref().unwrap().speed, 100.0);
            ctx.set_anim_looping(false);
            ctx.set_anim_fade_time(f32::INFINITY); // 非有限值：忽略
            assert_eq!(
                ctx.scene.entities[0].animator.as_ref().unwrap().fade_time,
                0.25
            );
            ctx.set_anim_fade_time(-1.0);
            ctx.set_anim_additive_clip("Survey");
            ctx.set_anim_layer_clip("upper", "Run");
            ctx.set_anim_layer_clip("missing", "Nope"); // 层名不存在：no-op
        }

        let owner_animator = scene.entities[0].animator.as_ref().unwrap();
        assert_eq!(owner_animator.clip, "Run");
        assert_eq!(owner_animator.speed, 100.0);
        assert!(!owner_animator.looping);
        assert_eq!(owner_animator.fade_time, 0.0);
        assert_eq!(owner_animator.additive.as_ref().unwrap().clip, "Survey");
        assert_eq!(owner_animator.layers[0].clip, "Run");

        let other_animator = scene.entities[1].animator.as_ref().unwrap();
        assert_eq!(other_animator.clip, "Walk", "不能改到别的实体");
        assert_eq!(other_animator.speed, 1.0);
        assert!(other_animator.looping);
        assert_eq!(other_animator.fade_time, 0.25);
        assert!(other_animator.additive.is_none());
        assert!(other_animator.layers.is_empty());

        // 没挂 animator 的实体：全部静默 no-op，不 panic、不写日志。
        {
            let mut ctx = GameCtx {
                scene: &mut scene,
                physics: None,
                input: None,
                audio: None,
                logs: &mut logs,
                commands: &mut commands,
                source: "AnimProbe",
                scene_name: "",
                scene_order: &[],
                loader: None,
                clock: None,
                entity: plain,
                delta: 1.0 / 60.0,
                time: 0.0,
            };
            ctx.set_anim_clip("Run");
            ctx.set_anim_speed(2.0);
            ctx.set_anim_looping(false);
            ctx.set_anim_fade_time(0.1);
            ctx.set_anim_additive_clip("Survey");
            ctx.set_anim_layer_clip("upper", "Run");
        }
        assert!(logs.is_empty(), "no-op 不该刷日志: {logs:?}");
    }

    /// 粒子运行态 API：开关发射 / burst / rate，全部落在组件的运行态字段上。
    ///
    /// burst 是重点：它必须**可累积**（连按两帧 = 两次爆发相加），且被模拟层
    /// 消费后清零 —— 这两条决定了"按下开火键"的手感。
    #[test]
    fn particle_runtime_api_sits_on_the_component() {
        use xgame_core::ParticleEmitter;

        let mut scene = Scene::new("particles");
        let mut actor = Entity::new("torch");
        actor.particle = Some(ParticleEmitter::fire());
        scene.add_entity(actor);
        let id = scene.entities[0].id;

        let mut logs = Vec::new();
        let mut commands = Vec::new();
        {
            let mut ctx = GameCtx {
                scene: &mut scene,
                physics: None,
                input: None,
                audio: None,
                logs: &mut logs,
                commands: &mut commands,
                source: "TorchScript",
                scene_name: "",
                scene_order: &[],
                loader: None,
                clock: None,
                entity: id,
                delta: 1.0 / 60.0,
                time: 0.0,
            };

            assert!(ctx.entity_particle_emitting(id), "默认火焰在发射");
            ctx.set_entity_particle_emitting(id, false);
            assert!(!ctx.entity_particle_emitting(id));

            // 关掉发射不该动 rate —— 两个开关是正交的。
            ctx.set_entity_particle_rate(id, 12.0);
            assert_eq!(ctx.scene.entities[0].particle.as_ref().unwrap().rate, 12.0);

            // burst 累积：两帧各 10 颗 = 20 颗待处理。
            ctx.entity_particle_burst(id, 10);
            ctx.entity_particle_burst(id, 10);
            assert_eq!(ctx.scene.entities[0].particle.as_ref().unwrap().burst, 20);

            // 非有限值钳成 0 而不是把 NaN 放进模拟层。
            ctx.set_entity_particle_rate(id, f32::INFINITY);
            assert_eq!(ctx.scene.entities[0].particle.as_ref().unwrap().rate, 0.0);
        }

        // 没挂发射器的实体：所有调用静默 no-op（不 panic、不插入组件）。
        let bare = Entity::new("bare");
        scene.add_entity(bare);
        let bare_id = scene.entities[1].id;
        let mut ctx = GameCtx {
            scene: &mut scene,
            physics: None,
            input: None,
            audio: None,
            logs: &mut logs,
            commands: &mut commands,
            source: "TorchScript",
            scene_name: "",
            scene_order: &[],
            loader: None,
            clock: None,
            entity: bare_id,
            delta: 1.0 / 60.0,
            time: 0.0,
        };
        assert!(!ctx.entity_particle_emitting(bare_id));
        ctx.entity_particle_burst(bare_id, 5);
        ctx.set_entity_particle_rate(bare_id, 100.0);
        ctx.set_entity_particle_emitting(bare_id, true);
        assert!(
            scene.entities[1].particle.is_none(),
            "没挂发射器时不该被 API 凭空创建出来"
        );
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
            audio: None,
            logs: &mut logs,
            commands: &mut commands,
            source: "Spinner",
            scene_name: "",
            scene_order: &[],
            loader: None,
            clock: None,
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

    /// 音频方法层：有 port 时命令按语义进束；没接音频（`None`）时静默 no-op。
    ///
    /// 这条测试钉住的是分层承诺 —— 脚本 API 不需要音频设备就能被完整验证。
    #[test]
    fn audio_api_buffers_commands_or_noops() {
        let mut scene = Scene::new("audio");
        scene.add_entity(Entity::new("speaker"));
        let id = scene.entities[0].id;
        let mut logs = Vec::new();
        let mut commands = Vec::new();
        let mut port = xgame_core::AudioPort::new();

        {
            let mut ctx = GameCtx {
                scene: &mut scene,
                physics: None,
                input: None,
                audio: Some(&mut port),
                logs: &mut logs,
                commands: &mut commands,
                source: "Sfx",
                scene_name: "",
                scene_order: &[],
                loader: None,
                clock: None,
                entity: id,
                delta: 1.0 / 60.0,
                time: 0.0,
            };
            ctx.play_sound("Resources/Audio/Beep.wav", 0.5, 1.5);
            ctx.play_sound_at("Resources/Audio/Boom.wav", 1.0, 2.0, 3.0, 0.8);
            ctx.stop_sound("Resources/Audio/Beep.wav");
            ctx.set_listener(5.0, 5.0, 5.0, 0.0, 0.0, -1.0);
            ctx.set_master_volume(2.0); // 超界 → port 内 clamp 到 1.0
            ctx.stop_all_sounds();
        }

        assert_eq!(port.master_volume, 1.0, "主音量应被 clamp");
        let batch = port.drain();
        // 注意：set_listener / set_master_volume 直接改 port 字段、不进命令束
        // —— 监听者与主音量是"当前状态"，不是"一次性事件"。
        assert_eq!(batch.len(), 4);
        match &batch[0] {
            xgame_core::AudioCommand::Play(p) => {
                assert_eq!(p.clip, "Resources/Audio/Beep.wav");
                assert_eq!(p.volume, 0.5);
                assert_eq!(p.pitch, 1.5);
                assert_eq!(p.position, None, "play_sound 不做空间化");
            }
            other => panic!("第 1 条应是 Play，得到 {other:?}"),
        }
        match &batch[1] {
            xgame_core::AudioCommand::Play(p) => {
                assert_eq!(p.position, Some([1.0, 2.0, 3.0]));
                assert_eq!(p.volume, 0.8);
            }
            other => panic!("第 2 条应是 Play，得到 {other:?}"),
        }
        assert!(matches!(&batch[2], xgame_core::AudioCommand::StopClip(c) if c == "Resources/Audio/Beep.wav"));
        assert!(matches!(&batch[3], xgame_core::AudioCommand::StopAll));

        // 没接音频：一条命令都不该产生，也不该 panic。
        let port2 = xgame_core::AudioPort::new();
        let mut ctx = GameCtx {
            scene: &mut scene,
            physics: None,
            input: None,
            audio: None,
            logs: &mut logs,
            commands: &mut commands,
            source: "Sfx",
            scene_name: "",
            scene_order: &[],
            loader: None,
            clock: None,
            entity: id,
            delta: 1.0 / 60.0,
            time: 0.0,
        };
        ctx.play_sound("x.wav", 1.0, 1.0);
        ctx.stop_all_sounds();
        assert!(port2.is_empty(), "没接音频时应静默 no-op");
    }

    // -----------------------------------------------------------------------
    // 预制体实例化
    // -----------------------------------------------------------------------

    /// 一个能在内存里返回预制体的假装载器（真装在 `ProjectHandle` 上，
    /// 这里测的是 spawn_prefab 自己的逻辑：通知 / 位置 / 挂父 / 失败路径）。
    pub(crate) struct FakePrefabLoader {
        pub(crate) prefab: Option<Prefab>,
    }

    impl SceneLoader for FakePrefabLoader {
        fn load_scene(&self, _relative: &str) -> Result<Scene, String> {
            Err("测试装载器不提供场景".to_string())
        }
        fn load_prefab(&self, _relative: &str) -> Result<Prefab, String> {
            self.prefab
                .clone()
                .ok_or_else(|| "预制体不存在".to_string())
        }
    }

    /// 造一个两层深、根带物理的预制体（Root -> Child）。
    pub(crate) fn sample_prefab() -> Prefab {
        let mut scene = Scene::new("Template");
        let root = scene.add_entity(Entity::new("Enemy"));
        let root_index = scene.index_of(root).unwrap();
        scene.entities[root_index].physics =
            Some(PhysicsBody::dynamic_ball(0.5));
        let child = scene.add_entity(
            Entity::new("Gun")
                .with_model("builtin://cube")
                .with_transform(Transform::from_translation(Vec3::new(0.0, 1.0, 0.0))),
        );
        let child_index = scene.index_of(child).unwrap();
        scene.set_parent(child_index, Some(root_index));
        Prefab::from_subtree(&scene, root_index)
    }

    #[test]
    fn spawn_prefab_instantiates_subtree_at_position() {
        let mut scene = Scene::new("Runtime");
        let loader = FakePrefabLoader {
            prefab: Some(sample_prefab()),
        };
        let mut logs = Vec::new();
        let mut commands = Vec::new();
        let root_id = {
            let mut ctx = GameCtx {
                scene: &mut scene,
                physics: None,
                input: None,
                audio: None,
                logs: &mut logs,
                commands: &mut commands,
                source: "Spawner",
                scene_name: "",
                scene_order: &[],
                loader: Some(&loader),
                clock: None,
                entity: EntityId::from_raw(0),
                delta: 1.0 / 60.0,
                time: 0.0,
            };
            ctx.spawn_prefab("Prefabs/Enemy.prefab.ron", Vec3::new(10.0, 0.0, -3.0))
                .expect("非空预制体应当能实例化")
        };
        assert_eq!(scene.entities.len(), 2, "整棵子树都该进场景");
        assert_eq!(
            scene.entities[0].transform.position,
            Vec3::new(10.0, 0.0, -3.0),
            "根的世界位置被入参覆盖"
        );
        // 子节点的**相对**位置原样保留（Unity Instantiate 语义）
        let child = &scene.entities[1];
        assert_eq!(child.transform.position, Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(child.parent, Some(root_id), "内部父引用被重映射");
        // 每个新实体都带着实例来源（编辑器 Play 里的 Prefab 面板仍认得出它）
        assert!(scene
            .entities
            .iter()
            .all(|e| e.prefab.as_ref().is_some_and(|p| p.source == "Prefabs/Enemy.prefab.ron")));
        // 每个新实体一条 EntitySpawned（运行时据此补刚体）——含根上那颗球
        assert_eq!(commands.len(), 2);
        assert!(matches!(
            &commands[0],
            ScriptCommand::EntitySpawned { physics: Some(_), .. }
        ));
        assert!(logs.is_empty(), "成功路径不该有日志");
    }

    #[test]
    fn spawn_prefab_under_reparents_and_keeps_local_offset() {
        let mut scene = Scene::new("Runtime");
        let player = scene.add_entity(Entity::new("Player"));
        let player_id = scene.entities[scene.index_of(player).unwrap()].id;
        let loader = FakePrefabLoader {
            prefab: Some(sample_prefab()),
        };
        let mut logs = Vec::new();
        let mut commands = Vec::new();
        let root_id = {
            let mut ctx = GameCtx {
                scene: &mut scene,
                physics: None,
                input: None,
                audio: None,
                logs: &mut logs,
                commands: &mut commands,
                source: "Weapon",
                scene_name: "",
                scene_order: &[],
                loader: Some(&loader),
                clock: None,
                entity: EntityId::from_raw(0),
                delta: 1.0 / 60.0,
                time: 0.0,
            };
            ctx.spawn_prefab_under(
                "Prefabs/Enemy.prefab.ron",
                player_id,
                Vec3::new(0.25, 1.5, 0.0),
            )
            .expect("非空预制体应当能实例化")
        };
        let root_index = scene.index_of(root_id).expect("根应当存在");
        assert_eq!(root_id, scene.entities[root_index].id);
        assert_eq!(
            scene.entities[root_index].transform.position,
            Vec3::new(0.25, 1.5, 0.0),
            "挂到父节点下时入参是**局部**位置"
        );
        assert_eq!(scene.parent_index(root_index), Some(scene.index_of(player).unwrap()));
        // 父节点带着子节点走：根的世界位置 = 父位置 + 局部偏移
        let world = scene.world_matrix(root_index).w_axis.truncate();
        assert!((world - Vec3::new(0.25, 1.5, 0.0)).length() < 1e-4);
        assert!(logs.is_empty());
    }

    #[test]
    fn spawn_prefab_without_loader_logs_and_returns_none() {
        let mut scene = Scene::new("Runtime");
        let mut logs = Vec::new();
        let mut commands = Vec::new();
        let result = {
            let mut ctx = GameCtx {
                scene: &mut scene,
                physics: None,
                input: None,
                audio: None,
                logs: &mut logs,
                commands: &mut commands,
                source: "Spawner",
                scene_name: "",
                scene_order: &[],
                loader: None,
                clock: None,
                entity: EntityId::from_raw(0),
                delta: 1.0 / 60.0,
                time: 0.0,
            };
            ctx.spawn_prefab("Prefabs/Enemy.prefab.ron", Vec3::ZERO)
        };
        assert!(result.is_none());
        assert_eq!(scene.entities.len(), 0, "失败不该留下半个实例");
        assert!(commands.is_empty());
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].level, LogLevel::Error);
        assert!(
            logs[0].message.contains("装载器"),
            "日志要说清是接线问题：{}", logs[0].message
        );
    }

    #[test]
    fn spawn_prefab_reports_unreadable_asset() {
        let mut scene = Scene::new("Runtime");
        let loader = FakePrefabLoader { prefab: None };
        let mut logs = Vec::new();
        let mut commands = Vec::new();
        let result = {
            let mut ctx = GameCtx {
                scene: &mut scene,
                physics: None,
                input: None,
                audio: None,
                logs: &mut logs,
                commands: &mut commands,
                source: "Spawner",
                scene_name: "",
                scene_order: &[],
                loader: Some(&loader),
                clock: None,
                entity: EntityId::from_raw(0),
                delta: 1.0 / 60.0,
                time: 0.0,
            };
            ctx.spawn_prefab("Prefabs/Missing.prefab.ron", Vec3::ZERO)
        };
        assert!(result.is_none());
        assert_eq!(scene.entities.len(), 0);
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].level, LogLevel::Error);
        assert!(logs[0].message.contains("Prefabs/Missing.prefab.ron"));
    }

    #[test]
    fn spawn_prefab_under_missing_parent_falls_back_to_root() {
        let mut scene = Scene::new("Runtime");
        let loader = FakePrefabLoader {
            prefab: Some(sample_prefab()),
        };
        let mut logs = Vec::new();
        let mut commands = Vec::new();
        let root_id = {
            let mut ctx = GameCtx {
                scene: &mut scene,
                physics: None,
                input: None,
                audio: None,
                logs: &mut logs,
                commands: &mut commands,
                source: "Weapon",
                scene_name: "",
                scene_order: &[],
                loader: Some(&loader),
                clock: None,
                entity: EntityId::from_raw(0),
                delta: 1.0 / 60.0,
                time: 0.0,
            };
            // 一个场景里不存在的实体 id
            ctx.spawn_prefab_under(
                "Prefabs/Enemy.prefab.ron",
                EntityId::from_raw(9999),
                Vec3::new(1.0, 0.0, 0.0),
            )
            .expect("父节点缺失只是退回场景根，不该整次失败")
        };
        let root_index = scene.index_of(root_id).expect("根应当存在");
        assert_eq!(scene.parent_index(root_index), None, "应当挂在场景根");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].level, LogLevel::Warn);
    }
}
