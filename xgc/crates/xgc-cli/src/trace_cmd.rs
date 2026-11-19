//! `xgc trace <世界.json> [帧]`：逐帧打印状态哈希，用于和重放 / 旧引擎对照。
//!
//! 只做无脚本固定步长（与 `run-world` 共用同一条"读世界 → 建 Simulation → 步进"路径）：
//! 脚本帧由 `run-scripts` 覆盖，逐帧 trace 先钉住"确定性运行时"这一层，脚本行为留给后续切片。
//! 输出必须逐字节确定——每行只放数值，不打印时间 / 路径，才能直接 diff 两个运行。
//!
//! `frame=N` 的含义是**已推进 N 步**：第 0 帧是装载后、未步进的状态
//! （`Simulation::new` 只建调度，不碰世界），所以接脚本 trace 或与旧端逐帧对照时
//! 要按这个口径对齐，不能把行号当帧号再错一位。

use std::process::ExitCode;

use super::world_cmd::{load_simulation, parse_frames};
use xgc_world::World;

/// 打印一行帧状态。`hash` 是 `World::state_hash()`，与 `run-world` 末行同一口径；
/// 实体数 / revision 一起带出来，哈希变了能立刻看出是"数量变了"还是"取值变了"。
fn print_frame(frame: u64, world: &World) {
    println!(
        "frame={frame} entities={} revision={} hash={:016x}",
        world.len(),
        world.revision(),
        world.state_hash()
    );
}

pub fn trace(path: Option<&str>, frames: Option<&str>) -> ExitCode {
    let Some(path) = path else {
        eprintln!("用法：xgc trace <世界.json> [帧数]");
        return ExitCode::from(2);
    };
    let frames = match parse_frames(frames) {
        Ok(frames) => frames,
        Err(code) => return code,
    };
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
    let world = simulation.world();
    println!(
        "trace: frames={frames} entities={} revision={} hash={:016x}",
        world.len(),
        world.revision(),
        world.state_hash()
    );
    ExitCode::SUCCESS
}
