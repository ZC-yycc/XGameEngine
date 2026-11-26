//! 同一份字段投影可生成拥有型 Value，也可直接混入状态哈希。

use std::collections::BTreeMap;

use crate::Value;

/// 按当前字段类型输出借用值。投影本身不存储另一份字段数据。
pub trait ValueProjection {
    fn project(&self, writer: &mut ValueWriter<'_>);

    fn projected_value(&self) -> Value {
        let mut value = Value::Bool(false);
        self.project(&mut ValueWriter(Target::Value(&mut value)));
        value
    }

    fn hash_projected(&self, hash: &mut u64) {
        self.project(&mut ValueWriter(Target::Hash(hash, false)));
    }
}

/// 手写投影可在栈上表达结构体和枚举载荷。
pub struct Record<'a, const N: usize>(pub [(&'a str, &'a dyn ValueProjection); N]);

impl<const N: usize> ValueProjection for Record<'_, N> {
    fn project(&self, writer: &mut ValueWriter<'_>) {
        writer.record(self.0);
    }
}

pub struct Variant<'a>(pub &'a str, pub &'a dyn ValueProjection);

impl ValueProjection for Variant<'_> {
    fn project(&self, writer: &mut ValueWriter<'_>) {
        writer.variant(self.0, self.1);
    }
}

/// 字段投影的两种出口共用容器遍历。哈希出口记录当前值是否是子节点。
pub struct ValueWriter<'a>(Target<'a>);

enum Target<'a> {
    Value(&'a mut Value),
    Hash(&'a mut u64, bool),
}

#[derive(Clone, Copy)]
enum FieldContainer {
    Struct,
    Map,
}

impl ValueWriter<'_> {
    pub fn string(&mut self, text: &str) {
        match &mut self.0 {
            Target::Value(out) => **out = Value::String(text.to_owned()),
            Target::Hash(hash, nested) => {
                mix_kind(hash, *nested, "string");
                if *nested {
                    mix(hash, text.len() as u64);
                }
                mix_text(hash, text);
            }
        }
    }

    pub fn list<T: ValueProjection>(&mut self, values: &[T]) {
        match &mut self.0 {
            Target::Value(out) => {
                **out = Value::List(
                    values
                        .iter()
                        .map(ValueProjection::projected_value)
                        .collect(),
                );
            }
            Target::Hash(hash, nested) => {
                mix_header(hash, *nested, "list", b'[', values.len());
                for value in values {
                    mix_child(hash, value);
                }
            }
        }
    }

    /// 字段名必须唯一，声明次序可与哈希次序不同。
    pub fn record<const N: usize>(&mut self, mut fields: [(&str, &dyn ValueProjection); N]) {
        // Value::Struct 使用 BTreeMap，哈希也必须按名字而非声明顺序处理。
        fields.sort_unstable_by_key(|(name, _)| *name);
        debug_assert!(fields.windows(2).all(|pair| pair[0].0 != pair[1].0));
        self.fields(FieldContainer::Struct, N, fields);
    }

    pub fn map<T: ValueProjection>(&mut self, fields: &BTreeMap<String, T>) {
        self.fields(
            FieldContainer::Map,
            fields.len(),
            fields.iter().map(|(name, value)| (name.as_str(), value)),
        );
    }

    pub fn variant(&mut self, variant: &str, payload: &dyn ValueProjection) {
        match &mut self.0 {
            Target::Value(out) => {
                **out = Value::Enum {
                    variant: variant.to_owned(),
                    payload: Box::new(payload.projected_value()),
                };
            }
            Target::Hash(hash, nested) => {
                mix_header(hash, *nested, "enum", b'e', variant.len());
                mix_text(hash, variant);
                mix_child(hash, payload);
            }
        }
    }

    fn fields<'a, T: ValueProjection + ?Sized + 'a>(
        &mut self,
        container: FieldContainer,
        len: usize,
        fields: impl IntoIterator<Item = (&'a str, &'a T)>,
    ) {
        match &mut self.0 {
            Target::Value(out) => {
                let values = fields
                    .into_iter()
                    .map(|(key, value)| (key.to_owned(), value.projected_value()))
                    .collect();
                **out = match container {
                    FieldContainer::Struct => Value::Struct(values),
                    FieldContainer::Map => Value::Map(values),
                };
            }
            Target::Hash(hash, nested) => {
                let (kind, tag) = match container {
                    FieldContainer::Struct => ("struct", b'{'),
                    FieldContainer::Map => ("map", b'm'),
                };
                mix_header(hash, *nested, kind, tag, len);
                for (key, value) in fields {
                    mix(hash, key.len() as u64);
                    mix_text(hash, key);
                    mix_child(hash, value);
                }
            }
        }
    }

    pub(super) fn owned(&mut self, value: Value) {
        match &mut self.0 {
            Target::Value(out) => **out = value,
            Target::Hash(..) => value.project(self),
        }
    }

    fn borrow_value(&mut self, value: &Value) {
        if let Target::Value(out) = &mut self.0 {
            **out = value.clone();
            return;
        }
        match value {
            Value::String(text) => self.string(text),
            Value::List(values) => self.list(values),
            Value::Struct(fields) => self.fields(
                FieldContainer::Struct,
                fields.len(),
                fields.iter().map(|(name, value)| (name.as_str(), value)),
            ),
            Value::Map(fields) => self.map(fields),
            Value::Enum { variant, payload } => self.variant(variant, payload.as_ref()),
            scalar => self.hash_scalar(scalar),
        }
    }

    fn hash_scalar(&mut self, value: &Value) {
        let Target::Hash(hash, nested) = &mut self.0 else {
            unreachable!();
        };
        mix_kind(hash, *nested, value.kind());
        match value {
            Value::Bool(value) => mix(hash, u64::from(*value)),
            Value::I64(value) => mix(hash, *value as u64),
            Value::F64(value) => mix(hash, value.to_bits()),
            Value::Entity(value) => mix(hash, *value),
            Value::Vec2(values) => mix_numbers(hash, values),
            Value::Vec3(values) => mix_numbers(hash, values),
            Value::Vec4(values) | Value::Quat(values) => mix_numbers(hash, values),
            Value::String(_)
            | Value::List(_)
            | Value::Struct(_)
            | Value::Map(_)
            | Value::Enum { .. } => {
                unreachable!()
            }
        }
    }
}

impl ValueProjection for Value {
    fn project(&self, writer: &mut ValueWriter<'_>) {
        writer.borrow_value(self);
    }
}

impl ValueProjection for &str {
    fn project(&self, writer: &mut ValueWriter<'_>) {
        writer.string(self);
    }
}

impl<T: ValueProjection> ValueProjection for &[T] {
    fn project(&self, writer: &mut ValueWriter<'_>) {
        writer.list(self);
    }
}

fn mix_child(hash: &mut u64, value: &(impl ValueProjection + ?Sized)) {
    value.project(&mut ValueWriter(Target::Hash(hash, true)));
}

fn mix_kind(hash: &mut u64, nested: bool, kind: &str) {
    // 根字段沿用无类型标签的旧编码，容器子节点才带标签。
    if nested {
        mix_text(hash, kind);
    }
}

fn mix_header(hash: &mut u64, nested: bool, kind: &str, tag: u8, len: usize) {
    mix_kind(hash, nested, kind);
    mix(hash, u64::from(tag));
    mix(hash, len as u64);
}

fn mix_numbers(hash: &mut u64, values: &[f64]) {
    for value in values {
        mix(hash, value.to_bits());
    }
}

pub(super) fn mix_text(hash: &mut u64, text: &str) {
    for byte in text.bytes() {
        mix(hash, u64::from(byte));
    }
}

fn mix(hash: &mut u64, value: u64) {
    *hash ^= value;
    *hash = hash.wrapping_mul(0x100000001b3);
}
