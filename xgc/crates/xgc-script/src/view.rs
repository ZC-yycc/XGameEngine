//! 每帧只读快照：脚本在 `update(ctx)` 里看到的实体状态。
//!
//! 写操作一律进命令队列，不直接改 `World`；快照在每帧脚本调用前刷新一次
//! （`runtime::refresh_view`），所以脚本看到的是**帧首**状态。脚本同帧多次
//! 写同一字段时，层栈那两个 setter 会看命令队列里未结算的写入（见 `legacy`）。

use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct EntityView {
    pub id: u64,
    pub name: String,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub velocity: [f32; 3],
    pub anim_parameters: Option<String>,
    /// `animation.layers` 的 RON 层栈（脚本按层名改权重 / 参数）。
    pub anim_layers: Option<String>,
    pub clicked: bool,
    pub hovered: bool,
    pub pressed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ScriptView {
    pub delta: f32,
    pub time: f32,
    pub entities: Vec<EntityView>,
    pub actions: BTreeMap<String, bool>,
}

impl ScriptView {
    pub fn entity(&self, id: u64) -> Option<&EntityView> {
        self.entities.iter().find(|entity| entity.id == id)
    }

    pub fn by_name(&self, name: &str) -> Option<&EntityView> {
        self.entities.iter().find(|entity| entity.name == name)
    }
}
