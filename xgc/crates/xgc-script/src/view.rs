//! 每帧只读快照：脚本在 `update(ctx)` 里看到的实体状态。
//!
//! 写操作一律进命令队列，不直接改 `World`；视图在每帧脚本调用前刷新一次
//! （`runtime::refresh_view`）。基础动画参数保留本帧待结算的最新值，层栈 setter
//! 则从命令队列读取待结算的值。

use std::collections::BTreeMap;

use xgc_world::{FieldPath, Value};

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
    /// 是否有 `core.velocity`；角色脚本第一次移动时可能要先补这个组件。
    pub has_velocity: bool,
    /// 角色控制器是否贴地（没有 `physics.character` 的实体恒为 false）。
    pub grounded: bool,
    pub anim_parameters: Option<Value>,
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
    /// 模拟量轴：`axis(ctx, "MoveX")` 读这里。与动作一样由宿主每帧写入。
    pub axes: BTreeMap<String, f32>,
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

    pub(crate) fn entity_mut(&mut self, id: u64) -> Option<&mut EntityView> {
        self.entities
            .iter_mut()
            .find(|entity| entity.id == id)
            .or_else(|| self.pending_spawns.get_mut(&id))
    }

    pub(crate) fn stage_animation_parameter(&mut self, id: u64, field: &str, value: &Value) {
        // 动态路径和按名 setter 共用本帧最新参数。实际校验及提交仍由世界事务负责。
        let Ok(path) = FieldPath::parse(field) else {
            return;
        };
        if path.root() != "parameters" || path.is_root() {
            return;
        }
        if let Some(parameters) = self
            .entity_mut(id)
            .and_then(|row| row.anim_parameters.as_mut())
        {
            let _ = path.replace(parameters, value.clone());
        }
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
