use std::collections::HashMap;

use xgc_base::{EntityId, EntityKey};
use xgc_world::{Component, GlobalTransform, Parent, Transform, World};

use crate::{System, SystemAccess};

/// 按父链合成全局变换。父实体没有全局变换时按根处理，环和依赖环的实体保留原值。
#[derive(Default)]
pub struct HierarchySystem {
    nodes: Vec<HierarchyNode>,
    locals: Vec<Transform>,
    indices: HashMap<EntityId, usize>,
    children: Vec<Children>,
    parent_first: Vec<(usize, Option<usize>)>,
    write_order: Vec<(EntityKey, usize)>,
    globals: Vec<GlobalTransform>,
}

#[derive(PartialEq, Eq)]
struct HierarchyNode {
    key: EntityKey,
    id: EntityId,
    parent: Option<EntityId>,
}

#[derive(Default)]
struct Children {
    first: Option<usize>,
    next: Option<usize>,
}

impl HierarchySystem {
    fn gather(&mut self, world: &World) -> bool {
        let mut topology_changed = false;
        self.locals.clear();
        world.for_each_hierarchy_input(|key, id, parent, local| {
            let topology = HierarchyNode {
                key,
                id,
                parent: parent.map(|parent| parent.entity),
            };
            let index = self.locals.len();
            // 类型化可变访问不会推进 revision，必须读取当帧 Parent 才能确认拓扑可复用。
            match self.nodes.get_mut(index) {
                Some(node) if *node == topology => {}
                Some(node) => {
                    *node = topology;
                    topology_changed = true;
                }
                None => {
                    self.nodes.push(topology);
                    topology_changed = true;
                }
            }
            self.locals.push(local.copied().unwrap_or_default());
        });
        topology_changed |= self.nodes.len() != self.locals.len();
        self.nodes.truncate(self.locals.len());
        topology_changed
    }

    fn rebuild_topology(&mut self) {
        self.parent_first.clear();
        self.write_order.clear();
        self.globals
            .resize(self.nodes.len(), GlobalTransform::default());
        if self.nodes.is_empty() {
            return;
        }

        self.indices.clear();
        self.indices.extend(
            self.nodes
                .iter()
                .enumerate()
                .map(|(index, node)| (node.id, index)),
        );
        self.children.clear();
        self.children
            .resize_with(self.nodes.len(), Children::default);
        for (index, node) in self.nodes.iter().enumerate() {
            if let Some(&parent) = node.parent.as_ref().and_then(|id| self.indices.get(id)) {
                self.children[index].next = self.children[parent].first;
                self.children[parent].first = Some(index);
            } else {
                self.parent_first.push((index, None));
            }
        }

        // 以同一缓冲作为队列和传播顺序，从根出发不会进入环或依赖环的分支。
        let mut cursor = 0;
        while let Some(&(parent, _)) = self.parent_first.get(cursor) {
            let mut child = self.children[parent].first;
            while let Some(index) = child {
                self.parent_first.push((index, Some(parent)));
                child = self.children[index].next;
            }
            cursor += 1;
        }

        self.write_order.extend(
            self.parent_first
                .iter()
                .map(|&(index, _)| (self.nodes[index].key, index)),
        );
        // 每次写入都会推进修订号，按持久 ID 写回才能让完整快照可重放。
        self.write_order
            .sort_unstable_by_key(|&(_, index)| self.nodes[index].id);
    }
}

impl System for HierarchySystem {
    fn name(&self) -> &'static str {
        "hierarchy"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![Parent::ID, Transform::ID],
            writes: vec![GlobalTransform::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        if self.gather(world) {
            self.rebuild_topology();
        }
        for &(index, parent) in &self.parent_first {
            let local = &self.locals[index];
            self.globals[index] = match parent {
                Some(parent) => GlobalTransform::compose(&self.globals[parent], local),
                None => GlobalTransform::from_local(local),
            };
        }
        world.write_hierarchy_globals(
            self.write_order
                .iter()
                .map(|&(key, index)| (key, &self.globals[index])),
        );
    }
}
