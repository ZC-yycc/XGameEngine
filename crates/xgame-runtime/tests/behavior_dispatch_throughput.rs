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

use xgame_core::{Entity, Scene};
use xgame_runtime::{BehaviorRegistry, Lifecycle, World, WorldConfig};

const N: usize = 1000;
const FRAMES: usize = 300;

/// no-op 组件：四个钩子都用默认空实现，量的就是纯分发开销。
#[derive(Default)]
struct Noop;
impl Lifecycle for Noop {}

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
