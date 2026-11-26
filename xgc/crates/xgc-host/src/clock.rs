use std::time::Duration;

/// 窗口卡顿后只追有限步，避免积压把渲染和输入一起拖住。
const MAX_CATCH_UP_STEPS: u32 = 4;

/// 用固定位长累积窗口时间，重绘本身不推进仿真。
pub struct FixedStepClock {
    step: Duration,
    elapsed: Duration,
}

impl FixedStepClock {
    pub fn new(fixed_dt: f32) -> Self {
        let step = Duration::from_secs_f32(fixed_dt);
        assert!(!step.is_zero(), "固定步长度必须大于零");
        Self {
            step,
            elapsed: Duration::ZERO,
        }
    }

    /// 返回本轮应推进的步数，以及距下一个固定步的时间。
    pub fn advance(&mut self, elapsed: Duration) -> (u32, Duration) {
        self.elapsed = self
            .elapsed
            .saturating_add(elapsed)
            .min(self.step * MAX_CATCH_UP_STEPS);
        let steps = (self.elapsed.as_nanos() / self.step.as_nanos()) as u32;
        self.elapsed -= self.step * steps;
        (steps, self.step - self.elapsed)
    }

    pub fn reset(&mut self) {
        self.elapsed = Duration::ZERO;
    }
}
