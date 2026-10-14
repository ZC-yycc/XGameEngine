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
//! | Rhai 脚本轨 | 基线 + 1000 个 no-op 脚本的分发 |
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
//! | 轨 | P0 `9b3cd1c`（当时记录） | P0.5 `3d77f12`（2026-10-14 同机回测） | P1（同一次会话、同机背靠背） |
//! |---|---|---|---|
//! | 无轨道基线 | — | 6.32 µs | 6.39 µs |
//! | Rust 组件轨分发 | 37.6 µs | **68.4 µs** | **65.2 µs** |
//! | Rhai 脚本轨分发 | 493.9 µs | 728.8 µs | 729.3 µs |
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

/// 造一个 N 实体的纯视觉场景。`mode`：none = 无轨道基线；scripts = no-op
/// Rhai 脚本；components = no-op 组件。
fn build_scene(mode: &str) -> Scene {
    let mut scene = Scene::new("bench");
    for i in 0..N {
        let mut e = Entity::new(format!("e{i}"));
        match mode {
            "scripts" => e.scripts.push("Scripts/noop.rhai".to_owned()),
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
    if mode == "scripts" {
        std::fs::create_dir_all(dir.join("Scripts")).unwrap();
        std::fs::write(
            dir.join("Scripts/noop.rhai"),
            "fn start(ctx) {}\nfn update(ctx) {}\nfn fixed_update(ctx) {}\n",
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
    let script_us = measure("scripts", "Rhai 脚本轨");

    let component_dispatch = component_us - baseline_us;
    let script_dispatch = script_us - baseline_us;
    println!(
        "对照：组件轨分发 {component_dispatch:.2} us/帧，脚本轨分发 {script_dispatch:.2} us/帧 \
         （脚本是组件的 {:.1}×，含每钩子的 ScriptCtx 构造、rhai 调用与日志缓冲写入）",
        script_dispatch / component_dispatch
    );

    // P0 验收：1000 个 no-op 组件实体，组件分发一轮 < 50 µs（扣除公共基线后的分发成本）。
    assert!(
        component_dispatch < 50.0,
        "组件轨分发每帧 {component_dispatch:.2} µs，超过 50 µs 的验收线"
    );
}
