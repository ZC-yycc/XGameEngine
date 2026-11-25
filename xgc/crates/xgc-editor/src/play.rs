//! 编辑器 Play / Scripts / 输入 / 停止。

use std::path::Path;

use xgc_render::MeshCache;
use xgc_script::ScriptRuntime;
use xgc_sim::{apply_root_motion, FrameRecord, InputMap, InputState, ParticleSystem, Simulation};
use xgc_world::{RootMotionLibrary, World};

use crate::{EditorState, PlaySession};

impl EditorState {
    pub fn play(&mut self) -> Result<(), String> {
        if self.play.is_some() {
            return Ok(());
        }
        let before = self.world.world_data();
        let mut world = World::new();
        world
            .restore_world_data(&before)
            .map_err(|err| format!("进入 Play 失败：{err}"))?;
        self.play = Some(PlaySession {
            before,
            simulation: Simulation::new(world),
            particles: ParticleSystem::new(),
            runtime: None,
            input: None,
            root_motion: RootMotionLibrary::new(),
        });
        Ok(())
    }

    /// 项目的脚本根与输入路径由清单确定，单场景才使用调用方提供的根。
    pub fn play_with_scripts(&mut self, standalone_root: &Path) -> Result<(), String> {
        if self.play.is_some() {
            return Ok(());
        }
        let before = self.world.world_data();
        let mut world = World::new();
        world
            .restore_world_data(&before)
            .map_err(|err| format!("进入 Play 失败：{err}"))?;
        let (root, input) = match &self.project {
            Some(project) => {
                let path = project
                    .input_path()
                    .map_err(|err| format!("读取项目输入映射失败：{err}"))?;
                (
                    project.root(),
                    path.as_deref().map(InputMap::load).transpose()?,
                )
            }
            None => (
                standalone_root,
                InputMap::load(&standalone_root.join("input.ron")).ok(),
            ),
        };
        let (runtime, mut meshes) = match xgc_asset::scan(root) {
            Ok(catalog) => (
                ScriptRuntime::with_catalog(root, catalog.clone()),
                MeshCache::with_catalog(root, catalog),
            ),
            Err(_) => (ScriptRuntime::new(root), MeshCache::new(root)),
        };
        // GUID 模型与脚本共用同一份资产目录，否则根运动会随宿主而变。
        let root_motion = meshes.root_motion_library(&world);
        self.play = Some(PlaySession {
            before,
            simulation: Simulation::new(world),
            particles: ParticleSystem::new(),
            runtime: Some(runtime),
            input,
            root_motion,
        });
        Ok(())
    }

    /// 把本帧按键 / 鼠标状态折叠成动作，供脚本 `action_pressed` 使用。
    pub fn apply_input(&mut self, state: &InputState) {
        let Some(session) = &mut self.play else {
            return;
        };
        let (Some(runtime), Some(map)) = (&mut session.runtime, &session.input) else {
            return;
        };
        for name in map.action_names() {
            runtime.set_action(&name, map.action_pressed(&name, state));
        }
        for name in map.axis_names() {
            runtime.set_axis(&name, map.axis_value(&name, state));
        }
    }

    pub fn input_map(&self) -> Option<&InputMap> {
        self.play
            .as_ref()
            .and_then(|session| session.input.as_ref())
    }

    pub fn script_entity_count(&self) -> usize {
        self.play
            .as_ref()
            .and_then(|session| session.runtime.as_ref())
            .map(ScriptRuntime::entity_count)
            .unwrap_or(0)
    }

    pub fn step(&mut self) -> Result<Option<FrameRecord>, String> {
        let Some(session) = &mut self.play else {
            return Ok(None);
        };
        let record = if let Some(runtime) = &mut session.runtime {
            session
                .simulation
                .step_with(Vec::new(), |world| runtime.run_frame(world, 1.0 / 60.0))
        } else {
            session.simulation.step(Vec::new())
        }
        .map_err(|err| format!("Play 帧失败：{err:?}"))?;
        apply_root_motion(
            session.simulation.world_mut(),
            1.0 / 60.0,
            &session.root_motion,
        );
        session
            .particles
            .update(1.0 / 60.0, session.simulation.world());
        Ok(Some(record))
    }

    pub fn stop(&mut self) -> Result<(), String> {
        let Some(session) = self.play.take() else {
            return Ok(());
        };
        let mut world = World::new();
        world
            .restore_world_data(&session.before)
            .map_err(|err| format!("退出 Play 失败：{err}"))?;
        self.world = world;
        self.selected = None;
        self.dirty = false;
        Ok(())
    }
}
