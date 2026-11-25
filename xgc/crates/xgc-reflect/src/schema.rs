use std::collections::HashSet;

use serde::Serialize;
use xgc_base::{Diagnostic, Severity, Subject};

use super::value::struct_fits;
use super::{FieldDescriptor, FieldType, Value};

/// 可序列化的字段 schema。
#[derive(Clone, Debug, Serialize)]
pub struct FieldSchema {
    pub name: String,
    pub ty: String,
    pub default: Value,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    pub doc: String,
    /// 元素节点的默认值供列表插入使用，即使列表本身缺省为空。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element: Option<Box<FieldSchema>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldSchema>,
}

impl FieldSchema {
    /// 校验整个值的类型和结构，数值范围仍由调用方判断。
    pub fn fits(&self, value: &Value) -> bool {
        match (self.ty.as_str(), value) {
            ("list", Value::List(values)) => self
                .element
                .as_ref()
                .is_some_and(|element| values.iter().all(|value| element.fits(value))),
            ("struct", Value::Struct(values)) => {
                struct_fits(values, &self.fields, |field| &field.name, FieldSchema::fits)
            }
            (ty, value) => FieldType::parse(ty).is_some_and(|ty| value.fits(&ty)),
        }
    }
}

impl From<FieldDescriptor> for FieldSchema {
    fn from(field: FieldDescriptor) -> Self {
        let (min, max) = field
            .range
            .map_or((None, None), |(min, max)| (Some(min), Some(max)));
        let ty = field.ty.as_str().to_string();
        let (element, fields) = match field.ty {
            FieldType::List(element) => (Some(Box::new(Self::from(*element))), Vec::new()),
            FieldType::Struct(fields) => (None, fields.into_iter().map(Self::from).collect()),
            _ => (None, Vec::new()),
        };
        Self {
            name: field.name.to_string(),
            ty,
            default: field.default,
            required: true,
            unit: field.unit.map(str::to_string),
            min,
            max,
            doc: field.doc.to_string(),
            element,
            fields,
        }
    }
}

pub(super) fn validate_fields(
    fields: &[FieldSchema],
    parent: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut names = HashSet::new();
    for field in fields {
        let path = format!("{parent}.{}", field.name);
        if !names.insert(field.name.as_str()) {
            diagnostics.push(Diagnostic::new(
                "SCHEMA.DUP_FIELD",
                Severity::Error,
                Subject::Field(path.clone()),
                "字段名重复",
            ));
        }
        validate_field(field, &path, diagnostics);
    }
}

fn validate_field(field: &FieldSchema, path: &str, diagnostics: &mut Vec<Diagnostic>) {
    if field.name.is_empty() {
        diagnostics.push(Diagnostic::new(
            "SCHEMA.EMPTY_FIELD",
            Severity::Error,
            Subject::Field(path.to_string()),
            "字段名为空",
        ));
    }
    if !field.fits(&field.default) {
        diagnostics.push(Diagnostic::new(
            "SCHEMA.DEFAULT_TYPE",
            Severity::Error,
            Subject::Field(path.to_string()),
            format!(
                "默认值类型 {} 与字段类型 {} 不一致",
                field.default.type_name(),
                field.ty
            ),
        ));
    }
    if let Some(element) = &field.element {
        validate_field(element, &format!("{path}[]"), diagnostics);
    }
    validate_fields(&field.fields, path, diagnostics);
}
