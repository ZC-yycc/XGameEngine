//! XGame Core 的世界：archetype 列式存储 + 生成 / 回收 + 查询。
#![deny(unsafe_code)]
use std::any::Any;
use std::collections::HashMap;
use xgc_base::{EntityId, EntityKey};
mod animation_components;
mod animation_events;
mod character_components;
mod collision;
mod command;
mod components;
mod data;
mod environment;
mod field_edit;
mod gameplay_components;
mod hash;
mod journal;
mod layer_components;
mod lod_components;
mod particle_components;
mod parts;
mod physics_components;
mod plugin_components;
mod prefab_components;
mod prefab_ops;
mod query;
mod registry;
mod render_components;
mod root_motion;
mod skeleton_components;
mod slope_components;
mod snapshot;
mod state_machine_components;
mod ui_components;
mod validate;
pub use animation_components::Animator;
pub use animation_events::{AnimEvent, FiredAnimEvent};
pub use character_components::{
    CharacterController, HEIGHT_DEFAULT, HEIGHT_MAX, RADIUS_DEFAULT, RADIUS_MAX, RADIUS_MIN,
    SKIN_WIDTH_DEFAULT, SKIN_WIDTH_MAX, SKIN_WIDTH_MIN, SLOPE_LIMIT_DEFAULT, SLOPE_LIMIT_MAX,
    SNAP_TO_GROUND_DEFAULT, SNAP_TO_GROUND_MAX, STEP_OFFSET_DEFAULT,
};
pub use collision::CollisionEvent;
pub use command::{Applied, Command, Plan};
pub use components::{GlobalTransform, Name, Parent, Transform, Velocity};
use data::ComponentFactory;
pub use data::{ComponentData, EntityData, WorldData};
pub use environment::{LightKind, SceneEnvironment, SceneLight};
pub use gameplay_components::{AudioGain, AudioListener, AudioMixer, AudioSource, Script};
pub use journal::{Journal, TxRecord};
pub use layer_components::{AnimLayer, AnimationLayers, LayerMode};
pub use lod_components::{LodGroup, LodLevel};
pub use particle_components::{
    EmissionShape, ParticleBlend, ParticleConfig, ParticleEmitter, ParticleQuad,
    MAX_PARTICLES_LIMIT,
};
pub use parts::WriteSet;
pub use physics_components::{BodyKind, PhysicsBody};
pub use plugin_components::PluginComponent;
pub use prefab_components::PrefabInstance;
pub use prefab_ops::{instantiate_prefab, remap_parent, revert_prefab};
pub use registry::builtin_registry;
pub use render_components::{Camera, Material, MeshRenderer};
pub use root_motion::{
    filter_axes, root_motion_delta, to_local_position, to_world_delta, MotionFade, MotionSpan,
    RootMotion, RootMotionClip, RootMotionLibrary, RootMotionNodeTrack, RootMotionSet,
    RootMotionTrack, ROOT_MOTION_AXES_DEFAULT,
};
pub use skeleton_components::{Bone, Skeleton, SkeletonPose};
pub use slope_components::Slope;
pub use snapshot::{EntitySnapshot, WorldSnapshot};
pub use state_machine_components::{
    AnimationState, StateDef, StateMachine, TransitionCondition, TransitionDef,
};
pub use ui_components::{UiElement, UiInput, UiInteraction, UiLayout};
pub use xgc_reflect::{AssetKind, Component, ComponentId, FieldPath, Value};
/// 列：任意组件的连续存储；crate 内部接口，公开但不承诺稳定性。
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
    fn hash_field_into(&self, row: usize, field: &str, hash: &mut u64);
    fn set_field(&mut self, row: usize, field: &str, value: &Value) -> Result<(), String>;
    fn validate_field(&self, row: usize, field: &str, value: &Value) -> Result<(), String>;
    fn field_names(&self) -> Vec<&'static str>;
    /// 只给 `state_hash` 用的字段名：作者数据字段可以在这里排除。
    fn state_field_names(&self) -> Vec<&'static str>;
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

    fn hash_field_into(&self, row: usize, field: &str, hash: &mut u64) {
        if let Some(item) = self.get(row) {
            item.hash_field_into(field, hash);
        }
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

    fn state_field_names(&self) -> Vec<&'static str> {
        T::state_fields()
            .into_iter()
            .map(|field| field.name)
            .collect()
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
    /// 哈希用的字段布局缓存：`columns` 是 `HashMap`（迭代序不定），而 `state_hash`
    /// 必须按组件 ID 升序、字段声明序逐项混入才能保证同输入同输出。每帧为每个实体
    /// 重排一次会让稳态帧分配爆炸（实测 60 帧 10 万次），所以按 archetype 只算一次。
    hash_layout: std::sync::OnceLock<Vec<(ComponentId, Vec<&'static str>)>>,
}

impl Archetype {
    fn new(ids: Vec<ComponentId>) -> Self {
        Self {
            entities: Vec::new(),
            columns: HashMap::with_capacity(ids.len()),
            hash_layout: std::sync::OnceLock::new(),
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
    /// 场景级环境（光 / 环境光）：不是实体，跟着世界一起存取。
    environment: SceneEnvironment,
    /// 本帧碰撞事件（瞬态；不进 `WorldData` / `state_hash` / 快照）。
    collision_events: Vec<CollisionEvent>,
    /// 本步命中的动画事件（瞬态；`step_with` 每步开头清空）。
    animation_events: Vec<FiredAnimEvent>,
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
            environment: SceneEnvironment::default(),
            collision_events: Vec::new(),
            animation_events: Vec::new(),
        };
        world.register_builtin_components();
        world
    }

    /// 换环境不推进 revision：它是场景配置，不是实体状态，`state_hash` 也不含它。
    pub fn set_environment(&mut self, environment: SceneEnvironment) {
        self.environment = environment;
    }

    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> EntityKey {
        let mut ids = B::component_ids();
        ids.sort_by_key(|id| id.0);
        let mut unique = ids.clone();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "bundle 含重复组件");

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

    /// 预留一段连续实体 ID（预制体实例化用），返回起始值。
    pub fn reserve_entity_ids(&mut self, count: u64) -> u64 {
        let start = self.next_id;
        self.next_id = self.next_id.saturating_add(count);
        start
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

    pub fn entity_revision(&self, id: EntityId) -> Option<u64> {
        self.meta_by_id(id).map(|(_, meta)| meta.revision)
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
