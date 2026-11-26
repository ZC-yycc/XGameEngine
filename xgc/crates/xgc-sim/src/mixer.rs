//! 软件混音：把 `AudioClip` 按总线 / 增益混成单声道 PCM，可写回 WAV。

use std::sync::Arc;

use xgc_base::EntityId;
use xgc_world::{audio_volume, AudioMixer, AudioSource, PlaySound, World};

use super::audio::{spatial_gain, AudioClip};

#[derive(Clone, Debug)]
struct Voice {
    clip: Arc<AudioClip>,
    sound: PlaySound,
    emitter: Option<EntityId>,
    frames: usize,
    phase: f64,
    step: f64,
}

impl Voice {
    fn mono_frame(&self, index: usize) -> f32 {
        let channels = usize::from(self.clip.channels);
        let offset = index * channels;
        let sum: f32 = self.clip.samples[offset..offset + channels].iter().sum();
        if sum.is_finite() {
            sum / channels as f32
        } else {
            0.0
        }
    }

    fn sample(&self, rate: f64) -> f32 {
        let cursor = self.phase / rate;
        let index = (cursor as usize).min(self.frames - 1);
        let start = self.mono_frame(index);
        let weight = (cursor - index as f64) as f32;
        if weight == 0.0 {
            return start;
        }
        let next = if index + 1 < self.frames {
            index + 1
        } else if self.sound.looping {
            0
        } else {
            return start;
        };
        start * (1.0 - weight) + self.mono_frame(next) * weight
    }
}

#[derive(Clone, Debug)]
pub struct SoftwareMixer {
    sample_rate: u32,
    mix: AudioMixer,
    listener_position: [f32; 3],
    voices: Vec<Voice>,
}

impl SoftwareMixer {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1),
            mix: AudioMixer::default(),
            listener_position: [0.0; 3],
            voices: Vec::new(),
        }
    }

    pub fn set_mix(&mut self, mix: AudioMixer) {
        self.mix = mix;
    }

    pub fn set_listener_position(&mut self, position: [f32; 3]) {
        self.listener_position = position;
    }

    pub fn play(&mut self, clip: Arc<AudioClip>, sound: PlaySound) {
        self.start_voice(clip, sound, None);
    }

    pub fn play_emitter(&mut self, clip: Arc<AudioClip>, sound: PlaySound, emitter: EntityId) {
        self.start_voice(clip, sound, Some(emitter));
    }

    fn start_voice(
        &mut self,
        clip: Arc<AudioClip>,
        mut sound: PlaySound,
        emitter: Option<EntityId>,
    ) {
        let frames = clip.frames();
        if frames == 0 || clip.sample_rate == 0 {
            return;
        }
        sound.volume = audio_volume(sound.volume);
        let pitch = if sound.pitch.is_finite() {
            sound.pitch.clamp(0.05, 16.0)
        } else {
            1.0
        };
        // 相位以输出采样率为单位，整数步进无需逐帧累加除法的舍入误差。
        let step = f64::from(clip.sample_rate) * f64::from(pitch);
        self.voices.push(Voice {
            clip,
            sound,
            emitter,
            frames,
            phase: 0.0,
            step,
        });
    }

    /// 按持久 ID 跟随组件所有者，实体删除或移除声源后只停它自己的声部。
    pub fn sync_emitters(&mut self, world: &World) {
        self.voices.retain_mut(|voice| {
            let Some(emitter) = voice.emitter else {
                return true;
            };
            let Some(key) = world.key(emitter) else {
                return false;
            };
            if world.get::<AudioSource>(key).is_none() {
                return false;
            }
            if let Some(position) = &mut voice.sound.position {
                *position = world.audio_emitter_position(key);
            }
            true
        });
    }

    pub fn stop_clip(&mut self, clip: &str) {
        self.voices.retain(|voice| voice.sound.clip != clip);
    }

    pub fn stop_all(&mut self) {
        self.voices.clear();
    }

    pub fn voices(&self) -> usize {
        self.voices.len()
    }

    /// 混出 `frames` 个单声道采样；结束后停止的声部自动移除。
    pub fn mix(&mut self, frames: usize) -> Vec<f32> {
        let mut output = vec![0.0; frames];
        self.mix_into(&mut output);
        output
    }

    /// 覆盖调用方的单声道缓冲，按声部加入顺序累加，结束的声部原地移除。
    pub fn mix_into(&mut self, output: &mut [f32]) {
        output.fill(0.0);
        let rate = f64::from(self.sample_rate);
        self.voices.retain_mut(|voice| {
            let end = voice.frames as f64 * rate;
            let volume = voice.sound.volume;
            let volume = voice.sound.position.map_or(volume, |source| {
                spatial_gain(volume, source, self.listener_position)
            });
            let gain = audio_volume(volume) * self.mix.bus_volume(voice.sound.bus);
            for sample in output.iter_mut() {
                if voice.phase >= end {
                    break;
                }
                *sample += voice.sample(rate) * gain;
                voice.phase += voice.step;
                if voice.sound.looping && voice.phase >= end {
                    voice.phase %= end;
                }
            }
            voice.phase < end
        });
        let master = audio_volume(self.mix.master);
        for sample in output {
            let value = *sample * master;
            *sample = if value.is_nan() {
                0.0
            } else {
                value.clamp(-1.0, 1.0)
            };
        }
    }
}

/// 写单声道 16-bit WAV，便于离线检查混音结果。
pub fn write_wav_mono(
    path: &std::path::Path,
    sample_rate: u32,
    samples: &[f32],
) -> Result<(), String> {
    let data_len = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.max(1).to_le_bytes());
    bytes.extend_from_slice(&(sample_rate.max(1) * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(path, bytes).map_err(|err| format!("写入 {} 失败：{err}", path.display()))
}
