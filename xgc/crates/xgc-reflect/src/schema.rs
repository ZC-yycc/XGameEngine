use std::collections::HashSet;

use serde::Serialize;
use xgc_base::{Diagnostic, Severity, Subject};

use super::value::struct_fits;
use super::{ComponentSchema, FieldDescriptor, FieldPath, FieldType, Value};

/// 资产字段的用途由声明处标记，检查、迁移和打包共用同一目录。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Model,
    Material,
    Script,
    Audio,
    Image,
    Prefab,
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset: Option<AssetKind>,
    pub doc: String,
    /// 列表元素或映射值的描述，其默认值供插入使用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element: Option<Box<FieldSchema>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldSchema>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<FieldSchema>,
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
            ("map", Value::Map(values)) => self
                .element
                .as_ref()
                .is_some_and(|element| values.values().all(|value| element.fits(value))),
            ("enum", Value::Enum { variant, payload }) => self
                .variant(variant)
                .is_some_and(|branch| branch.fits(payload)),
            (ty, value) => FieldType::parse(ty).is_some_and(|ty| value.fits(&ty)),
        }
    }

    pub fn variant(&self, name: &str) -> Option<&Self> {
        self.variants.iter().find(|branch| branch.name == name)
    }

    fn visit_asset_references(
        &self,
        value: &Value,
        path: &str,
        visit: &mut impl FnMut(AssetKind, &str, &str),
    ) {
        match value {
            Value::String(reference) => {
                if let Some(kind) = self.asset {
                    visit(kind, path, reference);
                }
            }
            Value::List(values) => {
                if let Some(element) = &self.element {
                    for (index, value) in values.iter().enumerate() {
                        element.visit_asset_references(value, &format!("{path}[{index}]"), visit);
                    }
                }
            }
            Value::Struct(values) => {
                for field in &self.fields {
                    if let Some(value) = values.get(&field.name) {
                        field.visit_asset_references(
                            value,
                            &format!("{path}.{}", field.name),
                            visit,
                        );
                    }
                }
            }
            Value::Map(values) => {
                if let Some(element) = &self.element {
                    for (key, value) in values {
                        let segment = FieldPath::key_segment(key);
                        element.visit_asset_references(value, &format!("{path}{segment}"), visit);
                    }
                }
            }
            Value::Enum { variant, payload } => {
                if let Some(branch) = self.variant(variant) {
                    branch.visit_asset_references(payload, &format!("{path}.{variant}"), visit);
                }
            }
            _ => {}
        }
    }
}

impl ComponentSchema {
    /// 只遍历数据中实际出现的资产字段。空串由消费方按字段用途处理。
    pub fn visit_asset_references(
        &self,
        values: &[(String, Value)],
        mut visit: impl FnMut(AssetKind, &str, &str),
    ) {
        for field in &self.fields {
            if let Some((_, value)) = values.iter().find(|(name, _)| *name == field.name) {
                field.visit_asset_references(value, &field.name, &mut visit);
            }
        }
    }
}

impl From<FieldDescriptor> for FieldSchema {
    fn from(field: FieldDescriptor) -> Self {
        let (min, max) = field
            .range
            .map_or((None, None), |(min, max)| (Some(min), Some(max)));
        let ty = field.ty.as_str().to_string();
        let (element, fields, variants) = match field.ty {
            FieldType::List(element) | FieldType::Map(element) => {
                (Some(Box::new(Self::from(*element))), Vec::new(), Vec::new())
            }
            FieldType::Struct(fields) => (
                None,
                fields.into_iter().map(Self::from).collect(),
                Vec::new(),
            ),
            FieldType::Enum(variants) => (
                None,
                Vec::new(),
                variants.into_iter().map(Self::from).collect(),
            ),
            _ => (None, Vec::new(), Vec::new()),
        };
        Self {
            name: field.name.to_string(),
            ty,
            default: field.default,
            required: true,
            unit: field.unit.map(str::to_string),
            min,
            max,
            step: field.step,
            read_only: field.read_only,
            group: field.group.map(str::to_string),
            asset: field.asset,
            doc: field.doc.to_string(),
            element,
            fields,
            variants,
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

fn validate_edit_metadata(field: &FieldSchema, path: &str, diagnostics: &mut Vec<Diagnostic>) {
    let scalar = matches!(field.ty.as_str(), "i64" | "u32" | "f32" | "f64");
    let draggable = scalar || matches!(field.ty.as_str(), "vec2" | "vec3" | "vec4" | "quat");
    let error = |code, message| {
        Diagnostic::new(
            code,
            Severity::Error,
            Subject::Field(path.to_string()),
            message,
        )
    };
    if field
        .step
        .is_some_and(|step| !draggable || !step.is_finite() || step <= 0.0)
    {
        diagnostics.push(error("SCHEMA.INVALID_STEP", "步长必须是数值控件的正有限数"));
    }
    if let (Some(min), Some(max)) = (field.min, field.max) {
        if !scalar || !min.is_finite() || !max.is_finite() || min > max {
            diagnostics.push(error(
                "SCHEMA.INVALID_RANGE",
                "范围必须是有序的有限标量边界",
            ));
        }
    }
    if field
        .group
        .as_deref()
        .is_some_and(|group| group.trim().is_empty())
    {
        diagnostics.push(error("SCHEMA.EMPTY_GROUP", "字段分组名不能为空"));
    }
    if field.asset.is_some() && field.ty != "string" {
        diagnostics.push(error("SCHEMA.INVALID_ASSET", "资产引用必须是字符串字段"));
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
    validate_edit_metadata(field, path, diagnostics);
    if let Some(element) = &field.element {
        validate_field(element, &format!("{path}[]"), diagnostics);
    }
    validate_fields(&field.fields, path, diagnostics);
    validate_fields(&field.variants, path, diagnostics);
    for branch in &field.variants {
        if !branch.name.is_empty()
            && !super::FieldPath::parse(&branch.name).is_ok_and(|path| path.is_root())
        {
            diagnostics.push(Diagnostic::new(
                "SCHEMA.INVALID_VARIANT",
                Severity::Error,
                Subject::Field(format!("{path}.{}", branch.name)),
                "分支名不能用于字段路径",
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_metadata_on_nested_fields() {
        let number = FieldDescriptor::new("number", FieldType::F64, Value::F64(0.0))
            .range(2.0, 1.0)
            .step(f64::NAN)
            .group(" ");
        let list = FieldDescriptor::new(
            "items",
            FieldType::List(Box::new(number)),
            Value::List(vec![]),
        );
        let mut diagnostics = Vec::new();
        validate_fields(&[FieldSchema::from(list)], "test", &mut diagnostics);
        assert_eq!(
            diagnostics.iter().map(|item| item.code).collect::<Vec<_>>(),
            [
                "SCHEMA.INVALID_STEP",
                "SCHEMA.INVALID_RANGE",
                "SCHEMA.EMPTY_GROUP",
            ]
        );
    }
}
