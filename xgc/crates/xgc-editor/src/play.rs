//! 编辑器的 Play 会话与固定步进。

use std::path::Path;
use std::time::Duration;

use xgc_host::{asset_paths, FixedStepClock, ScriptedRunner};
use xgc_sim::{FrameRecord, InputMap, InputState};
use xgc_world::{HostUiInput, World};

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

    pub(crate) fn asset_paths(&self) -> xgc_asset::AssetPaths {
        let search_paths = self.project.as_ref().map_or(&[][..], |project| {
            project.manifest().search_paths.as_slice()
        });
        asset_paths(self.asset_root(), search_paths)
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
        let (runner, _) = ScriptedRunner::with_assets(world, self.asset_paths());
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
        self.advance_play_with_io(elapsed, || None, |_| {})
    }

    pub(crate) fn advance_play_with_io(
        &mut self,
        elapsed: Duration,
        mut input: impl FnMut() -> Option<HostUiInput>,
        mut audio: impl FnMut(&[f32]),
    ) -> Result<Option<Duration>, String> {
        let Some(session) = self.play.as_mut().filter(|session| !session.paused) else {
            return Ok(None);
        };
        let (steps, wait) = session.clock.advance(elapsed);
        for _ in 0..steps {
            let result = match input() {
                Some(input) => session.runner.step_with_ui_input(input),
                None => session.runner.step(),
            };
            if let Err(err) = result {
                self.pause();
                return Err(err);
            }
            audio(session.runner.audio_samples());
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

#[cfg(test)]
mod tests {
    use super::*;
    use xgc_host::AUDIO_SAMPLE_RATE;
    use xgc_world::Script;

    #[test]
    fn submitting_audio_delivers_each_catch_up_step_in_order() {
        let root = std::env::temp_dir().join(format!("xgc_play_pcm_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let wave = root.join("tone.wav");
        xgc_sim::write_wav_mono(&wave, AUDIO_SAMPLE_RATE, &[0.5; 2400]).unwrap();
        let sample = xgc_sim::load_wav(&wave).unwrap().samples[0];
        std::fs::write(
            root.join("tone.lua"),
            r#"
            local frame = 0
            function start(ctx) play_sound(ctx, 'tone.wav', 1, 1) end
            function update(ctx)
                frame = frame + 1
                set_master_volume(ctx, frame * 0.25)
            end
            "#,
        )
        .unwrap();
        let mut world = World::new();
        world.spawn((Script {
            path: "tone.lua".into(),
        },));
        let path = root.join("scene.json");
        xgc_store::save_world(&path, &world.world_data()).unwrap();
        let mut editor = EditorState::open(&path).unwrap();
        editor.play().unwrap();

        let mut blocks = Vec::new();
        let dt = Duration::from_secs_f32(1.0 / 60.0);
        editor
            .advance_play_with_io(dt * 3, || None, |pcm| blocks.push((pcm.len(), pcm[0])))
            .unwrap();
        assert_eq!(blocks, [0.25, 0.5, 0.75].map(|gain| (800, sample * gain)));
        editor.pause();
        assert_eq!(
            editor.advance_play_with_io(dt, || None, |_| panic!("暂停不能输出新块")),
            Ok(None)
        );
        editor.stop();
        std::fs::remove_dir_all(root).unwrap();
    }
}
