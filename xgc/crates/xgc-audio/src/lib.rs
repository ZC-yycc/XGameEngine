//! 音频设备输出：把 PCM 样本推到默认输出设备；无设备时返回可诊断错误。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use xgc_sim::AudioClip;

pub fn output_devices() -> Result<Vec<String>, String> {
    let host = cpal::default_host();
    let devices = host
        .output_devices()
        .map_err(|err| format!("枚举输出设备失败：{err}"))?;
    Ok(devices
        .filter_map(|device| {
            device
                .description()
                .ok()
                .map(|description| description.name().to_string())
        })
        .collect())
}

pub struct Player {
    stream: cpal::Stream,
    duration: Duration,
}

impl Player {
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// 播放到结束（阻塞）；CLI 与离线工具共用。
    pub fn wait(&self) {
        std::thread::sleep(self.duration + Duration::from_millis(50));
    }

    pub fn stream(&self) -> &cpal::Stream {
        &self.stream
    }
}

/// 把单声道 / 多声道 clip 线性重采样到设备采样率后播放。
pub fn play_clip(clip: &AudioClip) -> Result<Player, String> {
    if clip.samples.is_empty() {
        return Err("音频为空".to_string());
    }
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "没有默认输出设备".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|err| format!("读取输出配置失败：{err}"))?;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let channels = usize::from(config.channels);
    let output_rate = config.sample_rate;
    let samples = resample(clip, output_rate);
    let duration = Duration::from_secs_f64(samples.len() as f64 / f64::from(output_rate.max(1)));
    let buffer = Arc::new(Mutex::new(VecDeque::from(samples)));
    let err_fn = move |err| {
        eprintln!("音频输出错误：{err}");
    };
    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let queue = Arc::clone(&buffer);
            device.build_output_stream(
                config,
                move |data: &mut [f32], _| fill_f32(data, channels, &queue),
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let queue = Arc::clone(&buffer);
            device.build_output_stream(
                config,
                move |data: &mut [i16], _| fill_i16(data, channels, &queue),
                err_fn,
                None,
            )
        }
        other => return Err(format!("不支持的输出样本格式：{other:?}")),
    }
    .map_err(|err| format!("创建输出流失败：{err}"))?;
    stream
        .play()
        .map_err(|err| format!("启动输出流失败：{err}"))?;
    Ok(Player { stream, duration })
}

fn resample(clip: &AudioClip, output_rate: u32) -> Vec<f32> {
    let channels = usize::from(clip.channels.max(1));
    let mut mono = Vec::with_capacity(clip.samples.len() / channels);
    for frame in clip.samples.chunks(channels) {
        let sum: f32 = frame.iter().sum();
        mono.push(sum / channels as f32);
    }
    if clip.sample_rate == output_rate || mono.is_empty() {
        return mono;
    }
    let ratio = f64::from(clip.sample_rate.max(1)) / f64::from(output_rate.max(1));
    let length = ((mono.len() as f64) / ratio).round().max(1.0) as usize;
    let mut output = Vec::with_capacity(length);
    for index in 0..length {
        let position = index as f64 * ratio;
        let left = position.floor() as usize;
        let right = (left + 1).min(mono.len() - 1);
        let t = (position - left as f64) as f32;
        output.push(mono[left.min(mono.len() - 1)] * (1.0 - t) + mono[right] * t);
    }
    output
}

fn fill_f32(data: &mut [f32], channels: usize, queue: &Arc<Mutex<VecDeque<f32>>>) {
    let mut queue = queue.lock().unwrap_or_else(|error| error.into_inner());
    for frame in data.chunks_mut(channels.max(1)) {
        let sample = queue.pop_front().unwrap_or(0.0);
        for value in frame {
            *value = sample;
        }
    }
}

fn fill_i16(data: &mut [i16], channels: usize, queue: &Arc<Mutex<VecDeque<f32>>>) {
    let mut queue = queue.lock().unwrap_or_else(|error| error.into_inner());
    for frame in data.chunks_mut(channels.max(1)) {
        let sample = queue.pop_front().unwrap_or(0.0).clamp(-1.0, 1.0);
        let value = (sample * f32::from(i16::MAX)) as i16;
        for slot in frame {
            *slot = value;
        }
    }
}
