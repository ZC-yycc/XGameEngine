//! `xgc trace <世界.json> [帧] [资产根]`：逐帧打印状态哈希，用于和重放 / 旧引擎对照。
//!
//! 不带资产根时只跑无脚本固定步长（与 `run-world` 共用同一条"读世界 → 建 Simulation →
//! 步进"路径）；带资产根时改走 `ScriptedRunner`（与 `run-scripts` 同一条"系统 → 脚本 →
//! 粒子"路径），脚本帧与无脚本帧因此能直接对比，而不是拿两个各写一遍的循环结果比较。
//! 输出必须逐字节确定——每行只放数值，不打印时间 / 路径，才能直接 diff 两个运行。
//!
//! `frame=N` 的含义是**已推进 N 步**：第 0 帧是装载后、未步进的状态
//! （`Simulation::new` 只建调度，不碰世界），所以接脚本 trace 或与旧端逐帧对照时
//! 要按这个口径对齐，不能把行号当帧号再错一位。

use std::path::Path;
use std::process::ExitCode;

use super::runtime_cmd::ScriptedRunner;
use super::world_cmd::{load_simulation, load_world_state, parse_frames};
use xgc_world::World;

/// 打印一行帧状态。`hash` 是 `World::state_hash()`，与 `run-world` / `run-scripts`
/// 末行同一口径；实体数 / revision 一起带出来，哈希变了能立刻看出是"数量变了"还是
/// "取值变了"。
fn print_frame(frame: u64, world: &World) {
    println!(
        "frame={frame} entities={} revision={} hash={:016x}",
        world.len(),
        world.revision(),
        world.state_hash()
    );
}

fn print_summary(frames: u64, world: &World) {
    println!(
        "trace: frames={frames} entities={} revision={} hash={:016x}",
        world.len(),
        world.revision(),
        world.state_hash()
    );
}

pub fn trace(path: Option<&str>, frames: Option<&str>, assets_root: Option<&str>) -> ExitCode {
    let Some(path) = path else {
        eprintln!("用法：xgc trace <世界.json> [帧数] [资产根]");
        return ExitCode::from(2);
    };
    let frames = match parse_frames(frames) {
        Ok(frames) => frames,
        Err(code) => return code,
    };
    match assets_root {
        Some(assets_root) => trace_scripts(path, frames, Path::new(assets_root)),
        None => trace_plain(path, frames),
    }
}

/// 无脚本逐帧 trace：与 `run-world` 同一条路径。
fn trace_plain(path: &str, frames: u64) -> ExitCode {
    let mut simulation = match load_simulation(path) {
        Ok(simulation) => simulation,
        Err(code) => return code,
    };
    // 第 0 帧是装载后的原始状态：对照时它回答"输入世界是否一致"，
    // 后面的帧才回答"同一步进序列是否产生同一轨迹"。
    print_frame(0, simulation.world());
    for frame in 1..=frames {
        if let Err(err) = simulation.step(Vec::new()) {
            eprintln!("运行失败：{err:?}");
            return ExitCode::from(2);
        }
        print_frame(frame, simulation.world());
    }
    print_summary(frames, simulation.world());
    ExitCode::SUCCESS
}

/// 带脚本逐帧 trace：与 `run-scripts` 同一条 `ScriptedRunner` 路径。
///
/// 这里不先跑完再打印：脚本可能读上一帧位置 / 时间，逐帧读世界与真实运行等价，
/// 而"跑完再看最终状态"会把中间帧的差异丢掉。
fn trace_scripts(path: &str, frames: u64, assets_root: &Path) -> ExitCode {
    let world = match load_world_state(path) {
        Ok(world) => world,
        Err(code) => return code,
    };
    let mut runner = ScriptedRunner::new(world, assets_root);
    print_frame(0, runner.world());
    for frame in 1..=frames {
        if let Err(err) = runner.step() {
            eprintln!("第 {frame} 帧运行失败：{err}");
            return ExitCode::from(2);
        }
        print_frame(frame, runner.world());
    }
    print_summary(frames, runner.world());
    ExitCode::SUCCESS
}
