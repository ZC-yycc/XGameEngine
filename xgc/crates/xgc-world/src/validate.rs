//! 世界数据体检：组件 / 字段 / 值类型 / 重复项，全部走注册表这一份 schema。

use std::collections::HashSet;

use xgc_base::diagnostics as codes;
use xgc_base::{Diagnostic, Severity, Subject};
use xgc_reflect::{FieldType, Registry};

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
                    let Some(ty) = FieldType::parse(&field.ty) else {
                        diagnostics.push(field_diag(
                            codes::WORLD_UNKNOWN_TYPE,
                            &schema.name,
                            name,
                            format!("字段 {}.{name} 声明了未知类型 {}", schema.name, field.ty),
                        ));
                        continue;
                    };
                    if !value.fits(ty) {
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
                    if let Some(number) = numeric(value) {
                        if field.min.is_some_and(|min| number < min)
                            || field.max.is_some_and(|max| number > max)
                        {
                            diagnostics.push(field_diag(
                                codes::WORLD_RANGE,
                                &schema.name,
                                name,
                                format!(
                                    "字段 {}.{name} 超出范围 [{:?}, {:?}]：{number}",
                                    schema.name, field.min, field.max
                                ),
                            ));
                        }
                    }
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

fn numeric(value: &xgc_reflect::Value) -> Option<f64> {
    match value {
        xgc_reflect::Value::I64(value) => Some(*value as f64),
        xgc_reflect::Value::F64(value) => Some(*value),
        _ => None,
    }
}
