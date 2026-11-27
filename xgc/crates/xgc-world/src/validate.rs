//! 世界数据体检：组件 / 字段 / 值类型 / 重复项，全部走注册表这一份 schema。

use std::collections::HashSet;

use xgc_base::diagnostics as codes;
use xgc_base::{Diagnostic, Severity, Subject};
use xgc_reflect::{FieldPath, FieldSchema, FieldType, Registry, Value};

use super::{World, WorldData};

impl World {
    pub fn validate(&self, registry: &Registry) -> Vec<Diagnostic> {
        Self::validate_data(&self.world_data(), registry)
    }

    /// 不依赖活动世界，直接体检可序列化数据；CLI / Agent / 迁移都走这里。
    pub fn validate_data(data: &WorldData, registry: &Registry) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        let mut entity_ids = HashSet::new();

        for entity in &data.entities {
            if let Err(error) = entity.id.validate() {
                diagnostics.push(diag(
                    codes::WORLD_INVALID_ENTITY_ID,
                    entity.id,
                    error.to_string(),
                ));
            }
            if !entity_ids.insert(entity.id) {
                diagnostics.push(diag(
                    codes::WORLD_DUP_ENTITY,
                    entity.id,
                    format!("实体 ID 重复：{}", entity.id),
                ));
            }

            let mut components = HashSet::new();
            for component in &entity.components {
                if !components.insert(component.component) {
                    diagnostics.push(diag(
                        codes::WORLD_DUP_COMPONENT,
                        entity.id,
                        format!("实体 {} 重复携带组件 {}", entity.id, component.component),
                    ));
                }

                let Some(schema) = registry.get(component.component) else {
                    diagnostics.push(diag(
                        codes::WORLD_UNKNOWN_COMPONENT,
                        entity.id,
                        format!("实体 {} 携带未注册组件 {}", entity.id, component.component),
                    ));
                    continue;
                };

                let mut fields = HashSet::new();
                for (name, value) in &component.fields {
                    if !fields.insert(name.as_str()) {
                        diagnostics.push(field_diag(
                            codes::WORLD_DUP_FIELD,
                            &schema.name,
                            name,
                            format!("组件 {} 重复字段 {name}", schema.name),
                        ));
                        continue;
                    }
                    let Some(field) = schema.fields.iter().find(|field| field.name == *name) else {
                        diagnostics.push(field_diag(
                            codes::WORLD_UNKNOWN_FIELD,
                            &schema.name,
                            name,
                            format!("组件 {} 没有字段 {name}", schema.name),
                        ));
                        continue;
                    };
                    if !matches!(field.ty.as_str(), "list" | "struct" | "map" | "enum")
                        && FieldType::parse(&field.ty).is_none()
                    {
                        diagnostics.push(field_diag(
                            codes::WORLD_UNKNOWN_TYPE,
                            &schema.name,
                            name,
                            format!("字段 {}.{name} 声明了未知类型 {}", schema.name, field.ty),
                        ));
                        continue;
                    }
                    if !field.fits(value) {
                        diagnostics.push(field_diag(
                            codes::WORLD_TYPE_MISMATCH,
                            &schema.name,
                            name,
                            format!(
                                "字段 {}.{name} 期望 {}，实际 {}",
                                schema.name,
                                field.ty,
                                value.kind()
                            ),
                        ));
                        continue;
                    }
                    validate_ranges(field, value, &schema.name, name, &mut diagnostics);
                }
            }
        }

        diagnostics
    }
}

fn diag(code: &'static str, entity: xgc_base::EntityId, message: String) -> Diagnostic {
    Diagnostic::new(code, Severity::Error, Subject::Entity(entity), message)
}

fn field_diag(code: &'static str, component: &str, field: &str, message: String) -> Diagnostic {
    Diagnostic::new(
        code,
        Severity::Error,
        Subject::Field(format!("{component}.{field}")),
        message,
    )
}

fn validate_ranges(
    field: &FieldSchema,
    value: &Value,
    component: &str,
    path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let number = match value {
        Value::List(values) => {
            if let Some(element) = &field.element {
                for (index, item) in values.iter().enumerate() {
                    validate_ranges(
                        element,
                        item,
                        component,
                        &format!("{path}[{index}]"),
                        diagnostics,
                    );
                }
            }
            return;
        }
        Value::Struct(values) => {
            for child in &field.fields {
                if let Some(item) = values.get(&child.name) {
                    validate_ranges(
                        child,
                        item,
                        component,
                        &format!("{path}.{}", child.name),
                        diagnostics,
                    );
                }
            }
            return;
        }
        Value::Map(values) => {
            if let Some(element) = &field.element {
                for (key, item) in values {
                    validate_ranges(
                        element,
                        item,
                        component,
                        &format!("{path}{}", FieldPath::key_segment(key)),
                        diagnostics,
                    );
                }
            }
            return;
        }
        Value::Enum { variant, payload } => {
            if let Some(branch) = field.variant(variant) {
                validate_ranges(
                    branch,
                    payload,
                    component,
                    &format!("{path}.{variant}"),
                    diagnostics,
                );
            }
            return;
        }
        Value::I64(value) => *value as f64,
        Value::F64(value) => *value,
        _ => return,
    };
    if field
        .min
        .is_some_and(|min| !number.is_finite() || number < min)
        || field
            .max
            .is_some_and(|max| !number.is_finite() || number > max)
    {
        diagnostics.push(field_diag(
            codes::WORLD_RANGE,
            component,
            path,
            format!(
                "字段 {component}.{path} 超出范围 [{:?}, {:?}]：{number}",
                field.min, field.max
            ),
        ));
    }
}
