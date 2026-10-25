//! 双轨语言架构 P0.5 的**演示插件**：一个真正的 Rust 轨道项目。
//!
//! # 这个项目在证明什么
//!
//! `projects/BehaviorsDemo` 本身就是一个 cdylib crate（workspace 成员）。编辑器
//! 按下 ▶ Play 时会探测到项目根的 `Cargo.toml`、后台 `cargo build`、把 DLL 同步进
//! `Plugins/` 再加载 —— 全程 UI 不阻塞。场景里同时挂着：
//!
//! - `Spinner`（Rust，本文件）—— 与 `Scripts/Spinner.lua` **同款行为**
//!   （绕 Y 轴 75°/s），用来直观对照两条轨道跑出同一份效果；
//! - `Bobber`（Rust）—— 上下浮动，展示组件自己持有跨帧状态；
//! - `Spinner`（Lua 脚本，另一实体）—— 证明两条轨道**共用一个运行时、互不干扰**。
//!
//! 两个组件都派生了 [`Inspect`]（P1）：编辑器 Inspector 里会出现
//! `speed`、`amplitude`、`hz` 三个可拖的滑块 —— 状态字段（`base_y` 等）用
//! `#[inspect(skip)]` 挡在外面。
//!
//! # 注册表只有一份真相源
//!
//! [`register_all`] 同时被 `export_plugin!`（发布路径）与单元测试（验证路径）
//! 调用 —— 保证"测试覆盖的就是加载进编辑器的那张表"，两边不会各写一份然后漂移。

use xgame::prelude::*;

/// 与 `Scripts/Spinner.lua` 同值：绕 Y 轴每秒转 75 度（组件的默认参数）。
const SPIN_SPEED: f32 = 75.0;

/// 与 Lua 版同款：绕 Y 轴匀速旋转。
///
/// 帧率无关靠 `delta()`：乘以 dt 才是"每秒 75 度"，否则机器越快转得越快。
///
/// `speed` 是**可检视字段**（P1）：编辑器 Inspector 里会画成一个
/// `-720 ~ 720 °/s` 的滑块，拖一下下一次 update 就生效 —— 这正是
/// `#[derive(Inspect)]` 要买到的东西。
///
/// `Default` 手写而不是派生：`#[derive(Default)]` 会把 `f32` 归零，组件就
/// 不转了。组件的默认参数本来就该由作者给，派生只是在没有合理默认值时省事。
#[derive(Inspect)]
pub struct Spinner {
    #[inspect(
        min = -720.0,
        max = 720.0,
        speed = 5.0,
        unit = "°/s",
        tooltip = "绕 Y 轴的角速度，负值反向"
    )]
    pub speed: f32,
}

impl Default for Spinner {
    fn default() -> Self {
        Self { speed: SPIN_SPEED }
    }
}

impl Lifecycle for Spinner {
    fn update(&mut self, ctx: &mut Ctx, dt: f32) {
        let r = ctx.rot();
        ctx.set_rot(Vec3::new(r.x, r.y + self.speed * dt, r.z));
    }
}

/// 上下浮动 —— 组件**自己持有状态**（起始高度 + 累计时间）。
///
/// 这是与脚本的一个刻意对照：Lua 脚本没有实例字段可用（状态得靠 `state_*` 表或
/// 实体身上挂东西），而 Rust 组件天生就是一个结构体，状态归属清清楚楚。
///
/// 字段分两类，`#[inspect(...)]` 把这件事写在了字段上：
/// - `amplitude` / `hz` 是**调参**（进编辑器，能在 Play 中拖）；
/// - `base_y` / `elapsed` / `configured` 是**状态**（`#[inspect(skip)]`）——
///   它们由 `start` 决定，让用户在 Inspector 里改只会得到一个被下一次
///   `update` 覆盖的假象。
#[derive(Inspect)]
pub struct Bobber {
    /// 浮动幅度（米）。
    #[inspect(min = 0.0, max = 5.0, speed = 0.05, unit = "m", tooltip = "浮动幅度")]
    amplitude: f32,
    /// 浮动频率（Hz）。
    #[inspect(min = 0.0, max = 5.0, speed = 0.05, unit = "Hz", tooltip = "浮动频率")]
    hz: f32,

    /// `start` 时记下的起始高度（浮动围绕它对称）。
    #[inspect(skip)]
    base_y: f32,
    /// 累计时间。用 `delta()` 自己攒而不是 `time()`，是为了让"组件何时被
    /// 创建"这件事只影响 `start`，行为完全可预测。
    #[inspect(skip)]
    elapsed: f32,
    /// 本组件自己的参数（与别的 Bobber 实例互不影响）。
    #[inspect(skip)]
    configured: bool,
}

impl Default for Bobber {
    fn default() -> Self {
        Self {
            amplitude: BOB_AMPLITUDE,
            hz: BOB_HZ,
            base_y: 0.0,
            elapsed: 0.0,
            configured: false,
        }
    }
}

/// 浮动幅度（米）与频率（Hz）—— 组件的默认参数。
const BOB_AMPLITUDE: f32 = 0.6;
const BOB_HZ: f32 = 0.5;

impl Lifecycle for Bobber {
    fn start(&mut self, ctx: &mut Ctx) {
        self.base_y = ctx.pos().y;
        self.elapsed = 0.0;
        self.configured = true;
        ctx.log("Bobber 已接管这个实体的 Y 轴");
    }

    fn update(&mut self, ctx: &mut Ctx, dt: f32) {
        debug_assert!(self.configured, "update 跑在 start 之前");
        self.elapsed += dt;
        let mut p = ctx.pos();
        p.y = self.base_y + (self.elapsed * self.hz * std::f32::consts::TAU).sin() * self.amplitude;
        ctx.set_pos(p);
    }

    fn on_destroy(&mut self, ctx: &mut Ctx) {
        ctx.log("Bobber 交还了这个实体");
    }
}

/// 本插件的全部注册（发布路径与测试共用，见模块文档）。
fn register_all(registry: &mut BehaviorRegistry) {
    registry.register::<Spinner>("Spinner");
    registry.register::<Bobber>("Bobber");
}

xgame::export_plugin!(|registry| {
    register_all(registry);
});

#[cfg(test)]
mod tests {
    use super::*;

    /// 注册表里必须正好是这两个名字。
    ///
    /// 这条测试守着 `export_plugin!` 那一侧的真相源：如果有人在闭包里加了
    /// 组件却忘了 `register_all`（或反过来），场景里挂的名字就会静默失效 ——
    /// 那是最难查的一类 bug（表现是"实体纹丝不动"，没有任何报错）。
    #[test]
    fn registers_exactly_the_documented_components() {
        let mut registry = BehaviorRegistry::new();
        register_all(&mut registry);

        let mut names: Vec<String> = registry.names().into_iter().map(str::to_owned).collect();
        names.sort();
        assert_eq!(names, ["Bobber", "Spinner"], "组件名必须与文档一致");
    }

    /// 组件真的能驱动实体（Rust 轨道在**本项目**里的最小闭环）。
    ///
    /// 没有组件路径的旧实现下 `actor` 纹丝不动，断言必红。
    #[test]
    fn spinner_drives_entity_through_world() {
        let mut scene = Scene::new("demo");
        let mut actor = Entity::new("spinner");
        actor.behaviors.push("Spinner".to_owned());
        scene.add_entity(actor);

        let mut registry = BehaviorRegistry::new();
        register_all(&mut registry);
        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default())
            .with_behavior_registry(registry);

        // 60 帧 × 1/60s = 1 秒 ⇒ 应当正好转过约 75 度。
        for _ in 0..60 {
            world.update(1.0 / 60.0);
        }

        let actor = world.scene().find_by_name("spinner").expect("实体应当还在");
        assert_ne!(
            actor.transform.rotation,
            glam::Quat::IDENTITY,
            "update 应当已经把实体转起来"
        );
        // glam 的返回元组**按 EulerRot 字母排**：YXZ ⇒ (y, x, z)，所以 yaw 在第 0 位
        // （按 (x,y,z) 理解会取到恒为 0 的 pitch —— 这个坑值得写下来）。
        let (yaw, _, _) = actor.transform.rotation.to_euler(glam::EulerRot::YXZ);
        let degrees = yaw.to_degrees();
        assert!(
            (degrees - 75.0).abs() < 1.0,
            "1 秒应当转过约 75 度，实际 {degrees:.2}°"
        );
    }

    /// Bobber 的浮动是围绕 `start` 高度对称的（证明组件自己的状态被保留了）。
    #[test]
    fn bobber_keeps_its_own_state() {
        let mut scene = Scene::new("bob");
        let mut actor = Entity::new("bobber");
        actor.behaviors.push("Bobber".to_owned());
        actor.transform.position = Vec3::new(0.0, 3.0, 0.0);
        scene.add_entity(actor);

        let mut registry = BehaviorRegistry::new();
        register_all(&mut registry);
        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default())
            .with_behavior_registry(registry);

        let mut min_y = f32::MAX;
        let mut max_y = f32::MIN;
        for _ in 0..240 {
            world.update(1.0 / 60.0);
            let y = world
                .scene()
                .find_by_name("bobber")
                .expect("实体应当还在")
                .transform
                .position
                .y;
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }

        // 起始 3.0，幅度 0.6 ⇒ 应当在 [2.4, 3.6] 之间来回。
        assert!(
            min_y < 2.8 && max_y > 3.2,
            "应当真的浮起来了，实际 [{min_y:.3}, {max_y:.3}]"
        );
        assert!(
            (min_y - 2.4).abs() < 0.05 && (max_y - 3.6).abs() < 0.05,
            "浮动必须围绕 start 高度 3.0 对称，实际 [{min_y:.3}, {max_y:.3}]"
        );
    }

    /// `#[derive(Inspect)]` 生成的字段表，以及"调参真的改变行为"这条闭环（P1）。
    ///
    /// 守三件事：
    /// - 表的元数据正确（名字、类型、单位、边界）—— 编辑器画控件全靠它；
    /// - **状态字段被 `skip` 掉** —— 漏一个就会让用户在 Inspector 里改到一个
    ///   下一帧就被覆盖的假值，比不给改更糟；
    /// - 经 `inspect_set` 写进去的值真的改变了运行结果（不是只改了显示）。
    #[test]
    fn field_table_matches_and_tuning_changes_behavior() {
        let mut registry = BehaviorRegistry::new();
        register_all(&mut registry);

        // -- Spinner：表 + 调参 ------------------------------------------------
        let table = registry
            .field_table("Spinner")
            .expect("Spinner 应当有字段表");
        assert_eq!(table.len(), 1);
        assert_eq!(table[0].name, "speed");
        assert_eq!(table[0].kind, FieldKind::F32);
        assert_eq!(table[0].unit, "°/s");
        assert_eq!(table[0].min, Some(-720.0));
        assert_eq!(table[0].max, Some(720.0));
        assert!(table[0].has_range(), "有边界 ⇒ 编辑器画滑块而不是裸拖拽");

        // -- Bobber：2 个调参字段，3 个状态字段被 skip ------------------------
        let table = registry.field_table("Bobber").expect("Bobber 应当有字段表");
        let names: Vec<&str> = table.iter().map(|f| f.name).collect();
        assert_eq!(
            names,
            ["amplitude", "hz"],
            "base_y / elapsed / configured 是状态，必须被 skip"
        );
        assert_eq!(table[0].unit, "m");
        assert_eq!(table[1].unit, "Hz");

        // -- 调参 → 行为真的变了 ----------------------------------------------
        let mut scene = Scene::new("tune");
        let mut actor = Entity::new("spinner");
        actor.behaviors.push("Spinner".to_owned());
        scene.add_entity(actor);

        let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default())
            .with_behavior_registry(registry);

        let id = world
            .scene()
            .find_by_name("spinner")
            .expect("实体应当在")
            .id;
        {
            let (_, comp) = world.behavior_at_mut(id, 0).expect("Spinner 已装配");
            assert_eq!(
                comp.inspect_get(0),
                FieldValue::F32(75.0),
                "默认值来自手写的 Default（派生 Default 会把 f32 归零）"
            );
            comp.inspect_set(0, FieldValue::F32(150.0));
            // 写错类型必须被忽略 —— Lua 侧传错类型的常见后果。
            comp.inspect_set(0, FieldValue::String("fast".into()));
        }

        // 1 秒 × 150°/s = 150°。刻意不跑 2 秒：`to_euler` 把 yaw 归一化到
        // (-180, 180]，300° 会变成 -60°，断言就得自己处理绕圈 ——
        // 第一版就是这么写的，看到"实际 -60.00°"才想起来。
        for _ in 0..60 {
            world.update(1.0 / 60.0);
        }
        let actor = world.scene().find_by_name("spinner").expect("实体应当还在");
        let (yaw, _, _) = actor.transform.rotation.to_euler(glam::EulerRot::YXZ);
        let degrees = yaw.to_degrees();
        assert!(
            (degrees - 150.0).abs() < 2.0,
            "调到 150°/s 后 1 秒应当转过约 150 度，实际 {degrees:.2}° —— \
             说明 inspect_set 的值没有真正进入组件"
        );
    }
}
