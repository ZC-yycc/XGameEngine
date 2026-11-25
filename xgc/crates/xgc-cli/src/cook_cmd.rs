//! `xgc cook <项目根> <输出目录>`：复制运行时资产并写 cook.json。

use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

use xgc_asset::scan;
use xgc_base::path::normalize_project_relative;
use xgc_script::ScriptRuntime;
use xgc_sim::Simulation;
use xgc_store::{load_world, save_world, save_world_binary};
use xgc_world::{Component, LodGroup, World, WorldData};

use crate::assets_cmd::lod_models;

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
    let keep = referenced_paths(Path::new(root), &data);
    let mut manifest =
        match xgc_asset::cook_project_filtered(Path::new(root), Path::new(out), keep.as_ref()) {
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
    if let Err(err) = save_world_binary(&Path::new(out).join("world.xgcb"), &data) {
        eprintln!("写入二进制 cooked world 失败：{err}");
        return ExitCode::from(1);
    }
    for name in ["world.json", "world.xgcb"] {
        match xgc_asset::file_entry(Path::new(out), name) {
            Ok(entry) => manifest.files.push(entry),
            Err(err) => {
                eprintln!("{err}");
                return ExitCode::from(1);
            }
        }
    }
    manifest.files.sort_by(|a, b| a.path.cmp(&b.path));
    if let Err(err) = xgc_asset::write_manifest(Path::new(out), &manifest) {
        eprintln!("写入 manifest 失败：{err}");
        return ExitCode::from(1);
    }
    println!(
        "pack: {} 个文件（含 world.json） -> {}",
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
    if let Err(differences) = xgc_asset::verify_manifest(dir) {
        for line in &differences {
            eprintln!("{line}");
        }
        return ExitCode::from(1);
    }
    let frames: u64 = match frames.map(str::parse::<u64>).transpose() {
        Ok(frames) => frames.unwrap_or(60),
        Err(_) => {
            eprintln!("帧数不是整数");
            return ExitCode::from(2);
        }
    };
    let data = match if dir.join("world.xgcb").exists() {
        xgc_store::load_world_binary(&dir.join("world.xgcb"))
    } else {
        load_world(&dir.join("world.json"))
    } {
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

fn referenced_paths(root: &Path, data: &WorldData) -> Option<BTreeSet<String>> {
    let catalog = scan(root).ok()?;
    let mut keep = BTreeSet::new();
    for entity in &data.entities {
        for component in &entity.components {
            let guids = component.fields.iter().filter_map(|(_, value)| {
                value
                    .as_string()
                    .filter(|value| value.starts_with("guid://"))
            });
            let lod = (component.component == LodGroup::ID)
                .then(|| lod_models(component))
                .into_iter()
                .flatten()
                .map(|(_, model)| model);
            for reference in guids.chain(lod) {
                if reference.starts_with("guid://") {
                    let Some(path) = catalog.resolve(reference) else {
                        continue;
                    };
                    if let Ok(relative) = path.strip_prefix(root) {
                        keep.insert(relative.to_string_lossy().replace('\\', "/"));
                    }
                } else if let Ok(relative) = normalize_project_relative(reference) {
                    keep.insert(relative);
                }
            }
        }
    }
    Some(keep)
}
