//! XGC 独立播放器核心：从 cooked 目录加载世界并驱动脚本。

use std::path::Path;

use xgc_asset::scan;
use xgc_render::MeshCache;
use xgc_script::ScriptRuntime;
use xgc_sim::{apply_root_motion, Simulation};
use xgc_store::{load_world, ProjectSession};
use xgc_world::{World, WorldData};

mod input;
pub mod window;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerStats {
    pub frames: u64,
    pub entities: usize,
    pub scripts: usize,
    pub state_hash: u64,
}

/// 固定 60Hz 跑 `frames` 帧；`world.json`、脚本与资产都从 `dir` 读取。
///
/// `dir` 里有 `project.xgc` 时按项目打开：入口场景来自清单，资产与脚本根
/// 仍是 `dir`。项目与 cooked 目录都不认单文件参数——资产相对路径需要一个根。
pub fn load_world_dir(dir: &Path) -> Result<WorldData, String> {
    load_world_dir_scene(dir, None)
}

/// 同 [`load_world_dir`]，但项目目录可以按清单登记名（或唯一文件名主干）选场景。
///
/// cooked 目录没有场景清单，传 `Some` 直接报错，避免“选了场景却还是入口”的静默。
pub fn load_world_dir_scene(dir: &Path, scene: Option<&str>) -> Result<WorldData, String> {
    if dir.join("project.xgc").is_file() {
        let session = ProjectSession::open(dir).map_err(|err| format!("读取项目失败：{err}"))?;
        return match scene {
            Some(name) => {
                let scene = resolve_scene_name(&session, name)?;
                session
                    .load_scene(&scene)
                    .map_err(|err| format!("读取场景 {scene} 失败：{err}"))
            }
            None => session
                .load_entry()
                .map_err(|err| format!("读取入口场景失败：{err}")),
        };
    }
    if scene.is_some() {
        return Err("cooked 目录没有场景清单，不能按名字选择场景".to_string());
    }
    xgc_asset::verify_manifest(dir)
        .map_err(|differences| format!("cooked 文件校验失败：{}", differences.join("；")))?;
    let binary = dir.join("world.xgcb");
    if binary.exists() {
        xgc_store::load_world_binary(&binary)
    } else {
        load_world(&dir.join("world.json"))
    }
    .map_err(|err| format!("读取 cooked world 失败：{err}"))
}

/// 场景名解析：先按清单登记名精确匹配，再按唯一文件名主干匹配（`character` →
/// `scenes/character.json`）。清单里出现多个同名主干时取登记顺序第一个。
fn resolve_scene_name(session: &ProjectSession, requested: &str) -> Result<String, String> {
    let scenes = &session.manifest().scenes;
    if scenes.iter().any(|scene| scene == requested) {
        return Ok(requested.to_string());
    }
    let mut matches = scenes.iter().filter(|scene| {
        Path::new(scene)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem == requested)
    });
    match (matches.next(), matches.next()) {
        (Some(scene), None) => Ok(scene.clone()),
        (Some(_), Some(_)) => Err(format!(
            "场景名 {requested} 匹配到多个文件，请写清单里的完整路径"
        )),
        _ => Err(format!(
            "项目清单里没有场景 {requested}；已登记：{}",
            scenes.join(", ")
        )),
    }
}

pub fn runtime_for(dir: &Path) -> ScriptRuntime {
    match scan(dir) {
        Ok(catalog) => ScriptRuntime::with_catalog(dir, catalog),
        Err(_) => ScriptRuntime::new(dir),
    }
}

pub fn run_cooked(dir: &Path, frames: u64) -> Result<PlayerStats, String> {
    run_cooked_scene(dir, frames, None)
}

/// 同 [`run_cooked`]，项目目录可以指定场景；cooked 目录传 `Some` 报错。
pub fn run_cooked_scene(
    dir: &Path,
    frames: u64,
    scene: Option<&str>,
) -> Result<PlayerStats, String> {
    let data = load_world_dir_scene(dir, scene)?;
    let mut world = World::new();
    world
        .restore_world_data(&data)
        .map_err(|err| format!("恢复世界失败：{err}"))?;
    let mut runtime = runtime_for(dir);
    // 宿主必须和 CLI / editor 用同一张根运动表：漏接会让同一份世界在两种宿主里
    // 跑出不同的角色位置（旧端 `apply_root_motion` 在动画之后、渲染之前）。
    let root_motion = {
        let mut meshes = MeshCache::new(dir);
        meshes.root_motion_library(&world)
    };
    let mut simulation = Simulation::new(world);
    for _ in 0..frames {
        simulation
            .step_with(Vec::new(), |world| runtime.run_frame(world, 1.0 / 60.0))
            .map_err(|err| format!("运行失败：{err:?}"))?;
        apply_root_motion(simulation.world_mut(), 1.0 / 60.0, &root_motion);
    }
    let world = simulation.world();
    Ok(PlayerStats {
        frames,
        entities: world.len(),
        scripts: runtime.entity_count(),
        state_hash: world.state_hash(),
    })
}
