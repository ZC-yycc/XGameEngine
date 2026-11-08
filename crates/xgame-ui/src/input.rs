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

use xgame_core::{EntityId, Scene, UiKind, UiState};
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


// ---------------------------------------------------------------------------
// 焦点环（手柄 / 键盘导航）
// ---------------------------------------------------------------------------

/// 焦点导航方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusDir {
    Left,
    Right,
    Up,
    Down,
}

impl FocusDir {
    /// 该方向在屏幕上的单位向量（y 向下，与布局坐标系一致）。
    fn axis(self) -> Vec2 {
        match self {
            FocusDir::Left => Vec2::new(-1.0, 0.0),
            FocusDir::Right => Vec2::new(1.0, 0.0),
            FocusDir::Up => Vec2::new(0.0, -1.0),
            FocusDir::Down => Vec2::new(0.0, 1.0),
        }
    }
}

/// 一帧的焦点导航输入（边沿，不是电平 —— 按住不连发）。
///
/// 这里刻意不依赖 `xgame-input`：翻译"哪些键 / 哪个摇杆"是宿主的活（见
/// `xgame-runtime` 的 `World::focus_input`），UI 层只关心"这一帧有没有按下左"。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FocusInput {
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
    /// 确认键（手柄南键 / Enter / 小键盘 Enter）**刚按下**。
    pub confirm: bool,
}

impl FocusInput {
    /// 有没有任何导航动作（宿主用它决定"要不要为了焦点多跑一遍"）。
    pub fn is_idle(&self) -> bool {
        !(self.left || self.right || self.up || self.down || self.confirm)
    }
}

/// 焦点环：跨帧只记"焦点在谁身上"。
#[derive(Debug, Default, Clone)]
pub struct UiFocus {
    focused: Option<EntityId>,
}

impl UiFocus {
    /// 当前焦点（`None` = 还没用键盘 / 手柄选过）。
    pub fn focused(&self) -> Option<EntityId> {
        self.focused
    }

    /// 直接指定焦点（脚本 / 菜单初始化用；指向不可交互的元素会在下一帧被清掉）。
    pub fn set(&mut self, id: Option<EntityId>) {
        self.focused = id;
    }

    /// 清空（换场景 / 重进 Play 时用）。
    pub fn clear(&mut self) {
        self.focused = None;
    }

    /// 推进一帧：按输入移动焦点、写回 `state.focused`，返回本帧被确认的元素。
    ///
    /// # 语义
    ///
    /// - **一次一帧只走一步**：同一帧按下两个方向（斜着推摇杆）时按
    ///   左 → 右 → 上 → 下 取第一个，不跳对角；
    /// - **不回绕**：到边就停在原地（Unity 的默认导航同款）。回绕会让"按了三下
    ///   又回到起点"，在菜单里很容易误触；
    /// - **焦点实体一旦不可见 / 不可交互就清空**（它可能刚被脚本隐藏或删掉），
    ///   下一次推方向重选 —— 而不是把焦点留在幽灵元素上；
    /// - 确认键只把 `clicked` 置一帧（与鼠标点击同款），不模拟"按住"：按住确认
    ///   不该让按钮一直是按下态。
    ///
    /// `items` 必须是本帧 `layout::visit` 的输出（绘制顺序），邻居判定的
    /// 平手也用它在数组里的先后决定，因此结果与帧率无关、可复现。
    pub fn update(
        &mut self,
        scene: &mut Scene,
        items: &[LayoutItem],
        input: FocusInput,
    ) -> Option<EntityId> {
        // -- 1. 焦点还有效吗 --
        let still_valid = self
            .focused
            .and_then(|id| scene.find_entity(id))
            .and_then(|index| scene.entities[index].ui.as_ref())
            .is_some_and(|ui| ui.visible && ui.interactable);
        if !still_valid {
            self.focused = None;
        }

        // -- 2. 移动焦点 --
        let direction = if input.left {
            Some(FocusDir::Left)
        } else if input.right {
            Some(FocusDir::Right)
        } else if input.up {
            Some(FocusDir::Up)
        } else if input.down {
            Some(FocusDir::Down)
        } else {
            None
        };
        if let Some(direction) = direction {
            if let Some(next) = neighbor(scene, items, self.focused, direction) {
                self.focused = Some(next);
            } else if self.focused.is_none() {
                // 一个可交互元素都没有：保持 `None`，别造出假焦点。
                self.focused = None;
            }
        }

        // -- 3. 写回标志（`update_interaction` 刚把整份 state 清零过）--
        let mut confirmed = None;
        if let Some(focused) = self.focused {
            if let Some(index) = scene.find_entity(focused) {
                if let Some(ui) = scene.entities[index].ui.as_mut() {
                    ui.state.focused = true;
                    if input.confirm {
                        ui.state.clicked = true;
                        confirmed = Some(focused);
                    }
                }
            }
        }
        confirmed
    }
}

/// 候选元素：**按钮** + 可见 + 可交互 + 矩形非空。
///
/// 为什么限定 `UiKind::Button` 而不是"凡是 `interactable` 就行"：反序列化时
/// `interactable` 缺省为 `true`（`default_true`），于是场景里每个装饰用的大面板
/// 都是"可交互"的 —— 恰好又覆盖整个 HUD 区域，导航会先选中那块背景板，按方向键
/// 只在板子内部原地打转。手柄能"选"的东西必须是**能按下去**的东西，所以候选集
/// 与"有按下语义的控件"对齐（Unity 里也是只有 `Selectable` 进导航，`Image`
/// 即便 `raycastTarget` 也不会被选中）。
///
/// 想用手柄点到自绘的可点卡片：把它的 `kind` 改成 `Button` 再覆盖样式即可。
fn navigable(scene: &Scene, item: &LayoutItem) -> bool {
    item.rect.width() > 0.0
        && item.rect.height() > 0.0
        && scene
            .entities
            .get(item.index)
            .and_then(|entity| entity.ui.as_ref())
            .is_some_and(|ui| ui.kind == UiKind::Button && ui.visible && ui.interactable)
}

/// 从 `from` 出发沿 `direction` 找最近的候选；`from = None` 时取绘制顺序里的
/// 首个（向右 / 向下）或末个（向左 / 向上）。
///
/// 打分 = 主方向距离 + 1.5 × 垂直偏移：**同一条"行"上的更好**。两列按钮里往下
/// 走，正下方那个的垂直偏移是 0，斜对角那个要多付 1.5 倍 — 于是"往下"不会跳到
/// 隔壁列。平手由绘制顺序决定（先画的胜），与 `items` 的顺序绑定，可复现。
fn neighbor(
    scene: &Scene,
    items: &[LayoutItem],
    from: Option<EntityId>,
    direction: FocusDir,
) -> Option<EntityId> {
    let axis = direction.axis();
    let perpendicular = Vec2::new(-axis.y, axis.x);
    let Some(from) = from else {
        let first = items.iter().find(|item| navigable(scene, item))?;
        let last = items.iter().rev().find(|item| navigable(scene, item))?;
        return Some(match direction {
            FocusDir::Right | FocusDir::Down => first.id,
            FocusDir::Left | FocusDir::Up => last.id,
        });
    };
    let origin = items
        .iter()
        .find(|item| item.id == from)
        .map(|item| item.rect.center())?;

    let mut best: Option<(f32, EntityId)> = None;
    for item in items {
        if item.id == from || !navigable(scene, item) {
            continue;
        }
        let delta = item.rect.center() - origin;
        let primary = delta.dot(axis);
        // 至少挪过 2px 才算"在那个方向上"：同一行的按钮中心可能差几分之一像素，
        // 不设门槛会让"按右"跳到视觉上同一格的邻居。
        if primary <= 2.0 {
            continue;
        }
        let score = primary + 1.5 * delta.dot(perpendicular).abs();
        if best.is_none_or(|(best_score, _)| score < best_score) {
            best = Some((score, item.id));
        }
    }
    best.map(|(_, id)| id)
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


    // -----------------------------------------------------------------------
    // 焦点环
    // -----------------------------------------------------------------------

    /// 2×2 的按钮网格：左上 a(10,10,100×40)、右上 b(200,10)、左下 c(10,100)、右下 d(200,100)。
    fn scene_with_grid() -> (Scene, EntityId, EntityId, EntityId, EntityId) {
        let mut scene = Scene::new("ui-focus");
        let mut ids = Vec::new();
        for (name, x, y) in [("A", 10.0, 10.0), ("B", 200.0, 10.0), ("C", 10.0, 100.0), ("D", 200.0, 100.0)]
        {
            let mut button = Entity::new(name);
            button.ui = Some(UiElement::button(name).at([0.0, 0.0], x, y, 100.0, 40.0));
            ids.push(button.id);
            scene.entities.push(button);
        }
        (scene, ids[0], ids[1], ids[2], ids[3])
    }

    /// 推方向 → 焦点按几何位置走一格；右 / 下 / 左 / 上各走一次回到起点。
    #[test]
    fn focus_moves_between_buttons_along_the_grid() {
        let (mut scene, a, b, c, _d) = scene_with_grid();
        let items = layout(&scene);
        let mut focus = UiFocus::default();

        // 没有焦点时按"右"落到绘制顺序里的第一个。
        focus.update(&mut scene, &items, FocusInput { right: true, ..Default::default() });
        assert_eq!(focus.focused(), Some(a), "首次按方向键应当选中第一个可交互元素");

        focus.update(&mut scene, &items, FocusInput { right: true, ..Default::default() });
        assert_eq!(focus.focused(), Some(b), "右边是 B");

        // 到边不回绕：B 再往右还在 B。
        focus.update(&mut scene, &items, FocusInput { right: true, ..Default::default() });
        assert_eq!(focus.focused(), Some(b), "已经在最右，不该回绕到 A");

        focus.update(&mut scene, &items, FocusInput { down: true, ..Default::default() });
        // B 的正下方是 D —— 但这里只断言"往下走到了第二行"，D 不在返回值里也会红。
        assert_ne!(focus.focused(), Some(b), "往下必须有移动");
        assert_ne!(focus.focused(), Some(a), "往下不该横跳到 A");

        focus.update(&mut scene, &items, FocusInput { left: true, ..Default::default() });
        assert_eq!(focus.focused(), Some(c), "从第二行往左是 C");
    }

    /// 悬停与焦点是两条独立通道：指针停在 A 上不改变焦点，焦点也不点亮 hover。
    #[test]
    fn focus_and_hover_are_independent() {
        let (mut scene, a, _b, _c, _d) = scene_with_grid();
        let items = layout(&scene);
        let mut focus = UiFocus::default();
        focus.set(Some(a));

        // 指针停在 A 上（会写 hovered），同时焦点在 A 上。
        let mut interaction = UiInteraction::default();
        let mut events = Vec::new();
        update_interaction(
            &mut scene,
            &items,
            Some(pointer(50.0, 30.0)),
            &mut interaction,
            &mut events,
        );
        focus.update(&mut scene, &items, FocusInput::default());
        let s = state(&scene, a);
        assert!(s.hovered && s.focused, "两种标志可以同时成立");
        assert!(!s.clicked, "只是悬停 + 焦点不该算点击");
    }

    /// 确认键把 `clicked` 置一帧（与鼠标点击同款），并返回被确认的元素。
    #[test]
    fn confirm_clicks_the_focused_button_once() {
        let (mut scene, a, _b, _c, _d) = scene_with_grid();
        let items = layout(&scene);
        let mut focus = UiFocus::default();
        focus.set(Some(a));

        let confirmed = focus.update(&mut scene, &items, FocusInput { confirm: true, ..Default::default() });
        assert_eq!(confirmed, Some(a));
        assert!(state(&scene, a).clicked, "确认应当让 ui_clicked 为真");

        // 下一帧不按确认：`clicked` 回到 false（它是"本帧"标志）。
        // 真实帧序是 `update_interaction`（先整份清零）→ `UiFocus::update`，
        // 所以这里必须走一遍前者，否则测的是"没人清标志"的假现场。
        let items = layout(&scene);
        let mut interaction = UiInteraction::default();
        let mut events = Vec::new();
        update_interaction(&mut scene, &items, None, &mut interaction, &mut events);
        focus.update(&mut scene, &items, FocusInput::default());
        assert!(!state(&scene, a).clicked, "确认只对按下的那一帧成立");
    }

    /// 焦点元素被隐藏 / 关掉交互 / 删掉 → 焦点清空，下一次推方向重选。
    #[test]
    fn focus_is_dropped_when_the_target_is_no_longer_interactive() {
        let (mut scene, a, b, _c, _d) = scene_with_grid();
        let mut focus = UiFocus::default();
        focus.set(Some(a));

        // 让 A 不可交互。
        let index = scene.find_entity(a).unwrap();
        scene.entities[index].ui.as_mut().unwrap().interactable = false;
        let items = layout(&scene);
        focus.update(&mut scene, &items, FocusInput::default());
        assert_eq!(focus.focused(), None, "不可交互的元素不该继续持有焦点");
        assert!(!state(&scene, a).focused);

        // 重选时跳过不可交互的 A，落到 B。
        focus.update(&mut scene, &items, FocusInput { down: true, ..Default::default() });
        assert_eq!(focus.focused(), Some(b), "导航必须跳过不可交互的元素");
    }

    /// 可交互的**装饰面板**不是导航候选：它盖住整个 HUD，选中它等于方向键原地打转。
    ///
    /// 反序列化时 `interactable` 缺省为 `true`（`default_true`），所以场景里的背景板
    /// 天然"可交互"—— 候选集因此按 `kind` 收窄到按钮。这条用真实的层级（面板套按钮）
    /// 钉住，而不是靠 `UiElement::panel` 恰好把 `interactable` 建成 `false`。
    #[test]
    fn interactive_panels_are_not_focus_candidates() {
        let mut scene = Scene::new("ui-focus-panel");
        let mut panel = Entity::new("HUD");
        let mut ui = UiElement::panel(400.0, 300.0);
        // 故意写成"可交互"（场景反序列化后的真实状态）。
        ui.interactable = true;
        panel.ui = Some(ui);
        let panel_id = panel.id;
        scene.entities.push(panel);

        let mut button = Entity::new("Play");
        button.ui = Some(UiElement::button("Play").at([0.0, 0.0], 20.0, 120.0, 120.0, 40.0));
        let button_id = button.id;
        scene.entities.push(button);

        // 面板是纯装饰：它自己不该被选中，也不该被算作"棋盘"的一部分。
        let items = layout(&scene);
        let mut focus = UiFocus::default();
        focus.update(&mut scene, &items, FocusInput { right: true, ..Default::default() });
        assert_eq!(focus.focused(), Some(button_id), "首次导航应当落到按钮而不是背景板");
        assert_ne!(focus.focused(), Some(panel_id));

        // 再推一次方向也还在按钮上（只有一个候选，且不会回绕到面板）。
        focus.update(&mut scene, &items, FocusInput { right: true, ..Default::default() });
        assert_eq!(focus.focused(), Some(button_id), "唯一候选就是停在那一个上");
        assert!(!state(&scene, panel_id).focused, "面板永远不该有焦点");
    }

    /// 对角候选要付垂直偏移的代价：两列两行时"往下"不跳到隔壁列。
    #[test]
    fn diagonal_candidates_cost_more_than_the_one_below() {
        let mut scene = Scene::new("ui-focus-diag");
        // 左下角一个按钮，右下角一个**更近**的斜对角，正下方一个**稍远**的。
        let mut ids = Vec::new();
        for (name, x, y, w) in [
            ("Top", 100.0, 10.0, 60.0),
            ("Below", 100.0, 200.0, 60.0),
            ("Diag", 220.0, 120.0, 60.0),
        ] {
            let mut button = Entity::new(name);
            button.ui = Some(UiElement::button(name).at([0.0, 0.0], x, y, w, 30.0));
            ids.push(button.id);
            scene.entities.push(button);
        }
        let items = layout(&scene);
        let mut focus = UiFocus::default();
        focus.set(Some(ids[0]));
        focus.update(&mut scene, &items, FocusInput { down: true, ..Default::default() });
        assert_eq!(
            focus.focused(),
            Some(ids[1]),
            "正下方（垂直偏移 0）应当胜出，即使斜对角更近"
        );
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
