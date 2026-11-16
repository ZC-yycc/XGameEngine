//! Agent 查询 IR 的字段路径、过滤器与标量比较。

use serde_json::Value as Json;
use xgc_reflect::ComponentId;
use xgc_world::{EntityData, Value};

#[derive(Clone, Debug)]
pub(super) struct FieldRef {
    pub(super) component: ComponentId,
    pub(super) component_name: String,
    pub(super) field: String,
}

impl FieldRef {
    pub(super) fn label(&self) -> String {
        format!("{}:{}", self.component_name, self.field)
    }
}

pub(super) struct Filter {
    pub(super) path: FieldRef,
    pub(super) axis: Option<usize>,
    pub(super) op: FilterOp,
    pub(super) value: Json,
}

impl Filter {
    pub(super) fn matches(&self, entity: &EntityData) -> bool {
        entity
            .components
            .iter()
            .find(|component| component.component == self.path.component)
            .and_then(|component| {
                component
                    .fields
                    .iter()
                    .find(|(name, _)| name == &self.path.field)
                    .map(|(_, value)| value)
            })
            .is_some_and(|value| compare(value, self.axis, self.op, &self.value))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FilterOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug, PartialEq)]
enum Scalar {
    Num(f64),
    Text(String),
    Bool(bool),
}

pub(super) fn axis_index(axis: &str) -> Result<usize, String> {
    match axis {
        "x" => Ok(0),
        "y" => Ok(1),
        "z" => Ok(2),
        "w" => Ok(3),
        other => Err(format!("未知轴：{other}（只认 x / y / z / w）")),
    }
}

fn scalar(value: &Value, axis: Option<usize>) -> Option<Scalar> {
    match value {
        Value::Bool(value) => Some(Scalar::Bool(*value)),
        Value::I64(value) => Some(Scalar::Num(*value as f64)),
        Value::F64(value) => Some(Scalar::Num(*value)),
        Value::Entity(value) => Some(Scalar::Num(*value as f64)),
        Value::String(value) => Some(Scalar::Text(value.clone())),
        Value::Vec2(values) => axis
            .and_then(|axis| values.get(axis))
            .map(|value| Scalar::Num(*value)),
        Value::Vec3(values) => axis
            .and_then(|axis| values.get(axis))
            .map(|value| Scalar::Num(*value)),
        Value::Vec4(values) | Value::Quat(values) => axis
            .and_then(|axis| values.get(axis))
            .map(|value| Scalar::Num(*value)),
    }
}

fn json_scalar(value: &Json) -> Option<Scalar> {
    match value {
        Json::Number(number) => number.as_f64().map(Scalar::Num),
        Json::String(text) => Some(Scalar::Text(text.clone())),
        Json::Bool(value) => Some(Scalar::Bool(*value)),
        _ => None,
    }
}

fn compare(value: &Value, axis: Option<usize>, op: FilterOp, expected: &Json) -> bool {
    let actual = scalar(value, axis);
    let expected_scalar = json_scalar(expected);
    match op {
        FilterOp::Eq => match (&actual, &expected_scalar) {
            (Some(actual), Some(expected)) => actual == expected,
            _ => serde_json::to_value(value).ok().as_ref() == Some(expected),
        },
        FilterOp::Ne => !compare(value, axis, FilterOp::Eq, expected),
        FilterOp::Lt | FilterOp::Le | FilterOp::Gt | FilterOp::Ge => {
            match (&actual, &expected_scalar) {
                (Some(Scalar::Num(actual)), Some(Scalar::Num(expected))) => match op {
                    FilterOp::Lt => actual < expected,
                    FilterOp::Le => actual <= expected,
                    FilterOp::Gt => actual > expected,
                    FilterOp::Ge => actual >= expected,
                    _ => false,
                },
                (Some(Scalar::Text(actual)), Some(Scalar::Text(expected))) => match op {
                    FilterOp::Lt => actual < expected,
                    FilterOp::Le => actual <= expected,
                    FilterOp::Gt => actual > expected,
                    FilterOp::Ge => actual >= expected,
                    _ => false,
                },
                _ => false,
            }
        }
    }
}
