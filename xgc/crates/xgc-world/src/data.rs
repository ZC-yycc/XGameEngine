//! 可序列化的实体数据与组件工厂。
//!
//! `EntityData` 是命令 / Agent 协议里的稳定形态：组件 ID + 字段值。反序列化后
//! 由 [`ComponentFactory`] 构造真实组件列；这样 `Command` 可以序列化，而不需要
//! 把类型擦除列写进 JSON。

use std::any::Any;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use xgc_base::EntityId;
use xgc_reflect::{Component, ComponentId, Value};

use super::{Column, EntitySnapshot, Name, Transform, Velocity, World};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityData {
    pub id: EntityId,
    pub components: Vec<ComponentData>,
}

impl EntityData {
    pub fn new(id: EntityId) -> Self {
        Self {
            id,
            components: Vec::new(),
        }
    }

    pub fn with_id(mut self, id: EntityId) -> Self {
        self.id = id;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComponentData {
    pub component: ComponentId,
    pub fields: Vec<(String, Value)>,
}

type CreateFn = fn(&[(String, Value)]) -> Result<Box<dyn Any>, String>;
type EmptyColumnFn = fn() -> Box<dyn Column>;
type PushAnyFn = fn(&mut Box<dyn Column>, Box<dyn Any>) -> Result<(), String>;

pub(crate) struct ComponentFactory {
    create: CreateFn,
    empty_column: EmptyColumnFn,
    push_any: PushAnyFn,
}

impl ComponentFactory {
    fn new<T: Component>() -> Self {
        Self {
            create: create_component::<T>,
            empty_column: empty_column::<T>,
            push_any: push_component::<T>,
        }
    }

    fn create(&self, fields: &[(String, Value)]) -> Result<Box<dyn Any>, String> {
        (self.create)(fields)
    }

    fn empty_column(&self) -> Box<dyn Column> {
        (self.empty_column)()
    }

    fn push_any(&self, column: &mut Box<dyn Column>, value: Box<dyn Any>) -> Result<(), String> {
        (self.push_any)(column, value)
    }
}

fn create_component<T: Component>(fields: &[(String, Value)]) -> Result<Box<dyn Any>, String> {
    let mut value = T::default_value();
    for (field, field_value) in fields {
        value
            .set_field(field, field_value)
            .map_err(|message| format!("{}: {message}", T::NAME))?;
    }
    Ok(Box::new(value))
}

fn empty_column<T: Component>() -> Box<dyn Column> {
    Box::new(Vec::<T>::new())
}

fn push_component<T: Component>(
    column: &mut Box<dyn Column>,
    value: Box<dyn Any>,
) -> Result<(), String> {
    let value = value
        .downcast::<T>()
        .map_err(|_| format!("{} 组件类型不匹配", T::NAME))?;
    let values = column
        .as_any_mut()
        .downcast_mut::<Vec<T>>()
        .ok_or_else(|| format!("{} 列类型不匹配", T::NAME))?;
    values.push(*value);
    Ok(())
}

impl World {
    pub(crate) fn register_builtin_components(&mut self) {
        self.register_component::<Transform>();
        self.register_component::<Velocity>();
        self.register_component::<Name>();
    }

    pub fn register_component<T: Component>(&mut self) {
        self.factories.insert(T::ID, ComponentFactory::new::<T>());
    }

    /// 把实体转成可序列化的稳定形态。
    pub fn entity_data(&self, id: EntityId) -> Option<EntityData> {
        let snapshot = self.capture_entity(id)?;
        let mut components = Vec::with_capacity(snapshot.component_ids().len());
        for component in snapshot.component_ids() {
            let fields = snapshot.fields(component)?;
            components.push(ComponentData { component, fields });
        }
        Some(EntityData { id, components })
    }

    /// 把可序列化数据构造成一列一行的实体快照；组件 / 字段 / 类型都在这里校验。
    pub fn snapshot_from_data(&self, data: &EntityData) -> Result<EntitySnapshot, String> {
        let mut components = Vec::with_capacity(data.components.len());
        let mut seen = HashSet::new();

        for component in &data.components {
            if !seen.insert(component.component) {
                return Err(format!("组件重复：{}", component.component));
            }
            let mut field_names = HashSet::new();
            for (name, _) in &component.fields {
                if !field_names.insert(name.as_str()) {
                    return Err(format!("字段重复：{}.{}", component.component, name));
                }
            }

            let factory = self
                .factories
                .get(&component.component)
                .ok_or_else(|| format!("组件未注册：{}", component.component))?;
            let value = factory.create(&component.fields)?;
            let mut column = factory.empty_column();
            factory.push_any(&mut column, value)?;
            components.push((component.component, column));
        }

        components.sort_by_key(|(component, _)| component.0);
        Ok(EntitySnapshot::from_parts(data.id, 0, components))
    }
}
