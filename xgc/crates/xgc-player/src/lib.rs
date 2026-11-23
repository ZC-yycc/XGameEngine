//! XGC 独立播放器核心：从 cooked 目录加载世界并驱动脚本。

use std::path::Path;

use xgc_asset::scan;
use xgc_render::MeshCache;
use xgc_script::ScriptRuntime;
use xgc_sim::{apply_root_motion, Simulation};
use xgc_store::load_world;
use xgc_world::{World, WorldData};

pub mod window;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerStats {
    pub frames: u64,
    pub entities: usize,
    pub scripts: usize,
    pub state_hash: u64,
}

/// 固定 60Hz 跑 `frames` 帧；`world.json`、脚本与资产都从 `dir` 读取。
pub fn load_world_dir(dir: &Path) -> Result<WorldData, String> {
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

pub fn runtime_for(dir: &Path) -> ScriptRuntime {
    match scan(dir) {
        Ok(catalog) => ScriptRuntime::with_catalog(dir, catalog),
        Err(_) => ScriptRuntime::new(dir),
    }
}

pub fn run_cooked(dir: &Path, frames: u64) -> Result<PlayerStats, String> {
    let data = load_world_dir(dir)?;
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
