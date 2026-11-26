//! 帧末混音由所有宿主共用，无头运行也能读取确定性的 PCM。

use std::collections::HashMap;
use std::sync::Arc;

use xgc_asset::AssetPaths;
use xgc_script::ScriptRuntime;
use xgc_sim::{load_wav, AudioClip, SoftwareMixer};
use xgc_world::{AudioCommand, World};

pub const AUDIO_SAMPLE_RATE: u32 = 48_000;

pub(crate) struct AudioStage {
    assets: AssetPaths,
    clips: HashMap<String, Option<Arc<AudioClip>>>,
    mixer: SoftwareMixer,
    samples: Vec<f32>,
}

impl AudioStage {
    pub(crate) fn new(assets: AssetPaths, fixed_dt: f32) -> Self {
        Self {
            assets,
            clips: HashMap::new(),
            mixer: SoftwareMixer::new(AUDIO_SAMPLE_RATE),
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
        self.mixer.mix_into(&mut self.samples);
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
