//! Agent 查询的字段路径、过滤器与翻页游标。

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use serde_json::{json, Value as Json};
use xgc_base::EntityId;
use xgc_reflect::{ComponentId, FieldPath, Registry, Value};
use xgc_world::{EntityData, World};

pub(super) const QUERY_DEFAULT_LIMIT: u64 = 100;
pub(super) const QUERY_MAX_LIMIT: u64 = 1000;
const CURSOR_PREFIX: &str = "q1:";

#[derive(Clone, Copy)]
pub(super) struct QueryEpoch(u64);

impl QueryEpoch {
    pub(super) fn new() -> Self {
        // 同进程的不同会话与世界替换都递增。随机起点让进程重启也换一段游标序号。
        static NEXT_EPOCH: OnceLock<AtomicU64> = OnceLock::new();
        let next = NEXT_EPOCH.get_or_init(|| AtomicU64::new(RandomState::new().hash_one(())));
        Self(next.fetch_add(1, Ordering::Relaxed))
    }

    pub(super) fn cursor(self, revision: u64, last_id: EntityId) -> String {
        format!(
            "{CURSOR_PREFIX}{:016x}:{revision:016x}:{:016x}",
            self.0,
            last_id.raw()
        )
    }

    pub(super) fn resume(self, revision: u64, value: &Json) -> Result<EntityId, String> {
        let (epoch, saved_revision, last_id) = value
            .as_str()
            .and_then(decode_cursor)
            .ok_or_else(|| "cursor 格式无效".to_string())?;
        if epoch != self.0 || saved_revision != revision {
            return Err("cursor 已失效，请从查询首项重新开始".to_string());
        }
        Ok(last_id)
    }
}

fn decode_cursor(token: &str) -> Option<(u64, u64, EntityId)> {
    let (epoch, tail) = token.strip_prefix(CURSOR_PREFIX)?.split_once(':')?;
    let (revision, last_id) = tail.split_once(':')?;
    Some((
        u64::from_str_radix(epoch, 16).ok()?,
        u64::from_str_radix(revision, 16).ok()?,
        EntityId::from_raw(u64::from_str_radix(last_id, 16).ok()?),
    ))
}

#[derive(Clone, Debug)]
pub(super) struct FieldRef {
    pub(super) component: ComponentId,
    pub(super) label: String,
    pub(super) ty: String,
    pub(super) path: FieldPath,
}

impl FieldRef {
    pub(super) fn read<T>(
        &self,
        world: &World,
        id: EntityId,
        project: impl FnOnce(&Value) -> T,
    ) -> Option<T> {
        let root = world.get_field(id, self.component, self.path.root())?;
        self.path.read(&root).ok().map(project)
    }
}

pub(super) struct Projection<'a> {
    field: &'a FieldRef,
    default: Option<&'a Value>,
}

impl<'a> Projection<'a> {
    pub(super) fn new(field: &'a FieldRef, defaults: Option<&'a Registry>) -> Self {
        let default = defaults
            .and_then(|registry| registry.get(field.component))
            .and_then(|component| {
                component
                    .fields
                    .iter()
                    .find(|root| root.name == field.path.root())
            })
            .and_then(|root| field.path.read(&root.default).ok());
        Self { field, default }
    }

    // 缺失字段仍返回 null。默认值对应的空位由调用方按行式或列式协议记录。
    fn cell(
        &self,
        world: &World,
        id: EntityId,
        encode: impl FnOnce(&Value) -> Json,
    ) -> Option<Json> {
        self.field
            .read(world, id, |value| {
                if self
                    .default
                    .is_some_and(|default| same_value(value, default))
                {
                    None
                } else {
                    Some(encode(value))
                }
            })
            .unwrap_or(Some(Json::Null))
    }
}

fn same_value(left: &Value, right: &Value) -> bool {
    fn same_floats(left: &[f64], right: &[f64]) -> bool {
        left.iter()
            .zip(right)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    }

    // 保留负零和嵌套容器里的浮点位模式，以便消费者能无损补回省略的值。
    match (left, right) {
        (Value::F64(a), Value::F64(b)) => a.to_bits() == b.to_bits(),
        (Value::Vec2(a), Value::Vec2(b)) => same_floats(a, b),
        (Value::Vec3(a), Value::Vec3(b)) => same_floats(a, b),
        (Value::Vec4(a), Value::Vec4(b)) | (Value::Quat(a), Value::Quat(b)) => same_floats(a, b),
        (Value::List(a), Value::List(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_value(a, b))
        }
        (Value::Struct(a), Value::Struct(b)) | (Value::Map(a), Value::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|((ak, av), (bk, bv))| ak == bk && same_value(av, bv))
        }
        (
            Value::Enum {
                variant: a,
                payload: av,
            },
            Value::Enum {
                variant: b,
                payload: bv,
            },
        ) => a == b && same_value(av, bv),
        _ => left == right,
    }
}

pub(super) fn omit_entity_defaults(entity: &mut EntityData, registry: &Registry) {
    for component in &mut entity.components {
        if let Some(schema) = registry.get(component.component) {
            component.fields.retain(|(name, value)| {
                !schema
                    .fields
                    .iter()
                    .any(|field| field.name == *name && same_value(value, &field.default))
            });
        }
    }
}

/// 列目录声明了字段类型，省略模式可把安全整数范围内的浮点值用短数字表示。
fn column_value(value: &Value, compact: bool) -> Json {
    fn number(value: f64, compact: bool) -> Json {
        const MAX_SAFE_INTEGER: f64 = ((1_u64 << 53) - 1) as f64;
        if compact
            && value.abs() <= MAX_SAFE_INTEGER
            && value.fract() == 0.0
            && value.to_bits() != (-0.0_f64).to_bits()
        {
            Json::from(value as i64)
        } else {
            Json::from(value)
        }
    }

    let vector =
        |values: &[f64]| Json::Array(values.iter().map(|&value| number(value, compact)).collect());
    let encode = |value: &Value| column_value(value, compact);
    match value {
        Value::Bool(value) => Json::from(*value),
        Value::I64(value) => Json::from(*value),
        Value::F64(value) => number(*value, compact),
        Value::String(value) => Json::String(value.clone()),
        Value::Entity(value) => Json::String(EntityId::from_raw(*value).to_string()),
        Value::Vec2(values) => vector(values),
        Value::Vec3(values) => vector(values),
        Value::Vec4(values) | Value::Quat(values) => vector(values),
        Value::List(values) => Json::Array(values.iter().map(encode).collect()),
        Value::Struct(values) | Value::Map(values) => Json::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), encode(value)))
                .collect(),
        ),
        Value::Enum { variant, payload } => Json::Object(
            [
                ("variant".into(), Json::String(variant.clone())),
                ("payload".into(), encode(payload)),
            ]
            .into_iter()
            .collect(),
        ),
    }
}

pub(super) fn projected_rows(world: &World, ids: &[EntityId], fields: &[Projection<'_>]) -> Json {
    Json::Array(
        ids.iter()
            .map(|&id| {
                let mut values = serde_json::Map::new();
                for projection in fields {
                    if let Some(value) = projection.cell(world, id, |value| {
                        serde_json::to_value(value).unwrap_or(Json::Null)
                    }) {
                        values.insert(projection.field.label.clone(), value);
                    }
                }
                let mut row = json!({"id": id});
                row["values"] = Json::Object(values);
                row
            })
            .collect(),
    )
}

pub(super) fn projected_columns(
    world: &World,
    ids: &[EntityId],
    fields: &[Projection<'_>],
    omit_defaults: bool,
) -> (Json, Json) {
    let mut columns: Vec<_> = std::iter::once(json!({"name": "id", "type": "entity"}))
        .chain(fields.iter().map(|projection| {
            let field = projection.field;
            json!({"name": field.label, "type": field.ty})
        }))
        .collect();
    let mut defaults = Vec::new();
    if omit_defaults {
        defaults.resize_with(fields.len(), Vec::<[usize; 2]>::new);
    }
    let rows = ids
        .iter()
        .enumerate()
        .map(|(row_index, &id)| {
            let mut row = Vec::with_capacity(fields.len() + 1);
            row.push(Json::String(id.to_string()));
            let mut end = row.len();
            for (column, projection) in fields.iter().enumerate() {
                match projection.cell(world, id, |value| column_value(value, omit_defaults)) {
                    Some(value) => {
                        row.push(value);
                        end = row.len();
                    }
                    None => {
                        let ranges = &mut defaults[column];
                        if let Some(last) = ranges.last_mut().filter(|last| last[1] == row_index) {
                            last[1] += 1;
                        } else {
                            ranges.push([row_index, row_index + 1]);
                        }
                        row.push(Json::Null);
                    }
                }
            }
            row.truncate(end);
            Json::Array(row)
        })
        .collect();
    // 区间以本页为准且右端不含，尾部空位可省掉而不丢失和缺失值的区别。
    for (column, ranges) in columns.iter_mut().skip(1).zip(defaults) {
        if !ranges.is_empty() {
            column["default_rows"] = json!(ranges);
        }
    }
    (Json::Array(columns), Json::Array(rows))
}

pub(super) struct Filter {
    pub(super) path: FieldRef,
    pub(super) axis: Option<usize>,
    pub(super) op: FilterOp,
    pub(super) value: Json,
}

impl Filter {
    pub(super) fn matches(&self, world: &World, id: EntityId) -> bool {
        self.path
            .read(world, id, |value| {
                compare(value, self.axis, self.op, &self.value)
            })
            .unwrap_or(false)
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
        Value::List(_) | Value::Struct(_) | Value::Map(_) | Value::Enum { .. } => None,
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
