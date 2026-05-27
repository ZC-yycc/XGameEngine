//! 游戏时钟与时间工具。
//!
//! `GameClock` 提供帧时间跟踪和固定步长累计器，用于将物理/动画等系统的
//! 更新频率与渲染帧率解耦。

use serde::{Deserialize, Serialize};

/// 游戏时钟：跟踪累计时间、帧 Delta，并提供固定步长累计器。
///
/// 每个渲染帧调用 `tick(dt)`，然后循环调用 `consume_fixed_update()`
/// 来执行固定频率的更新（如物理模拟）。
///
/// # Example
///
/// ```ignore
/// let mut clock = GameClock::new().with_fixed_dt(1.0 / 60.0);
///
/// // 每帧：
/// clock.tick(frame_dt);
/// while clock.consume_fixed_update() {
///     physics_world.step(clock.fixed_dt());
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameClock {
    /// 从游戏开始经过的总时间（秒）。
    total_time: f32,
    /// 上一帧的 Delta 时间（秒）。
    delta_time: f32,
    /// 固定步长累计器。
    accumulator: f32,
    /// 固定步长大小（秒）。
    fixed_dt: f32,
    /// 每帧允许的最大固定步长数（安全限幅，防止"死亡螺旋"）。
    max_steps: u32,
    /// 时间缩放系数（1.0 = 实时，0.5 = 半速，2.0 = 双倍速）。
    time_scale: f32,
}

impl GameClock {
    /// 创建一个新时钟，默认 60Hz 固定步长。
    pub fn new() -> Self {
        Self {
            total_time: 0.0,
            delta_time: 0.0,
            accumulator: 0.0,
            fixed_dt: 1.0 / 60.0,
            max_steps: 5,
            time_scale: 1.0,
        }
    }

    /// 设置固定步长大小（秒）。
    pub fn with_fixed_dt(mut self, dt: f32) -> Self {
        self.fixed_dt = dt;
        self
    }

    /// 推进一帧（必须在帧开始时调用）。
    ///
    /// `dt` 是原始帧 Delta（秒），会被 `time_scale` 缩放。
    pub fn tick(&mut self, dt: f32) {
        let scaled_dt = dt * self.time_scale;
        self.delta_time = scaled_dt;
        self.total_time += scaled_dt;
        self.accumulator += scaled_dt;
    }

    /// 消耗一个固定步长。
    ///
    /// 返回 `true` 表示可以执行一次固定步长更新（如物理步进）；
    /// 返回 `false` 表示累计器不足以支撑一个完整步长。
    ///
    /// 调用后必须用 `reset_step_counter()` 重置步数限制。
    pub fn consume_fixed_update(&mut self) -> bool {
        if self.accumulator >= self.fixed_dt && self.max_steps > 0 {
            self.accumulator -= self.fixed_dt;
            self.max_steps -= 1;
            true
        } else {
            false
        }
    }

    /// 在完成本帧所有固定更新后调用，重置步数限制。
    pub fn reset_step_counter(&mut self) {
        self.max_steps = 5;
    }

    /// 获取总游戏时间。
    pub fn total_time(&self) -> f32 {
        self.total_time
    }

    /// 获取上一帧的 Delta 时间（经过 time_scale 缩放）。
    pub fn delta_time(&self) -> f32 {
        self.delta_time
    }

    /// 获取固定步长大小。
    pub fn fixed_dt(&self) -> f32 {
        self.fixed_dt
    }

    /// 获取当前时间缩放系数。
    pub fn time_scale(&self) -> f32 {
        self.time_scale
    }

    /// 设置时间缩放系数。
    pub fn set_time_scale(&mut self, scale: f32) {
        self.time_scale = scale.max(0.0);
    }

    /// 获取两个固定步长之间的插值因子（0..1），用于渲染插值。
    pub fn alpha(&self) -> f32 {
        self.accumulator / self.fixed_dt
    }

    /// 固定步长累计器中的剩余时间（用于调试）。
    pub fn accumulator(&self) -> f32 {
        self.accumulator
    }
}

impl Default for GameClock {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Timer（倒计时器）
// ---------------------------------------------------------------------------

/// 一次性或循环倒计时器。
///
/// # Example
///
/// ```ignore
/// let mut timer = Timer::once(2.0); // 2 秒后触发
/// timer.tick(0.016);
/// if timer.finished() { ... }
/// ```
#[derive(Debug, Clone)]
pub struct Timer {
    duration: f32,
    elapsed: f32,
    repeating: bool,
    finished: bool,
}

impl Timer {
    /// 创建一个一次性计时器，持续 `duration` 秒后触发一次。
    pub fn once(duration: f32) -> Self {
        Self {
            duration: duration.max(0.0),
            elapsed: 0.0,
            repeating: false,
            finished: false,
        }
    }

    /// 创建一个循环计时器，每 `interval` 秒触发一次。
    pub fn repeating(interval: f32) -> Self {
        Self {
            duration: interval.max(0.0),
            elapsed: 0.0,
            repeating: true,
            finished: false,
        }
    }

    /// 推进计时器。
    pub fn tick(&mut self, dt: f32) {
        if self.finished {
            return;
        }
        self.elapsed += dt;
        if self.elapsed >= self.duration {
            if self.repeating {
                self.elapsed -= self.duration;
            } else {
                self.finished = true;
            }
        }
    }

    /// 计时器是否已到触发点。
    pub fn finished(&self) -> bool {
        self.finished
    }

    /// 重置计时器。
    pub fn reset(&mut self) {
        self.elapsed = 0.0;
        self.finished = false;
    }

    /// 进度：0.0（开始）到 1.0（结束）。
    pub fn progress(&self) -> f32 {
        if self.duration > 0.0 {
            (self.elapsed / self.duration).min(1.0)
        } else {
            1.0
        }
    }

    /// 距离触发的剩余时间。
    pub fn remaining(&self) -> f32 {
        (self.duration - self.elapsed).max(0.0)
    }
}

// ---------------------------------------------------------------------------
// Stopwatch（秒表）
// ---------------------------------------------------------------------------

/// 简单的秒表——累计时间。
#[derive(Debug, Clone, Default)]
pub struct Stopwatch {
    elapsed: f32,
    running: bool,
}

impl Stopwatch {
    pub fn new() -> Self {
        Self {
            elapsed: 0.0,
            running: false,
        }
    }

    /// 开始计时。
    pub fn start(&mut self) {
        self.running = true;
    }

    /// 停止计时。
    pub fn stop(&mut self) {
        self.running = false;
    }

    /// 重置为 0。
    pub fn reset(&mut self) {
        self.elapsed = 0.0;
    }

    /// 清除并停止。
    pub fn clear(&mut self) {
        self.elapsed = 0.0;
        self.running = false;
    }

    /// 推进秒表（仅在运行状态累计）。
    pub fn tick(&mut self, dt: f32) {
        if self.running {
            self.elapsed += dt;
        }
    }

    /// 当前累计时间。
    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    /// 是否正在运行。
    pub fn is_running(&self) -> bool {
        self.running
    }
}
