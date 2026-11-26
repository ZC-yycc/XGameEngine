//! 原生窗口共用的 PCM 输出，设备缺失只影响声音。

use xgc_audio::PcmStream;

use crate::AUDIO_SAMPLE_RATE;

#[derive(Default)]
pub struct WindowAudio {
    stream: Option<PcmStream>,
    attempted: bool,
}

impl WindowAudio {
    /// 首个有声音的固定步才启动设备，静音场景的窗口无需音频硬件。
    pub fn submit(&mut self, samples: &[f32]) {
        if !self.attempted && samples.iter().any(|sample| *sample != 0.0) {
            self.attempted = true;
            match PcmStream::open(AUDIO_SAMPLE_RATE) {
                Ok(stream) => self.stream = Some(stream),
                Err(err) => eprintln!("audio: 窗口音频不可用：{err}"),
            }
        }
        if let Some(stream) = &self.stream {
            stream.push(samples);
        }
    }

    pub fn clear(&self) {
        if let Some(stream) = &self.stream {
            stream.clear();
        }
    }
}
