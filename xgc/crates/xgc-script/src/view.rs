//! 每帧只读快照：脚本在 `update(ctx)` 里看到的实体状态。
//!
//! 写操作一律进命令队列，不直接改 `World`；视图在每帧脚本调用前刷新一次
//! （`runtime::refresh_view`）。动画参数和层栈保留本帧待结算的最新值，供按名
//! 的 Lua setter 和动态路径写入按调用顺序累积。

use std::collections::BTreeMap;

use xgc_base::{EntityId, EntityKey};
use xgc_world::{
    AnimationLayers, AnimationState, BodyKind, CharacterController, Component, ComponentId,
    FieldPath, Name, PhysicsBody, Transform, UiElement, UiInteraction, Value, Velocity, World,
};

use crate::audio_api::{AudioCheckpoint, ScriptAudio};
use crate::legacy::View;

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
    pub body_kind: Option<BodyKind>,
    /// 角色控制器是否贴地（没有 `physics.character` 的实体恒为 false）。
    pub grounded: bool,
    pub anim_parameters: Option<Value>,
    /// `animation.layers` 的结构化帧内视图。
    pub anim_layers: Option<Value>,
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

impl EntityView {
    pub(crate) fn refresh(&mut self, world: &World, id: EntityId, key: EntityKey) {
        self.id = id.raw();
        let transform = world.get::<Transform>(key).copied().unwrap_or_default();
        self.position = transform.position;
        self.rotation = transform.rotation;

        let velocity = world.get::<Velocity>(key);
        self.has_velocity = velocity.is_some();
        self.velocity = velocity.map_or([0.0; 3], |velocity| velocity.linear);
        self.body_kind = world.get::<PhysicsBody>(key).map(PhysicsBody::kind);
        self.grounded = world
            .get::<CharacterController>(key)
            .is_some_and(|character| character.grounded);
        if let Some(name) = world.get::<Name>(key) {
            self.name.clone_from(&name.value);
        } else {
            self.name.clear();
        }

        // 动态字段路径和按名 setter 共享待结算的 Value，只投影它们实际会改的字段。
        self.anim_layers = world
            .get::<AnimationLayers>(key)
            .and_then(|layers| layers.get_field("layers"));
        self.anim_parameters = world
            .get::<AnimationState>(key)
            .and_then(|state| state.get_field("parameters"));

        if let Some(element) = world.get::<UiElement>(key) {
            let ui = self.ui.get_or_insert_with(UiElementView::default);
            ui.kind.clone_from(&element.kind);
            ui.text.clone_from(&element.text);
            ui.anchor_min = element.anchor_min;
            ui.anchor_max = element.anchor_max;
            ui.offset_min = element.offset_min;
            ui.offset_max = element.offset_max;
            ui.visible = element.visible;
            ui.interactable = element.interactable;
        } else {
            self.ui = None;
        }
        let interaction = world.get::<UiInteraction>(key);
        self.has_interaction = interaction.is_some();
        let interaction = interaction.copied().unwrap_or_default();
        self.clicked = interaction.clicked;
        self.hovered = interaction.hovered;
        self.pressed = interaction.pressed;
        self.focused = interaction.focused;
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ActionState {
    pub down: bool,
    pub pressed: bool,
    pub released: bool,
}

#[derive(Clone, Debug)]
struct MountCheckpoint {
    spawn_cursor: u64,
    audio: AudioCheckpoint,
    body_velocity_count: usize,
    entities: Vec<EntityView>,
}

#[derive(Clone, Debug, Default)]
pub struct ScriptView {
    pub delta: f32,
    pub time: f32,
    pub entities: Vec<EntityView>,
    pub actions: BTreeMap<String, ActionState>,
    /// 模拟量轴：`axis(ctx, "MoveX")` 读这里。与动作一样由宿主每帧写入。
    pub axes: BTreeMap<String, f32>,
    /// 同一播放会话内所有脚本共用音量与命令队列。
    pub audio: ScriptAudio,
    /// 刚体指令跟随本帧脚本批次提交，初始化预检会撤销候选写入。
    pub(crate) body_velocities: Vec<(EntityId, [f32; 3])>,
    /// 下一帧可用的实体 ID（每帧从世界的 ID 分配游标重置）。
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
    mount_checkpoint: Option<MountCheckpoint>,
}

impl ScriptView {
    /// 顶层初始化可能写帧内视图，只记录它碰过的行，避免每次初装都复制整张表。
    pub(crate) fn initialize<T>(
        view: &View,
        keep_effects: bool,
        load: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        {
            let mut view = view.borrow_mut();
            debug_assert!(view.mount_checkpoint.is_none());
            view.mount_checkpoint = Some(MountCheckpoint {
                spawn_cursor: view.spawn_cursor,
                audio: view.audio.checkpoint(),
                body_velocity_count: view.body_velocities.len(),
                entities: Vec::new(),
            });
        }
        let result = load();
        let mut view = view.borrow_mut();
        let checkpoint = view.mount_checkpoint.take().expect("初始化已建立检查点");
        if !keep_effects || result.is_err() {
            for saved in checkpoint.entities {
                if let Some(row) = view.entity_mut(saved.id) {
                    *row = saved;
                }
            }
            view.spawn_cursor = checkpoint.spawn_cursor;
            view.pending_spawns
                .retain(|id, _| *id < checkpoint.spawn_cursor);
            view.audio.restore(checkpoint.audio);
            view.body_velocities
                .truncate(checkpoint.body_velocity_count);
        }
        result
    }

    pub(crate) fn action_mut(&mut self, name: &str) -> &mut ActionState {
        if !self.actions.contains_key(name) {
            self.actions.insert(name.to_owned(), ActionState::default());
        }
        self.actions.get_mut(name).expect("动作状态已插入")
    }

    pub fn entity(&self, id: u64) -> Option<&EntityView> {
        self.entities
            .iter()
            .find(|entity| entity.id == id)
            .or_else(|| self.pending_spawns.get(&id))
    }

    pub(crate) fn entity_mut(&mut self, id: u64) -> Option<&mut EntityView> {
        let row = self
            .entities
            .iter_mut()
            .find(|entity| entity.id == id)
            .or_else(|| self.pending_spawns.get_mut(&id))?;
        if let Some(checkpoint) = &mut self.mount_checkpoint {
            if !checkpoint.entities.iter().any(|saved| saved.id == id) {
                checkpoint.entities.push(row.clone());
            }
        }
        Some(row)
    }

    pub(crate) fn stage_animation_field(
        &mut self,
        id: u64,
        component: ComponentId,
        field: &str,
        value: &Value,
    ) {
        // 视图跟随调用顺序，实际校验与提交由世界事务负责。
        if component != AnimationState::ID && component != AnimationLayers::ID {
            return;
        }
        let Ok(path) = FieldPath::parse(field) else {
            return;
        };
        let Some(row) = self.entity_mut(id) else {
            return;
        };
        let staged = match (component, path.root()) {
            (AnimationState::ID, "parameters") => row.anim_parameters.as_mut(),
            (AnimationLayers::ID, "layers") => row.anim_layers.as_mut(),
            _ => None,
        };
        if let Some(staged) = staged {
            let _ = path.replace(staged, value.clone());
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
