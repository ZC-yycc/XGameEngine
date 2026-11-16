//! XGame Core 的世界：archetype 列式存储 + 生成 / 回收 + 查询。
//!
#![deny(unsafe_code)]
use std::any::Any;
use std::collections::HashMap;
use xgc_base::{EntityId, EntityKey};
pub use xgc_reflect::Component;
mod animation_components;
mod character_components;
mod command;
mod components;
mod data;
mod gameplay_components;
mod hash;
mod journal;
mod lod_components;
mod particle_components;
mod physics_components;
mod plugin_components;
mod query;
mod registry;
mod render_components;
mod snapshot;
mod ui_components;
mod validate;
pub use animation_components::Animator;
pub use character_components::CharacterController;
pub use command::{Applied, Command, Plan};
pub use components::{GlobalTransform, Name, Parent, Transform, Velocity};
use data::ComponentFactory;
pub use data::{ComponentData, EntityData, WorldData};
pub use gameplay_components::{AudioSource, Script};
pub use journal::{Journal, TxRecord};
pub use lod_components::LodGroup;
pub use particle_components::ParticleEmitter;
pub use physics_components::PhysicsBody;
pub use plugin_components::PluginComponent;
pub use registry::builtin_registry;
pub use render_components::{Camera, Material, MeshRenderer};
pub use snapshot::{EntitySnapshot, WorldSnapshot};
pub use ui_components::{UiElement, UiInteraction, UiLayout};
pub use xgc_reflect::{ComponentId, Value};
/// 列：任意组件的连续存储。只暴露类型擦除后的最小操作。
///
/// 这是 crate 内部接口；`Bundle` 的签名需要它，所以公开但不对使用方承诺稳定性。
#[doc(hidden)]
pub trait Column: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn swap_remove(&mut self, index: usize);
    fn get_field(&self, row: usize, field: &str) -> Option<Value>;
    fn set_field(&mut self, row: usize, field: &str, value: &Value) -> Result<(), String>;
    fn validate_field(&self, row: usize, field: &str, value: &Value) -> Result<(), String>;
    fn field_names(&self) -> Vec<&'static str>;
    fn clone_box(&self) -> Box<dyn Column>;
    fn empty_like(&self) -> Box<dyn Column>;
    fn push_from(&mut self, source: &dyn Column, row: usize);
}

impl<T: Component> Column for Vec<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn len(&self) -> usize {
        Vec::len(self)
    }

    fn swap_remove(&mut self, index: usize) {
        Vec::swap_remove(self, index);
    }

    fn get_field(&self, row: usize, field: &str) -> Option<Value> {
        self.get(row)?.get_field(field)
    }

    fn set_field(&mut self, row: usize, field: &str, value: &Value) -> Result<(), String> {
        let values = self
            .as_any_mut()
            .downcast_mut::<Vec<T>>()
            .ok_or_else(|| format!("{} 列类型不一致", T::NAME))?;
        let item = values
            .get_mut(row)
            .ok_or_else(|| format!("{} 行号越界：{row}", T::NAME))?;
        item.set_field(field, value)
    }

    fn validate_field(&self, row: usize, field: &str, value: &Value) -> Result<(), String> {
        let values = self
            .as_any()
            .downcast_ref::<Vec<T>>()
            .ok_or_else(|| format!("{} 列类型不一致", T::NAME))?;
        let item = values
            .get(row)
            .ok_or_else(|| format!("{} 行号越界：{row}", T::NAME))?;
        item.validate_field(field, value)
    }

    fn field_names(&self) -> Vec<&'static str> {
        T::fields().into_iter().map(|field| field.name).collect()
    }

    fn clone_box(&self) -> Box<dyn Column> {
        Box::new(self.clone())
    }

    fn empty_like(&self) -> Box<dyn Column> {
        Box::new(Vec::<T>::new())
    }

    fn push_from(&mut self, source: &dyn Column, row: usize) {
        let values = source
            .as_any()
            .downcast_ref::<Vec<T>>()
            .expect("组件快照的列类型不一致");
        let value = values.get(row).expect("组件快照行号越界").clone();
        self.push(value);
    }
}

struct Archetype {
    entities: Vec<EntityKey>,
    columns: HashMap<ComponentId, Box<dyn Column>>,
}

impl Archetype {
    fn new(ids: Vec<ComponentId>) -> Self {
        Self {
            entities: Vec::new(),
            columns: HashMap::with_capacity(ids.len()),
        }
    }

    /// 删除一行；返回被搬到该行的实体（如果是最后一行则为 None）。
    fn swap_remove_row(&mut self, row: usize) -> Option<EntityKey> {
        let last = self.entities.last().copied();
        for column in self.columns.values_mut() {
            column.swap_remove(row);
        }
        self.entities.swap_remove(row);
        if row < self.entities.len() {
            last
        } else {
            None
        }
    }
}

/// 一次 spawn 携带的组件集合。用元组表达，避免运行期反射开销。
pub trait Bundle {
    fn component_ids() -> Vec<ComponentId>;
    fn insert_into(self, columns: &mut HashMap<ComponentId, Box<dyn Column>>);
}

fn push<T: Component>(columns: &mut HashMap<ComponentId, Box<dyn Column>>, value: T) {
    let column = columns
        .entry(T::ID)
        .or_insert_with(|| Box::new(Vec::<T>::new()));
    let typed = column
        .as_any_mut()
        .downcast_mut::<Vec<T>>()
        .expect("组件 ID 与列类型不一致");
    typed.push(value);
}

impl Bundle for () {
    fn component_ids() -> Vec<ComponentId> {
        Vec::new()
    }

    fn insert_into(self, _columns: &mut HashMap<ComponentId, Box<dyn Column>>) {}
}

impl<T: Component> Bundle for (T,) {
    fn component_ids() -> Vec<ComponentId> {
        vec![T::ID]
    }

    fn insert_into(self, columns: &mut HashMap<ComponentId, Box<dyn Column>>) {
        push(columns, self.0);
    }
}

impl<A: Component, B: Component> Bundle for (A, B) {
    fn component_ids() -> Vec<ComponentId> {
        vec![A::ID, B::ID]
    }

    fn insert_into(self, columns: &mut HashMap<ComponentId, Box<dyn Column>>) {
        push(columns, self.0);
        push(columns, self.1);
    }
}

impl<A: Component, B: Component, C: Component> Bundle for (A, B, C) {
    fn component_ids() -> Vec<ComponentId> {
        vec![A::ID, B::ID, C::ID]
    }

    fn insert_into(self, columns: &mut HashMap<ComponentId, Box<dyn Column>>) {
        push(columns, self.0);
        push(columns, self.1);
        push(columns, self.2);
    }
}

impl<A: Component, B: Component, C: Component, D: Component> Bundle for (A, B, C, D) {
    fn component_ids() -> Vec<ComponentId> {
        vec![A::ID, B::ID, C::ID, D::ID]
    }

    fn insert_into(self, columns: &mut HashMap<ComponentId, Box<dyn Column>>) {
        push(columns, self.0);
        push(columns, self.1);
        push(columns, self.2);
        push(columns, self.3);
    }
}

#[derive(Clone, Copy)]
pub(crate) struct EntityMeta {
    id: EntityId,
    generation: u32,
    alive: bool,
    archetype: u32,
    row: u32,
    revision: u64,
}

/// 世界：唯一写者，实体 / archetype / 列都在这里。
pub struct World {
    metas: Vec<EntityMeta>,
    free: Vec<u32>,
    by_id: HashMap<EntityId, EntityKey>,
    archetypes: Vec<Archetype>,
    archetype_index: HashMap<Vec<ComponentId>, u32>,
    factories: HashMap<ComponentId, ComponentFactory>,
    next_id: u64,
    revision: u64,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> Self {
        let mut world = Self {
            metas: Vec::new(),
            free: Vec::new(),
            by_id: HashMap::new(),
            archetypes: Vec::new(),
            archetype_index: HashMap::new(),
            factories: HashMap::new(),
            next_id: 0,
            revision: 0,
        };
        world.register_builtin_components();
        world
    }

    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> EntityKey {
        let mut ids = B::component_ids();
        ids.sort_by_key(|id| id.0);
        let unique = {
            let mut copy = ids.clone();
            copy.dedup();
            copy.len()
        };
        assert_eq!(unique, ids.len(), "bundle 含重复组件");

        let archetype = self.archetype_for(&ids);
        let key = self.alloc_key();
        let id = EntityId::from_raw(self.next_id);
        self.next_id += 1;
        self.by_id.insert(id, key);

        let arch = &mut self.archetypes[archetype as usize];
        bundle.insert_into(&mut arch.columns);
        arch.entities.push(key);
        let row = (arch.entities.len() - 1) as u32;

        let meta = &mut self.metas[key.index() as usize];
        meta.id = id;
        meta.alive = true;
        meta.archetype = archetype;
        meta.row = row;
        self.revision += 1;
        meta.revision = self.revision;
        key
    }

    pub fn despawn(&mut self, key: EntityKey) -> bool {
        if self.despawn_raw(key) {
            self.bump_revision();
            true
        } else {
            false
        }
    }

    pub(crate) fn despawn_raw(&mut self, key: EntityKey) -> bool {
        let index = key.index() as usize;
        let Some(meta) = self.metas.get(index).copied() else {
            return false;
        };
        if !meta.alive || meta.generation != key.generation() {
            return false;
        }

        let row = meta.row as usize;
        let moved = self.archetypes[meta.archetype as usize].swap_remove_row(row);
        if let Some(moved_key) = moved {
            self.metas[moved_key.index() as usize].row = row as u32;
        }
        self.metas[index].alive = false;
        self.by_id.remove(&meta.id);
        self.free.push(index as u32);
        true
    }

    pub fn get<T: Component>(&self, key: EntityKey) -> Option<&T> {
        let meta = self.meta(key)?;
        let arch = self.archetypes.get(meta.archetype as usize)?;
        let column = arch.columns.get(&T::ID)?;
        let values = column.as_any().downcast_ref::<Vec<T>>()?;
        values.get(meta.row as usize)
    }

    pub fn get_mut<T: Component>(&mut self, key: EntityKey) -> Option<&mut T> {
        let meta = self.meta(key)?;
        let arch = self.archetypes.get_mut(meta.archetype as usize)?;
        let column = arch.columns.get_mut(&T::ID)?;
        let values = column.as_any_mut().downcast_mut::<Vec<T>>()?;
        values.get_mut(meta.row as usize)
    }

    pub fn has<T: Component>(&self, key: EntityKey) -> bool {
        self.get::<T>(key).is_some()
    }

    /// 单组件不可变遍历。只访问包含 `T` 的 archetype。
    pub fn for_each<T: Component>(&self, mut f: impl FnMut(EntityKey, &T)) {
        for arch in &self.archetypes {
            let Some(column) = arch.columns.get(&T::ID) else {
                continue;
            };
            let Some(values) = column.as_any().downcast_ref::<Vec<T>>() else {
                continue;
            };
            for (key, value) in arch.entities.iter().copied().zip(values.iter()) {
                f(key, value);
            }
        }
    }

    /// 单组件可变遍历。借用被拆成 `entities` / `columns` 两个字段，不越界。
    pub fn for_each_mut<T: Component>(&mut self, mut f: impl FnMut(EntityKey, &mut T)) {
        for arch in &mut self.archetypes {
            let Archetype {
                entities, columns, ..
            } = arch;
            let Some(column) = columns.get_mut(&T::ID) else {
                continue;
            };
            let Some(values) = column.as_any_mut().downcast_mut::<Vec<T>>() else {
                continue;
            };
            for (key, value) in entities.iter().copied().zip(values.iter_mut()) {
                f(key, value);
            }
        }
    }

    pub fn count<T: Component>(&self) -> usize {
        self.archetypes
            .iter()
            .filter_map(|arch| arch.columns.get(&T::ID))
            .map(|column| column.len())
            .sum()
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn is_alive(&self, key: EntityKey) -> bool {
        self.meta(key).is_some()
    }

    pub fn entity(&self, key: EntityKey) -> Option<EntityId> {
        self.meta(key).map(|meta| meta.id)
    }

    pub fn key(&self, id: EntityId) -> Option<EntityKey> {
        self.by_id.get(&id).copied()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn archetype_count(&self) -> usize {
        self.archetypes.len()
    }

    pub fn get_field(&self, id: EntityId, component: ComponentId, field: &str) -> Option<Value> {
        let (_, meta) = self.meta_by_id(id)?;
        let arch = self.archetypes.get(meta.archetype as usize)?;
        arch.columns
            .get(&component)?
            .get_field(meta.row as usize, field)
    }

    /// 单字段写入：结构不变，revision +1，实体 revision 跟着更新。
    pub fn set_field(
        &mut self,
        id: EntityId,
        component: ComponentId,
        field: &str,
        value: &Value,
    ) -> Result<(), String> {
        self.set_field_raw(id, component, field, value)?;
        let revision = self.bump_revision();
        self.touch(id, revision);
        Ok(())
    }

    pub fn entity_revision(&self, id: EntityId) -> Option<u64> {
        self.meta_by_id(id).map(|(_, meta)| meta.revision)
    }

    pub(crate) fn set_field_raw(
        &mut self,
        id: EntityId,
        component: ComponentId,
        field: &str,
        value: &Value,
    ) -> Result<Value, String> {
        let (_, meta) = self
            .meta_by_id(id)
            .ok_or_else(|| format!("实体不存在：{id}"))?;
        let arch = self
            .archetypes
            .get_mut(meta.archetype as usize)
            .ok_or_else(|| "archetype 不存在".to_string())?;
        let column = arch
            .columns
            .get_mut(&component)
            .ok_or_else(|| format!("实体 {id} 没有组件 {component}"))?;
        let row = meta.row as usize;
        let before = column
            .get_field(row, field)
            .ok_or_else(|| format!("组件 {component} 没有字段 {field}"))?;
        column.set_field(row, field, value)?;
        Ok(before)
    }

    pub(crate) fn meta_by_id(&self, id: EntityId) -> Option<(usize, EntityMeta)> {
        let key = *self.by_id.get(&id)?;
        let index = key.index() as usize;
        let meta = *self.metas.get(index)?;
        if meta.alive && meta.generation == key.generation() {
            Some((index, meta))
        } else {
            None
        }
    }

    pub(crate) fn bump_revision(&mut self) -> u64 {
        self.revision += 1;
        self.revision
    }

    pub(crate) fn touch(&mut self, id: EntityId, revision: u64) {
        if let Some((index, _)) = self.meta_by_id(id) {
            self.metas[index].revision = revision;
        }
    }

    fn meta(&self, key: EntityKey) -> Option<EntityMeta> {
        let meta = self.metas.get(key.index() as usize).copied()?;
        if meta.alive && meta.generation == key.generation() {
            Some(meta)
        } else {
            None
        }
    }

    fn alloc_key(&mut self) -> EntityKey {
        if let Some(index) = self.free.pop() {
            let meta = &mut self.metas[index as usize];
            meta.generation = meta.generation.wrapping_add(1);
            meta.alive = true;
            EntityKey::new(index, meta.generation)
        } else {
            let index = self.metas.len() as u32;
            self.metas.push(EntityMeta {
                id: EntityId::from_raw(0),
                generation: 0,
                alive: true,
                archetype: 0,
                row: 0,
                revision: 0,
            });
            EntityKey::new(index, 0)
        }
    }

    fn archetype_for(&mut self, ids: &[ComponentId]) -> u32 {
        if let Some(&index) = self.archetype_index.get(ids) {
            return index;
        }
        let index = self.archetypes.len() as u32;
        self.archetypes.push(Archetype::new(ids.to_vec()));
        self.archetype_index.insert(ids.to_vec(), index);
        index
    }
}
