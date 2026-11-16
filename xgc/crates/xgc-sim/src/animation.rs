//! 动画剪辑采样：平移关键帧的线性插值。
//!
//! 这是资产 / 组件之外的纯数据层；`AnimatorSystem` 目前只推进时钟，后续把
//! `clip` 映射到 `AnimationClip` 后再把采样结果写进变换。

use std::collections::BTreeMap;

use serde::Deserialize;

use xgc_world::{Animator, Component, Transform, World};

use super::{System, SystemAccess};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TranslationKey {
    pub time: f32,
    pub value: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RotationKey {
    pub time: f32,
    pub value: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaleKey {
    pub time: f32,
    pub value: [f32; 3],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationClip {
    pub name: String,
    pub duration: f32,
    pub translation: Vec<TranslationKey>,
    pub rotation: Vec<RotationKey>,
    pub scale: Vec<ScaleKey>,
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
            rotation: Vec::new(),
            scale: Vec::new(),
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

    pub fn with_rotation(mut self, keys: Vec<RotationKey>) -> Self {
        self.rotation = keys
            .into_iter()
            .filter(|key| key.time.is_finite())
            .collect();
        self.rotation.sort_by(|a, b| a.time.total_cmp(&b.time));
        self
    }

    pub fn with_scale(mut self, keys: Vec<ScaleKey>) -> Self {
        self.scale = keys
            .into_iter()
            .filter(|key| key.time.is_finite())
            .collect();
        self.scale.sort_by(|a, b| a.time.total_cmp(&b.time));
        self
    }

    /// 旋转关键帧 slerp；没有关键帧时返回单位四元数。
    pub fn sample_rotation(&self, time: f32) -> [f32; 4] {
        let Some(first) = self.rotation.first() else {
            return [0.0, 0.0, 0.0, 1.0];
        };
        let last = self.rotation.last().unwrap();
        if !time.is_finite() || time <= first.time {
            return normalize_quat(first.value);
        }
        if time >= last.time {
            return normalize_quat(last.value);
        }
        let upper = self.rotation.partition_point(|key| key.time <= time);
        let left = &self.rotation[upper - 1];
        let right = &self.rotation[upper];
        let span = right.time - left.time;
        let t = if span > f32::EPSILON {
            (time - left.time) / span
        } else {
            0.0
        };
        slerp(left.value, right.value, t)
    }

    pub fn sample_scale(&self, time: f32) -> [f32; 3] {
        let Some(first) = self.scale.first() else {
            return [1.0; 3];
        };
        let last = self.scale.last().unwrap();
        if !time.is_finite() || time <= first.time {
            return first.value;
        }
        if time >= last.time {
            return last.value;
        }
        let upper = self.scale.partition_point(|key| key.time <= time);
        let left = &self.scale[upper - 1];
        let right = &self.scale[upper];
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

    pub fn sample_looped_rotation(&self, time: f32) -> [f32; 4] {
        self.sample_rotation(loop_time(time, self.duration))
    }

    pub fn sample_looped_scale(&self, time: f32) -> [f32; 3] {
        self.sample_scale(loop_time(time, self.duration))
    }
}

fn loop_time(time: f32, duration: f32) -> f32 {
    if duration > 0.0 {
        time.rem_euclid(duration)
    } else {
        time
    }
}

fn normalize_quat(mut q: [f32; 4]) -> [f32; 4] {
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if len <= f32::EPSILON {
        return [0.0, 0.0, 0.0, 1.0];
    }
    for value in &mut q {
        *value /= len;
    }
    q
}

pub fn slerp(mut a: [f32; 4], mut b: [f32; 4], t: f32) -> [f32; 4] {
    a = normalize_quat(a);
    b = normalize_quat(b);
    let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    if dot < 0.0 {
        for value in &mut b {
            *value = -*value;
        }
        dot = -dot;
    }
    if dot > 0.9995 {
        let mut out = [0.0; 4];
        for index in 0..4 {
            out[index] = a[index] + (b[index] - a[index]) * t;
        }
        return normalize_quat(out);
    }
    let theta = dot.clamp(-1.0, 1.0).acos();
    let sin_theta = theta.sin();
    let wa = ((1.0 - t) * theta).sin() / sin_theta;
    let wb = (t * theta).sin() / sin_theta;
    normalize_quat([
        a[0] * wa + b[0] * wb,
        a[1] * wa + b[1] * wb,
        a[2] * wa + b[2] * wb,
        a[3] * wa + b[3] * wb,
    ])
}

/// `base * sample` 的 Hamilton 积，用于把剪辑旋转叠到基础姿态上。
pub fn mul_quat(base: [f32; 4], sample: [f32; 4]) -> [f32; 4] {
    normalize_quat([
        base[3] * sample[0] + base[0] * sample[3] + base[1] * sample[2] - base[2] * sample[1],
        base[3] * sample[1] - base[0] * sample[2] + base[1] * sample[3] + base[2] * sample[0],
        base[3] * sample[2] + base[0] * sample[1] - base[1] * sample[0] + base[2] * sample[3],
        base[3] * sample[3] - base[0] * sample[0] - base[1] * sample[1] - base[2] * sample[2],
    ])
}

#[derive(Debug, Deserialize)]
struct ClipFile {
    name: String,
    #[serde(default)]
    duration: f32,
    #[serde(default)]
    translation: Vec<KeyFile>,
    #[serde(default)]
    rotation: Vec<RotationFile>,
    #[serde(default)]
    scale: Vec<ScaleFile>,
}

#[derive(Debug, Deserialize)]
struct RotationFile {
    time: f32,
    value: [f32; 4],
}

#[derive(Debug, Deserialize)]
struct ScaleFile {
    time: f32,
    value: [f32; 3],
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
    let rotation = file
        .rotation
        .into_iter()
        .map(|key| RotationKey {
            time: key.time,
            value: key.value,
        })
        .collect();
    let scale = file
        .scale
        .into_iter()
        .map(|key| ScaleKey {
            time: key.time,
            value: key.value,
        })
        .collect();
    Ok(AnimationClip::new(file.name, file.duration, keys)
        .with_rotation(rotation)
        .with_scale(scale))
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

    /// 扫描目录下的 `.ron` 剪辑文件；按路径排序保证确定性，坏文件只记录不中止。
    pub fn load_dir(dir: &std::path::Path) -> Result<(Self, Vec<String>), String> {
        let entries = std::fs::read_dir(dir)
            .map_err(|err| format!("读取剪辑目录 {} 失败：{err}", dir.display()))?;
        let mut paths = Vec::new();
        for entry in entries {
            let path = entry
                .map_err(|err| format!("读取剪辑目录项失败：{err}"))?
                .path();
            if path.extension().is_some_and(|ext| ext == "ron") {
                paths.push(path);
            }
        }
        paths.sort();
        let mut library = Self::new();
        let mut issues = Vec::new();
        for path in paths {
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(err) => {
                    issues.push(format!("{}: 读取失败：{err}", path.display()));
                    continue;
                }
            };
            match load_clip_ron(&text) {
                Ok(clip) => {
                    library.insert(clip);
                }
                Err(err) => issues.push(format!("{}: {err}", path.display())),
            }
        }
        Ok((library, issues))
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.clips.keys().map(String::as_str)
    }
}

/// 把剪辑平移关键帧写到 Transform：位置 = base_position + 采样偏移。
#[derive(Clone, Debug, Default)]
pub struct AnimationPlaybackSystem {
    pub library: AnimationLibrary,
}

impl System for AnimationPlaybackSystem {
    fn name(&self) -> &'static str {
        "animation_apply"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![Animator::ID],
            writes: vec![Transform::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        let mut samples = Vec::new();
        world.for_each::<Animator>(|key, animator| {
            let Some(offset) = self.library.sample_looped(&animator.clip, animator.time) else {
                return;
            };
            let Some(clip) = self.library.get(&animator.clip) else {
                return;
            };
            let sample_scale = clip.sample_looped_scale(animator.time);
            let base = animator.base_position;
            let mut position = [
                base[0] + offset[0],
                base[1] + offset[1],
                base[2] + offset[2],
            ];
            let mut rotation = mul_quat(
                animator.base_rotation,
                clip.sample_looped_rotation(animator.time),
            );
            let mut scale = [
                animator.base_scale[0] * sample_scale[0],
                animator.base_scale[1] * sample_scale[1],
                animator.base_scale[2] * sample_scale[2],
            ];
            if animator.blend_duration > 0.0 && !animator.blend_from.is_empty() {
                if let Some(previous) = self.library.get(&animator.blend_from) {
                    let old_offset = previous.sample_looped(animator.time);
                    let old_rotation = mul_quat(
                        animator.base_rotation,
                        previous.sample_looped_rotation(animator.time),
                    );
                    let old_scale = previous.sample_looped_scale(animator.time);
                    let t = (animator.blend_elapsed / animator.blend_duration).clamp(0.0, 1.0);
                    for axis in 0..3 {
                        let old = base[axis] + old_offset[axis];
                        position[axis] = old + (position[axis] - old) * t;
                        let old = animator.base_scale[axis] * old_scale[axis];
                        scale[axis] = old + (scale[axis] - old) * t;
                    }
                    rotation = slerp(old_rotation, rotation, t);
                }
            }
            samples.push((key, position, rotation, scale));
        });
        for (key, position, rotation, scale) in samples {
            if let Some(transform) = world.get_mut::<Transform>(key) {
                transform.position = position;
                transform.rotation = rotation;
                transform.scale = scale;
            }
        }
    }
}
