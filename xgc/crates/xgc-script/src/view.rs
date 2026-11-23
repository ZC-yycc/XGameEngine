//! 每帧只读快照：脚本在 `update(ctx)` 里看到的实体状态。
//!
//! 写操作一律进命令队列，不直接改 `World`；快照在每帧脚本调用前刷新一次
//! （`runtime::refresh_view`），所以脚本看到的是**帧首**状态。脚本同帧多次
//! 写同一字段时，层栈那两个 setter 会看命令队列里未结算的写入（见 `legacy`）。

use std::collections::BTreeMap;

/// `ui.element`（comp `0x0601`）的只读快照：布局 setter 需要「当前尺寸 / 位置」
/// 做增量修改，getter 需要文本 / 可见 / 可交互。
#[derive(Clone, Debug, Default)]
pub struct UiElementView {
    /// `panel` / `text` / `image` / `button`。焦点 setter 只认 `button`（旧端同）。
    pub kind: String,
    pub anchor_min: [f32; 2],
    pub anchor_max: [f32; 2],
    pub offset_min: [f32; 2],
    pub offset_max: [f32; 2],
    pub text: String,
    pub visible: bool,
    pub interactable: bool,
}

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
    /// `ui.element` 快照（非 UI 实体为 `None`）。
    pub ui: Option<UiElementView>,
    pub clicked: bool,
    pub hovered: bool,
    pub pressed: bool,
    /// 只有带 `ui.interaction` 的实体才有意义；焦点 setter / getter 读它。
    pub focused: bool,
    /// 实体是否带 `ui.interaction`（焦点命令只能写给有该组件的实体）。
    pub has_interaction: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ScriptView {
    pub delta: f32,
    pub time: f32,
    pub entities: Vec<EntityView>,
    pub actions: BTreeMap<String, bool>,
    /// 下一帧可用的实体 ID（每帧从 `WorldData::next_id` 重置）。
    ///
    /// 旧端 `ui_spawn` 用 `EntityId::new()` 单调分配；这里用世界计数器派生，
    /// 保证同一帧多次 spawn 与已有的待结算实体不撞号。
    pub spawn_cursor: u64,
    /// 本帧 `ui_spawn_*` 生成、命令还没结算的实体。
    ///
    /// 同帧要能对新 id 接着调 `ui_has` / setter / `ui_set_focus`（旧端 spawn 后
    /// 直接改运行时元素，天然可见）。命令在帧末才 apply，所以这里补一份快照，
    /// 结算后下一帧由 `refresh_view` 从世界重建、清空。
    pub pending_spawns: BTreeMap<u64, EntityView>,
}

impl ScriptView {
    pub fn entity(&self, id: u64) -> Option<&EntityView> {
        self.entities
            .iter()
            .find(|entity| entity.id == id)
            .or_else(|| self.pending_spawns.get(&id))
    }

    pub fn by_name(&self, name: &str) -> Option<&EntityView> {
        self.entities
            .iter()
            .find(|entity| entity.name == name)
            .or_else(|| {
                self.pending_spawns
                    .values()
                    .find(|entity| entity.name == name)
            })
    }
}
