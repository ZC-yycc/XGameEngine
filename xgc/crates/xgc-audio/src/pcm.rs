//! 窗口固定步产生的 PCM 以连续的采样时钟送入输出设备。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cpal::{FromSample, SizedSample};

use super::{default_output, start_output};

/// 一条持续输出的设备流。设备时钟不参与世界步进。
pub struct PcmStream {
    _stream: cpal::Stream,
    queue: SharedQueue,
}

impl PcmStream {
    pub fn open(input_rate: u32) -> Result<Self, String> {
        if input_rate == 0 {
            return Err("PCM 输入采样率必须大于零".to_string());
        }
        let (device, config) = default_output()?;
        let output_rate = config.sample_rate();
        if output_rate == 0 {
            return Err("设备输出采样率必须大于零".to_string());
        }
        // 给渲染抖动留一点余量，长时间停帧后也不追播过时的音频。
        let capacity = output_rate.div_ceil(10) as usize;
        let queue = Arc::new(Mutex::new(PcmQueue::new(input_rate, output_rate, capacity)));
        let stream = start_output(&device, config, &queue)?;
        Ok(Self {
            _stream: stream,
            queue,
        })
    }

    pub fn push(&self, samples: &[f32]) {
        access_queue(&self.queue).push(samples);
    }

    /// 退出或暂停播放时丢弃尾部，下一块从新的一段采样开始。
    pub fn clear(&self) {
        access_queue(&self.queue).clear();
    }
}

pub(super) type SharedQueue = Arc<Mutex<PcmQueue>>;

pub(super) fn access_queue(queue: &SharedQueue) -> std::sync::MutexGuard<'_, PcmQueue> {
    queue.lock().unwrap_or_else(|err| err.into_inner())
}

pub(super) struct PcmQueue {
    samples: VecDeque<f32>,
    capacity: usize,
    input_rate: u64,
    output_rate: u64,
    next_phase: u64,
    previous: Option<f32>,
}

impl PcmQueue {
    fn new(input_rate: u32, output_rate: u32, capacity: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(capacity),
            capacity,
            input_rate: u64::from(input_rate),
            output_rate: u64::from(output_rate),
            next_phase: u64::from(input_rate),
            previous: None,
        }
    }

    pub(super) fn from_device_samples(samples: Vec<f32>, sample_rate: u32) -> Self {
        Self {
            capacity: samples.len(),
            samples: samples.into(),
            input_rate: u64::from(sample_rate),
            output_rate: u64::from(sample_rate),
            next_phase: u64::from(sample_rate),
            previous: None,
        }
    }

    fn enqueue(&mut self, sample: f32) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    fn push(&mut self, samples: &[f32]) {
        for &sample in samples {
            let sample = if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            if let Some(previous) = self.previous.replace(sample) {
                // 相位以输入样点间隔为基准，跨固定步边界时保留上一样点。
                while self.next_phase <= self.output_rate {
                    let weight = self.next_phase as f32 / self.output_rate as f32;
                    self.enqueue(previous * (1.0 - weight) + sample * weight);
                    self.next_phase += self.input_rate;
                }
                self.next_phase -= self.output_rate;
            } else {
                self.enqueue(sample);
            }
        }
    }

    fn clear(&mut self) {
        self.samples.clear();
        self.previous = None;
        self.next_phase = self.input_rate;
    }

    pub(super) fn fill<T: SizedSample + FromSample<f32>>(
        &mut self,
        data: &mut [T],
        channels: usize,
    ) {
        for frame in data.chunks_mut(channels.max(1)) {
            let value = T::from_sample(self.samples.pop_front().unwrap_or(0.0));
            frame.fill(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_keeps_fractional_phase_across_fixed_steps() {
        let mut queue = PcmQueue::new(4_000, 6_000, 16);
        let mut first = [0.0; 2];
        queue.push(&[0.0, 0.75]);
        queue.fill(&mut first, 1);
        assert_eq!(first, [0.0, 0.5]);
        let mut second = [0.0_f32; 3];
        queue.push(&[-0.75, 0.0]);
        queue.fill(&mut second, 1);
        for (actual, expected) in second.into_iter().zip([0.25, -0.75, -0.25]) {
            assert!((actual - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn callback_drains_a_bounded_queue_and_clears_old_playback() {
        let mut queue = PcmQueue::new(48_000, 48_000, 3);
        let capacity = queue.samples.capacity();
        queue.push(&[0.25, -0.25, 0.75, f32::NAN]);
        let mut out = [1.0_f32; 8];
        queue.fill(&mut out, 2);
        assert_eq!(out, [-0.25, -0.25, 0.75, 0.75, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(queue.samples.capacity(), capacity);
        queue.push(&[0.25]);
        queue.clear();
        queue.push(&[-0.75]);
        queue.fill(&mut out, 2);
        assert_eq!(out, [-0.75, -0.75, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    }
}
