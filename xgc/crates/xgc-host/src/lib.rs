//! 编辑器、播放器和命令行共用资产装载与脚本步进。

#![deny(unsafe_code)]

mod audio;
mod clock;
#[cfg(feature = "window-input")]
pub mod window;

pub use audio::AUDIO_SAMPLE_RATE;
pub use clock::FixedStepClock;

use std::path::Path;

use audio::AudioStage;

use xgc_asset::{AssetPaths, SearchPaths};
use xgc_render::MeshCache;
use xgc_script::ScriptRuntime;
use xgc_sim::{apply_root_motion, InputMap, InputState, ParticleSystem, Simulation};
use xgc_world::{HostUiInput, ParticleQuad, RootMotionLibrary, World};

/// 宿主只有在明确知道内容根时才扫描 GUID 目录，避免空白编辑世界扫描工作目录。
pub fn asset_paths(root: Option<&Path>, search_paths: &[String]) -> AssetPaths {
    let catalog = root.and_then(|root| match xgc_asset::scan(root) {
        Ok(catalog) => Some(catalog),
        Err(err) => {
            eprintln!("warning: 资产根 {} 扫描失败（{err}）", root.display());
            None
        }
    });
    AssetPaths::new(
        root.unwrap_or_else(|| Path::new(".")),
        SearchPaths::new(search_paths),
        catalog,
    )
}

pub struct ScriptedRunner {
    simulation: Simulation,
    runtime: ScriptRuntime,
    particles: ParticleSystem,
    root_motion: RootMotionLibrary,
    audio: AudioStage,
}

impl ScriptedRunner {
    /// 复用脚本与模型的 GUID 目录，并把预热后的网格缓存交给窗口渲染器。
    pub fn with_assets(world: World, assets: AssetPaths) -> (Self, MeshCache) {
        let runtime = ScriptRuntime::with_assets(assets.clone());
        let mut meshes = MeshCache::with_assets(assets.clone());
        let root_motion = meshes.root_motion_library(&world);
        let simulation = Simulation::new(world);
        let audio = AudioStage::new(assets, simulation.fixed_dt());
        (
            Self {
                simulation,
                runtime,
                particles: ParticleSystem::new(),
                root_motion,
                audio,
            },
            meshes,
        )
    }

    /// 世界与脚本在调度计划的各自阶段执行，根运动和粒子在步末结算。返回零基帧号。
    pub fn step(&mut self) -> Result<u64, String> {
        // 手动单步没有新的设备采样，不能沿用上个窗口步的按住状态。
        if self.world().host_ui_input().is_some() {
            return self.step_with_ui_input(HostUiInput::default());
        }
        self.advance()
    }

    /// 窗口边沿只在当前固定步可见。
    pub fn step_with_ui_input(&mut self, input: HostUiInput) -> Result<u64, String> {
        self.simulation.set_ui_input(input);
        self.advance()
    }

    fn advance(&mut self) -> Result<u64, String> {
        let dt = self.simulation.fixed_dt();
        let frame = self
            .simulation
            .advance_with(Vec::new(), |world| self.runtime.run_frame(world, dt))
            .map_err(|err| {
                self.audio.clear_samples();
                format!("运行失败：{err:?}")
            })?;
        if let Some(err) = self.runtime.take_error() {
            self.audio.clear_samples();
            return Err(format!("第 {} 帧运行失败：{err}", frame + 1));
        }
        apply_root_motion(self.simulation.world_mut(), dt, &self.root_motion);
        self.particles.update(dt, self.simulation.world());
        self.audio
            .update(&mut self.runtime, self.simulation.world());
        Ok(frame)
    }

    pub fn world(&self) -> &World {
        self.simulation.world()
    }

    pub fn fixed_dt(&self) -> f32 {
        self.simulation.fixed_dt()
    }

    pub fn set_parallel_threads(&mut self, threads: usize) {
        self.simulation.set_parallel_threads(threads);
    }

    pub fn script_count(&self) -> usize {
        self.runtime.entity_count()
    }

    /// 每次重写所有动作与轴，把松开后的 false / 0 也送给脚本。
    pub fn apply_input(&mut self, map: &InputMap, state: &InputState) {
        for name in map.actions.keys() {
            self.runtime
                .set_action(name, map.action_pressed(name, state));
        }
        for name in map.axes.keys() {
            self.runtime.set_axis(name, map.axis_value(name, state));
        }
    }

    pub fn particle_quads(&self) -> &[ParticleQuad] {
        self.particles.quads()
    }

    /// 最近一次固定步的单声道 PCM，采样率为 [`AUDIO_SAMPLE_RATE`]。
    pub fn audio_samples(&self) -> &[f32] {
        self.audio.samples()
    }

    pub fn into_parts(self) -> (World, Vec<ParticleQuad>) {
        (
            self.simulation.into_world(),
            self.particles.quads().to_vec(),
        )
    }
}
