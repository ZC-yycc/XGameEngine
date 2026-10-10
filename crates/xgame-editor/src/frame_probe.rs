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
        Self {
            out,
            start: Instant::now(),
            last: None,
            frames: 0,
        }
    }

    pub fn enabled(&self) -> bool {
        self.out.is_some()
    }

    /// 每帧调用一次。
    ///
    /// **不用 `BufWriter`**：实测脚本是 `taskkill /F` 收尾的，进程没机会跑 `Drop`，
    /// 留在用户态缓冲里的行会整段丢掉 —— 而"丢掉的恰好是最后那段"正是最要命的地方
    /// （它就是被测的阶段）。一次 `write` 系统调用换掉这个风险，对测量精度的影响
    /// （几十微秒）远小于丢数据的代价。
    pub fn tick(&mut self, now: Instant, live: bool) {
        let Some(out) = self.out.as_mut() else {
            return;
        };
        let t_us = now.saturating_duration_since(self.start).as_micros();
        let dt_us = self
            .last
            .map(|l| now.saturating_duration_since(l).as_micros())
            .unwrap_or(0);
        self.last = Some(now);
        self.frames += 1;
        // 写失败就算了：这是诊断输出，不该因为它把编辑器带崩。
        let _ = writeln!(out, "t_us={t_us} dt_us={dt_us} live={}", live as u8);
    }
}

impl Drop for FrameProbe {
    fn drop(&mut self) {
        if let Some(out) = self.out.as_mut() {
            let _ = out.flush();
        }
    }
}
