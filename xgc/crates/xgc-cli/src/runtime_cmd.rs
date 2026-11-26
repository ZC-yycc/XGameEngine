//! CLI 运行宿主：世界恢复后统一驱动固定步系统与脚本。

use std::path::Path;
use std::process::ExitCode;

use xgc_host::ScriptedRunner;
use xgc_store::save_world;
use xgc_world::World;

use crate::world_cmd::{load_world_state, parse_frames};

pub(crate) fn advance_world(
    world: World,
    assets_root: &Path,
    frames: u64,
) -> Result<World, String> {
    advance_world_with_particles(world, assets_root, frames).map(|(world, _)| world)
}

/// 粒子运行态不落进 `WorldData`；截图宿主要拿最终四边形，而普通 headless 宿主只丢掉它。
pub(crate) fn advance_world_with_particles(
    world: World,
    assets_root: &Path,
    frames: u64,
) -> Result<(World, Vec<xgc_world::ParticleQuad>), String> {
    if frames == 0 {
        return Ok((world, Vec::new()));
    }
    let (mut runner, _) = ScriptedRunner::with_assets(world, Some(assets_root));
    for _ in 0..frames {
        runner.step()?;
    }
    Ok(runner.into_parts())
}

pub(crate) fn run_scripts(
    world_path: Option<&str>,
    assets_root: Option<&str>,
    frames: Option<&str>,
    output: Option<&str>,
) -> ExitCode {
    let (Some(world_path), Some(assets_root)) = (world_path, assets_root) else {
        eprintln!("用法：xgc run-scripts <世界.json> <资产根> [帧数] [输出.json]");
        return ExitCode::from(2);
    };
    let frames: u64 = match parse_frames(frames) {
        Ok(frames) => frames,
        Err(code) => return code,
    };
    let world = match load_world_state(world_path) {
        Ok(world) => world,
        Err(code) => return code,
    };
    let world = match advance_world(world, Path::new(assets_root), frames) {
        Ok(world) => world,
        Err(err) => {
            // `err` 已经带「第 N 帧运行失败：」前缀，再套一层会变成双层措辞。
            eprintln!("{err}");
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
    if let Some(output) = output {
        if let Err(err) = save_world(Path::new(output), &record) {
            eprintln!("写出世界失败：{err}");
            return ExitCode::from(2);
        }
        println!("已写出世界：{output}");
    }
    ExitCode::SUCCESS
}
