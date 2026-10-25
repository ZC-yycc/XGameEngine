//! 帧间隔探针（诊断用，环境变量开关）。
//!
//! 设了 `XGAME_FRAME_LOG=<文件路径>` 时，每出一帧就往那个文件追一行：
//!
//! ```text
//! t_us=<相对启动的微秒> dt_us=<距上一帧的微秒> live=<0|1>
//! ```
//!
//! # 为什么要有它
//!
//! "编辑器卡不卡"这件事，光看 CPU 占用是量不出来的：CPU 高可能是帧率稳但活多，
//! 也可能是帧很少但每帧都重。真正要看的是**帧间隔的分布** —— 是稳定的 16.7ms，
//! 还是一串 40ms / 80ms 的长帧，还是干脆一段没有帧的空档。
//!
//! 有了 `t_us` 也能反过来算"某段时间里到底出了几帧"，这正是判断
//! 「纯事件驱动 vs 连续出帧」哪一档更跟手所需要的证据。
//!
//! # 为什么不直接打到 stderr
//!
//! 实测脚本是 `DETACHED_PROCESS` 启动编辑器的，stdout / stderr 会被重定向，
//! 与 `env_logger` 的输出混在一起、顺序也不保证。独立文件最省事、最好解析。

use std::fs::File;
use std::io::Write;
use std::time::Instant;

/// 未开启时为 `None`，一路无操作。
pub struct FrameProbe {
    out: Option<File>,
    start: Instant,
    last: Option<Instant>,
    frames: u64,
}

impl FrameProbe {
    /// 读 `XGAME_FRAME_LOG`；没设或文件建不出来就退化成空实现（不影响功能）。
    pub fn from_env() -> Self {
        let out = std::env::var_os("XGAME_FRAME_LOG").and_then(|path| {
            File::create(path)
                .map_err(|e| log::warn!("帧探针无法创建日志文件: {e}"))
                .ok()
        });
        let mut this = Self {
            out,
            start: Instant::now(),
            last: None,
            frames: 0,
        };
        // 头行：把相对时钟锚到墙钟，分析端才能和外部注入时间戳（`*.inj` 用
        // `SystemTime` 记）对到同一条时间轴上。不以 `t_us=` 开头，旧解析器自动跳过。
        if let Some(out) = this.out.as_mut() {
            let wall0 = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            let _ = writeln!(out, "wall0={wall0:.6}");
        }
        this
    }

    pub fn enabled(&self) -> bool {
        self.out.is_some()
    }

    /// 每帧调用一次。
    ///
    /// - `t_in` / `t_out`：本帧应用闭包（输入处理 + UI 构建 + 渲染提交）的起止时刻，
    ///   `w_us` 是闭包内工作耗时。它与 `dt_us` 的差就是「egui 镶嵌 + 上屏 + 唤醒」
    ///   那一段 —— 拆开才能看出一帧到底花在哪边。
    /// - `pointer`：本帧结束时的指针位置。配合驱动脚本的注入时间戳（`*.inj`）可以
    ///   算出「输入注入 → 被应用看到」的真实延迟。
    /// - `raw_time`：本 pass 的 `RawInput::time` 是否存在。eframe 每次 `ctx.run`
    ///   都喂带时间的新输入；而 egui 的丢弃重跑（`request_discard`）第二遍拿的是
    ///   被 `take()` 掏空的输入（`time == None`）。据此能把「同一次 run 的第二遍」
    ///   和「新一帧」分开 —— 帧探针统计的 update() 次数 ≠ 上屏次数就差在这。
    /// - `will_discard`：本 pass 是否已请求丢弃（将再跑一遍）。
    /// - `pass_nr`：egui 累计 pass 序号，每遍 +1，用来核对分组没有漏。
    ///
    /// **不用 `BufWriter`**：实测脚本是 `taskkill /F` 收尾的，进程没机会跑 `Drop`，
    /// 留在用户态缓冲里的行会整段丢掉 —— 而"丢掉的恰好是最后那段"正是最要命的地方
    /// （它就是被测的阶段）。一次 `write` 系统调用换掉这个风险，对测量精度的影响
    /// （几十微秒）远小于丢数据的代价。
    /// 因为要记的维度多（计时/分组/指针/策略各司其职），参数数量超了默认限制。
    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        t_in: Instant,
        t_out: Instant,
        live: bool,
        pointer: Option<(f32, f32)>,
        raw_time: bool,
        will_discard: bool,
        pass_nr: u64,
    ) {
        let Some(out) = self.out.as_mut() else {
            return;
        };
        let t_us = t_out.saturating_duration_since(self.start).as_micros();
        let dt_us = self
            .last
            .map(|l| t_out.saturating_duration_since(l).as_micros())
            .unwrap_or(0);
        let w_us = t_out.saturating_duration_since(t_in).as_micros();
        self.last = Some(t_out);
        self.frames += 1;
        let ptr = pointer
            .map(|(x, y)| format!("px={x:.0} py={y:.0}"))
            .unwrap_or_default();
        // 写失败就算了：这是诊断输出，不该因为它把编辑器带崩。
        let _ = writeln!(
            out,
            "t_us={t_us} dt_us={dt_us} w_us={w_us} {ptr} rt={} d={} p={pass_nr} live={}",
            raw_time as u8, will_discard as u8, live as u8
        );
    }
}

impl Drop for FrameProbe {
    fn drop(&mut self) {
        if let Some(out) = self.out.as_mut() {
            let _ = out.flush();
        }
    }
}
