//! 音频设备输出：把 PCM 样本推到默认输出设备；无设备时返回可诊断错误。

mod pcm;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use xgc_sim::AudioClip;

pub use pcm::PcmStream;
use pcm::{access_queue, PcmQueue, SharedQueue};

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
    let (device, config) = default_output()?;
    let output_rate = config.sample_rate();
    let samples = resample(clip, output_rate);
    let duration = Duration::from_secs_f64(samples.len() as f64 / f64::from(output_rate.max(1)));
    let queue = Arc::new(Mutex::new(PcmQueue::from_device_samples(
        samples,
        output_rate,
    )));
    let stream = start_output(&device, config, &queue)?;
    Ok(Player { stream, duration })
}

fn default_output() -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| "没有默认输出设备".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|err| format!("读取输出配置失败：{err}"))?;
    Ok((device, supported))
}

fn start_output(
    device: &cpal::Device,
    supported: cpal::SupportedStreamConfig,
    queue: &SharedQueue,
) -> Result<cpal::Stream, String> {
    let format = supported.sample_format();
    let config = supported.config();
    let queue = Arc::clone(queue);
    let stream = match format {
        cpal::SampleFormat::F32 => build_output::<f32>(device, config, queue),
        cpal::SampleFormat::I16 => build_output::<i16>(device, config, queue),
        cpal::SampleFormat::U16 => build_output::<u16>(device, config, queue),
        other => return Err(format!("不支持的输出样本格式：{other:?}")),
    }?;
    stream
        .play()
        .map_err(|err| format!("启动输出流失败：{err}"))?;
    Ok(stream)
}

fn build_output<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    queue: SharedQueue,
) -> Result<cpal::Stream, String> {
    let channels = usize::from(config.channels);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| access_queue(&queue).fill(data, channels),
            |err| eprintln!("音频输出错误：{err}"),
            None,
        )
        .map_err(|err| format!("创建输出流失败：{err}"))
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
