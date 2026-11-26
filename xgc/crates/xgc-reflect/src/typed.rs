//! Rust 字段类型与现有递归值之间的投影，供派生生成的代码调用。

use std::collections::BTreeMap;

use xgc_base::EntityId;

use crate::{FieldDescriptor, FieldType, Value, ValueProjection, ValueWriter};

/// 派生按字段的 Rust 类型生成描述、读值和写值。容器递归复用其元素的投影。
/// 写入先完整解码，再由调用方替换原字段。
pub trait ReflectValue: Sized {
    /// 列表新增元素时使用的描述名。
    const ELEMENT_NAME: &'static str = "item";

    fn field_type() -> FieldType;
    fn to_value(&self) -> Value;
    fn from_value(value: &Value) -> Result<Self, String>;

    /// 与 `to_value().hash_into(hash)` 同序，可直接读取字段以免物化拥有型值。
    fn hash_into(&self, hash: &mut u64) {
        self.hash_projected(hash);
    }

    #[doc(hidden)]
    fn project_value(&self, writer: &mut ValueWriter<'_>) {
        writer.owned(self.to_value());
    }

    fn descriptor(name: &'static str, default: &Self) -> FieldDescriptor {
        FieldDescriptor::new(name, Self::field_type(), default.to_value())
    }
}

impl<T: ReflectValue> ValueProjection for T {
    fn project(&self, writer: &mut ValueWriter<'_>) {
        self.project_value(writer);
    }
}

macro_rules! scalar {
    ($ty:ty, $kind:ident, $variant:ident, $encode:expr, $decode:expr) => {
        impl ReflectValue for $ty {
            fn field_type() -> FieldType {
                FieldType::$kind
            }

            fn to_value(&self) -> Value {
                Value::$variant(($encode)(self))
            }

            fn from_value(value: &Value) -> Result<Self, String> {
                if let Value::$variant(raw) = value {
                    ($decode)(raw).ok_or_else(|| format!("超出 {} 可表示的范围", stringify!($kind)))
                } else {
                    Err(type_error(Self::field_type().as_str(), value))
                }
            }
        }
    };
}

scalar!(bool, Bool, Bool, |v: &bool| *v, |v: &bool| Some(*v));
scalar!(i64, I64, I64, |v: &i64| *v, |v: &i64| Some(*v));
scalar!(u32, U32, I64, |v: &u32| i64::from(*v), |v: &i64| {
    u32::try_from(*v).ok()
});
scalar!(f32, F32, F64, |v: &f32| f64::from(*v), |v: &f64| {
    (f64::from(-f32::MAX)..=f64::from(f32::MAX))
        .contains(v)
        .then_some(*v as f32)
});
scalar!(f64, F64, F64, |v: &f64| *v, |v: &f64| v
    .is_finite()
    .then_some(*v));
scalar!(
    EntityId,
    Entity,
    Entity,
    |v: &EntityId| v.raw(),
    |v: &u64| { Some(EntityId::from_raw(*v)) }
);

impl ReflectValue for String {
    fn field_type() -> FieldType {
        FieldType::String
    }

    fn to_value(&self) -> Value {
        Value::String(self.clone())
    }

    fn from_value(value: &Value) -> Result<Self, String> {
        value
            .as_string()
            .map(str::to_owned)
            .ok_or_else(|| type_error("string", value))
    }

    fn project_value(&self, writer: &mut ValueWriter<'_>) {
        writer.string(self);
    }
}

impl<T: ReflectValue + Default> ReflectValue for Vec<T> {
    fn field_type() -> FieldType {
        FieldType::List(Box::new(T::descriptor(T::ELEMENT_NAME, &T::default())))
    }

    fn to_value(&self) -> Value {
        self.projected_value()
    }

    fn project_value(&self, writer: &mut ValueWriter<'_>) {
        writer.list(self);
    }

    fn from_value(value: &Value) -> Result<Self, String> {
        let Value::List(items) = value else {
            return Err(type_error("list", value));
        };
        items
            .iter()
            .enumerate()
            .map(|(index, item)| T::from_value(item).map_err(|err| format!("[{index}]：{err}")))
            .collect()
    }
}

impl<T: ReflectValue + Default> ReflectValue for BTreeMap<String, T> {
    fn field_type() -> FieldType {
        FieldType::Map(Box::new(T::descriptor("value", &T::default())))
    }

    fn to_value(&self) -> Value {
        self.projected_value()
    }

    fn project_value(&self, writer: &mut ValueWriter<'_>) {
        writer.map(self);
    }

    fn from_value(value: &Value) -> Result<Self, String> {
        let Value::Map(entries) = value else {
            return Err(type_error("map", value));
        };
        entries
            .iter()
            .map(|(key, value)| {
                T::from_value(value)
                    .map(|typed| (key.clone(), typed))
                    .map_err(|err| format!("[{key:?}]：{err}"))
            })
            .collect()
    }
}

fn type_error(expected: &str, value: &Value) -> String {
    format!("期望 {expected}，得到 {}", value.type_name())
}
