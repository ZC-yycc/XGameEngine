//! 关键帧布局和 TRS 采样。输出访问器的组序只在这里解释。

use xgc_world::CubicSplineSegment;

use super::{lerp, normalize_quat, slerp, AnimationChannel, AnimationPath, Interpolation};

impl AnimationChannel {
    pub(in crate::glb) fn has_complete_keys(&self) -> bool {
        !self.times.is_empty() && self.values.len() == self.times.len() * self.frame_len()
    }

    fn frame_len(&self) -> usize {
        self.interpolation.values_per_key() * self.path.components()
    }

    fn frame_start(&self, key: usize) -> usize {
        key * self.frame_len()
    }

    fn value_start(&self, key: usize) -> usize {
        let value_slot = usize::from(self.interpolation == Interpolation::CubicSpline);
        self.frame_start(key) + value_slot * self.path.components()
    }

    fn read_components<const N: usize>(&self, start: usize) -> [f32; N] {
        let components = self.path.components();
        let mut out = [0.0; N];
        out[..components].copy_from_slice(&self.values[start..start + components]);
        out
    }

    pub(in crate::glb) fn key_value<const N: usize>(&self, key: usize) -> [f32; N] {
        self.read_components(self.value_start(key))
    }

    fn outgoing_start(&self, key: usize) -> usize {
        self.value_start(key) + self.path.components()
    }

    pub(in crate::glb) fn cubic_key<const N: usize>(&self, key: usize) -> [[f32; N]; 3] {
        [
            self.read_components(self.frame_start(key)),
            self.key_value(key),
            self.read_components(self.outgoing_start(key)),
        ]
    }

    pub(super) fn sample(&self, time: f32) -> Option<[f32; 4]> {
        if !self.has_complete_keys() {
            return None;
        }
        if self.interpolation == Interpolation::CubicSpline {
            return Some(self.sample_cubic(time));
        }
        let last = self.times.len() - 1;
        if time <= self.times[0] {
            return Some(self.key_value(0));
        }
        if time >= self.times[last] {
            return Some(self.key_value(last));
        }
        let mut index = 0;
        while index + 1 < self.times.len() && self.times[index + 1] <= time {
            index += 1;
        }
        let (start, end) = (self.times[index], self.times[index + 1]);
        let u = if end > start {
            (time - start) / (end - start)
        } else {
            0.0
        };
        let a = self.key_value(index);
        if self.interpolation == Interpolation::Step {
            return Some(a);
        }
        let b = self.key_value(index + 1);
        Some(match self.path {
            AnimationPath::Rotation => slerp(a, b, u),
            _ => [
                lerp(a[0], b[0], u),
                lerp(a[1], b[1], u),
                lerp(a[2], b[2], u),
                0.0,
            ],
        })
    }

    fn sample_cubic(&self, time: f32) -> [f32; 4] {
        let segment = CubicSplineSegment::at(&self.times, time);
        let a = self.key_value(segment.lower);
        if segment.lower == segment.upper {
            return match self.path {
                AnimationPath::Rotation => normalize_quat(a),
                _ => a,
            };
        }
        let b = self.key_value(segment.upper);
        let value = segment.sample(
            a,
            self.read_components(self.outgoing_start(segment.lower)),
            b,
            self.read_components(self.frame_start(segment.upper)),
        );
        if self.path != AnimationPath::Rotation {
            return value;
        }
        // 分量插值会离开单位球，近零长度时用端点走短弧，防止把无效旋转写入矩阵。
        let length_squared = value
            .iter()
            .map(|component| component * component)
            .sum::<f32>();
        if length_squared.is_finite() && length_squared > 1e-12 {
            normalize_quat(value)
        } else {
            slerp(a, b, segment.fraction)
        }
    }
}
