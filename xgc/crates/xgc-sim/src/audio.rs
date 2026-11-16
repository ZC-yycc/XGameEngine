//! 音频空间化：监听者距离衰减写入 `audio.gain`。

use std::collections::BTreeMap;

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

/// 解码后的单声道 / 多声道 PCM 素材；采样按帧交错。
#[derive(Clone, Debug, PartialEq)]
pub struct AudioClip {
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl AudioClip {
    pub fn frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / usize::from(self.channels)
        }
    }

    pub fn duration(&self) -> f32 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames() as f32 / self.sample_rate as f32
        }
    }

    pub fn peak(&self) -> f32 {
        self.samples
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
    }
}

/// 极简 WAV：PCM16 / Float32，RIFF little-endian。
pub fn load_wav(path: &std::path::Path) -> Result<AudioClip, String> {
    let data =
        std::fs::read(path).map_err(|err| format!("读取 WAV {} 失败：{err}", path.display()))?;
    parse_wav(&data).map(|mut clip| {
        clip.name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        clip
    })
}

fn parse_wav(data: &[u8]) -> Result<AudioClip, String> {
    if data.get(..4) != Some(b"RIFF") || data.get(8..12) != Some(b"WAVE") {
        return Err("不是 RIFF/WAVE 文件".to_string());
    }
    let mut pos = 12;
    let mut format = None;
    let mut samples = Vec::new();
    let mut sample_rate = 0;
    let mut channels = 0;
    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let size = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = data
            .get(pos + 8..pos + 8 + size)
            .ok_or_else(|| "WAV 块越界".to_string())?;
        if id == b"fmt " {
            if body.len() < 16 {
                return Err("WAV fmt 块过短".to_string());
            }
            let tag = u16::from_le_bytes(body[0..2].try_into().unwrap());
            channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
            sample_rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            let bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
            format = Some((tag, bits));
        } else if id == b"data" {
            let Some((tag, bits)) = format else {
                return Err("WAV data 早于 fmt".to_string());
            };
            match (tag, bits) {
                (1, 16) => {
                    for chunk in body.chunks_exact(2) {
                        let value = i16::from_le_bytes([chunk[0], chunk[1]]);
                        samples.push(f32::from(value) / 32768.0);
                    }
                }
                (3, 32) => {
                    for chunk in body.chunks_exact(4) {
                        samples.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
                    }
                }
                _ => {
                    return Err(format!(
                        "WAV 只支持 PCM16 / Float32，得到 tag={tag} bits={bits}"
                    ))
                }
            }
        }
        pos += 8 + size + (size & 1);
    }
    if samples.is_empty() {
        return Err("WAV 没有采样数据".to_string());
    }
    Ok(AudioClip {
        name: String::new(),
        sample_rate,
        channels,
        samples,
    })
}

/// 目录内 `.wav` 素材，按文件名排序保证确定性。
#[derive(Clone, Debug, Default)]
pub struct AudioLibrary {
    clips: BTreeMap<String, AudioClip>,
}

impl AudioLibrary {
    pub fn load_dir(dir: &std::path::Path) -> Result<(Self, Vec<String>), String> {
        let entries = std::fs::read_dir(dir)
            .map_err(|err| format!("读取音频目录 {} 失败：{err}", dir.display()))?;
        let mut paths = Vec::new();
        for entry in entries {
            let path = entry
                .map_err(|err| format!("读取音频目录项失败：{err}"))?
                .path();
            if path.extension().is_some_and(|ext| ext == "wav") {
                paths.push(path);
            }
        }
        paths.sort();
        let mut library = Self::default();
        let mut issues = Vec::new();
        for path in paths {
            match load_wav(&path) {
                Ok(clip) => {
                    library.clips.insert(clip.name.clone(), clip);
                }
                Err(err) => issues.push(format!("{}: {err}", path.display())),
            }
        }
        Ok((library, issues))
    }

    pub fn get(&self, name: &str) -> Option<&AudioClip> {
        self.clips.get(name)
    }

    pub fn len(&self) -> usize {
        self.clips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.clips.keys().map(String::as_str)
    }
}
