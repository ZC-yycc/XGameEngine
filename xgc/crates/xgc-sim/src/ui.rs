//! UI 布局：锚点 / 偏移 → 屏幕矩形，父节点相对布局与可见性继承。

use std::cell::RefCell;
use std::collections::HashMap;

use xgc_base::EntityId;
use xgc_world::{Component, Parent, UiElement, UiInput, UiInteraction, UiLayout, World};

use super::{System, SystemAccess};

#[derive(Clone, Copy, Debug)]
pub struct UiLayoutSystem {
    pub viewport: [f32; 2],
}

impl Default for UiLayoutSystem {
    fn default() -> Self {
        Self {
            viewport: [1280.0, 720.0],
        }
    }
}

#[derive(Clone, Copy)]
struct Node {
    key: xgc_base::EntityKey,
    parent: Option<EntityId>,
    anchor_min: [f32; 2],
    anchor_max: [f32; 2],
    offset_min: [f32; 2],
    offset_max: [f32; 2],
    visible: bool,
    clip_children: bool,
    state: LayoutState,
}

#[derive(Clone, Copy)]
enum LayoutState {
    Pending,
    Visiting,
    Resolved(UiLayout),
}

#[derive(Default)]
struct LayoutScratch {
    nodes: HashMap<EntityId, Node>,
    ids: Vec<EntityId>,
}

thread_local! {
    // 同线程的布局实例只复用容器容量。每次运行都从当前世界读取输入。
    static LAYOUT_SCRATCH: RefCell<LayoutScratch> = RefCell::new(LayoutScratch::default());
}

impl System for UiLayoutSystem {
    fn name(&self) -> &'static str {
        "ui_layout"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![UiElement::ID, Parent::ID],
            writes: vec![UiLayout::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        LAYOUT_SCRATCH.with(|slot| {
            let mut scratch = slot.borrow_mut();
            let LayoutScratch { nodes, ids } = &mut *scratch;
            nodes.clear();
            ids.clear();
            world.for_each2::<UiElement, UiLayout>(|key, element, _| {
                let Some(entity) = world.entity(key) else {
                    return;
                };
                let parent = world.get::<Parent>(key).map(|parent| parent.entity);
                ids.push(entity);
                nodes.insert(
                    entity,
                    Node {
                        key,
                        parent,
                        anchor_min: element.anchor_min,
                        anchor_max: element.anchor_max,
                        offset_min: element.offset_min,
                        offset_max: element.offset_max,
                        visible: element.visible,
                        clip_children: element.clip_children,
                        state: LayoutState::Pending,
                    },
                );
            });
            // ID 唯一，原地排序足以固定循环父链的求解起点。
            ids.sort_unstable();
            let viewport_rect = [0.0, 0.0, self.viewport[0], self.viewport[1]];
            let viewport = UiLayout {
                rect: viewport_rect,
                clip: viewport_rect,
                visible: true,
            };
            for &id in ids.iter() {
                resolve(id, nodes, viewport);
            }
            for id in ids {
                let node = &nodes[id];
                if let LayoutState::Resolved(result) = node.state {
                    if let Some(layout) = world.get_mut::<UiLayout>(node.key) {
                        *layout = result;
                    }
                }
            }
        });
    }
}

/// 交互状态：按 `ui.input` 的光标 / 点击更新 hover / press / click。
#[derive(Clone, Copy, Debug, Default)]
pub struct UiInteractionSystem;

impl System for UiInteractionSystem {
    fn name(&self) -> &'static str {
        "ui_interaction"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![UiElement::ID, UiInput::ID, UiLayout::ID],
            writes: vec![UiInteraction::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        let input = world.ui_input();
        let cursor = input.and_then(|input| {
            (input.cursor[0] >= 0.0 && input.cursor[1] >= 0.0).then_some(input.cursor)
        });
        let click = input.is_some_and(|input| input.click);
        let mut interactable = HashMap::new();
        world.for_each::<UiElement>(|key, element| {
            interactable.insert(key, element.interactable);
        });
        world.for_each2_mut::<UiLayout, UiInteraction>(|key, layout, state| {
            let hovered = cursor.is_some_and(|point| {
                interactable.get(&key).copied().unwrap_or(false)
                    && layout.visible
                    && contains(layout.rect, point)
                    && contains(layout.clip, point)
            });
            state.hovered = hovered;
            state.pressed = hovered && click;
            state.clicked = hovered && click;
        });
    }
}

fn nav_from(value: &str) -> Option<UiNav> {
    match value {
        "tab" => Some(UiNav::Tab),
        "up" => Some(UiNav::Up),
        "down" => Some(UiNav::Down),
        "left" => Some(UiNav::Left),
        "right" => Some(UiNav::Right),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiNav {
    Tab,
    Up,
    Down,
    Left,
    Right,
}

/// 焦点导航：Tab 循环，方向键按几何选邻；`confirm` 同时点击。
#[derive(Clone, Copy, Debug, Default)]
pub struct UiFocusSystem;

impl System for UiFocusSystem {
    fn name(&self) -> &'static str {
        "ui_focus"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![UiElement::ID, UiInput::ID, UiLayout::ID, UiInteraction::ID],
            writes: vec![UiInput::ID, UiInteraction::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        let Some(input) = world.ui_input() else {
            return;
        };
        let nav = nav_from(input.nav);
        if nav.is_none() && !input.confirm {
            return;
        }
        let mut interactable = HashMap::new();
        world.for_each::<UiElement>(|key, element| {
            interactable.insert(key, element.interactable);
        });
        let mut focusable: Vec<(EntityId, xgc_base::EntityKey, [f32; 2])> = Vec::new();
        let mut current = None;
        world.for_each::<UiInteraction>(|key, state| {
            if state.focused {
                if let Some(entity) = world.entity(key) {
                    current = Some(entity);
                }
            }
            if !interactable.get(&key).copied().unwrap_or(false) {
                return;
            }
            let Some(layout) = world.get::<UiLayout>(key) else {
                return;
            };
            if !layout.visible {
                return;
            }
            if let Some(entity) = world.entity(key) {
                focusable.push((
                    entity,
                    key,
                    [
                        layout.rect[0] + layout.rect[2] * 0.5,
                        layout.rect[1] + layout.rect[3] * 0.5,
                    ],
                ));
            }
        });
        if focusable.is_empty() {
            return;
        }
        focusable.sort_by_key(|(entity, _, _)| *entity);
        let current_index =
            current.and_then(|entity| focusable.iter().position(|(id, _, _)| *id == entity));
        let target = match nav {
            Some(UiNav::Tab) => current_index.map_or(0, |index| (index + 1) % focusable.len()),
            Some(direction) => neighbor(&focusable, current_index, direction),
            None => current_index.unwrap_or(0),
        };
        let key = focusable[target].1;
        world.for_each_mut::<UiInteraction>(|_, state| {
            state.focused = false;
            if input.confirm {
                state.clicked = false;
            }
        });
        if let Some(state) = world.get_mut::<UiInteraction>(key) {
            state.focused = true;
            if input.confirm {
                state.clicked = true;
                state.pressed = true;
            }
        }
        if let Some(input) = world.ui_input_mut() {
            input.nav = "none";
            input.confirm = false;
        }
    }
}

fn neighbor(
    focusable: &[(EntityId, xgc_base::EntityKey, [f32; 2])],
    current: Option<usize>,
    nav: UiNav,
) -> usize {
    let Some(current) = current else {
        return 0;
    };
    let from = focusable[current].2;
    let mut best: Option<(f32, EntityId, usize)> = None;
    for (index, (entity, _, center)) in focusable.iter().enumerate() {
        if index == current {
            continue;
        }
        let dx = center[0] - from[0];
        let dy = center[1] - from[1];
        let (primary, perpendicular) = match nav {
            UiNav::Left if dx < 0.0 => (-dx, dy.abs()),
            UiNav::Right if dx > 0.0 => (dx, dy.abs()),
            UiNav::Up if dy < 0.0 => (-dy, dx.abs()),
            UiNav::Down if dy > 0.0 => (dy, dx.abs()),
            _ => continue,
        };
        let score = primary + perpendicular * 1.5;
        if best.is_none_or(|(best_score, best_entity, _)| {
            score < best_score || (score == best_score && *entity < best_entity)
        }) {
            best = Some((score, *entity, index));
        }
    }
    best.map(|(_, _, index)| index).unwrap_or(current)
}

fn resolve(
    id: EntityId,
    nodes: &mut HashMap<EntityId, Node>,
    viewport: UiLayout,
) -> Option<(UiLayout, bool)> {
    let node = nodes.get_mut(&id)?;
    let input = *node;
    match input.state {
        LayoutState::Resolved(layout) => return Some((layout, input.clip_children)),
        LayoutState::Visiting => return Some((viewport, input.clip_children)),
        LayoutState::Pending => node.state = LayoutState::Visiting,
    }
    let (parent, clip_children) = input
        .parent
        .and_then(|parent| resolve(parent, nodes, viewport))
        .unwrap_or((viewport, false));
    let parent_rect = parent.rect;
    let clip = if clip_children {
        intersect(parent.clip, parent_rect)
    } else {
        parent.clip
    };
    let rect = [
        parent_rect[0] + input.anchor_min[0] * parent_rect[2] + input.offset_min[0],
        parent_rect[1] + input.anchor_min[1] * parent_rect[3] + input.offset_min[1],
        (input.anchor_max[0] - input.anchor_min[0]) * parent_rect[2] + input.offset_max[0]
            - input.offset_min[0],
        (input.anchor_max[1] - input.anchor_min[1]) * parent_rect[3] + input.offset_max[1]
            - input.offset_min[1],
    ];
    let layout = UiLayout {
        rect,
        clip,
        visible: input.visible && parent.visible,
    };
    nodes.get_mut(&id).unwrap().state = LayoutState::Resolved(layout);
    Some((layout, input.clip_children))
}

fn intersect(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let x = a[0].max(b[0]);
    let y = a[1].max(b[1]);
    let right = (a[0] + a[2]).min(b[0] + b[2]);
    let bottom = (a[1] + a[3]).min(b[1] + b[3]);
    [x, y, (right - x).max(0.0), (bottom - y).max(0.0)]
}

/// 命中测试：可见、可交互、点在矩形与裁剪区内的最上层（EntityId 最大）元素。
pub fn hit_test(world: &World, point: [f32; 2]) -> Option<xgc_base::EntityKey> {
    let mut best: Option<(EntityId, xgc_base::EntityKey)> = None;
    world.for_each::<UiLayout>(|key, layout| {
        if !layout.visible || !contains(layout.rect, point) || !contains(layout.clip, point) {
            return;
        }
        if world
            .get::<UiElement>(key)
            .is_some_and(|element| !element.interactable)
        {
            return;
        }
        let Some(entity) = world.entity(key) else {
            return;
        };
        if best.is_none_or(|(top, _)| entity > top) {
            best = Some((entity, key));
        }
    });
    best.map(|(_, key)| key)
}

fn contains(rect: [f32; 4], point: [f32; 2]) -> bool {
    point[0] >= rect[0]
        && point[1] >= rect[1]
        && point[0] <= rect[0] + rect[2]
        && point[1] <= rect[1] + rect[3]
}
