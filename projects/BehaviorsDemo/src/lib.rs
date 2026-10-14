//! 双轨语言架构 P0.5 的**演示插件**：一个真正的 Rust 轨道项目。
//!
//! # 这个项目在证明什么
//!
//! `projects/BehaviorsDemo` 本身就是一个 cdylib crate（workspace 成员）。编辑器
//! 按下 ▶ Play 时会探测到项目根的 `Cargo.toml`、后台 `cargo build`、把 DLL 同步进
//! `Plugins/` 再加载 —— 全程 UI 不阻塞。场景里同时挂着：
//!
//! - `Spinner`（Rust，本文件）—— 与 `Scripts/Spinner.rhai` **同款行为**
//!   （绕 Y 轴 75°/s），用来直观对照两条轨道跑出同一份效果；
//! - `Bobber`（Rust）—— 上下浮动，展示组件自己持有跨帧状态；
//! - `Spinner`（Rhai，另一实体）—— 证明两条轨道**共用一个运行时、互不干扰**。
//!
//! # 注册表只有一份真相源
//!
//! [`register_all`] 同时被 `export_plugin!`（发布路径）与单元测试（验证路径）
//! 调用 —— 保证"测试覆盖的就是加载进编辑器的那张表"，两边不会各写一份然后漂移。

use xgame::prelude::*;

/// 与 `Scripts/Spinner.rhai` 同值：绕 Y 轴每秒转 75 度。
const SPIN_SPEED: f32 = 75.0;

/// 与 Rhai 版同款：绕 Y 轴匀速旋转。
///
/// 帧率无关靠 `delta()`：乘以 dt 才是"每秒 75 度"，否则机器越快转得越快。
#[derive(Default)]
pub struct Spinner;

impl Lifecycle for Spinner {
    fn update(&mut self, ctx: &mut Ctx, dt: f32) {
        let r = ctx.rot();
        ctx.set_rot(Vec3::new(r.x, r.y + SPIN_SPEED * dt, r.z));
    }
}

/// 上下浮动 —— 组件**自己持有状态**（起始高度 + 累计时间）。
///
/// 这是与脚本的一个刻意对照：Rhai 脚本没有实例字段可用（状态得靠全局表或
/// 实体身上挂东西），而 Rust 组件天生就是一个结构体，状态归属清清楚楚。
#[derive(Default)]
pub struct Bobber {
    /// `start` 时记下的起始高度（浮动围绕它对称）。
    base_y: f32,
    /// 累计时间。用 `delta()` 自己攒而不是 `time()`，是为了让"组件何时被
    /// 创建"这件事只影响 `start`，行为完全可预测。
    elapsed: f32,
    /// 本组件自己的参数（与别的 Bobber 实例互不影响）。
    configured: bool,
}

/// 浮动幅度（米）与频率（Hz）。
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
        p.y = self.base_y + (self.elapsed * BOB_HZ * std::f32::consts::TAU).sin() * BOB_AMPLITUDE;
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
        assert!(min_y < 2.8 && max_y > 3.2, "应当真的浮起来了，实际 [{min_y:.3}, {max_y:.3}]");
        assert!(
            (min_y - 2.4).abs() < 0.05 && (max_y - 3.6).abs() < 0.05,
            "浮动必须围绕 start 高度 3.0 对称，实际 [{min_y:.3}, {max_y:.3}]"
        );
    }
}
