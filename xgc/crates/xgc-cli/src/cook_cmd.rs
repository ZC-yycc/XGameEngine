//! `xgc cook <项目根> <输出目录>`：复制运行时资产并写 cook.json。

use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

use xgc_asset::{resolve_read_path, AssetCatalog, CookRuntime, SearchPaths};
use xgc_base::path::normalize_project_relative;
use xgc_host::{asset_paths, ScriptedRunner};
use xgc_store::{load_world, save_world, save_world_binary};
use xgc_world::{builtin_registry, World, WorldData};

use crate::runtime_cmd::load_native_project;

fn cook_runtime(root: &Path) -> Result<CookRuntime, String> {
    Ok(match load_native_project(root)? {
        Some(project) => CookRuntime {
            search_paths: project.search_paths,
            input: project.input,
        },
        None => CookRuntime::standalone(root),
    })
}

pub fn cook(root: Option<&str>, out: Option<&str>) -> ExitCode {
    let (Some(root), Some(out)) = (root, out) else {
        eprintln!("用法：xgc cook <项目根> <输出目录>");
        return ExitCode::from(2);
    };
    let root = Path::new(root);
    let runtime = match cook_runtime(root) {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };
    match xgc_asset::cook_project_with_runtime(root, Path::new(out), runtime, None) {
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
    let root = Path::new(root);
    let runtime = match cook_runtime(root) {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };
    let search = SearchPaths::new(&runtime.search_paths);
    let registry = builtin_registry();
    let mut manifest = match xgc_asset::cook_project_with_dependencies(
        root,
        Path::new(out),
        runtime,
        |root, catalog| referenced_paths(root, &data, catalog, &search, &registry),
        |root, catalog, path| {
            let Ok(prefab) = xgc_store::load_prefab(path) else {
                return BTreeSet::new();
            };
            referenced_paths(root, &prefab, catalog, &search, &registry)
        },
    ) {
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
    if let Err(err) = xgc_asset::write_manifest(Path::new(out), &mut manifest) {
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
    let search_paths = match xgc_asset::verify_manifest(dir) {
        Ok(manifest) => manifest.map_or_else(Vec::new, |manifest| manifest.runtime.search_paths),
        Err(differences) => {
            for line in &differences {
                eprintln!("{line}");
            }
            return ExitCode::from(1);
        }
    };
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
    let (mut runner, _) = ScriptedRunner::with_assets(world, asset_paths(Some(dir), &search_paths));
    for _ in 0..frames {
        if let Err(err) = runner.step() {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    }
    let world = runner.world();
    println!(
        "run-cooked: frames={frames} entities={} scripts={} hash={:016x}",
        world.len(),
        runner.script_count(),
        world.state_hash()
    );
    ExitCode::SUCCESS
}

fn referenced_paths(
    root: &Path,
    data: &WorldData,
    catalog: &AssetCatalog,
    search: &SearchPaths,
    registry: &xgc_reflect::Registry,
) -> BTreeSet<String> {
    let mut keep = BTreeSet::new();
    let direct = SearchPaths::default();
    for component in data.entities.iter().flat_map(|entity| &entity.components) {
        if let Some(schema) = registry.get(component.component) {
            schema.visit_asset_references(&component.fields, |kind, _, reference| {
                let search = if kind.uses_search_paths() {
                    search
                } else {
                    &direct
                };
                let Ok(path) = resolve_read_path(root, reference, search, Some(catalog)) else {
                    return;
                };
                if let Some(relative) = path
                    .strip_prefix(root)
                    .ok()
                    .and_then(Path::to_str)
                    .and_then(|path| normalize_project_relative(path).ok())
                {
                    keep.insert(relative);
                }
            });
        }
    }
    keep
}
