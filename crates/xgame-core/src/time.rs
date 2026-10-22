//! 游戏时钟与时间工具。
//!
//! `GameClock` 提供帧时间跟踪和固定步长累计器，用于将物理/动画等系统的
//! 更新频率与渲染帧率解耦。

use serde::{Deserialize, Serialize};

/// 时间缩放的上界。
///
/// 上界的存在理由不是精度，而是**给死亡螺旋留出口**：`max_steps` 每帧只放行
/// 5 个固定步，100 倍速已经保证"每帧都跑满预算"，再往上只会把累计器堆成
/// 无穷大 —— 之后调回 1 倍速时要连着几百帧补步，一帧卡死。Unity 同样把
/// 上界钉在 100。
pub const MAX_TIME_SCALE: f32 = 100.0;

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

    /// 设置时间缩放系数（钳在 `0.0..=MAX_TIME_SCALE`）。
    ///
    /// `0` = 冻结模拟：`delta_time`、`total_time`、固定步累计器都不再前进，
    /// 但调用方（`World::update`）**照常每帧被调用** —— 暂停菜单要靠这一点
    /// 才能响应点击。这与"宿主干脆不推进帧"是两件事（编辑器 Pause 按钮走后者）。
    ///
    /// 非有限值（NaN / ±∞）**忽略**，保住上一档而不是悄悄变成 0：缩放值通常来自
    /// 脚本表达式（`1.0 / 0.0` 这种），静默暂停会让作者以为引擎卡死。
    pub fn set_time_scale(&mut self, scale: f32) {
        if !scale.is_finite() {
            return;
        }
        self.time_scale = scale.clamp(0.0, MAX_TIME_SCALE);
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

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 缩放的**唯一**作用点必须是 `tick`：`delta_time` / `total_time` /
    /// 固定步累计器三者同源，才不会出现"物理慢速、脚本却按实时走"。
    #[test]
    fn scale_is_applied_to_every_clock_channel() {
        let mut clock = GameClock::new().with_fixed_dt(1.0 / 60.0);
        clock.set_time_scale(0.5);

        clock.tick(1.0 / 30.0);

        assert!((clock.delta_time() - 1.0 / 60.0).abs() < 1e-6);
        assert!((clock.total_time() - 1.0 / 60.0).abs() < 1e-6);
        assert!(
            (clock.accumulator() - 1.0 / 60.0).abs() < 1e-6,
            "累计器吃的是缩放后的 dt，否则固定步会按实时跑"
        );
        assert!(clock.consume_fixed_update(), "半速下仍然积满了一步");
    }

    /// `time_scale = 0` 时三路都不前进，但 `tick` 本身仍被调用 ——
    /// 暂停靠的就是"帧照来、时间不走"。
    ///
    /// 注意暂停那一刻累计器里可能还压着**不到一步**的零头：它不会被清零，
    /// 恢复后最多补一步（≤ 1 个 fixed_dt）。这是有意的——把零头抹掉等于
    /// 让每次暂停都吃掉一点时间，长期会让"暂停/恢复"改变物理轨迹。
    #[test]
    fn zero_scale_freezes_time_without_stopping_the_caller() {
        let mut clock = GameClock::new().with_fixed_dt(1.0 / 60.0);
        clock.tick(1.0 / 60.0);
        assert!(clock.consume_fixed_update(), "先把这个整步消费掉");
        let frozen_at = clock.total_time();

        clock.set_time_scale(0.0);
        for _ in 0..10 {
            clock.tick(1.0 / 60.0);
            assert!(!clock.consume_fixed_update(), "暂停期间不该有固定步");
        }

        assert_eq!(clock.delta_time(), 0.0);
        assert_eq!(clock.total_time(), frozen_at);
        assert!(
            clock.accumulator() < clock.fixed_dt(),
            "暂停不清零头，但零头必须小于一步"
        );
    }

    /// 负值是作者的笔误（Unity 直接当 0 处理），NaN / ∞ 是表达式算炸了 ——
    /// 前者钳到 0，后者**忽略**（保住上一档），都不能静默把游戏冻住。
    #[test]
    fn bad_scales_are_sanitized_instead_of_freezing_silently() {
        let mut clock = GameClock::new();

        clock.set_time_scale(-2.0);
        assert_eq!(clock.time_scale(), 0.0, "负值钳到 0");

        clock.set_time_scale(3.0);
        clock.set_time_scale(f32::NAN);
        assert_eq!(clock.time_scale(), 3.0, "NaN 忽略，保留上一档");
        clock.set_time_scale(f32::INFINITY);
        assert_eq!(clock.time_scale(), 3.0, "∞ 忽略，保留上一档");

        clock.set_time_scale(1e9);
        assert_eq!(clock.time_scale(), MAX_TIME_SCALE, "上界钳到 100");
    }

    /// 加速受"每帧最多 5 个固定步"的预算约束：预算是防死亡螺旋的，
    /// 不能被时间缩放绕过（否则 100 倍速一帧就能跑出 6000 步）。
    #[test]
    fn accelerated_time_still_respects_the_step_budget() {
        let mut clock = GameClock::new().with_fixed_dt(1.0 / 60.0);
        clock.set_time_scale(10.0);
        clock.tick(0.1); // 10 倍速 × 0.1s = 1s 游戏时间 ≈ 60 步

        let mut steps = 0;
        while clock.consume_fixed_update() {
            steps += 1;
        }
        assert_eq!(steps, 5, "每帧预算仍是 5 步");

        clock.reset_step_counter();
        let mut steps = 0;
        while clock.consume_fixed_update() {
            steps += 1;
        }
        assert_eq!(steps, 5, "重置后可以继续消费剩余的累计时间");
    }
}
