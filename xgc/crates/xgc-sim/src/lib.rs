//! XGC 最小帧循环：按帧应用命令，逐帧记录 revision / state hash。
//!
//! 这一版还没有系统调度；它先把**确定性的时间边界**钉住：同一份 Scenario
//! （初始 WorldData + 每帧命令）跑两遍，trace 必须逐帧一致。后续物理 / 脚本 /
//! 动画系统接进 `play` 的帧边界，不改变这里的可回放契约。

#![deny(unsafe_code)]

use serde::{Deserialize, Serialize};
use xgc_base::Diagnostic;
use xgc_world::{Command, World, WorldData};

pub const TRACE_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameCommands {
    pub frame: u64,
    pub commands: Vec<Command>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    pub initial: WorldData,
    pub frames: u64,
    pub commands: Vec<FrameCommands>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameRecord {
    pub frame: u64,
    pub revision: u64,
    pub state_hash: u64,
    pub entities: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    pub format_version: u32,
    pub frames: Vec<FrameRecord>,
}

impl Trace {
    fn new() -> Self {
        Self {
            format_version: TRACE_FORMAT_VERSION,
            frames: Vec::new(),
        }
    }
}

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

/// 跑完整场景；命令在对应帧开始、记录该帧 hash 之前应用。
pub fn play(scenario: &Scenario) -> Result<(World, Trace), SimError> {
    for group in &scenario.commands {
        if group.frame >= scenario.frames {
            return Err(SimError::InvalidScenario(format!(
                "命令帧 {} 超出总帧数 {}",
                group.frame, scenario.frames
            )));
        }
    }

    let mut world = World::new();
    world
        .restore_world_data(&scenario.initial)
        .map_err(SimError::Restore)?;

    let mut trace = Trace::new();
    for frame in 0..scenario.frames {
        for group in scenario
            .commands
            .iter()
            .filter(|group| group.frame == frame)
        {
            let plan = world.plan(group.commands.clone());
            if !plan.is_valid() {
                return Err(SimError::InvalidPlan {
                    frame,
                    diagnostics: plan.diagnostics,
                });
            }
            world
                .apply(&plan)
                .map_err(|diagnostics| SimError::Apply { frame, diagnostics })?;
        }

        trace.frames.push(FrameRecord {
            frame,
            revision: world.revision(),
            state_hash: world.state_hash(),
            entities: world.len(),
        });
    }
    Ok((world, trace))
}

/// 重跑场景并逐帧比对 trace；第一条不一致立即返回。
pub fn verify(scenario: &Scenario, expected: &Trace) -> Result<(), SimError> {
    let (_, actual) = play(scenario)?;
    if actual.frames.len() != expected.frames.len() {
        return Err(SimError::InvalidScenario(format!(
            "trace 帧数 {} 与场景 {} 不一致",
            expected.frames.len(),
            actual.frames.len()
        )));
    }
    for (expected, actual) in expected.frames.iter().zip(actual.frames.iter()) {
        if expected.frame != actual.frame || expected.state_hash != actual.state_hash {
            return Err(SimError::TraceMismatch {
                frame: actual.frame,
                expected_hash: expected.state_hash,
                actual_hash: actual.state_hash,
            });
        }
    }
    Ok(())
}
