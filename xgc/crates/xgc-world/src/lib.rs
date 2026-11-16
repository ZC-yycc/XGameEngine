//! XGame Core 的世界：archetype 列式存储 + 生成 / 回收 + 查询。
//!
//! M0 刻意只做**安全**版本：列用 `Vec<T>` 装箱成 trait object，查询时按组件
//! 类型 downcast。它已经不是 `Vec<Entity>` 线性扫描：同一组件的值连续存放，
//! 查询只走包含该组件的 archetype。M1 再把列索引静态化、加命令缓冲与变更位图。

#![deny(unsafe_code)]

use std::any::Any;
use std::collections::HashMap;

use xgc_base::{EntityId, EntityKey};
use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

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
struct EntityMeta {
    id: EntityId,
    generation: u32,
    alive: bool,
    archetype: u32,
    row: u32,
}

/// 世界：唯一写者，实体 / archetype / 列都在这里。
pub struct World {
    metas: Vec<EntityMeta>,
    free: Vec<u32>,
    by_id: HashMap<EntityId, EntityKey>,
    archetypes: Vec<Archetype>,
    archetype_index: HashMap<Vec<ComponentId>, u32>,
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
        Self {
            metas: Vec::new(),
            free: Vec::new(),
            by_id: HashMap::new(),
            archetypes: Vec::new(),
            archetype_index: HashMap::new(),
            next_id: 0,
            revision: 0,
        }
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
        key
    }

    pub fn despawn(&mut self, key: EntityKey) -> bool {
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
        self.revision += 1;
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

/// 内建：局部变换。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

impl Component for Transform {
    const ID: ComponentId = ComponentId(0x0101);
    const NAME: &'static str = "core.transform";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "局部变换：相对父节点的平移 / 旋转 / 缩放"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("position", FieldType::Vec3, Value::Vec3([0.0; 3]))
                .unit("m")
                .doc("相对父节点的平移"),
            FieldDescriptor::new(
                "rotation",
                FieldType::Quat,
                Value::Quat([0.0, 0.0, 0.0, 1.0]),
            )
            .doc("单位四元数"),
            FieldDescriptor::new("scale", FieldType::Vec3, Value::Vec3([1.0; 3]))
                .unit("x")
                .doc("逐轴缩放"),
        ]
    }
}

/// 内建：速度（M0 测试与后续系统共用）。
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Velocity {
    pub linear: [f32; 3],
}

impl Component for Velocity {
    const ID: ComponentId = ComponentId(0x0102);
    const NAME: &'static str = "core.velocity";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "线速度"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("linear", FieldType::Vec3, Value::Vec3([0.0; 3]))
                .unit("m/s")
                .doc("世界空间线速度"),
        ]
    }
}

/// 内建：名字（验证 String 组件的 clone / drop 与 schema）。
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Name {
    pub value: String,
}

impl Component for Name {
    const ID: ComponentId = ComponentId(0x0103);
    const NAME: &'static str = "core.name";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "显示名；不是身份，身份只认 EntityId"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("value", FieldType::String, Value::String(String::new()))
                .doc("显示名"),
        ]
    }
}
