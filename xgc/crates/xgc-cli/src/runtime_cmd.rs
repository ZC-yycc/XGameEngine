//! CLI 运行宿主：世界恢复后统一驱动固定步系统与脚本。

use std::path::Path;
use std::process::ExitCode;

use xgc_asset::scan as scan_assets;
use xgc_script::ScriptRuntime;
use xgc_sim::{ParticleSystem, Simulation};
use xgc_store::load_world;
use xgc_world::World;

pub(crate) fn advance_world(
    world: World,
    assets_root: &Path,
    frames: u64,
) -> Result<World, String> {
    advance_world_with_particles(world, assets_root, frames).map(|(world, _)| world)
}

/// 固定帧推进，并在每帧系统 / 脚本之后同步推进粒子。
///
/// 粒子运行态不落进 `WorldData`；截图宿主要拿最终四边形，而普通 headless 宿主只丢掉它。
pub(crate) fn advance_world_with_particles(
    world: World,
    assets_root: &Path,
    frames: u64,
) -> Result<(World, Vec<xgc_world::ParticleQuad>), String> {
    if frames == 0 {
        return Ok((world, Vec::new()));
    }
    let mut runtime = match scan_assets(assets_root) {
        Ok(catalog) => ScriptRuntime::with_catalog(assets_root, catalog),
        Err(_) => ScriptRuntime::new(assets_root),
    };
    let mut simulation = Simulation::new(world);
    let mut particles = ParticleSystem::new();
    let fixed_dt = simulation.fixed_dt();
    for frame in 0..frames {
        simulation
            .step_with(Vec::new(), |world| runtime.run_frame(world, fixed_dt))
            .map_err(|err| format!("第 {frame} 帧运行失败：{err:?}"))?;
        particles.update(fixed_dt, simulation.world());
    }
    Ok((simulation.into_world(), particles.quads().to_vec()))
}

pub(crate) fn run_scripts(
    world_path: Option<&str>,
    assets_root: Option<&str>,
    frames: Option<&str>,
) -> ExitCode {
    let (Some(world_path), Some(assets_root)) = (world_path, assets_root) else {
        eprintln!("用法：xgc run-scripts <世界.json> <资产根> [帧数]");
        return ExitCode::from(2);
    };
    let frames: u64 = match frames.map(str::parse::<u64>).transpose() {
        Ok(frames) => frames.unwrap_or(60),
        Err(_) => {
            eprintln!("帧数不是整数");
            return ExitCode::from(2);
        }
    };
    let data = match load_world(Path::new(world_path)) {
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
    let world = match advance_world(world, Path::new(assets_root), frames) {
        Ok(world) => world,
        Err(err) => {
            eprintln!("运行失败：{err}");
            return ExitCode::from(2);
        }
    };
    let record = world.world_data();
    println!(
        "run-scripts: frames={frames} entities={} revision={} hash={:016x}",
        record.entities.len(),
        record.revision,
        world.state_hash()
    );
    ExitCode::SUCCESS
}
