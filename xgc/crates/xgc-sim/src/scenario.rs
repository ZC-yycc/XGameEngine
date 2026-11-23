//! 场景 / trace 数据模型：确定性重放的输入输出只有这几份 serde 结构。

use serde::{Deserialize, Serialize};
use xgc_world::{Command, WorldData};

pub const TRACE_FORMAT_VERSION: u32 = 1;

pub(crate) fn default_fixed_dt() -> f32 {
    1.0 / 60.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameCommands {
    pub frame: u64,
    pub commands: Vec<Command>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    pub initial: WorldData,
    pub frames: u64,
    #[serde(default = "default_fixed_dt")]
    pub fixed_dt: f32,
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
    pub(crate) fn new() -> Self {
        Self {
            format_version: TRACE_FORMAT_VERSION,
            frames: Vec::new(),
        }
    }
}
