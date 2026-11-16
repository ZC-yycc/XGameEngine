//! 动画剪辑采样：平移关键帧的线性插值。
//!
//! 这是资产 / 组件之外的纯数据层；`AnimatorSystem` 目前只推进时钟，后续把
//! `clip` 映射到 `AnimationClip` 后再把采样结果写进变换。

use std::collections::BTreeMap;

use serde::Deserialize;

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

#[derive(Debug, Deserialize)]
struct ClipFile {
    name: String,
    #[serde(default)]
    duration: f32,
    #[serde(default)]
    translation: Vec<KeyFile>,
}

#[derive(Debug, Deserialize)]
struct KeyFile {
    time: f32,
    value: [f32; 3],
}

/// RON 剪辑文件 → `AnimationClip`；字段缺失按默认值。
pub fn load_clip_ron(text: &str) -> Result<AnimationClip, String> {
    let file: ClipFile = ron::from_str(text).map_err(|err| format!("剪辑解析失败：{err}"))?;
    let keys = file
        .translation
        .into_iter()
        .map(|key| TranslationKey {
            time: key.time,
            value: key.value,
        })
        .collect();
    Ok(AnimationClip::new(file.name, file.duration, keys))
}

/// 按名字索引的剪辑库；BTreeMap 保证遍历 / 导出顺序确定。
#[derive(Clone, Debug, Default)]
pub struct AnimationLibrary {
    clips: BTreeMap<String, AnimationClip>,
}

impl AnimationLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, clip: AnimationClip) -> Option<AnimationClip> {
        let name = clip.name.clone();
        self.clips.insert(name, clip)
    }

    pub fn get(&self, name: &str) -> Option<&AnimationClip> {
        self.clips.get(name)
    }

    pub fn len(&self) -> usize {
        self.clips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    pub fn sample_looped(&self, name: &str, time: f32) -> Option<[f32; 3]> {
        self.get(name).map(|clip| clip.sample_looped(time))
    }
}
