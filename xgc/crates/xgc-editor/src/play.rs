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

    /// 进入 Play 并在每帧驱动 `script.lua`；可选读取 `<root>/input.ron`。
    pub fn play_with_scripts(&mut self, root: &Path) -> Result<(), String> {
        if self.play.is_some() {
            return Ok(());
        }
        let before = self.world.world_data();
        let mut world = World::new();
        world
            .restore_world_data(&before)
            .map_err(|err| format!("进入 Play 失败：{err}"))?;
        let runtime = match xgc_asset::scan(root) {
            Ok(catalog) => ScriptRuntime::with_catalog(root, catalog),
            Err(_) => ScriptRuntime::new(root),
        };
        let input = std::fs::read_to_string(root.join("input.ron"))
            .ok()
            .and_then(|text| InputMap::parse_ron(&text).ok());
        // 根运动表在进入 Play 时建一次：Editor 与 CLI / player 必须同源，
        // 否则同一份 Fox 在编辑器里会比命令行少走一段路。
        let root_motion = {
            let mut meshes = MeshCache::new(root);
            meshes.root_motion_library(&world)
        };
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
