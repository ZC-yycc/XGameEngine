//! XGC 独立播放器核心：从 cooked 目录加载世界并驱动脚本。

use std::path::Path;

use xgc_asset::scan;
use xgc_script::ScriptRuntime;
use xgc_sim::Simulation;
use xgc_store::load_world;
use xgc_world::World;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerStats {
    pub frames: u64,
    pub entities: usize,
    pub scripts: usize,
    pub state_hash: u64,
}

/// 固定 60Hz 跑 `frames` 帧；`world.json`、脚本与资产都从 `dir` 读取。
pub fn run_cooked(dir: &Path, frames: u64) -> Result<PlayerStats, String> {
    let data = load_world(&dir.join("world.json"))
        .map_err(|err| format!("读取 cooked world 失败：{err}"))?;
    let mut world = World::new();
    world
        .restore_world_data(&data)
        .map_err(|err| format!("恢复世界失败：{err}"))?;
    let mut runtime = match scan(dir) {
        Ok(catalog) => ScriptRuntime::with_catalog(dir, catalog),
        Err(_) => ScriptRuntime::new(dir),
    };
    let mut simulation = Simulation::new(world);
    for _ in 0..frames {
        simulation
            .step_with(Vec::new(), |world| runtime.run_frame(world, 1.0 / 60.0))
            .map_err(|err| format!("运行失败：{err:?}"))?;
    }
    let world = simulation.world();
    Ok(PlayerStats {
        frames,
        entities: world.len(),
        scripts: runtime.entity_count(),
        state_hash: world.state_hash(),
    })
}
