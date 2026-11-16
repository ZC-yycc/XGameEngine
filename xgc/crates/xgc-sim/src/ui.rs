//! UI 布局：锚点 / 偏移 → 屏幕矩形，父节点相对布局与可见性继承。

use std::collections::{HashMap, HashSet};

use xgc_base::EntityId;
use xgc_world::{Component, Parent, UiElement, UiInteraction, UiLayout, World};

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

struct Node {
    key: xgc_base::EntityKey,
    parent: Option<EntityId>,
    anchor_min: [f32; 2],
    anchor_max: [f32; 2],
    offset_min: [f32; 2],
    offset_max: [f32; 2],
    visible: bool,
    clip_children: bool,
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
        let viewport = self.viewport;
        let mut nodes: HashMap<EntityId, Node> = HashMap::new();
        world.for_each2::<UiElement, UiLayout>(|key, element, _| {
            let Some(entity) = world.entity(key) else {
                return;
            };
            let parent = world.get::<Parent>(key).map(|parent| parent.entity);
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
                },
            );
        });
        if nodes.is_empty() {
            return;
        }
        let mut ids: Vec<EntityId> = nodes.keys().copied().collect();
        ids.sort();
        let mut rects: HashMap<EntityId, [f32; 4]> = HashMap::new();
        let mut clips: HashMap<EntityId, [f32; 4]> = HashMap::new();
        let mut visibility: HashMap<EntityId, bool> = HashMap::new();
        let mut visiting = HashSet::new();
        for id in &ids {
            resolve(
                *id,
                &nodes,
                viewport,
                &mut rects,
                &mut clips,
                &mut visibility,
                &mut visiting,
            );
        }
        for id in ids {
            if let Some(node) = nodes.get(&id) {
                if let (Some(rect), Some(clip), Some(visible)) =
                    (rects.get(&id), clips.get(&id), visibility.get(&id))
                {
                    if let Some(layout) = world.get_mut::<UiLayout>(node.key) {
                        layout.rect = *rect;
                        layout.clip = *clip;
                        layout.visible = *visible;
                    }
                }
            }
        }
    }
}

/// 交互状态：按当前光标 / 点击更新 hover / press / click。
#[derive(Clone, Copy, Debug, Default)]
pub struct UiInteractionSystem {
    pub cursor: Option<[f32; 2]>,
    pub click: bool,
}

impl System for UiInteractionSystem {
    fn name(&self) -> &'static str {
        "ui_interaction"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![UiElement::ID, UiLayout::ID],
            writes: vec![UiInteraction::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        let mut interactable = HashMap::new();
        world.for_each::<UiElement>(|key, element| {
            interactable.insert(key, element.interactable);
        });
        let cursor = self.cursor;
        let click = self.click;
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
pub struct UiFocusSystem {
    pub next: bool,
    pub confirm: bool,
    pub nav: Option<UiNav>,
}

impl System for UiFocusSystem {
    fn name(&self) -> &'static str {
        "ui_focus"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![UiElement::ID, UiLayout::ID, UiInteraction::ID],
            writes: vec![UiInteraction::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        if !self.next && !self.confirm && self.nav.is_none() {
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
        let target = match self.nav {
            Some(UiNav::Tab) => current_index.map_or(0, |index| (index + 1) % focusable.len()),
            Some(direction) => neighbor(&focusable, current_index, direction),
            None if self.next => current_index.map_or(0, |index| (index + 1) % focusable.len()),
            None => current_index.unwrap_or(0),
        };
        let key = focusable[target].1;
        world.for_each_mut::<UiInteraction>(|_, state| {
            state.focused = false;
            if self.confirm {
                state.clicked = false;
            }
        });
        if let Some(state) = world.get_mut::<UiInteraction>(key) {
            state.focused = true;
            if self.confirm {
                state.clicked = true;
                state.pressed = true;
            }
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
    nodes: &HashMap<EntityId, Node>,
    viewport: [f32; 2],
    rects: &mut HashMap<EntityId, [f32; 4]>,
    clips: &mut HashMap<EntityId, [f32; 4]>,
    visibility: &mut HashMap<EntityId, bool>,
    visiting: &mut HashSet<EntityId>,
) -> Option<([f32; 4], bool)> {
    if let (Some(rect), Some(visible)) = (rects.get(&id), visibility.get(&id)) {
        return Some((*rect, *visible));
    }
    let node = nodes.get(&id)?;
    let viewport_rect = [0.0, 0.0, viewport[0], viewport[1]];
    if !visiting.insert(id) {
        return Some((viewport_rect, true));
    }
    let (parent_rect, parent_visible) = node
        .parent
        .and_then(|parent| resolve(parent, nodes, viewport, rects, clips, visibility, visiting))
        .unwrap_or((viewport_rect, true));
    let parent_clip = node
        .parent
        .and_then(|parent| clips.get(&parent).copied())
        .unwrap_or(viewport_rect);
    let clip = match node.parent.and_then(|parent| nodes.get(&parent)) {
        Some(parent) if parent.clip_children => intersect(parent_clip, parent_rect),
        _ => parent_clip,
    };
    let rect = [
        parent_rect[0] + node.anchor_min[0] * parent_rect[2] + node.offset_min[0],
        parent_rect[1] + node.anchor_min[1] * parent_rect[3] + node.offset_min[1],
        (node.anchor_max[0] - node.anchor_min[0]) * parent_rect[2] + node.offset_max[0]
            - node.offset_min[0],
        (node.anchor_max[1] - node.anchor_min[1]) * parent_rect[3] + node.offset_max[1]
            - node.offset_min[1],
    ];
    let visible = node.visible && parent_visible;
    visiting.remove(&id);
    rects.insert(id, rect);
    clips.insert(id, clip);
    visibility.insert(id, visible);
    Some((rect, visible))
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
