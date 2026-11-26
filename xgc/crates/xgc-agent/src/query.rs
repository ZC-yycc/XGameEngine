//! Agent 查询的字段路径、过滤器与翻页游标。

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use serde_json::Value as Json;
use xgc_base::EntityId;
use xgc_reflect::{ComponentId, FieldPath, Value};
use xgc_world::World;

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
