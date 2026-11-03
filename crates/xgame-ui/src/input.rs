//! 指针交互：悬停 / 按下 / 点击，写回 `UiElement::state`。
//!
//! # 状态机（Unity 的 Button 语义）
//!
//! ```text
//!            按下在元素上                 在元素上松开
//!   Idle ───────────────► Pressed ─────────────────► Click（一帧）
//!     ▲                      │
//!     └────── 在别处松开 ─────┘（取消，不触发 Click）
//! ```
//!
//! 关键点：**按下之后就"捕获"了指针**。把鼠标拖出按钮再拖回来松开仍然算点击，
//! 拖到按钮外的空白处松开才算取消 —— 这与桌面 UI 的惯例一致（Windows / macOS
//! 的按钮都是这个行为），也是用户手指的直觉。
//!
//! # 为什么写回组件而不是返回事件表
//!
//! 脚本（`ui_clicked(ctx, id)`）与组件（`element.state.clicked`）都要读它。
//! 写回 `Scene` 之后这两个消费者都只需要 `&Scene`，不必给 `GameCtx` 加字段、
//! 也不必维护一张"实体 → 事件"的旁路表（那张表还要处理实体被删的情况）。
//! 事件表仍然返回，但只用于测试与调试输出。

use xgame_core::{EntityId, Scene, UiState};
use xgame_math::glam::Vec2;

use crate::layout::{hit_test, LayoutItem};

/// 本帧指针状态（由宿主从窗口 / egui 映射到**视口物理像素**）。
///
/// **位置与按键是两件事**：`pos = None` 只表示"指针不在视口里"，按键电平与
/// 边沿仍然有效。早先把两者合成一个 `Option`，于是"在视口外松手"这一帧
/// 整个被丢掉 —— 捕获永远留在按钮上，按钮卡在按下态直到下一次在别处点击。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiPointer {
    /// 指针位置（视口左上角原点，物理像素）。`None` = 指针不在视口内。
    pub pos: Option<Vec2>,
    /// 主键是否按住（电平）。
    pub down: bool,
    /// 本帧是否刚按下（边沿）。
    pub pressed: bool,
    /// 本帧是否刚松开（边沿）。
    pub released: bool,
}

/// 交互事件（调试 / 测试用；玩法侧读 `UiElement::state`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiEvent {
    /// 事件主体。
    pub entity: EntityId,
    /// 事件类型。
    pub kind: UiEventKind,
}

/// 事件类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiEventKind {
    /// 指针进入元素。
    HoverEnter,
    /// 指针离开元素。
    HoverExit,
    /// 在元素上按下。
    Press,
    /// 松开（无论是否在元素上）。
    Release,
    /// 完整的"按下 + 在元素上松开"。
    Click,
}

/// 跨帧保持的交互状态（按下捕获 + 上一次悬停）。
#[derive(Clone, Debug, Default)]
pub struct UiInteraction {
    hovered: Option<EntityId>,
    pressed: Option<EntityId>,
}

impl UiInteraction {
    /// 当前悬停的实体。
    pub fn hovered(&self) -> Option<EntityId> {
        self.hovered
    }

    /// 当前被按下捕获的实体。
    pub fn pressed(&self) -> Option<EntityId> {
        self.pressed
    }

    /// 清空（退出 Play / 切换场景时调用）：不留跨场景的悬停残影。
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn press(&mut self, hit: Option<EntityId>, events: &mut Vec<UiEvent>) {
        self.pressed = hit;
        if let Some(entity) = hit {
            events.push(UiEvent {
                entity,
                kind: UiEventKind::Press,
            });
        }
    }

    fn release(&mut self, scene: &mut Scene, hit: Option<EntityId>, events: &mut Vec<UiEvent>) {
        let Some(captured) = self.pressed.take() else {
            return;
        };
        if hit == Some(captured) {
            if let Some(ui) = scene
                .find_entity(captured)
                .and_then(|i| scene.entities[i].ui.as_mut())
            {
                ui.state.clicked = true;
            }
            events.push(UiEvent {
                entity: captured,
                kind: UiEventKind::Click,
            });
        }
        events.push(UiEvent {
            entity: captured,
            kind: UiEventKind::Release,
        });
    }
}

/// 推进一帧交互：命中测试 → 状态机 → 写回 `UiElement::state`。
///
/// `items` 必须是本帧 [`crate::layout::visit`] 的输出（绘制顺序）。
/// `pointer` 为 `None` 表示指针不在视口内（编辑器里鼠标在别的面板上）。
pub fn update_interaction(
    scene: &mut Scene,
    items: &[LayoutItem],
    pointer: Option<UiPointer>,
    interaction: &mut UiInteraction,
    events: &mut Vec<UiEvent>,
) {
    events.clear();

    // -- 0. 清掉上一帧的标志位 --
    //
    // 全量扫描带 UI 的实体：不可见 / 被父节点隐藏的元素不在 `items` 里，
    // 它们的旧标志必须一并清掉，否则隐藏之后 `ui_clicked` 还会一直为真。
    for entity in &mut scene.entities {
        if let Some(ui) = entity.ui.as_mut() {
            ui.state = UiState::default();
        }
    }

    // -- 1. 上一帧捕获的实体可能已被删除 / 摘掉 UI --
    if let Some(pressed) = interaction.pressed {
        let still_valid = scene
            .find_entity(pressed)
            .and_then(|i| scene.entities[i].ui.as_ref())
            .map(|ui| ui.visible && ui.interactable)
            .unwrap_or(false);
        if !still_valid {
            interaction.pressed = None;
        }
    }

    let hit = pointer
        .and_then(|p| p.pos)
        .and_then(|pos| hit_test(scene, items, pos));
    let hit_id = hit.map(|item| item.id);

    // -- 2. 悬停变化（指针移出视口也算离开）--
    if interaction.hovered != hit_id {
        if let Some(prev) = interaction.hovered {
            events.push(UiEvent {
                entity: prev,
                kind: UiEventKind::HoverExit,
            });
        }
        if let Some(next) = hit_id {
            events.push(UiEvent {
                entity: next,
                kind: UiEventKind::HoverEnter,
            });
        }
        interaction.hovered = hit_id;
    }

    // -- 3/4. 消费边沿，最终捕获必须与电平一致 --
    if let Some(p) = pointer {
        // 同帧两个边沿都发生过：最终松开 = Press → Release，最终按住则
        // Release → Press。固定先按后松会把新一轮按住也清掉。
        if p.released && p.down {
            interaction.release(scene, hit_id, events);
        }
        if p.pressed {
            interaction.press(hit_id, events);
        }
        if p.released && !p.down {
            interaction.release(scene, hit_id, events);
        }

        // 指针在视口内但没按着、也没捕获 —— 保证不会留下"幽灵按下"。
        if !p.down && !p.released && !p.pressed {
            interaction.pressed = None;
        }
    }
    // 指针离开视口时（`pointer == None`）：悬停已在第 2 步清空，
    // 按下捕获**保留** —— 用户可能在视口外松手，那一帧照样要收尾。

    // -- 5. 写回本帧标志 --
    for item in items {
        let Some(entity) = scene.entities.get_mut(item.index) else {
            continue;
        };
        let Some(ui) = entity.ui.as_mut() else {
            continue;
        };
        if hit_id == Some(item.id) {
            ui.state.hovered = true;
        }
        if interaction.pressed == Some(item.id) {
            ui.state.pressed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgame_core::{Entity, UiElement};
    use xgame_math::glam::Vec2;

    use crate::layout::{visit, UiTree};

    /// 场景：一个 100×40 的按钮，位于 (10,10)。
    fn scene_with_button() -> (Scene, EntityId) {
        let mut scene = Scene::new("ui-input");
        let mut button = Entity::new("Button");
        button.ui = Some(UiElement::button("OK").at([0.0, 0.0], 10.0, 10.0, 100.0, 40.0));
        let id = button.id;
        scene.entities.push(button);
        (scene, id)
    }

    fn layout(scene: &Scene) -> Vec<LayoutItem> {
        let mut tree = UiTree::new();
        tree.build(scene);
        let mut items = Vec::new();
        visit(scene, &tree, Vec2::new(800.0, 600.0), &mut items);
        items
    }

    fn state(scene: &Scene, id: EntityId) -> UiState {
        scene.entities[scene.find_entity(id).unwrap()]
            .ui
            .as_ref()
            .unwrap()
            .state
    }

    /// 一步交互：先算布局（不可变借用），再推进状态机（可变借用）。
    fn step(
        scene: &mut Scene,
        pointer: Option<UiPointer>,
        interaction: &mut UiInteraction,
        events: &mut Vec<UiEvent>,
    ) {
        let items = layout(scene);
        update_interaction(scene, &items, pointer, interaction, events);
    }

    fn pointer(x: f32, y: f32) -> UiPointer {
        UiPointer {
            pos: Some(Vec2::new(x, y)),
            down: false,
            pressed: false,
            released: false,
        }
    }

    /// 指针离开视口：位置没了，按键状态照旧带过来。
    fn pointer_outside(down: bool, pressed: bool, released: bool) -> UiPointer {
        UiPointer {
            pos: None,
            down,
            pressed,
            released,
        }
    }

    /// 完整点击：按下 → 悬停 → 松开，clicked 只在松开那一帧为真。
    #[test]
    fn press_and_release_inside_is_a_click() {
        let (mut scene, id) = scene_with_button();
        let mut interaction = UiInteraction::default();
        let mut events = Vec::new();

        let down = UiPointer {
            down: true,
            pressed: true,
            ..pointer(50.0, 30.0)
        };
        step(&mut scene, Some(down), &mut interaction, &mut events);
        assert!(state(&scene, id).hovered);
        assert!(state(&scene, id).pressed);
        assert!(!state(&scene, id).clicked, "按下那一帧还不算点击");
        assert_eq!(interaction.pressed(), Some(id));

        let up = UiPointer {
            down: false,
            released: true,
            ..pointer(50.0, 30.0)
        };
        step(&mut scene, Some(up), &mut interaction, &mut events);
        assert!(state(&scene, id).clicked);
        assert!(!state(&scene, id).pressed);
        assert_eq!(interaction.pressed(), None);
        assert!(events.iter().any(|e| e.kind == UiEventKind::Click));
    }

    #[test]
    fn release_then_repress_keeps_the_new_capture() {
        let (mut scene, id) = scene_with_button();
        let mut interaction = UiInteraction::default();
        let mut events = Vec::new();
        let down = UiPointer {
            down: true,
            pressed: true,
            ..pointer(50.0, 30.0)
        };
        step(&mut scene, Some(down), &mut interaction, &mut events);
        step(
            &mut scene,
            Some(UiPointer {
                released: true,
                ..down
            }),
            &mut interaction,
            &mut events,
        );
        assert!(state(&scene, id).clicked);
        assert_eq!(interaction.pressed(), Some(id), "松开后再按下应保留新捕获");
        assert!(state(&scene, id).pressed);
        assert_eq!(
            events.iter().map(|event| event.kind).collect::<Vec<_>>(),
            vec![UiEventKind::Click, UiEventKind::Release, UiEventKind::Press]
        );
    }

    /// 拖出按钮再松开：取消，不触发点击（与桌面按钮一致）。
    #[test]
    fn release_outside_cancels_click() {
        let (mut scene, id) = scene_with_button();
        let mut interaction = UiInteraction::default();
        let mut events = Vec::new();

        let down = UiPointer {
            down: true,
            pressed: true,
            ..pointer(50.0, 30.0)
        };
        step(&mut scene, Some(down), &mut interaction, &mut events);

        let up_outside = UiPointer {
            down: false,
            released: true,
            ..pointer(400.0, 300.0)
        };
        step(&mut scene, Some(up_outside), &mut interaction, &mut events);
        assert!(!state(&scene, id).clicked);
        assert_eq!(interaction.pressed(), None);
    }

    /// 拖出按钮后**又拖回来**松开：仍然算点击（按下即捕获）。
    #[test]
    fn dragging_out_and_back_still_clicks() {
        let (mut scene, id) = scene_with_button();
        let mut interaction = UiInteraction::default();
        let mut events = Vec::new();

        let down = UiPointer {
            down: true,
            pressed: true,
            ..pointer(50.0, 30.0)
        };
        step(&mut scene, Some(down), &mut interaction, &mut events);

        let drag = UiPointer {
            down: true,
            ..pointer(400.0, 300.0)
        };
        step(&mut scene, Some(drag), &mut interaction, &mut events);
        assert!(!state(&scene, id).hovered, "拖出后不再悬停");

        let back = UiPointer {
            down: true,
            ..pointer(50.0, 30.0)
        };
        step(&mut scene, Some(back), &mut interaction, &mut events);

        let up = UiPointer {
            down: false,
            released: true,
            ..pointer(50.0, 30.0)
        };
        step(&mut scene, Some(up), &mut interaction, &mut events);
        assert!(state(&scene, id).clicked);
    }

    /// 指针离开视口：悬停清掉，且不会在下一帧留下幽灵按下。
    #[test]
    fn leaving_viewport_clears_hover() {
        let (mut scene, id) = scene_with_button();
        let mut interaction = UiInteraction::default();
        let mut events = Vec::new();

        step(
            &mut scene,
            Some(pointer(50.0, 30.0)),
            &mut interaction,
            &mut events,
        );
        assert!(state(&scene, id).hovered);

        step(&mut scene, None, &mut interaction, &mut events);
        assert!(!state(&scene, id).hovered);
        assert_eq!(interaction.hovered(), None);
    }

    /// 在视口外松手必须收尾：捕获清空、发出 Release、不触发 Click。
    ///
    /// 早先 `UiPointer` 是一个 `Option`（指针不在视口就整条丢掉），于是这一帧
    /// 的 `released` 边沿也没了 —— 捕获永远留在按钮上，按钮一直亮着"按下"，
    /// 直到下一次在别处点击。位置与按键必须是两件事。
    #[test]
    fn release_outside_the_viewport_clears_the_capture() {
        let (mut scene, id) = scene_with_button();
        let mut interaction = UiInteraction::default();
        let mut events = Vec::new();

        let down = UiPointer {
            down: true,
            pressed: true,
            ..pointer(50.0, 30.0)
        };
        step(&mut scene, Some(down), &mut interaction, &mut events);
        assert_eq!(interaction.pressed(), Some(id));

        // 指针移出视口后松手：位置没了，松手边沿还在。
        step(
            &mut scene,
            Some(pointer_outside(false, false, true)),
            &mut interaction,
            &mut events,
        );
        assert_eq!(interaction.pressed(), None, "视口外松手必须清掉捕获");
        assert!(!state(&scene, id).clicked, "在按钮外松手不算点击");
        assert!(
            events.iter().any(|e| e.kind == UiEventKind::Release),
            "要给捕获方发 Release，脚本才收得到松手"
        );

        // 回到按钮上也不该再显示按下。
        step(
            &mut scene,
            Some(pointer(50.0, 30.0)),
            &mut interaction,
            &mut events,
        );
        assert!(!state(&scene, id).pressed, "捕获已清，不能再亮按下");
    }

    /// 按住不放、把指针拖到视口外再拖回来松手：仍然算点击（按下即捕获）。
    #[test]
    fn holding_outside_the_viewport_keeps_the_capture() {
        let (mut scene, id) = scene_with_button();
        let mut interaction = UiInteraction::default();
        let mut events = Vec::new();

        let down = UiPointer {
            down: true,
            pressed: true,
            ..pointer(50.0, 30.0)
        };
        step(&mut scene, Some(down), &mut interaction, &mut events);

        step(
            &mut scene,
            Some(pointer_outside(true, false, false)),
            &mut interaction,
            &mut events,
        );
        assert_eq!(interaction.pressed(), Some(id), "按住不动，捕获不该丢");
        assert!(!state(&scene, id).hovered, "视口外没有悬停");

        let up = UiPointer {
            down: false,
            released: true,
            ..pointer(50.0, 30.0)
        };
        step(&mut scene, Some(up), &mut interaction, &mut events);
        assert!(state(&scene, id).clicked, "拖回来松手仍算点击");
    }

    /// 父节点隐藏 = 整棵子树不参与命中（Unity `SetActive(false)` 语义）。
    ///
    /// 只按"元素自己的 visible"过滤是不够的：隐藏的 HUD 若仍在命中表里，
    /// 会继续吃掉点击，玩家点了没反应还查不出原因。
    #[test]
    fn hidden_parent_makes_children_unhittable() {
        let mut scene = Scene::new("hidden-parent");
        let mut panel = Entity::new("Panel");
        panel.ui = Some(UiElement::panel(400.0, 300.0).at([0.0, 0.0], 0.0, 0.0, 400.0, 300.0));
        let panel_id = panel.id;
        scene.entities.push(panel);

        let mut button = Entity::new("Button");
        button.parent = Some(panel_id);
        button.ui = Some(UiElement::button("OK").at([0.0, 0.0], 10.0, 10.0, 100.0, 40.0));
        let button_id = button.id;
        scene.entities.push(button);

        let mut interaction = UiInteraction::default();
        let mut events = Vec::new();
        let down = UiPointer {
            down: true,
            pressed: true,
            ..pointer(50.0, 30.0)
        };
        step(&mut scene, Some(down), &mut interaction, &mut events);
        assert_eq!(interaction.pressed(), Some(button_id), "可见时应能按下");

        // 松手复位，再隐藏父节点。
        let up = UiPointer {
            down: false,
            released: true,
            ..pointer(50.0, 30.0)
        };
        step(&mut scene, Some(up), &mut interaction, &mut events);
        let panel_index = scene.find_entity(panel_id).unwrap();
        scene.entities[panel_index].ui.as_mut().unwrap().visible = false;

        let down = UiPointer {
            down: true,
            pressed: true,
            ..pointer(50.0, 30.0)
        };
        step(&mut scene, Some(down), &mut interaction, &mut events);
        assert_eq!(interaction.pressed(), None, "隐藏父节点下的按钮不该被按下");
        assert!(!state(&scene, button_id).pressed);
        assert!(!state(&scene, button_id).hovered, "也不该有悬停");
    }

    /// 装饰元素（interactable = false）不吃点击。
    #[test]
    fn decorative_panel_is_transparent_to_clicks() {
        let mut scene = Scene::new("deco");
        let mut panel = Entity::new("Panel");
        panel.ui = Some(UiElement::panel(400.0, 400.0).at([0.0, 0.0], 0.0, 0.0, 400.0, 400.0));
        let panel_id = panel.id;
        scene.entities.push(panel);

        let mut interaction = UiInteraction::default();
        let mut events = Vec::new();
        let down = UiPointer {
            down: true,
            pressed: true,
            ..pointer(100.0, 100.0)
        };
        step(&mut scene, Some(down), &mut interaction, &mut events);

        assert!(!state(&scene, panel_id).pressed);
        assert_eq!(interaction.pressed(), None);
    }
}
