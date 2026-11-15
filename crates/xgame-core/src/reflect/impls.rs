//! 标量与容器的 [`ReflectItem`] / [`ReflectList`] 实现。
//!
//! 结构体的实现由 `#[derive(Reflect)]` 生成，不在这里。

use glam::{Vec2, Vec3, Vec4};

use super::traits::{ReflectItem, ReflectList};
use super::value::{ValueRef, ValueRefMut};

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
impl<T: ReflectItem + Default> ReflectItem for Vec<T> {
    fn as_value(&self) -> ValueRef<'_> {
        ValueRef::List(self)
    }
    fn as_value_mut(&mut self) -> ValueRefMut<'_> {
        ValueRefMut::List(self)
    }
}

impl<T: ReflectItem + Default> ReflectList for Vec<T> {
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
}

/// 定长数组 `[T; N]` 的只读视图。
///
/// 不实现 `ReflectList`：长度是类型的一部分，增删没有意义。编辑器对它的展示
/// 与 `Vec` 一致，但"加一项"按钮必须禁用。
impl<T: ReflectItem, const N: usize> ReflectItem for [T; N] {
    fn as_value(&self) -> ValueRef<'_> {
        ValueRef::List(self)
    }
    fn as_value_mut(&mut self) -> ValueRefMut<'_> {
        ValueRefMut::List(self)
    }
}

impl<T: ReflectItem, const N: usize> ReflectList for [T; N] {
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
}
