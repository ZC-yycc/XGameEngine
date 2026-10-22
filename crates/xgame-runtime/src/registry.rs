//! 行为注册表：组件名（`Entity::behaviors` 里的字符串）→ 构造与分发函数。
//!
//! 与 UE 的模块启动注册同构：插件（或宿主自己）在入口处 `register::<T>("Name")`，
//! `World` 装配时按场景里声明的名字实例化。类型信息只活在注册表里 ——
//! 场景文件存的永远是**名字**（纯数据、可 diff，不含任何类型路径）。
//!
//! # 钩子函数为什么在装配时拷进实例
//!
//! 分发热路径每帧要跑"实体数 × 阶段数"轮；若每轮回注册表再查一次
//! （`HashMap<String>` + 字符串哈希），1000 实体 × 3 阶段就是每帧 ~3000 次
//! 哈希查找 —— 实测占组件轨分发成本的大头（微基准 69 µs，见
//! `tests/behavior_dispatch_throughput.rs`）。所以实例化时把四个钩子函数
//! 指针连同 `'static` 注册名一起拷进 [`BehaviorInstance`]：热路径零哈希，
//! 只剩函数指针调用。注册表退化为"装配期的解析器"（插件热重载时整表重建）。

use std::collections::HashMap;

use crate::lifecycle::{Behavior, Ctx};

/// 统一形状的钩子函数指针（`start` / `on_destroy` 忽略 `dt`）。
///
/// 统一签名是为了让四个指针装进一个 `Copy` 结构体，随实例一起拷贝。
pub(crate) type HookFn = fn(&mut Box<dyn Behavior>, &mut Ctx, f32);

/// 碰撞钩子的函数指针形状（多一个"对方实体"参数）。
pub(crate) type CollisionHookFn =
    fn(&mut Box<dyn Behavior>, &mut Ctx, xgame_core::EntityId);

/// 四个钩子（装配期从注册项拷出，见模块文档"为什么拷进实例"）。
#[derive(Clone, Copy)]
pub(crate) struct Hooks {
    pub(crate) start: HookFn,
    pub(crate) update: HookFn,
    pub(crate) fixed_update: HookFn,
    pub(crate) collision_enter: CollisionHookFn,
    pub(crate) collision_exit: CollisionHookFn,
    pub(crate) on_destroy: HookFn,
}

/// 一条注册项：把具体类型 `T` 的构造与钩子擦除成函数指针。
struct BehaviorEntry {
    /// 注册名。同时用作组件日志的归属名 —— `'static` 让每次派发
    /// 构造 `GameCtx` 时零分配（字符串直接借用，不 clone）。
    name: &'static str,
    construct: fn() -> Box<dyn Behavior>,
    hooks: Hooks,
    /// 字段表（注册期从一个默认实例上取一次 —— 表是静态的，之后与实例无关）。
    fields: &'static [xgame_core::FieldDesc],
}

/// 一个已实例化的组件（实体身上的一件行为）。
///
/// `started` 是组件轨自己的 start 标记（按**组件**记而不是按实体记 ——
/// 省掉热路径上一张 `HashSet<EntityId>` 的每帧哈希）。
pub(crate) struct BehaviorInstance {
    pub(crate) name: &'static str,
    pub(crate) component: Box<dyn Behavior>,
    pub(crate) hooks: Hooks,
    pub(crate) started: bool,
}

/// 行为组件注册表。
///
/// 默认空表；`World` 持有一份，通过插件入口函数的句柄交给插件写入
/// （刻意不做全局 `static`：多测试并行时全局表互相污染，且"注册发生在哪"
/// 会变成隐式协议）。
#[derive(Default)]
pub struct BehaviorRegistry {
    entries: HashMap<String, BehaviorEntry>,
}

impl BehaviorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个行为组件类型（`T` 必须能默认构造 —— P0 的组件参数尚未
    /// 进场景文件，实例一律从 `Default` 起步）。
    ///
    /// 约束是 [`Behavior`]（= `Lifecycle` + `Inspect`）：编辑器要在不链接用户
    /// 代码的前提下渲染组件字段，只能靠实例自己带的字段表 —— 所以"可检视"是
    /// 组件契约的一部分。不想暴露字段的组件用 `#[derive(Inspect)]` 配
    /// `#[inspect(skip)]`，或者 `xgame_core::impl_no_inspect!` 一行空实现。
    ///
    /// 同名重复注册时后者覆盖前者：后加载的插件说了算（热更插件的预期行为）。
    pub fn register<T>(&mut self, name: &'static str)
    where
        T: Behavior + Default + 'static,
    {
        self.entries.insert(
            name.to_owned(),
            BehaviorEntry {
                name,
                construct: || Box::<T>::default(),
                hooks: Hooks {
                    start: |component, ctx, _| component.start(ctx),
                    update: |component, ctx, dt| component.update(ctx, dt),
                    fixed_update: |component, ctx, dt| component.fixed_update(ctx, dt),
                    collision_enter: |component, ctx, other| {
                        component.on_collision_enter(ctx, other)
                    },
                    collision_exit: |component, ctx, other| {
                        component.on_collision_exit(ctx, other)
                    },
                    on_destroy: |component, ctx, _| component.on_destroy(ctx),
                },
                // 表是 `'static` 的，所以能从一个临时实例上取下来存住 ——
                // 这一次构造换来编辑器在 Edit 模式（还没有实例）时零分配地
                // 画出字段。
                fields: T::default().inspect_fields(),
            },
        );
    }

    /// 名字是否已注册（编辑器提示 / 宿主自检用）。
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// 按名字构造一个组件实例；未注册返回 `None`（调用方记日志跳过）。
    ///
    /// 返回的 trait 对象是 [`Behavior`]，可以直接 `as_mut` 成 `&mut dyn Inspect`
    /// 交给编辑器渲染字段（编辑器不链接用户代码，只能走这条路）。
    pub fn construct(&self, name: &str) -> Option<Box<dyn Behavior>> {
        self.entries.get(name).map(|e| (e.construct)())
    }

    /// 按名字取字段表；未注册返回 `None`。
    ///
    /// 表是**静态**的（注册期从一个默认实例上取一次，此后与实例无关）——
    /// 编辑器在 Edit 模式下只画字段、还没有组件实例时用它，零分配。
    pub fn field_table(&self, name: &str) -> Option<&'static [xgame_core::FieldDesc]> {
        self.entries.get(name).map(|e| e.fields)
    }

    /// 已注册的名字列表（编辑器下拉、日志诊断用）。
    pub fn names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.entries.values().map(|e| e.name).collect();
        names.sort_unstable();
        names
    }

    /// 按名字构造一个已解析的组件实例（注册名 + 钩子副本；热路径不再查表）；
    /// 未注册返回 `None`（调用方记日志跳过）。
    pub(crate) fn construct_instance(&self, name: &str) -> Option<BehaviorInstance> {
        self.entries.get(name).map(|e| BehaviorInstance {
            name: e.name,
            component: (e.construct)(),
            hooks: e.hooks,
            started: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::Lifecycle;
    use xgame_core::{EntityId, Scene};
    use xgame_script::GameCtx;

    use std::sync::atomic::{AtomicU32, Ordering};

    static STARTS: AtomicU32 = AtomicU32::new(0);

    #[derive(Default)]
    struct Counting;

    // 测试组件不在编辑器里调参，走空实现（`#[derive(Inspect)]` 在本 crate 里
    // 解析不到 `::xgame`，见 xgame-derive 的"路径假设"）。
    xgame_core::impl_no_inspect!(Counting);

    impl Lifecycle for Counting {
        fn start(&mut self, _ctx: &mut Ctx) {
            STARTS.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn registered_types_construct_and_dispatch() {
        let mut registry = BehaviorRegistry::new();
        registry.register::<Counting>("Counting");
        assert!(registry.contains("Counting"));
        assert_eq!(registry.names(), vec!["Counting"]);

        let mut instance = registry.construct_instance("Counting").expect("应能构造");
        assert_eq!(instance.name, "Counting");
        assert!(!instance.started);
        // 分发一条最小 GameCtx（只为了走通函数指针这条路）。
        let mut scene = Scene::new("t");
        let mut logs = Vec::new();
        let mut commands = Vec::new();
        let mut ctx = GameCtx {
            scene: &mut scene,
            physics: None,
            input: None,
            audio: None,
            logs: &mut logs,
            commands: &mut commands,
            source: "test",
            scene_name: "",
            entity: EntityId::from_raw(0),
            delta: 0.016,
            time: 0.0,
        };
        (instance.hooks.start)(&mut instance.component, &mut ctx, 0.0);
        assert_eq!(STARTS.load(Ordering::Relaxed), 1);

        // 未注册的名字：构造返回 None（不 panic）。
        assert!(registry.construct_instance("Nope").is_none());
        assert!(registry.construct("Nope").is_none());
    }
}
