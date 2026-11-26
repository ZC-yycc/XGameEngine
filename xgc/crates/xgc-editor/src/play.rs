//! 编辑器的 Play 会话与固定步进。

use std::path::Path;
use std::time::Duration;

use xgc_render::MeshCache;
use xgc_script::ScriptRuntime;
use xgc_sim::{apply_root_motion, FrameRecord, InputMap, InputState, ParticleSystem, Simulation};
use xgc_world::World;

use crate::{EditorState, PlaySession};

/// 窗口卡顿后只追有限步，避免补跑积压拖住编辑器。
const MAX_CATCH_UP_STEPS: u32 = 4;

impl EditorState {
    pub(crate) fn asset_root(&self) -> Option<&Path> {
        self.project_root().or_else(|| {
            self.path().and_then(Path::parent).map(|parent| {
                if parent.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    parent
                }
            })
        })
    }

    pub fn play(&mut self) -> Result<(), String> {
        if self.is_playing() {
            return Ok(());
        }
        let asset_root = self.asset_root();
        let root = asset_root.unwrap_or_else(|| Path::new("."));
        let input = if let Some(project) = &self.project {
            project
                .input_path()
                .map_err(|err| format!("读取项目输入映射失败：{err}"))?
        } else {
            let path = root.join("input.ron");
            path.try_exists()
                .map_err(|err| format!("{}：读取输入映射失败：{err}", path.display()))?
                .then_some(path)
        };
        let input = input.as_deref().map(InputMap::load).transpose()?;

        let mut world = World::new();
        world
            .restore_world_data(&self.world.world_data())
            .map_err(|err| format!("进入 Play 失败：{err}"))?;
        let (runtime, mut meshes) = match asset_root.and_then(|root| xgc_asset::scan(root).ok()) {
            Some(catalog) => (
                ScriptRuntime::with_catalog(root, catalog.clone()),
                MeshCache::with_catalog(root, catalog),
            ),
            None => (ScriptRuntime::new(root), MeshCache::new(root)),
        };
        // GUID 模型与脚本共用同一份资产目录，否则根运动会随宿主而变。
        let root_motion = meshes.root_motion_library(&world);
        self.play = Some(PlaySession {
            selected: self.selected,
            simulation: Simulation::new(world),
            particles: ParticleSystem::new(),
            runtime,
            input,
            paused: false,
            elapsed: Duration::ZERO,
            root_motion,
        });
        Ok(())
    }

    /// 每次重写全部动作和轴，松开按键后的 false / 0 也要送给脚本。
    pub fn apply_input(&mut self, state: &InputState) {
        let Some(session) = &mut self.play else {
            return;
        };
        let Some(map) = &session.input else {
            return;
        };
        for name in map.actions.keys() {
            session
                .runtime
                .set_action(name, map.action_pressed(name, state));
        }
        for name in map.axes.keys() {
            session.runtime.set_axis(name, map.axis_value(name, state));
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
            .map_or(0, |session| session.runtime.entity_count())
    }

    pub fn is_paused(&self) -> bool {
        self.play.as_ref().is_some_and(|session| session.paused)
    }

    pub fn pause(&mut self) {
        if let Some(session) = &mut self.play {
            session.paused = true;
            session.elapsed = Duration::ZERO;
        }
    }

    pub fn resume(&mut self) {
        if let Some(session) = &mut self.play {
            session.paused = false;
        }
    }

    /// 按经过的真实时间运行固定步，返回下一步前的重绘等待时间。
    pub fn advance_play(&mut self, elapsed: Duration) -> Result<Option<Duration>, String> {
        let Some(session) = self.play.as_mut().filter(|session| !session.paused) else {
            return Ok(None);
        };
        let fixed_step = Duration::from_secs_f32(session.simulation.fixed_dt());
        session.elapsed = session
            .elapsed
            .saturating_add(elapsed)
            .min(fixed_step * MAX_CATCH_UP_STEPS);
        while session.elapsed >= fixed_step {
            session.elapsed -= fixed_step;
            if let Err(err) = session.run_frame() {
                self.pause();
                return Err(err);
            }
        }
        Ok(Some(fixed_step - session.elapsed))
    }

    pub fn step(&mut self) -> Result<Option<FrameRecord>, String> {
        self.play
            .as_mut()
            .filter(|session| session.paused)
            .map(PlaySession::run_frame)
            .transpose()
    }

    /// 编辑世界、选择和历史一直留在编辑态，退出只丢弃播放副本。
    pub fn stop(&mut self) {
        self.play = None;
    }
}

impl PlaySession {
    fn run_frame(&mut self) -> Result<FrameRecord, String> {
        let dt = self.simulation.fixed_dt();
        let record = self
            .simulation
            .step_with(Vec::new(), |world| self.runtime.run_frame(world, dt))
            .map_err(|err| format!("Play 帧失败：{err:?}"))?;
        apply_root_motion(self.simulation.world_mut(), dt, &self.root_motion);
        self.particles.update(dt, self.simulation.world());
        if let Some(err) = self.runtime.take_error() {
            return Err(format!("Play 第 {} 帧脚本失败：{err}", record.frame));
        }
        Ok(record)
    }
}
