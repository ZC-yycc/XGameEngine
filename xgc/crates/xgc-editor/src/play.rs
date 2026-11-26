//! 编辑器的 Play 会话与固定步进。

use std::path::Path;
use std::time::Duration;

use xgc_host::{FixedStepClock, ScriptedRunner};
use xgc_sim::{FrameRecord, InputMap, InputState};
use xgc_world::World;

use crate::{EditorState, PlaySession};

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
        let (runner, _) = ScriptedRunner::with_assets(world, asset_root);
        self.play = Some(PlaySession {
            selected: self.selected,
            clock: FixedStepClock::new(runner.fixed_dt()),
            runner,
            input,
            paused: false,
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
        session.runner.apply_input(map, state);
    }

    pub fn input_map(&self) -> Option<&InputMap> {
        self.play
            .as_ref()
            .and_then(|session| session.input.as_ref())
    }

    pub fn script_entity_count(&self) -> usize {
        self.play
            .as_ref()
            .map_or(0, |session| session.runner.script_count())
    }

    pub fn is_paused(&self) -> bool {
        self.play.as_ref().is_some_and(|session| session.paused)
    }

    pub fn pause(&mut self) {
        if let Some(session) = &mut self.play {
            session.paused = true;
            session.clock.reset();
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
        let (steps, wait) = session.clock.advance(elapsed);
        for _ in 0..steps {
            if let Err(err) = session.runner.step() {
                self.pause();
                return Err(err);
            }
        }
        Ok(Some(wait))
    }

    pub fn step(&mut self) -> Result<Option<FrameRecord>, String> {
        let Some(session) = self.play.as_mut().filter(|session| session.paused) else {
            return Ok(None);
        };
        let frame = session.runner.step()?;
        let world = session.runner.world();
        Ok(Some(FrameRecord {
            frame,
            revision: world.revision(),
            state_hash: world.state_hash(),
            entities: world.len(),
        }))
    }

    /// 编辑世界、选择和历史一直留在编辑态，退出只丢弃播放副本。
    pub fn stop(&mut self) {
        self.play = None;
    }
}
