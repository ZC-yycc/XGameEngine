//! 音频空间化：监听者距离衰减写入 `audio.gain`。

use xgc_world::{AudioGain, AudioListener, AudioMixer, AudioSource, Component, Transform, World};

use super::{System, SystemAccess};

#[derive(Clone, Copy, Debug, Default)]
pub struct AudioMixerSystem;

impl System for AudioMixerSystem {
    fn name(&self) -> &'static str {
        "audio_mixer"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![AudioListener::ID, AudioMixer::ID, AudioSource::ID],
            writes: vec![AudioGain::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        let mut mixer = AudioMixer::default();
        world.for_each::<AudioMixer>(|_, config| {
            mixer = *config;
        });
        let mut listener = None;
        world.for_each::<AudioListener>(|key, config| {
            if config.active && listener.is_none() {
                listener = world
                    .get::<Transform>(key)
                    .map(|transform| transform.position);
            }
        });
        let mut updates = Vec::new();
        world.for_each::<AudioSource>(|key, source| {
            if world.get::<AudioGain>(key).is_none() {
                return;
            }
            let Some(transform) = world.get::<Transform>(key) else {
                return;
            };
            let gain = match (source.spatial, listener) {
                (true, Some(listener)) => {
                    let dx = transform.position[0] - listener[0];
                    let dy = transform.position[1] - listener[1];
                    let dz = transform.position[2] - listener[2];
                    let distance = (dx * dx + dy * dy + dz * dz).sqrt();
                    source.volume / (1.0 + distance)
                }
                _ => source.volume,
            };
            let bus = mixer.bus(&source.bus) * mixer.master;
            updates.push((key, (gain * bus).clamp(0.0, 4.0)));
        });
        for (key, gain) in updates {
            if let Some(output) = world.get_mut::<AudioGain>(key) {
                output.gain = gain;
            }
        }
    }
}
