//! `xgc run-world <世界.json> [帧]`：无脚本跑固定步长并打印末帧。
//!
//! 逐帧命令（`run-world` / `trace`）共用 `parse_frames` + `load_simulation`：
//! 两条命令必须是同一条"读世界 → 建 Simulation → 步进"路径，否则 trace 与
//! run-world 会在没有人察觉的情况下分叉（脚本帧、层级重算这类差异最容易漏）。

use std::path::Path;
use std::process::ExitCode;

use xgc_migrate::verify_migration as verify_data;
use xgc_sim::Simulation;
use xgc_store::load_world;
use xgc_world::World;

/// 解析 `[帧数]` 参数：缺省 60 帧；非整数打印提示并返回退出码 2。
pub(crate) fn parse_frames(frames: Option<&str>) -> Result<u64, ExitCode> {
    match frames.map(str::parse::<u64>).transpose() {
        Ok(frames) => Ok(frames.unwrap_or(60)),
        Err(_) => {
            eprintln!("帧数不是整数");
            Err(ExitCode::from(2))
        }
    }
}

/// 读世界文件并装成可步进的 `Simulation`（load_world → restore_world_data → Simulation::new）。
pub(crate) fn load_simulation(path: &str) -> Result<Simulation, ExitCode> {
    let data = match load_world(Path::new(path)) {
        Ok(data) => data,
        Err(err) => {
            eprintln!("加载世界失败：{err}");
            return Err(ExitCode::from(2));
        }
    };
    let mut world = World::new();
    if let Err(err) = world.restore_world_data(&data) {
        eprintln!("恢复世界失败：{err}");
        return Err(ExitCode::from(2));
    }
    Ok(Simulation::new(world))
}

pub fn run_world(path: Option<&str>, frames: Option<&str>) -> ExitCode {
    let Some(path) = path else {
        eprintln!("用法：xgc run-world <世界.json> [帧数]");
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
    for _ in 0..frames {
        if let Err(err) = simulation.step(Vec::new()) {
            eprintln!("运行失败：{err:?}");
            return ExitCode::from(2);
        }
    }
    let world = simulation.world();
    println!(
        "run-world: frames={frames} entities={} revision={} hash={:016x}",
        world.len(),
        world.revision(),
        world.state_hash()
    );
    ExitCode::SUCCESS
}

/// `xgc verify-migration <旧场景.ron> <世界.json>`：逐实体 / 字段对比。
pub fn verify(old: Option<&str>, new: Option<&str>) -> ExitCode {
    let (Some(old), Some(new)) = (old, new) else {
        eprintln!("用法：xgc verify-migration <旧场景.ron> <世界.json>");
        return ExitCode::from(2);
    };
    let text = match std::fs::read_to_string(Path::new(old)) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("读取旧场景失败：{err}");
            return ExitCode::from(2);
        }
    };
    let data = match load_world(Path::new(new)) {
        Ok(data) => data,
        Err(err) => {
            eprintln!("读取世界失败：{err}");
            return ExitCode::from(2);
        }
    };
    match verify_data(&text, &data) {
        Ok(()) => {
            println!("verify-migration: 一致");
            ExitCode::SUCCESS
        }
        Err(differences) => {
            for line in &differences {
                eprintln!("{line}");
            }
            eprintln!("verify-migration: {} 处差异", differences.len());
            ExitCode::from(1)
        }
    }
}
