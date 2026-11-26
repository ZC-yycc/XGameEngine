//! 实体快照：结构命令（spawn / despawn）的 before-image。
//!
//! 快照保存“一列一份、每列一行”的类型擦除列；恢复时按组件 ID 找目标 archetype，
//! 用 `Column::push_from` 克隆回真实列。这样不需要运行期组件构造器，也不需要 unsafe。

use xgc_base::{EntityId, EntityKey};
use xgc_reflect::{ComponentId, Value};

use super::{Column, World};

pub struct EntitySnapshot {
    id: EntityId,
    revision: u64,
    pub(crate) components: Vec<(ComponentId, Box<dyn Column>)>,
}

impl EntitySnapshot {
    pub fn id(&self) -> EntityId {
        self.id
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn with_id(mut self, id: EntityId) -> Self {
        self.id = id;
        self
    }

    pub fn component_ids(&self) -> Vec<ComponentId> {
        self.components.iter().map(|(id, _)| *id).collect()
    }

    pub fn get(&self, component: ComponentId, field: &str) -> Option<Value> {
        self.components
            .iter()
            .find(|(id, _)| *id == component)
            .and_then(|(_, column)| column.get_field(0, field))
    }

    pub fn fields(&self, component: ComponentId) -> Option<Vec<(String, Value)>> {
        let (_, column) = self.components.iter().find(|(id, _)| *id == component)?;
        Some(
            column
                .field_names()
                .into_iter()
                .filter_map(|name| {
                    column
                        .get_field(0, name)
                        .map(|value| (name.to_string(), value))
                })
                .collect(),
        )
    }

    pub(crate) fn from_parts(
        id: EntityId,
        revision: u64,
        components: Vec<(ComponentId, Box<dyn Column>)>,
    ) -> Self {
        Self {
            id,
            revision,
            components,
        }
    }
}

impl Clone for EntitySnapshot {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            revision: self.revision,
            components: self
                .components
                .iter()
                .map(|(id, column)| (*id, column.clone_box()))
                .collect(),
        }
    }
}

impl std::fmt::Debug for EntitySnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntitySnapshot")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .field("components", &self.component_ids())
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct WorldSnapshot {
    revision: u64,
    next_id: u64,
    entities: Vec<EntitySnapshot>,
}

impl WorldSnapshot {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn entities(&self) -> &[EntitySnapshot] {
        &self.entities
    }
}

impl World {
    pub fn capture_entity(&self, id: EntityId) -> Option<EntitySnapshot> {
        let (_, meta) = self.meta_by_id(id)?;
        let arch = self.archetypes.get(meta.archetype as usize)?;
        let mut component_ids: Vec<_> = arch.columns.keys().copied().collect();
        component_ids.sort_by_key(|component| component.0);

        let mut components = Vec::with_capacity(component_ids.len());
        for component in component_ids {
            let column = arch.columns.get(&component)?;
            let mut single = column.empty_like();
            single.push_from(column.as_ref(), meta.row as usize);
            components.push((component, single));
        }
        Some(EntitySnapshot {
            id,
            revision: meta.revision,
            components,
        })
    }

    /// 捕获整个世界：实体按 `EntityId` 排序，便于逐字节比较与回放。
    pub fn snapshot(&self) -> WorldSnapshot {
        let mut entities: Vec<_> = self
            .metas
            .iter()
            .filter(|meta| meta.alive)
            .filter_map(|meta| self.capture_entity(meta.id))
            .collect();
        entities.sort_by_key(|entity| entity.id().raw());
        WorldSnapshot {
            revision: self.revision,
            next_id: self.next_id,
            entities,
        }
    }

    /// 恢复整个世界的逻辑状态；保留 archetype 容器，清空实体后按快照重建。
    pub fn restore_snapshot(&mut self, snapshot: &WorldSnapshot) {
        self.host_ui_input = None;
        let keys: Vec<_> = self.by_id.values().copied().collect();
        for key in keys {
            let _ = self.despawn_raw(key);
        }
        self.next_id = snapshot.next_id;
        self.revision = snapshot.revision;
        for entity in &snapshot.entities {
            let _ = self.restore_entity_raw(entity.clone());
        }
    }

    /// 公开恢复入口：结构变化，revision +1。
    pub fn restore_entity(&mut self, snapshot: EntitySnapshot) -> Result<EntityId, String> {
        let id = self.restore_entity_raw(snapshot)?;
        let revision = self.bump_revision();
        self.touch(id, revision);
        Ok(id)
    }

    /// 交易内部入口：不推进 revision，由事务统一提交。
    pub(crate) fn restore_entity_raw(
        &mut self,
        snapshot: EntitySnapshot,
    ) -> Result<EntityId, String> {
        let id = snapshot.id;
        if self.by_id.contains_key(&id) {
            return Err(format!("实体 ID 已被占用：{id}"));
        }

        let mut component_ids: Vec<_> = snapshot.components.iter().map(|(id, _)| *id).collect();
        component_ids.sort_by_key(|component| component.0);
        let archetype = self.archetype_for(&component_ids);
        let key = self.alloc_key();
        self.by_id.insert(id, key);

        let arch = &mut self.archetypes[archetype as usize];
        for (component, source) in &snapshot.components {
            let target = arch
                .columns
                .entry(*component)
                .or_insert_with(|| source.empty_like());
            target.push_from(source.as_ref(), 0);
        }
        arch.entities.push(key);
        let row = (arch.entities.len() - 1) as u32;

        let meta = &mut self.metas[key.index() as usize];
        meta.id = id;
        meta.alive = true;
        meta.archetype = archetype;
        meta.row = row;
        meta.revision = snapshot.revision;
        self.next_id = self.next_id.max(id.raw() + 1);
        Ok(id)
    }

    pub(crate) fn key_of(&self, id: EntityId) -> Option<EntityKey> {
        self.by_id.get(&id).copied()
    }
}
