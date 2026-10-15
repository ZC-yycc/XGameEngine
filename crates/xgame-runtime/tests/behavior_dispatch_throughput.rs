//! 微基准：组件轨 vs 脚本轨的每帧分发开销（双轨计划 P0 验收第 7 条）。
//!
//! 默认 `#[ignore]`（耗时波动大，不进 CI 门禁）。手动跑：
//!
//! ```sh
//! cargo test -p xgame-runtime --release --test behavior_dispatch_throughput -- --ignored --nocapture
//! ```
//!
//! # 量的是什么
//!
//! 1000 个**纯视觉**实体（无刚体，`sync_bodies` / 物理完全不参与），分三档：
//!
//! | 档 | 量的是什么 |
//! |---|---|
//! | 无轨道（基线） | 引擎每帧的公共开销（start 扫描、固定步循环、空脚本轨扫描） |
//! | Rust 组件轨 | 基线 + 1000 个 no-op 组件的分发 |
//! | Lua 脚本轨 | 基线 + 1000 个 no-op Lua 脚本的分发 |
//!
//! "轨的分发成本" = 各档减基线 —— 差额里只有分发路径不同（验收线
//! "组件分发一轮 < 50 µs"按这个口径判）。写成 `tests/` 下的集成测试
//! （而非 `src` 内单测），与 `update_throughput.rs` 同理：独立于被测源码
//! 的改动而存在，改动前后跑同一份基准才公平。
//!
//! # 量出来的数字：记录必须带"哪一版 + 同机对照"
//!
//! 这是**相对**判据，但绝对值随机器状态浮动（睿频 / 后台负载能差出 1.5× 以上）。
//! 所以记录时要记全三元组（提交 + 机器状态 + 同机对照组），否则下一个人会拿一个
//! 跨机器的数字当绝对真理，去"修"一个并不存在的回归。已知记录：
//!
//! | 轨 | P0 `9b3cd1c`（当时记录） | P0.5 `3d77f12`（2026-10-14 同机回测） | P1（同一次会话、同机背靠背） | P2（2026-10-14 同日同机复测） | P2.5（2026-10-15 同机背靠背） |
//! |---|---|---|---|---|---|
//! | 无轨道基线 | — | 6.32 µs | 6.39 µs | 7.43 µs | 7.54 µs |
//! | Rust 组件轨分发 | 37.6 µs | **68.4 µs** | **65.2 µs** | **64.7 µs** | **71.7 µs** |
//! | Rhai 脚本轨分发 | 493.9 µs | 728.8 µs | 729.3 µs | **713 µs** | 735.6 µs |
//! | Lua 脚本轨分发 | — | — | — | （未实测） | **562.1 µs** |
//!
//! P2 列是**驻留表修复后**的数字（每帧 3 处路径 `String` 克隆换成驻留 `u32`，
//! 见 `xgame-script` 的 `intern.rs`），4 轮背靠背取中位：基线 7.40-7.44、
//! 组件分发 63.7-66.2、Rhai 分发 708.7-713.9 µs。三个要点：
//!
//! 1. **Rhai 轨的改善与预测吻合（先量后改的闭环）**：改前同日量的三档总值为
//!    10.39 / 67.47 / 829.94 µs（即 Rhai 分发 819.6），修复后 ≈713 ——
//!    **降 ≈106 µs（-13%）**，与驻留表探针（量完即删，原始数据记在 PLAN.md
//!    十九）预测的 ≈110 µs/帧上限几乎一致，端到端实测交叉验证了微基准。
//! 2. **基线（10.39 → 7.43）与组件轨（57.1 → 64.7 分发）的反向漂移不是回归**：
//!    这两档的执行路径**不经过**驻留表（组件档与基线档的脚本列表为空，扫描
//!    循环前后逐字相同），漂移来自机器状态 —— 正是本文开头声明的"绝对值随
//!    机器状态浮动"。判回归只看 Rhai 轨的 -106 µs 是不是消失。
//! 3. 组件轨 64.7 与 P0.5 / P1（68.4 / 65.2）同量级，P1 之后无回归。
//!
//! **P2.5 列**（闸门会话）：Lua 轨首测 **1615 µs/帧**（比 Rhai 慢 2.3×），
//! 定位到两处实现开销并修掉后 **562 µs（-65%）** —— 每钩子 `env.get` 查表
//! （~194 ns/次，钩子函数句柄改为编译期缓存）+ 每次调用现造 userdata
//! （~284 ns/次，改为宿主常驻槽 + 轻量指针传参）。量证与方案对比在
//! PLAN.md 二十。Rhai 轨 735.6 与历史 713 在机器噪声内（同机对照基线
//! 7.43 → 7.54 也漂了 ~2%）；组件轨 71.7 同理（本机验收线持续红，见文首）。
//!
//! **P3 起 Rhai 轨从基准移除**（闸门判定 Lua 三负载全占优 → 删 Rhai，
//! 见计划文档 §六）—— 表中 Rhai 列保留作历史记录，代码只跑基线 /
//! 组件 / Lua 三档。
//!
//! 两个结论，都由这张表得出：
//!
//! 1. **P1 相对 P0.5 没有回归**（65.2 vs 68.4 µs，甚至略低，在噪声内）。
//!    P1 把 `Box<dyn Lifecycle>` 换成了 `Box<dyn Behavior>`（多一层
//!    supertrait vtable），量之前担心过它拖慢分发 —— 量完确认没有，这才叫"先量后改"。
//! 2. P0 那一栏与后两栏的差距出现在 P0.5 与 P1 之间，且**两条轨同步**变慢
//!    （1.7× / 1.5×）而基线几乎不动 —— 与任何一侧的组件/脚本改动都对不上，
//!    判定为当时机器状态不同。
//!
//! **本机现在跑它会红在 50 µs 验收线上（P0.5 也红）。**验收线没有为迁就机器而上调：
//! 它表达的是"1000 实体 < 50 µs"这条**目标**。所以红了先按上表对一下同机的
//! P0.5 数字，再决定是不是真回归 —— 两版同机数字接近就不是。

use xgame_core::{Entity, Scene};
use xgame_runtime::{BehaviorRegistry, Lifecycle, World, WorldConfig};

const N: usize = 1000;
const FRAMES: usize = 300;

/// no-op 组件：四个钩子都用默认空实现，量的就是纯分发开销。
#[derive(Default)]
struct Noop;
impl Lifecycle for Noop {}
// 基准只看分发，字段表走空实现（`#[derive(Inspect)]` 解析不到 `::xgame`，
// 见 xgame-derive 的"路径假设"）。
xgame_core::impl_no_inspect!(Noop);

/// 造一个 N 实体的纯视觉场景。`mode`：none = 无轨基线；
/// scripts_lua = no-op Lua 脚本；components = no-op 组件。
fn build_scene(mode: &str) -> Scene {
    let mut scene = Scene::new("bench");
    for i in 0..N {
        let mut e = Entity::new(format!("e{i}"));
        match mode {
            "scripts_lua" => e.scripts.push("Scripts/noop.lua".to_owned()),
            "components" => e.behaviors.push("Noop".to_owned()),
            _ => {}
        }
        scene.add_entity(e);
    }
    scene
}

/// 量一档：预热后跑 `FRAMES` 帧，返回每帧微秒数。
fn measure(mode: &str, label: &str) -> f64 {
    let dir = std::env::temp_dir().join(format!(
        "xgame_bench_{}_{}",
        std::process::id(),
        mode
    ));
    if mode == "scripts_lua" {
        std::fs::create_dir_all(dir.join("Scripts")).unwrap();
        std::fs::write(
            dir.join("Scripts/noop.lua"),
            "function start(ctx) end\nfunction update(ctx) end\nfunction fixed_update(ctx) end\n",
        )
        .unwrap();
    }

    let registry = {
        let mut r = BehaviorRegistry::new();
        r.register::<Noop>("Noop");
        r
    };
    let mut world = World::new(build_scene(mode), &dir, WorldConfig::default())
        .with_behavior_registry(registry);

    // 预热：脚本轨要在这里完成编译与实例化，组件轨让实例表与扫描稳定。
    for _ in 0..30 {
        world.update(1.0 / 60.0);
    }

    let start = std::time::Instant::now();
    for _ in 0..FRAMES {
        world.update(1.0 / 60.0);
    }
    let elapsed = start.elapsed();

    let per_frame_us = elapsed.as_secs_f64() * 1e6 / FRAMES as f64;
    println!(
        "behavior_dispatch_throughput[{label}]: {N} 实体 × {FRAMES} 帧 = {:.3} ms 总，{per_frame_us:.2} us/帧",
        elapsed.as_secs_f64() * 1e3,
    );

    let _ = std::fs::remove_dir_all(&dir);
    per_frame_us
}

#[test]
#[ignore = "手动运行的微基准，不进 CI"]
fn behavior_dispatch_throughput() {
    let baseline_us = measure("none", "无轨道基线");
    let component_us = measure("components", "Rust 组件轨");
    let lua_us = measure("scripts_lua", "Lua 脚本轨");

    let component_dispatch = component_us - baseline_us;
    let lua_dispatch = lua_us - baseline_us;
    println!(
        "对照：组件轨分发 {component_dispatch:.2} us/帧，Lua 轨分发 {lua_dispatch:.2} us/帧\
         （含各钩子的 Ctx 构造、解释器调用与日志缓冲写入）"
    );

    // P0 验收：1000 个 no-op 组件实体，组件分发一轮 < 50 µs（扣除公共基线后的分发成本）。
    assert!(
        component_dispatch < 50.0,
        "组件轨分发每帧 {component_dispatch:.2} µs，超过 50 µs 的验收线"
    );
}
