//! 编辑器、播放器和命令行共用资产装载与脚本步进。

#![deny(unsafe_code)]

mod audio;
mod clock;
#[cfg(feature = "window-input")]
pub mod window;
#[cfg(feature = "window-audio")]
mod window_audio;

pub use audio::AUDIO_SAMPLE_RATE;
pub use clock::FixedStepClock;
#[cfg(feature = "window-audio")]
pub use window_audio::WindowAudio;

use std::path::Path;

use audio::AudioStage;

use xgc_asset::{AssetPaths, SearchPaths};
use xgc_render::MeshCache;
use xgc_script::ScriptRuntime;
use xgc_sim::{apply_root_motion, InputEdge, InputMap, InputState, ParticleSystem, Simulation};
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
    input_state: InputState,
    input_edges: Vec<InputEdge>,
}

impl ScriptedRunner {
    /// 复用脚本与模型的 GUID 目录，并把预热后的网格缓存交给窗口渲染器。
    pub fn with_assets(world: World, assets: AssetPaths) -> (Self, MeshCache) {
        Self::with_simulation(Simulation::new(world), assets)
    }

    /// 已选定固定步长的预览仍走同一套资产和脚本帧宿主。
    pub fn with_simulation(simulation: Simulation, assets: AssetPaths) -> (Self, MeshCache) {
        let runtime = ScriptRuntime::with_assets(assets.clone());
        let mut meshes = MeshCache::with_assets(assets.clone());
        let root_motion = meshes.root_motion_library(simulation.world());
        let audio = AudioStage::new(assets, simulation.fixed_dt());
        (
            Self {
                simulation,
                runtime,
                particles: ParticleSystem::new(),
                root_motion,
                audio,
                input_state: InputState::default(),
                input_edges: Vec::new(),
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
        self.runtime.script_count()
    }

    /// 编辑器在运行步之间检查已挂载的脚本，模拟帧本身不轮询磁盘。
    pub fn poll_script_reload(&mut self) -> Result<usize, String> {
        self.runtime.poll_reload()
    }

    /// 无窗口输入可在零个或多个固定步之间提交状态快照。
    pub fn apply_input(&mut self, map: &InputMap, state: &InputState) {
        self.input_edges.clear();
        if self.input_state != *state {
            state.append_edges_since(&self.input_state, &mut self.input_edges);
            self.input_state.clone_from(state);
        }
        Self::apply_input_sample(&mut self.runtime, map, state, &self.input_edges);
    }

    /// 只在即将执行一个固定步时取输入，边沿与 HUD 事件共享消费边界。
    #[cfg(feature = "window-input")]
    pub fn step_window(
        &mut self,
        map: Option<&InputMap>,
        input: &mut window::WindowInput,
    ) -> Result<u64, String> {
        if let Some(map) = map {
            Self::apply_input_sample(
                &mut self.runtime,
                map,
                input.gameplay(),
                input.gameplay_edges(),
            );
        }
        input.discard_gameplay_edges();
        self.step_with_ui_input(input.take_ui_input())
    }

    pub fn discard_action_edges(&mut self) {
        self.runtime.discard_action_edges();
    }

    pub fn clear_input(&mut self) {
        self.runtime.clear_actions();
        self.runtime.clear_axes();
        self.input_state = InputState::default();
        self.input_edges.clear();
    }

    fn apply_input_sample(
        runtime: &mut ScriptRuntime,
        map: &InputMap,
        state: &InputState,
        edges: &[InputEdge],
    ) {
        for name in map.actions.keys() {
            let action = map.action_input(name, state, edges);
            runtime.set_action_input(name, action.down, action.pressed, action.released);
        }
        for name in map.axes.keys() {
            runtime.set_axis(name, map.axis_value(name, state));
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
