//! 编辑器、播放器和命令行共用资产装载与脚本步进。

#![deny(unsafe_code)]

mod clock;
#[cfg(feature = "window-input")]
pub mod window;

pub use clock::FixedStepClock;

use std::path::Path;

use xgc_render::MeshCache;
use xgc_script::ScriptRuntime;
use xgc_sim::{apply_root_motion, InputMap, InputState, ParticleSystem, Simulation};
use xgc_world::{ParticleQuad, RootMotionLibrary, UiInput, World};

pub struct ScriptedRunner {
    simulation: Simulation,
    runtime: ScriptRuntime,
    particles: ParticleSystem,
    root_motion: RootMotionLibrary,
}

impl ScriptedRunner {
    /// 复用脚本与模型的 GUID 目录，并把预热后的网格缓存交给窗口渲染器。
    pub fn with_assets(world: World, asset_root: Option<&Path>) -> (Self, MeshCache) {
        let root = asset_root.unwrap_or_else(|| Path::new("."));
        let catalog = asset_root.and_then(|root| match xgc_asset::scan(root) {
            Ok(catalog) => Some(catalog),
            Err(err) => {
                eprintln!(
                    "warning: 资产根 {} 扫描失败（{err}），脚本按项目相对路径解析",
                    root.display()
                );
                None
            }
        });
        let (runtime, mut meshes) = match catalog {
            Some(catalog) => (
                ScriptRuntime::with_catalog(root, catalog.clone()),
                MeshCache::with_catalog(root, catalog),
            ),
            None => (ScriptRuntime::new(root), MeshCache::new(root)),
        };
        let root_motion = meshes.root_motion_library(&world);
        (
            Self {
                simulation: Simulation::new(world),
                runtime,
                particles: ParticleSystem::new(),
                root_motion,
            },
            meshes,
        )
    }

    /// 世界与脚本在调度计划的各自阶段执行，根运动和粒子在步末结算。返回零基帧号。
    pub fn step(&mut self) -> Result<u64, String> {
        let dt = self.simulation.fixed_dt();
        let frame = self
            .simulation
            .advance_with(Vec::new(), |world| self.runtime.run_frame(world, dt))
            .map_err(|err| format!("运行失败：{err:?}"))?;
        if let Some(err) = self.runtime.take_error() {
            return Err(format!("第 {} 帧运行失败：{err}", frame + 1));
        }
        apply_root_motion(self.simulation.world_mut(), dt, &self.root_motion);
        self.particles.update(dt, self.simulation.world());
        Ok(frame)
    }

    /// 窗口边沿只在当前固定步可见，手动步进也不会再次消费旧边沿。
    pub fn step_with_ui_input(&mut self, input: UiInput) -> Result<u64, String> {
        self.simulation.set_ui_input(input);
        let result = self.step();
        self.simulation.set_ui_input(UiInput {
            cursor: input.cursor,
            ..UiInput::default()
        });
        result
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

    pub fn into_parts(self) -> (World, Vec<ParticleQuad>) {
        (
            self.simulation.into_world(),
            self.particles.quads().to_vec(),
        )
    }
}
