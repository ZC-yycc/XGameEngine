//! 世界字段读写共用一条路径。组件只负责根字段与其 Rust 值的投影。

use xgc_base::EntityId;
use xgc_reflect::{ComponentId, FieldPath, Value};

use crate::{Column, World};

pub(crate) enum FieldEditError {
    Path(String),
    Value(String),
}

impl FieldEditError {
    pub(crate) fn message(self) -> String {
        match self {
            Self::Path(message) | Self::Value(message) => message,
        }
    }
}

pub(crate) fn is_nested(field: &str) -> bool {
    field.contains(['.', '[', ']'])
}

impl dyn Column {
    pub(crate) fn read_field(&self, row: usize, field: &str) -> Option<Value> {
        if !is_nested(field) {
            return self.get_field(row, field);
        }
        let path = FieldPath::parse(field).ok()?;
        let value = self.get_field(row, path.root())?;
        path.read(&value).ok().cloned()
    }

    pub(crate) fn write_field(
        &mut self,
        row: usize,
        field: &str,
        value: &Value,
    ) -> Result<Value, FieldEditError> {
        if !is_nested(field) {
            let before = self
                .get_field(row, field)
                .ok_or_else(|| FieldEditError::Path(format!("没有字段 {field}")))?;
            self.set_field(row, field, value)
                .map_err(FieldEditError::Value)?;
            return Ok(before);
        }
        let path = FieldPath::parse(field).map_err(FieldEditError::Path)?;
        let mut root = self
            .get_field(row, path.root())
            .ok_or_else(|| FieldEditError::Path(format!("没有字段 {}", path.root())))?;
        let before = path
            .replace(&mut root, value.clone())
            .map_err(FieldEditError::Path)?;
        self.set_field(row, path.root(), &root)
            .map_err(FieldEditError::Value)?;
        Ok(before)
    }
}

impl World {
    pub fn get_field(&self, id: EntityId, component: ComponentId, field: &str) -> Option<Value> {
        let (_, meta) = self.meta_by_id(id)?;
        self.archetypes
            .get(meta.archetype as usize)?
            .columns
            .get(&component)?
            .read_field(meta.row as usize, field)
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
        let column = self.archetypes[meta.archetype as usize]
            .columns
            .get_mut(&component)
            .ok_or_else(|| format!("实体 {id} 没有组件 {component}"))?;
        column
            .write_field(meta.row as usize, field, value)
            .map_err(FieldEditError::message)
    }
}
