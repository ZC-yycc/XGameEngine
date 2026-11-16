//! `xgc run-world <世界.json> [帧]`：无脚本跑固定步长并打印末帧。

use std::path::Path;
use std::process::ExitCode;

use xgc_migrate::verify_migration as verify_data;
use xgc_sim::Simulation;
use xgc_store::load_world;
use xgc_world::World;

pub fn run_world(path: Option<&str>, frames: Option<&str>) -> ExitCode {
    let Some(path) = path else {
        eprintln!("用法：xgc run-world <世界.json> [帧数]");
        return ExitCode::from(2);
    };
    let frames: u64 = match frames.map(str::parse::<u64>).transpose() {
        Ok(frames) => frames.unwrap_or(60),
        Err(_) => {
            eprintln!("帧数不是整数");
            return ExitCode::from(2);
        }
    };
    let data = match load_world(Path::new(path)) {
        Ok(data) => data,
        Err(err) => {
            eprintln!("加载世界失败：{err}");
            return ExitCode::from(2);
        }
    };
    let mut world = World::new();
    if let Err(err) = world.restore_world_data(&data) {
        eprintln!("恢复世界失败：{err}");
        return ExitCode::from(2);
    }
    let mut simulation = Simulation::new(world);
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
