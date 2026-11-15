//! 标量与容器的 [`ReflectItem`] / [`ReflectList`] / [`ReflectMap`] 实现。
//!
//! 结构体的实现由 `#[derive(Reflect)]` 生成，不在这里。

use std::collections::BTreeMap;

use glam::{Quat, Vec2, Vec3, Vec4};
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::traits::{ReflectItem, ReflectList, ReflectMap};
use super::value::{from_json_strict, ReflectError, ValueRef, ValueRefMut};

/// 标量的 `ReflectItem`：直接返回对应的 `ValueRef` 变体。
macro_rules! scalar_item {
    ($($ty:ty => $ref:ident / $ref_mut:ident,)*) => {
        $(
            impl ReflectItem for $ty {
                fn as_value(&self) -> ValueRef<'_> {
                    ValueRef::$ref(self)
                }
                fn as_value_mut(&mut self) -> ValueRefMut<'_> {
                    ValueRefMut::$ref_mut(self)
                }
            }
        )*
    };
}

scalar_item! {
    f32 => F32 / F32,
    f64 => F64 / F64,
    i32 => I32 / I32,
    i64 => I64 / I64,
    u32 => U32 / U32,
    u64 => U64 / U64,
    bool => Bool / Bool,
    Vec2 => Vec2 / Vec2,
    Vec3 => Vec3 / Vec3,
    Vec4 => Vec4 / Vec4,
    Quat => Quat / Quat,
}

/// `String` 单独实现：只读侧交出去的是 `&str` 而不是 `&String`。
impl ReflectItem for String {
    fn as_value(&self) -> ValueRef<'_> {
        ValueRef::Str(self.as_str())
    }
    fn as_value_mut(&mut self) -> ValueRefMut<'_> {
        ValueRefMut::Str(self)
    }
}

/// `Option<T>`：`None` 映射到 `ValueRef::None`（不是错误）。
///
/// 这样 derive 不必为 `Option` 字段生成 `match` —— 直接 `self.field.as_value()`
/// 就能拿到"有值"或"空"。
impl<T: ReflectItem> ReflectItem for Option<T> {
    fn as_value(&self) -> ValueRef<'_> {
        match self {
            Some(inner) => inner.as_value(),
            None => ValueRef::None,
        }
    }
    fn as_value_mut(&mut self) -> ValueRefMut<'_> {
        match self {
            Some(inner) => inner.as_value_mut(),
            None => ValueRefMut::None,
        }
    }
}

/// `Vec<T>` 既是列表，也是"能变成反射值"的类型（供"列表字段"直接取用）。
impl<T: ReflectItem + Default + DeserializeOwned> ReflectItem for Vec<T> {
    fn as_value(&self) -> ValueRef<'_> {
        ValueRef::List(self)
    }
    fn as_value_mut(&mut self) -> ValueRefMut<'_> {
        ValueRefMut::List(self)
    }
}

impl<T: ReflectItem + Default + DeserializeOwned> ReflectList for Vec<T> {
    fn len(&self) -> usize {
        Vec::len(self)
    }

    fn get(&self, index: usize) -> Option<ValueRef<'_>> {
        // 用闭包而不是 `map(ReflectItem::as_value)`：后者作为函数值使用时，
        // 高阶生命周期（`for<'a> fn(&'a T) -> ValueRef<'a>`）推不出来，会报 E0631。
        self.as_slice().get(index).map(|item| item.as_value())
    }

    fn get_mut(&mut self, index: usize) -> Option<ValueRefMut<'_>> {
        self.as_mut_slice()
            .get_mut(index)
            .map(|item| item.as_value_mut())
    }

    fn insert_default(&mut self, index: usize) -> bool {
        // 允许 `index == len()`（追加），超出才拒绝。
        if index > self.len() {
            return false;
        }
        self.insert(index, T::default());
        true
    }

    fn remove(&mut self, index: usize) -> bool {
        if index >= self.len() {
            return false;
        }
        self.remove(index);
        true
    }

    fn set_json(&mut self, index: usize, value: Value) -> Result<(), ReflectError> {
        let Some(slot) = self.as_mut_slice().get_mut(index) else {
            return Err(ReflectError::IndexOutOfRange {
                index,
                len: self.len(),
            });
        };
        let parsed = from_json_strict::<T>(value)?;
        *slot = parsed;
        Ok(())
    }
}

/// 定长数组 `[T; N]` 的只读视图。
///
/// 不实现 `ReflectList`：长度是类型的一部分，增删没有意义。编辑器对它的展示
/// 与 `Vec` 一致，但"加一项"按钮必须禁用。
impl<T: ReflectItem + DeserializeOwned, const N: usize> ReflectItem for [T; N] {
    fn as_value(&self) -> ValueRef<'_> {
        ValueRef::List(self)
    }
    fn as_value_mut(&mut self) -> ValueRefMut<'_> {
        ValueRefMut::List(self)
    }
}

impl<T: ReflectItem + DeserializeOwned, const N: usize> ReflectList for [T; N] {
    fn len(&self) -> usize {
        N
    }

    fn get(&self, index: usize) -> Option<ValueRef<'_>> {
        <[T]>::get(self, index).map(|item| item.as_value())
    }

    fn get_mut(&mut self, index: usize) -> Option<ValueRefMut<'_>> {
        <[T]>::get_mut(self, index).map(|item| item.as_value_mut())
    }

    fn insert_default(&mut self, _index: usize) -> bool {
        false
    }

    fn remove(&mut self, _index: usize) -> bool {
        false
    }

    fn set_json(&mut self, index: usize, value: Value) -> Result<(), ReflectError> {
        let Some(slot) = <[T]>::get_mut(self, index) else {
            return Err(ReflectError::IndexOutOfRange { index, len: N });
        };
        let parsed = from_json_strict::<T>(value)?;
        *slot = parsed;
        Ok(())
    }
}

/// `BTreeMap<String, V>`：动画参数表与插件参数表都走这里。
///
/// 只支持字符串键：路径的下一段就是键名，不需要另一套转义语法。
impl<V: ReflectItem + DeserializeOwned> ReflectMap for BTreeMap<String, V> {
    fn get(&self, key: &str) -> Option<ValueRef<'_>> {
        self.get(key).map(|item| item.as_value())
    }

    fn get_mut(&mut self, key: &str) -> Option<ValueRefMut<'_>> {
        self.get_mut(key).map(|item| item.as_value_mut())
    }

    fn set_json(&mut self, key: &str, value: Value) -> Result<(), ReflectError> {
        let Some(slot) = self.get_mut(key) else {
            return Err(ReflectError::NoSuchField {
                type_name: "map",
                field: key.to_string(),
            });
        };
        let parsed = from_json_strict::<V>(value)?;
        *slot = parsed;
        Ok(())
    }
}

impl<V: ReflectItem + DeserializeOwned> ReflectItem for BTreeMap<String, V> {
    fn as_value(&self) -> ValueRef<'_> {
        ValueRef::Map(self)
    }

    fn as_value_mut(&mut self) -> ValueRefMut<'_> {
        ValueRefMut::Map(self)
    }
}

/// 插件参数表里的值。它在磁盘上是带标签的 `FieldValue`，但反射读写按标量走，
/// 避免让每个脚本参数都多一层枚举包装。
impl ReflectItem for crate::inspect::FieldValue {
    fn as_value(&self) -> ValueRef<'_> {
        use crate::inspect::FieldValue;
        match self {
            FieldValue::F32(v) => ValueRef::F32(v),
            FieldValue::F64(v) => ValueRef::F64(v),
            FieldValue::I32(v) => ValueRef::I32(v),
            FieldValue::I64(v) => ValueRef::I64(v),
            FieldValue::U32(v) => ValueRef::U32(v),
            FieldValue::Bool(v) => ValueRef::Bool(v),
            FieldValue::String(v) => ValueRef::Str(v.as_str()),
            FieldValue::Vec2(v) => ValueRef::Vec2(v),
            FieldValue::Vec3(v) => ValueRef::Vec3(v),
            FieldValue::Vec4(v) => ValueRef::Vec4(v),
        }
    }

    fn as_value_mut(&mut self) -> ValueRefMut<'_> {
        use crate::inspect::FieldValue;
        match self {
            FieldValue::F32(v) => ValueRefMut::F32(v),
            FieldValue::F64(v) => ValueRefMut::F64(v),
            FieldValue::I32(v) => ValueRefMut::I32(v),
            FieldValue::I64(v) => ValueRefMut::I64(v),
            FieldValue::U32(v) => ValueRefMut::U32(v),
            FieldValue::Bool(v) => ValueRefMut::Bool(v),
            FieldValue::String(v) => ValueRefMut::Str(v),
            FieldValue::Vec2(v) => ValueRefMut::Vec2(v),
            FieldValue::Vec3(v) => ValueRefMut::Vec3(v),
            FieldValue::Vec4(v) => ValueRefMut::Vec4(v),
        }
    }
}
