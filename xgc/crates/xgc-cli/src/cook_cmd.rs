//! `xgc cook <项目根> <输出目录>`：复制运行时资产并写 cook.json。

use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

use xgc_asset::{resolve_read_path, AssetCatalog, CookRuntime, PackTarget, SearchPaths};
use xgc_base::path::normalize_project_relative;
use xgc_host::{asset_paths, ScriptedRunner};
use xgc_store::{load_world, load_world_binary, save_world, save_world_binary};
use xgc_world::{builtin_registry, World, WorldData};

use crate::runtime_cmd::load_native_runtime;

fn cook_runtime(root: &Path) -> Result<CookRuntime, String> {
    Ok(load_native_runtime(root)?.unwrap_or_else(|| CookRuntime::standalone(root)))
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
    let root = Path::new(root);
    let world_path = Path::new(world_path);
    // 先执行恢复，读取入参的错误仍保留原来的退出码和优先级。
    let target = PackTarget::prepare(root, world_path, Path::new(out));
    let data = match load_world(world_path) {
        Ok(data) => data,
        Err(err) => {
            eprintln!("读取世界失败：{err}");
            return ExitCode::from(2);
        }
    };
    let runtime = match cook_runtime(root) {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };
    let target = match target {
        Ok(target) => target,
        Err(err) => {
            eprintln!("烹饪失败：{err}");
            return ExitCode::from(1);
        }
    };
    let search = SearchPaths::new(&runtime.search_paths);
    let registry = builtin_registry();
    let (manifest, warning) = match xgc_asset::pack_project(
        &target,
        runtime,
        |root, catalog| referenced_paths(root, &data, catalog, &search, &registry),
        |root, catalog, path| {
            let Ok(prefab) = xgc_store::load_prefab(path) else {
                return BTreeSet::new();
            };
            referenced_paths(root, &prefab, catalog, &search, &registry)
        },
        |package| write_and_check_worlds(package, &data),
    ) {
        Ok(result) => result,
        Err(err) => {
            eprintln!("烹饪失败：{err}");
            return ExitCode::from(1);
        }
    };
    if let Some(warning) = warning {
        eprintln!("{warning}");
    }
    println!(
        "pack: {} 个文件（含 world.json） -> {}",
        manifest.files.len(),
        out
    );
    ExitCode::SUCCESS
}

fn write_and_check_worlds(package: &Path, data: &WorldData) -> Result<(), String> {
    let json = package.join("world.json");
    let binary = package.join("world.xgcb");
    save_world(&json, data).map_err(|err| format!("写入 cooked world 失败：{err}"))?;
    save_world_binary(&binary, data)
        .map_err(|err| format!("写入二进制 cooked world 失败：{err}"))?;
    let text_copy = load_world(&json).map_err(|err| format!("校验 cooked world 失败：{err}"))?;
    let binary_copy =
        load_world_binary(&binary).map_err(|err| format!("校验二进制 cooked world 失败：{err}"))?;
    if text_copy != binary_copy {
        return Err("cooked JSON 与二进制世界内容不同".into());
    }
    World::new()
        .restore_world_data(&binary_copy)
        .map_err(|err| format!("恢复 cooked world 失败：{err}"))
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
