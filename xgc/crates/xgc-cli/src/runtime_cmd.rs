//! CLI 运行宿主：世界恢复后统一驱动固定步系统与脚本。

use std::path::Path;
use std::process::ExitCode;

use xgc_asset::scan as scan_assets;
use xgc_render::MeshCache;
use xgc_script::ScriptRuntime;
use xgc_sim::{apply_root_motion, ParticleSystem, Simulation};
use xgc_store::save_world;
use xgc_world::{RootMotionLibrary, World};

use crate::world_cmd::{load_world_state, parse_frames};

pub(crate) fn advance_world(
    world: World,
    assets_root: &Path,
    frames: u64,
) -> Result<World, String> {
    advance_world_with_particles(world, assets_root, frames).map(|(world, _)| world)
}

/// 脚本 + 粒子驱动的固定步进器。
///
/// `advance_world*` 与 `xgc trace <世界.json> [帧] [资产根]` 共用这一条逐帧路径：顺序（系统 → 脚本 →
/// 粒子）只有脚本帧才看得出来，两处各写一遍迟早会分叉；trace 也要能逐步拿到世界，
/// 不能等跑完再读最终状态。
pub(crate) struct ScriptedRunner {
    runtime: ScriptRuntime,
    simulation: Simulation,
    particles: ParticleSystem,
    fixed_dt: f32,
    /// 根运动轨道表：来自模型资产（GLTF 解析后的平移通道），不属于世界数据；
    /// 每帧在系统 / 脚本之后由宿主应用一次（旧端同一序：动画 tick → 根运动）。
    root_motion: RootMotionLibrary,
}

impl ScriptedRunner {
    pub(crate) fn new(world: World, assets_root: &Path) -> Self {
        let (runtime, catalog) = match scan_assets(assets_root) {
            Ok(catalog) => (
                ScriptRuntime::with_catalog(assets_root, catalog.clone()),
                Some(catalog),
            ),
            Err(err) => {
                // 退化成项目相对路径解析是既有行为；但传错资产根时不能静默——
                // 不然 `xgc trace … <资产根>` 会看起来「脚本没生效」而不是「根传错了」。
                eprintln!(
                    "warning: 资产根 {} 扫描失败（{err}），脚本按项目相对路径解析",
                    assets_root.display()
                );
                (ScriptRuntime::new(assets_root), None)
            }
        };
        let mut meshes = match catalog.as_ref() {
            Some(catalog) => MeshCache::with_catalog(assets_root, catalog.clone()),
            None => MeshCache::new(assets_root),
        };
        let root_motion = meshes.root_motion_library(&world);
        let simulation = Simulation::new(world);
        let fixed_dt = simulation.fixed_dt();
        Self {
            runtime,
            simulation,
            particles: ParticleSystem::new(),
            fixed_dt,
            root_motion,
        }
    }

    /// 推进一帧。脚本错误必须往上传：静默跳过会让 trace 与真实运行悄悄分叉。
    ///
    /// `ScriptRuntime::run_frame` 自己只打 stderr（旧宿主依赖这个提示），所以这里再
    /// `take_error()` 取第一处错误转成 `Err`；调用方负责补帧号。
    pub(crate) fn step(&mut self) -> Result<(), String> {
        let Self {
            runtime,
            simulation,
            particles,
            fixed_dt,
            root_motion,
        } = self;
        simulation
            .step_with(Vec::new(), |world| runtime.run_frame(world, *fixed_dt))
            .map_err(|err| format!("{err:?}"))?;
        if let Some(err) = runtime.take_error() {
            return Err(err);
        }
        apply_root_motion(simulation.world_mut(), *fixed_dt, root_motion);
        particles.update(*fixed_dt, simulation.world());
        Ok(())
    }

    pub(crate) fn world(&self) -> &World {
        self.simulation.world()
    }

    pub(crate) fn into_parts(self) -> (World, Vec<xgc_world::ParticleQuad>) {
        (
            self.simulation.into_world(),
            self.particles.quads().to_vec(),
        )
    }
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
    let mut runner = ScriptedRunner::new(world, assets_root);
    for frame in 0..frames {
        runner
            .step()
            .map_err(|err| format!("第 {} 帧运行失败：{err}", frame + 1))?;
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
