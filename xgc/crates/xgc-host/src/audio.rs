//! 帧末混音由所有宿主共用，无头运行也能读取确定性的 PCM。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use xgc_asset::AssetPaths;
use xgc_base::{EntityId, EntityKey};
use xgc_script::ScriptRuntime;
use xgc_sim::{load_wav, AudioClip, SoftwareMixer};
use xgc_world::{AudioCommand, AudioSource, World};

pub const AUDIO_SAMPLE_RATE: u32 = 48_000;

pub(crate) struct AudioStage {
    assets: AssetPaths,
    clips: HashMap<String, Option<Arc<AudioClip>>>,
    mixer: SoftwareMixer,
    autoplayed: HashSet<EntityId>,
    pending_sources: Vec<(EntityId, EntityKey)>,
    samples: Vec<f32>,
}

impl AudioStage {
    pub(crate) fn new(assets: AssetPaths, fixed_dt: f32) -> Self {
        Self {
            assets,
            clips: HashMap::new(),
            mixer: SoftwareMixer::new(AUDIO_SAMPLE_RATE),
            autoplayed: HashSet::new(),
            pending_sources: Vec::new(),
            samples: vec![0.0; (AUDIO_SAMPLE_RATE as f32 * fixed_dt).round() as usize],
        }
    }

    pub(crate) fn update(&mut self, runtime: &mut ScriptRuntime, world: &World) {
        runtime.finish_audio_frame(world.audio_mixer(), |mix, listener, commands| {
            self.mixer.set_mix(mix);
            let position = listener
                .map(|pose| pose.position)
                .or_else(|| world.audio_listener_position());
            self.mixer
                .set_listener_position(position.unwrap_or([0.0; 3]));
            for command in commands {
                match command {
                    AudioCommand::Play(sound) => {
                        if let Some(clip) = self.clip(&sound.clip) {
                            self.mixer.play(clip, sound);
                        }
                    }
                    AudioCommand::StopClip(clip) => self.mixer.stop_clip(&clip),
                    AudioCommand::StopAll => self.mixer.stop_all(),
                }
            }
        });
        self.sync_autoplay(world);
        self.mixer.mix_into(&mut self.samples);
    }

    fn sync_autoplay(&mut self, world: &World) {
        self.mixer.sync_emitters(world);
        self.autoplayed.retain(|&id| {
            world
                .key(id)
                .is_some_and(|key| world.get::<AudioSource>(key).is_some())
        });
        world.for_each::<AudioSource>(|key, source| {
            if !source.autoplay || source.clip.is_empty() {
                return;
            }
            let id = world.entity(key).expect("查询返回存活实体");
            if self.autoplayed.insert(id) {
                self.pending_sources.push((id, key));
            }
        });
        // 声部以加入顺序累加。槽位回收与 archetype 迁移不应改变同帧的入队顺序。
        self.pending_sources.sort_unstable_by_key(|&(id, _)| id);
        for index in 0..self.pending_sources.len() {
            let (id, key) = self.pending_sources[index];
            let source = world.get::<AudioSource>(key).expect("刚收集的声源");
            if let Some(clip) = self.clip(&source.clip) {
                let sound = source.play_sound(world.audio_emitter_position(key));
                self.mixer.play_emitter(clip, sound, id);
            }
        }
        self.pending_sources.clear();
    }

    fn clip(&mut self, reference: &str) -> Option<Arc<AudioClip>> {
        if let Some(clip) = self.clips.get(reference) {
            return clip.clone();
        }
        let clip = match self
            .assets
            .resolve(reference)
            .map_err(|err| err.to_string())
            .and_then(|path| load_wav(&path))
        {
            Ok(clip) => Some(Arc::new(clip)),
            Err(err) => {
                eprintln!("audio: 读取 {reference} 失败：{err}");
                None
            }
        };
        self.clips.insert(reference.to_string(), clip.clone());
        clip
    }

    pub(crate) fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub(crate) fn clear_samples(&mut self) {
        self.samples.fill(0.0);
    }
}
