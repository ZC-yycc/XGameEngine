//! 动画剪辑采样：平移关键帧的线性插值。
//!
//! 这是资产 / 组件之外的纯数据层；`AnimatorSystem` 目前只推进时钟，后续把
//! `clip` 映射到 `AnimationClip` 后再把采样结果写进变换。

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TranslationKey {
    pub time: f32,
    pub value: [f32; 3],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationClip {
    pub name: String,
    pub duration: f32,
    pub translation: Vec<TranslationKey>,
}

impl AnimationClip {
    /// 关键帧按时间排序；NaN 时间被丢弃，保证采样确定性。
    pub fn new(name: impl Into<String>, duration: f32, keys: Vec<TranslationKey>) -> Self {
        let mut translation: Vec<_> = keys
            .into_iter()
            .filter(|key| key.time.is_finite())
            .collect();
        translation.sort_by(|a, b| a.time.total_cmp(&b.time));
        Self {
            name: name.into(),
            duration: duration.max(0.0),
            translation,
        }
    }

    pub fn sample_translation(&self, time: f32) -> [f32; 3] {
        let Some(first) = self.translation.first() else {
            return [0.0; 3];
        };
        let last = self.translation.last().unwrap();
        if !time.is_finite() || time <= first.time {
            return first.value;
        }
        if time >= last.time {
            return last.value;
        }
        let upper = self.translation.partition_point(|key| key.time <= time);
        let left = &self.translation[upper - 1];
        let right = &self.translation[upper];
        let span = right.time - left.time;
        let t = if span > f32::EPSILON {
            (time - left.time) / span
        } else {
            0.0
        };
        [
            left.value[0] + (right.value[0] - left.value[0]) * t,
            left.value[1] + (right.value[1] - left.value[1]) * t,
            left.value[2] + (right.value[2] - left.value[2]) * t,
        ]
    }

    /// 循环采样：duration > 0 时对时间取模，否则等价 `sample_translation`。
    pub fn sample_looped(&self, time: f32) -> [f32; 3] {
        if self.duration > 0.0 {
            self.sample_translation(time.rem_euclid(self.duration))
        } else {
            self.sample_translation(time)
        }
    }
}
