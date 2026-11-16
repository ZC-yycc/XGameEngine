//! 软件混音：把 `AudioClip` 按总线 / 增益混成单声道 PCM，可写回 WAV。

use std::collections::BTreeMap;

use super::audio::AudioClip;

#[derive(Clone, Debug)]
struct Voice {
    samples: Vec<f32>,
    frame: usize,
    gain: f32,
    bus: String,
    looping: bool,
}

#[derive(Clone, Debug)]
pub struct SoftwareMixer {
    pub sample_rate: u32,
    pub master: f32,
    buses: BTreeMap<String, f32>,
    voices: Vec<Voice>,
}

impl SoftwareMixer {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1),
            master: 1.0,
            buses: BTreeMap::new(),
            voices: Vec::new(),
        }
    }

    pub fn set_bus(&mut self, bus: impl Into<String>, volume: f32) {
        self.buses.insert(bus.into(), volume.clamp(0.0, 1.0));
    }

    pub fn play(&mut self, clip: &AudioClip, gain: f32, bus: &str, looping: bool) -> usize {
        let channels = usize::from(clip.channels.max(1));
        let mut samples = Vec::with_capacity(clip.frames());
        for frame in clip.samples.chunks(channels) {
            let sum: f32 = frame.iter().sum();
            samples.push(sum / channels as f32);
        }
        self.voices.push(Voice {
            samples,
            frame: 0,
            gain: gain.clamp(0.0, 4.0),
            bus: bus.to_string(),
            looping,
        });
        self.voices.len() - 1
    }

    pub fn voices(&self) -> usize {
        self.voices.len()
    }

    /// 混出 `frames` 个单声道采样；结束后停止的声部自动移除。
    pub fn mix(&mut self, frames: usize) -> Vec<f32> {
        let bus =
            |buses: &BTreeMap<String, f32>, name: &str| buses.get(name).copied().unwrap_or(1.0);
        let mut output = Vec::with_capacity(frames);
        for _ in 0..frames {
            let mut sample = 0.0_f32;
            for voice in &mut self.voices {
                if let Some(value) = voice.samples.get(voice.frame) {
                    sample += *value * voice.gain * bus(&self.buses, &voice.bus);
                }
                voice.frame += 1;
                if voice.frame >= voice.samples.len() {
                    if voice.looping && !voice.samples.is_empty() {
                        voice.frame = 0;
                    } else {
                        voice.frame = voice.samples.len();
                    }
                }
            }
            output.push((sample * self.master).clamp(-1.0, 1.0));
        }
        self.voices
            .retain(|voice| voice.looping || voice.frame < voice.samples.len());
        output
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
