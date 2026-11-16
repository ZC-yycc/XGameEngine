//! UI 布局：锚点 / 偏移 → 屏幕矩形，父节点相对布局与可见性继承。

use std::collections::{HashMap, HashSet};

use xgc_base::EntityId;
use xgc_world::{Component, Parent, UiElement, UiLayout, World};

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
                },
            );
        });
        if nodes.is_empty() {
            return;
        }
        let mut ids: Vec<EntityId> = nodes.keys().copied().collect();
        ids.sort();
        let mut rects: HashMap<EntityId, [f32; 4]> = HashMap::new();
        let mut visibility: HashMap<EntityId, bool> = HashMap::new();
        let mut visiting = HashSet::new();
        for id in &ids {
            resolve(
                *id,
                &nodes,
                viewport,
                &mut rects,
                &mut visibility,
                &mut visiting,
            );
        }
        for id in ids {
            if let Some(node) = nodes.get(&id) {
                if let (Some(rect), Some(visible)) = (rects.get(&id), visibility.get(&id)) {
                    if let Some(layout) = world.get_mut::<UiLayout>(node.key) {
                        layout.rect = *rect;
                        layout.visible = *visible;
                    }
                }
            }
        }
    }
}

fn resolve(
    id: EntityId,
    nodes: &HashMap<EntityId, Node>,
    viewport: [f32; 2],
    rects: &mut HashMap<EntityId, [f32; 4]>,
    visibility: &mut HashMap<EntityId, bool>,
    visiting: &mut HashSet<EntityId>,
) -> Option<([f32; 4], bool)> {
    if let (Some(rect), Some(visible)) = (rects.get(&id), visibility.get(&id)) {
        return Some((*rect, *visible));
    }
    let node = nodes.get(&id)?;
    if !visiting.insert(id) {
        return Some(([0.0, 0.0, viewport[0], viewport[1]], true));
    }
    let (parent_rect, parent_visible) = node
        .parent
        .and_then(|parent| resolve(parent, nodes, viewport, rects, visibility, visiting))
        .unwrap_or(([0.0, 0.0, viewport[0], viewport[1]], true));
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
    visibility.insert(id, visible);
    Some((rect, visible))
}
