//! 仿真错误：恢复 / 计划 / 应用 / 场景 / trace 比对五类。

use xgc_base::Diagnostic;

#[derive(Debug)]
pub enum SimError {
    Restore(String),
    InvalidPlan {
        frame: u64,
        diagnostics: Vec<Diagnostic>,
    },
    Apply {
        frame: u64,
        diagnostics: Vec<Diagnostic>,
    },
    InvalidScenario(String),
    TraceMismatch {
        frame: u64,
        expected_hash: u64,
        actual_hash: u64,
    },
}

impl std::fmt::Display for SimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Restore(message) => write!(f, "初始世界恢复失败：{message}"),
            Self::InvalidPlan { frame, diagnostics } => {
                write!(f, "第 {frame} 帧计划无效：{diagnostics:?}")
            }
            Self::Apply { frame, diagnostics } => {
                write!(f, "第 {frame} 帧应用失败：{diagnostics:?}")
            }
            Self::InvalidScenario(message) => write!(f, "场景无效：{message}"),
            Self::TraceMismatch {
                frame,
                expected_hash,
                actual_hash,
            } => write!(
                f,
                "第 {frame} 帧 state hash 不一致：期望 {expected_hash:016x}，实际 {actual_hash:016x}"
            ),
        }
    }
}

impl std::error::Error for SimError {}
