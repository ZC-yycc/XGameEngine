//! 帧分析（Profiler）的数据面：定长环形缓冲 + 窗口统计。
//!
//! # 为什么不用现成的 `frame_probe`
//!
//! [`crate::frame_probe`] 是**离线**诊断口：设了 `XGAME_FRAME_LOG` 才有数据、落到
//! 文件里、事后用脚本分析。Profiler 面板要的是**在线**读数 —— 每秒都在跑、不落盘、
//! 不分配，界面随时能画出最近的窗口。
//!
//! 两者刻意不合并：探针那份字段是为"输入延迟 / egui 丢弃重跑"设计的
//! （`rt` / `d` / `p` / 指针坐标），面板要的是"帧间隔 + 主循环各段耗时"的滚动窗口。
//! 合并的话探针得多背一条永不落盘的路径，面板还得反过来解析探针的文本格式。
//!
//! # 为什么统计单独成模块、单独被测
//!
//! 这一层是纯数据结构（不碰 egui、不碰 GPU），统计口径能被单测钉死，面板只剩
//! "把数字画出来"。口径错了在界面上几乎看不出来（图里永远有柱子、FPS 永远是
//! 一个正数），所以数字必须在这里被测住。
//!
//! # 两个容易踩的口径
//!
//! - **空闲帧不能混进 FPS**：帧调度（`frame_policy`）在静止界面上会停帧，停帧期间
//!   根本没有 `update()` 调用，所以"停帧后重新出帧的那一帧"的 `dt` 是整段空闲时长
//!   （几秒都可能）。把它算进平均帧时间，画面停止打字几秒再动一下，FPS 就会塌到个位数
//!   —— 那是测量假象，不是卡顿。所以 FPS 只统计 `live` 帧，空闲帧单独计数。
//! - **长帧要在 `p99` 里露出来**：平均值会被大量好帧稀释，用户感受到的是"最差的那
//!   几帧"。`p99_dt_us` 就是"最差的 1% 帧有多慢"，与平均值一起看才判断得出是
//!   "整体慢"还是"偶发尖刺"。

/// 滚动窗口长度（帧）。
///
/// 60 fps 下正好 10 秒：够看清"偶发长帧"的节奏，又不至于把一次 5 秒的卡顿
/// 稀释成一根细线。定长是硬约束 —— 稳态每帧只写一个槽位，不扩容、不分配。
pub const WINDOW_FRAMES: usize = 600;

/// 一帧的采样。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameSample {
    /// 与上一帧的间隔（微秒）。
    ///
    /// 取自 `ctx.input().stable_dt`，与状态栏 FPS 同源 —— 两处显示不同步的话，
    /// 用户会以为某一边在骗人。
    pub dt_us: u32,
    /// 本帧主循环工作耗时（微秒）：`update()` 从进入到返回，含界面构建与视口渲染提交。
    pub work_us: u32,
    /// 其中：运行时推进那一段（物理 / 脚本 / 音频，只在 Play 态非零）。
    pub runtime_us: u32,
    /// 其中：停靠区（各面板 + 视口渲染提交）那一段。
    pub ui_us: u32,
    /// 本帧之后是否继续出帧（帧调度的 `FramePlan::Now`）。
    ///
    /// 空闲停帧前的最后一帧是 `false`，它的 `dt` 是"上一次活跃到现在"的间隔，
    /// 统计时必须与连续帧分开（见模块文档）。
    pub live: bool,
}

impl FrameSample {
    /// 从微秒数构造并夹取到 `u32` 可表示范围。
    ///
    /// 抽象时钟可能在调试暂停 / 系统休眠后跳几百秒，`as u32` 会回绕成一个
    /// 看着很正常的小数字 —— 那种假读数比溢出更难查。
    pub fn from_micros(
        dt_us: u128,
        work_us: u128,
        runtime_us: u128,
        ui_us: u128,
        live: bool,
    ) -> Self {
        Self {
            dt_us: clamp_us(dt_us),
            work_us: clamp_us(work_us),
            runtime_us: clamp_us(runtime_us),
            ui_us: clamp_us(ui_us),
            live,
        }
    }
}

fn clamp_us(v: u128) -> u32 {
    v.min(u32::MAX as u128) as u32
}

/// 窗口内样本的统计结果（面板只认这个结构，不自己算）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Summary {
    /// 窗口内的样本数。
    pub frames: usize,
    /// 其中继续出帧的帧数。
    pub live_frames: usize,
    /// 其中"出完就停"的帧数。
    pub idle_frames: usize,
    /// 由 **live 帧** 的平均间隔折算的帧率；没有 live 帧时为 0。
    pub fps: f32,
    /// 全部样本的平均帧间隔（含空闲帧 —— 它描述的是"实际节拍"，不是"跑得多快"）。
    pub avg_dt_us: f32,
    pub min_dt_us: u32,
    pub max_dt_us: u32,
    /// 中位帧间隔：图的纵轴按它定标（一次长帧不该把其余样本压成一条线）。
    pub p50_dt_us: u32,
    /// 99 分位帧间隔：最差的 1% 帧有多慢。
    pub p99_dt_us: u32,
    pub avg_work_us: f32,
    pub max_work_us: u32,
    pub avg_runtime_us: f32,
    pub avg_ui_us: f32,
}

impl Summary {
    /// 窗口覆盖的时长（秒）。空闲帧的间隔也算在内 —— 它是窗口真实跨度。
    pub fn window_seconds(&self) -> f32 {
        self.avg_dt_us * self.frames as f32 / 1_000_000.0
    }
}

/// 帧采样环形缓冲 + 暂停 / 清空。
///
/// 只有 `record` 会写数据，且写的是预分配槽位（稳态零分配）。
#[derive(Debug)]
pub struct Profiler {
    samples: Vec<FrameSample>,
    /// 下一颗样本的写入位置（环形下标）。
    head: usize,
    /// 当前有效样本数（≤ 容量）。
    len: usize,
    /// 暂停时 `record` 直接丢样本 —— "冻结当前画面"是排查问题的基本操作，
    /// 边看边滚的图没法读具体某一帧。
    pub paused: bool,
    /// 自上次 `clear` 起的累计帧数（不受窗口长度限制）。
    recorded: u64,
}

impl Default for Profiler {
    fn default() -> Self {
        Self::new()
    }
}

impl Profiler {
    pub fn new() -> Self {
        Self {
            samples: vec![FrameSample::default(); WINDOW_FRAMES],
            head: 0,
            len: 0,
            paused: false,
            recorded: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.samples.len()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 自上次清空起累计记录了多少帧（窗口外的不算在 `len` 里，但这里算）。
    pub fn recorded(&self) -> u64 {
        self.recorded
    }

    /// 记录一帧。暂停时丢弃（不推进下标，也不计数）。
    pub fn record(&mut self, sample: FrameSample) {
        if self.paused {
            return;
        }
        self.samples[self.head] = sample;
        self.head = (self.head + 1) % self.samples.len();
        if self.len < self.samples.len() {
            self.len += 1;
        }
        self.recorded += 1;
    }

    /// 清空窗口与计数（暂停状态保留 —— 清空是"重新开始采"，不是"继续采"）。
    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
        self.recorded = 0;
    }

    /// 按时间顺序遍历窗口内的样本（最旧 → 最新）。
    pub fn iter(&self) -> impl Iterator<Item = &FrameSample> + '_ {
        let cap = self.samples.len();
        // 线性容器 `len < cap` 时 `head == len`，起点正好是 0；
        // 写满之后起点才是"最旧的那一格"。
        let start = if cap == 0 {
            0
        } else {
            (self.head + cap - self.len) % cap
        };
        (0..self.len).map(move |i| &self.samples[(start + i) % cap])
    }

    /// 窗口统计。空窗口返回全零（面板据此画"还没有数据"，而不是 NaN 柱子）。
    pub fn summary(&self) -> Summary {
        let n = self.len;
        if n == 0 {
            return Summary::default();
        }

        // 排序只为 99 分位：栈上定长数组，不进堆 —— 面板每帧都会调这个函数。
        let mut sorted = [0u32; WINDOW_FRAMES];
        let mut dt_sum = 0u64;
        let mut live_dt_sum = 0u64;
        let mut live = 0usize;
        let mut work_sum = 0u64;
        let mut max_work = 0u32;
        let mut physics_sum = 0u64;
        let mut ui_sum = 0u64;

        for (i, s) in self.iter().enumerate() {
            sorted[i] = s.dt_us;
            dt_sum += s.dt_us as u64;
            if s.live {
                live_dt_sum += s.dt_us as u64;
                live += 1;
            }
            work_sum += s.work_us as u64;
            max_work = max_work.max(s.work_us);
            physics_sum += s.runtime_us as u64;
            ui_sum += s.ui_us as u64;
        }
        sorted[..n].sort_unstable();

        let avg_dt = dt_sum as f32 / n as f32;
        let live_avg = if live > 0 {
            live_dt_sum as f32 / live as f32
        } else {
            0.0
        };
        let p99_index = (((n - 1) as f32) * 0.99).round() as usize;
        let p50_index = n / 2;

        Summary {
            frames: n,
            live_frames: live,
            idle_frames: n - live,
            fps: if live_avg > 0.0 {
                1_000_000.0 / live_avg
            } else {
                0.0
            },
            avg_dt_us: avg_dt,
            min_dt_us: sorted[0],
            max_dt_us: sorted[n - 1],
            p50_dt_us: sorted[p50_index.min(n - 1)],
            p99_dt_us: sorted[p99_index.min(n - 1)],
            avg_work_us: work_sum as f32 / n as f32,
            max_work_us: max_work,
            avg_runtime_us: physics_sum as f32 / n as f32,
            avg_ui_us: ui_sum as f32 / n as f32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(dt_us: u32, live: bool) -> FrameSample {
        FrameSample {
            dt_us,
            work_us: dt_us / 2,
            runtime_us: dt_us / 8,
            ui_us: dt_us / 4,
            live,
        }
    }

    /// 写满之后从最旧的一格开始遍历，写超容量时丢掉最旧的。
    #[test]
    fn ring_wraps_and_keeps_chronological_order() {
        let mut p = Profiler::new();
        assert_eq!(p.capacity(), WINDOW_FRAMES);
        for i in 0..(WINDOW_FRAMES + 100) {
            p.record(sample(i as u32, true));
        }
        assert_eq!(p.len(), WINDOW_FRAMES, "窗口长度固定，不扩容");
        assert_eq!(
            p.recorded(),
            (WINDOW_FRAMES + 100) as u64,
            "累计帧数不受窗口限制"
        );
        // 最旧的 100 颗被挤掉，第一颗应当是 dt=100。
        assert_eq!(p.iter().next().map(|s| s.dt_us), Some(100));
        // 最新的一颗在末尾。
        assert_eq!(
            p.iter().last().map(|s| s.dt_us),
            Some((WINDOW_FRAMES + 99) as u32)
        );
        // 时间顺序严格递增 —— 环形下标的环绕最容易在这里写错。
        let dts: Vec<u32> = p.iter().map(|s| s.dt_us).collect();
        assert!(
            dts.windows(2).all(|w| w[1] == w[0] + 1),
            "遍历顺序必须是最旧 → 最新"
        );
    }

    /// 暂停 = 停止采新样本，但窗口原样保留；恢复后接着写。
    #[test]
    fn pause_drops_samples_without_touching_window() {
        let mut p = Profiler::new();
        for i in 0..3 {
            p.record(sample(1000 + i, true));
        }
        p.paused = true;
        for _ in 0..10 {
            p.record(sample(5000, true));
        }
        assert_eq!(p.len(), 3, "暂停期间不采样本");
        assert_eq!(p.recorded(), 3, "暂停期间不计数");
        assert_eq!(p.iter().last().map(|s| s.dt_us), Some(1002));

        p.paused = false;
        p.record(sample(4000, true));
        assert_eq!(p.len(), 4);
        assert_eq!(p.iter().last().map(|s| s.dt_us), Some(4000));
    }

    /// 统计口径：FPS 只认 live 帧，min / max / p99 覆盖全部样本。
    #[test]
    fn summary_separates_idle_frames_from_fps() {
        let mut p = Profiler::new();
        p.record(sample(1000, true));
        p.record(sample(2000, true));
        p.record(sample(3000, true));
        // 停帧前那一帧：dt 是整段空闲时长（这里是 50ms）。
        p.record(sample(50_000, false));

        let s = p.summary();
        assert_eq!(s.frames, 4);
        assert_eq!(s.live_frames, 3);
        assert_eq!(s.idle_frames, 1);
        // live 帧均值 2000µs → 500 fps；空闲帧绝不参与。
        assert!((s.fps - 500.0).abs() < 0.01, "fps = {}, 期望 500", s.fps);
        assert_eq!(s.min_dt_us, 1000);
        assert_eq!(s.max_dt_us, 50_000);
        assert_eq!(s.p99_dt_us, 50_000, "n=4 时 p99 落在最大值上");
        assert_eq!(s.p50_dt_us, 3000, "中位数用来定纵轴，取排序后的第 n/2 个");
        assert!((s.avg_dt_us - 14_000.0).abs() < 0.01);
        assert!((s.avg_work_us - 7000.0).abs() < 0.01);
        assert_eq!(s.max_work_us, 25_000);
        assert!((s.avg_runtime_us - 1750.0).abs() < 0.01);
        assert!((s.avg_ui_us - 3500.0).abs() < 0.01);
    }

    /// 全是空闲帧时 FPS 报 0（而不是 inf / NaN）—— 面板据此显示"idle"。
    #[test]
    fn summary_of_idle_only_frames_reports_zero_fps() {
        let mut p = Profiler::new();
        p.record(sample(100_000, false));
        let s = p.summary();
        assert_eq!(s.fps, 0.0);
        assert_eq!(s.live_frames, 0);
        assert_eq!(s.idle_frames, 1);
        assert!(s.fps.is_finite());
    }

    /// 空窗口：全零、可安全参与除法和画图。
    #[test]
    fn empty_summary_is_all_zero() {
        let p = Profiler::new();
        let s = p.summary();
        assert_eq!(s.frames, 0);
        assert_eq!(s.fps, 0.0);
        assert_eq!(s.max_dt_us, 0);
        assert_eq!(s.window_seconds(), 0.0);
        assert!(p.iter().next().is_none());
    }

    /// 单颗样本时 p99 下标不能越界（`(n-1)*0.99` 在 n=1 时是 0，容易写错成 n-1+1）。
    #[test]
    fn single_sample_percentile_is_in_range() {
        let mut p = Profiler::new();
        p.record(sample(16_000, true));
        let s = p.summary();
        assert_eq!(s.p99_dt_us, 16_000);
        assert_eq!(s.p50_dt_us, 16_000);
        assert_eq!(s.min_dt_us, s.max_dt_us);
    }

    /// 清空回到初始状态，暂停标记保留。
    #[test]
    fn clear_resets_window_and_counter() {
        let mut p = Profiler::new();
        for _ in 0..10 {
            p.record(sample(16_000, true));
        }
        p.paused = true;
        p.clear();
        assert_eq!(p.len(), 0);
        assert_eq!(p.recorded(), 0);
        assert!(p.paused, "清空不改变暂停状态");
        // 清空后还能继续用（head 归零）。
        p.paused = false;
        p.record(sample(8000, true));
        assert_eq!(p.iter().next().map(|s| s.dt_us), Some(8000));
    }

    /// 微秒夹取：异常大的时长不会回绕成一个"看着正常"的小数字。
    #[test]
    fn from_micros_clamps_instead_of_wrapping() {
        let s = FrameSample::from_micros(u128::MAX, 0, 0, 0, true);
        assert_eq!(s.dt_us, u32::MAX);
        let s = FrameSample::from_micros(1234, 500, 100, 200, false);
        assert_eq!(
            (s.dt_us, s.work_us, s.runtime_us, s.ui_us),
            (1234, 500, 100, 200)
        );
        assert!(!s.live);
    }
}
