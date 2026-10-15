//! P2.5 语言闸门：Rhai vs Lua 三负载对比（决策规则事先声明，见计划文档 §六）。
//!
//! 默认 `#[ignore]`（手动运行的报告型基准，不进 CI 门禁）。手动跑：
//!
//! ```sh
//! cargo test -p xgame-runtime --release --test language_gate -- --ignored --nocapture
//! ```
//!
//! # 量的是什么（三负载与计划文档 §六 一一对应）
//!
//! | # | 负载 | 测什么 | 脚本内容 |
//! |---|---|---|---|
//! | 1 | no-op dispatch | 宿主调度 + 边界开销 | 四钩子全空 |
//! | 2 | API 吞吐 | API 边界穿越 | `update` 里 K 次 `pos` / `set_pos` / `action_pressed` |
//! | 3 | 纯计算 | 解释器本体 | `update` 里 I 次纯算术 while 循环（收尾一次 `set_pos` 用掉结果） |
//!
//! 每负载 **Rhai → Lua 先后同进程测**（背靠背，机器状态可比），打印比值
//! `Rhai / Lua`（>1 = Lua 更快）。与 `behavior_dispatch_throughput` 的分工：
//! 那边是"组件轨 vs 脚本轨"的引擎验收记录（有历史对照表），这边是两语言
//! 的**对决报告** —— 负载 1 的脚本同型照抄（no-op 四钩子），数字可互相印证。
//!
//! # 闸门规则（跑之前就定死，见计划文档 §六）
//!
//! - Lua **三负载全占优** → P3 删 Rhai（样例 / 文档迁 Lua）；
//! - 任何一负载 Rhai 反超 >20%（比值 < 0.833）→ 保留双宿主并重新评估。
//!
//! 本测试只打印判定、不 assert —— 闸门是**给人下决策**的报告，红一条断言
//! 说明不了语言优劣；结论由实施记录（PLAN.md）引用这里的数字后落笔。

use xgame_core::{Entity, Scene};
use xgame_runtime::{LogLevel, World, WorldConfig};

const FRAMES: usize = 300;
/// 预热帧数（脚本编译 / 实例化在内跑完，测量段不掺首次成本）。
const WARMUP: usize = 30;

/// 负载 1：实体数（与 `behavior_dispatch_throughput` 同规模，数字可互相印证）。
const N_DISPATCH: usize = 1000;
/// 负载 2：实体数与每实体每帧的 API 调用组数（一组 = pos + set_pos + action_pressed）。
const N_API: usize = 500;
const K_API: usize = 20;
/// 负载 3：实体数与每实体每帧的算术迭代数（Rhai 解释器慢，控制单次总时长）。
const N_COMPUTE: usize = 200;
const I_COMPUTE: usize = 1000;

// 三组脚本：每组 Rhai / Lua 逐语句同型 —— 两档唯一的差别只能是宿主与解释器。

const DISPATCH_RHAI: &str = "\
fn start(ctx) {}
fn update(ctx) {}
fn fixed_update(ctx) {}
fn on_destroy(ctx) {}
";

const DISPATCH_LUA: &str = "\
function start(ctx) end
function update(ctx) end
function fixed_update(ctx) end
function on_destroy(ctx) end
";

// 注意：Rhai 变量**天然可变**，没有 `mut` 关键字（写 `let mut x` 会把 `mut`
// 当变量名解析后撞上下一个 token 报编译错 —— 首跑就栽在这，假数据让 Rhai
// "赢"了两负载）。下面两组脚本用 K_API / I_COMPUTE 拼出来，两轨同源同循环数。

fn api_rhai() -> String {
    format!(
        "
fn update(ctx) {{
    let n = 0;
    while n < {K_API} {{
        let p = pos(ctx);
        set_pos(ctx, p.x, p.y, p.z);
        if action_pressed(ctx, \"Jump\") {{ log(ctx, \"unreachable\"); }}
        n += 1;
    }}
}}
"
    )
}

fn api_lua() -> String {
    format!(
        "
function update(ctx)
    local n = 0
    while n < {K_API} do
        local p = pos(ctx)
        set_pos(ctx, p.x, p.y, p.z)
        if action_pressed(ctx, \"Jump\") then log(ctx, \"unreachable\") end
        n = n + 1
    end
end
"
    )
}

fn compute_rhai() -> String {
    format!(
        "
fn update(ctx) {{
    let x = 0.0;
    let i = 0;
    while i < {I_COMPUTE} {{
        x += 1.0;
        i += 1;
    }}
    set_pos(ctx, x * 0.0, 0.0, 0.0);
}}
"
    )
}

fn compute_lua() -> String {
    format!(
        "
function update(ctx)
    local x = 0.0
    local i = 0
    while i < {I_COMPUTE} do
        x = x + 1.0
        i = i + 1
    end
    set_pos(ctx, x * 0.0, 0.0, 0.0)
end
"
    )
}

/// 一条轨（决定挂哪种扩展名的脚本、以及温度量哪一侧）。
#[derive(Clone, Copy)]
enum Track {
    Rhai,
    Lua,
}

impl Track {
    fn ext(self) -> &'static str {
        match self {
            Track::Rhai => "rhai",
            Track::Lua => "lua",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Track::Rhai => "Rhai",
            Track::Lua => "Lua",
        }
    }
}

/// 造 `n` 实体场景，每实体挂同一个脚本（两轨路径仅扩展名不同）。
fn build_scene(n: usize, track: Track) -> Scene {
    let mut scene = Scene::new("gate");
    let path = format!("Scripts/bench.{}", track.ext());
    for i in 0..n {
        let mut e = Entity::new(format!("e{i}"));
        e.scripts.push(path.clone());
        scene.add_entity(e);
    }
    scene
}

/// 量一档：写脚本 → 建 World → 预热 → 计时 `FRAMES` 帧，返回每帧微秒数。
fn measure(track: Track, source: &str, n: usize, label: &str) -> f64 {
    let dir = std::env::temp_dir().join(format!(
        "xgame_gate_{}_{}_{}",
        std::process::id(),
        track.ext(),
        label
    ));
    std::fs::create_dir_all(dir.join("Scripts")).unwrap();
    std::fs::write(
        dir.join(format!("Scripts/bench.{}", track.ext())),
        source,
    )
    .unwrap();

    let mut world = World::new(build_scene(n, track), &dir, WorldConfig::default());
    for _ in 0..WARMUP {
        world.update(1.0 / 60.0);
    }
    // 编译/运行错误守卫：脚本没真跑时测出来的"快"是假数据（首跑就栽在这——
    // Rhai 脚本 `let mut` 编译失败，负载 2/3 量到的只剩空调度，Rhai 假胜出）。
    // Error 级日志 = 有脚本没执行或中途炸了 → 直接红，不许端到端通过。
    let errors: Vec<String> = world
        .take_logs()
        .into_iter()
        .filter(|l| matches!(l.level, LogLevel::Error))
        .map(|l| format!("{:?}: {}", l.script, l.message))
        .collect();
    assert!(
        errors.is_empty(),
        "闸门[{label}/{}] 预热段有 {} 条错误日志（脚本没真跑，数据无效）: {:?}",
        track.label(),
        errors.len(),
        errors,
    );

    let start = std::time::Instant::now();
    for _ in 0..FRAMES {
        world.update(1.0 / 60.0);
    }
    let elapsed = start.elapsed();

    let per_frame_us = elapsed.as_secs_f64() * 1e6 / FRAMES as f64;
    println!(
        "language_gate[{label}/{}]: {n} 实体 × {FRAMES} 帧 = {:.3} ms 总，{per_frame_us:.2} us/帧",
        track.label(),
        elapsed.as_secs_f64() * 1e3,
    );

    let _ = std::fs::remove_dir_all(&dir);
    per_frame_us
}

/// 打印一负载的对比与比值，返回比值（Rhai / Lua；>1 = Lua 更快）。
fn compare(load_name: &str, rhai_us: f64, lua_us: f64) -> f64 {
    let ratio = rhai_us / lua_us;
    println!(
        "  负载 {load_name}: Rhai {rhai_us:.2} vs Lua {lua_us:.2} us/帧 → Rhai/Lua = {ratio:.2}×"
    );
    ratio
}

#[test]
#[ignore = "手动运行的报告型基准，不进 CI"]
fn language_gate() {
    println!("── P2.5 语言闸门：Rhai vs Lua 三负载（同进程背靠背，比值 >1 = Lua 更快）──");

    // 负载 1：no-op dispatch（宿主调度 + 边界开销）。
    let d_rhai = measure(Track::Rhai, DISPATCH_RHAI, N_DISPATCH, "dispatch");
    let d_lua = measure(Track::Lua, DISPATCH_LUA, N_DISPATCH, "dispatch");
    let r1 = compare("1 no-op dispatch", d_rhai, d_lua);

    // 负载 2：API 边界穿越（每实体每帧 K 组 pos + set_pos + action_pressed）。
    let api_rhai = api_rhai();
    let api_lua = api_lua();
    let a_rhai = measure(Track::Rhai, &api_rhai, N_API, "api");
    let a_lua = measure(Track::Lua, &api_lua, N_API, "api");
    let r2 = compare("2 API 吞吐", a_rhai, a_lua);

    // 负载 3：纯计算（解释器本体）。
    let compute_rhai = compute_rhai();
    let compute_lua = compute_lua();
    let c_rhai = measure(Track::Rhai, &compute_rhai, N_COMPUTE, "compute");
    let c_lua = measure(Track::Lua, &compute_lua, N_COMPUTE, "compute");
    let r3 = compare("3 纯计算", c_rhai, c_lua);

    // 闸门判定（规则见计划文档 §六；阈值 0.833 = Rhai 反超 20% 的倒数边界）。
    const THRESHOLD: f64 = 1.0 / 1.2;
    let ratios = [("1 no-op dispatch", r1), ("2 API 吞吐", r2), ("3 纯计算", r3)];
    let lua_all_better = ratios.iter().all(|&(_, r)| r > 1.0);
    let rhai_competitive: Vec<&str> = ratios
        .iter()
        .filter(|&&(_, r)| r < THRESHOLD)
        .map(|&(name, _)| name)
        .collect();

    println!("── 闸门判定（规则事先声明：三负载全占优 → 删 Rhai；任一负载 Rhai 反超 >20% → 保留双宿主）──");
    if lua_all_better {
        println!("结果：Lua 三负载全占优 → 按规则指向 P3「删 Rhai、样例/文档迁 Lua」。");
    } else if !rhai_competitive.is_empty() {
        println!(
            "结果：Rhai 在负载 [{}] 反超 >20% → 按规则保留双宿主并重新评估。",
            rhai_competitive.join(", ")
        );
    } else {
        println!(
            "结果：Lua 未三负载全占优，但 Rhai 也没在任一负载反超 >20%（差距都在 20% 内）→ \
             规则未覆盖的中间地带，需人工裁定（记进 PLAN.md 决策记录）。"
        );
    }
}
