//! 渲染姿势与根运动共用的 glTF 三次样条段定位和求值。

/// 一个采样时刻落到的关键帧段。时间轴由加载端验证为有限且严格递增。
pub struct CubicSplineSegment {
    pub lower: usize,
    pub upper: usize,
    pub fraction: f32,
    duration: f32,
}

impl CubicSplineSegment {
    pub fn at(times: &[f32], time: f32) -> Self {
        let last = times.len() - 1;
        let upper = times.partition_point(|&key| key < time).min(last);
        if upper == 0 || time >= times[upper] {
            return Self {
                lower: upper,
                upper,
                fraction: 0.0,
                duration: 0.0,
            };
        }
        let lower = upper - 1;
        let duration = times[upper] - times[lower];
        Self {
            lower,
            upper,
            fraction: (time - times[lower]) / duration,
            duration,
        }
    }

    pub fn sample<const N: usize>(
        &self,
        start: [f32; N],
        outgoing: [f32; N],
        end: [f32; N],
        incoming: [f32; N],
    ) -> [f32; N] {
        if self.lower == self.upper {
            return start;
        }
        let u = self.fraction;
        let u2 = u * u;
        let u3 = u2 * u;
        let h00 = 2.0 * u3 - 3.0 * u2 + 1.0;
        let h10 = u3 - 2.0 * u2 + u;
        let h01 = -2.0 * u3 + 3.0 * u2;
        let h11 = u3 - u2;
        // glTF 切线以秒为单位，须乘段长才可代入归一化到 [0, 1] 的基函数。
        std::array::from_fn(|i| {
            start[i] * h00
                + (outgoing[i] * self.duration) * h10
                + end[i] * h01
                + (incoming[i] * self.duration) * h11
        })
    }
}
