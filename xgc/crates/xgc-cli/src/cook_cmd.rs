//! `xgc cook <项目根> <输出目录>`：复制运行时资产并写 cook.json。

use std::path::Path;
use std::process::ExitCode;

use xgc_asset::scan;
use xgc_script::ScriptRuntime;
use xgc_sim::Simulation;
use xgc_store::{load_world, save_world};
use xgc_world::World;

pub fn cook(root: Option<&str>, out: Option<&str>) -> ExitCode {
    let (Some(root), Some(out)) = (root, out) else {
        eprintln!("用法：xgc cook <项目根> <输出目录>");
        return ExitCode::from(2);
    };
    match xgc_asset::cook_project(Path::new(root), Path::new(out)) {
        Ok(manifest) => {
            let bytes: u64 = manifest.files.iter().map(|file| file.size).sum();
            println!(
                "cook: {} 个文件，{} 字节 -> {}",
                manifest.files.len(),
                bytes,
                out
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("烹饪失败：{err}");
            ExitCode::from(1)
        }
    }
}

/// `xgc pack <世界.json> <项目根> <输出目录>`：烹饪资产 + 写 cooked world。
pub fn pack(world_path: Option<&str>, root: Option<&str>, out: Option<&str>) -> ExitCode {
    let (Some(world_path), Some(root), Some(out)) = (world_path, root, out) else {
        eprintln!("用法：xgc pack <世界.json> <项目根> <输出目录>");
        return ExitCode::from(2);
    };
    let data = match load_world(Path::new(world_path)) {
        Ok(data) => data,
        Err(err) => {
            eprintln!("读取世界失败：{err}");
            return ExitCode::from(2);
        }
    };
    let manifest = match xgc_asset::cook_project(Path::new(root), Path::new(out)) {
        Ok(manifest) => manifest,
        Err(err) => {
            eprintln!("烹饪失败：{err}");
            return ExitCode::from(1);
        }
    };
    if let Err(err) = save_world(&Path::new(out).join("world.json"), &data) {
        eprintln!("写入 cooked world 失败：{err}");
        return ExitCode::from(1);
    }
    println!(
        "pack: {} 个资产 + world.json -> {}",
        manifest.files.len(),
        out
    );
    ExitCode::SUCCESS
}

/// `xgc run-cooked <输出目录> [帧]`：从 cooked 目录加载世界并跑脚本。
pub fn run_cooked(dir: Option<&str>, frames: Option<&str>) -> ExitCode {
    let Some(dir) = dir else {
        eprintln!("用法：xgc run-cooked <输出目录> [帧数]");
        return ExitCode::from(2);
    };
    let dir = Path::new(dir);
    let frames: u64 = match frames.map(str::parse::<u64>).transpose() {
        Ok(frames) => frames.unwrap_or(60),
        Err(_) => {
            eprintln!("帧数不是整数");
            return ExitCode::from(2);
        }
    };
    let data = match load_world(&dir.join("world.json")) {
        Ok(data) => data,
        Err(err) => {
            eprintln!("读取 cooked world 失败：{err}");
            return ExitCode::from(2);
        }
    };
    let mut world = World::new();
    if let Err(err) = world.restore_world_data(&data) {
        eprintln!("恢复世界失败：{err}");
        return ExitCode::from(2);
    }
    let mut runtime = match scan(dir) {
        Ok(catalog) => ScriptRuntime::with_catalog(dir, catalog),
        Err(_) => ScriptRuntime::new(dir),
    };
    let mut simulation = Simulation::new(world);
    for _ in 0..frames {
        if let Err(err) =
            simulation.step_with(Vec::new(), |world| runtime.run_frame(world, 1.0 / 60.0))
        {
            eprintln!("运行失败：{err:?}");
            return ExitCode::from(2);
        }
    }
    let world = simulation.world();
    println!(
        "run-cooked: frames={frames} entities={} scripts={} hash={:016x}",
        world.len(),
        runtime.entity_count(),
        world.state_hash()
    );
    ExitCode::SUCCESS
}
