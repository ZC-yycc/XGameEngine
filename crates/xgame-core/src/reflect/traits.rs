//! 反射的三条访问契约。
//!
//! 分层理由：
//! - [`Reflect`] —— 结构体：按名字取字段，返回**借用**。
//! - [`ReflectList`] —— 列表：按下标取元素，且能增删。
//! - [`ReflectEnum`] —— 带载荷枚举：切变体 + 取载荷。
//!
//! 三者分开而不是合成一个大 trait，是因为它们的 dyn 能力不同：
//! `ValueRefMut` 要能装下任意一种，必须各自是独立 trait 对象。

use serde_json::Value;

use super::meta::TypeMeta;
use super::value::{ReflectError, ValueRef, ValueRefMut};

/// 可按名字读写字段的类型。
///
/// **不加 `Any` 约束**：反射不做向下转型（类型已经由 `TypeMeta` 描述清楚了），
/// 而 `Any` 要求 `'static`，会把"借用式的适配器"挡在门外 ——
/// 编辑器要把"枚举变体的载荷"包装成 `Reflect` 交给同一套字段渲染器，
/// 那个包装持有 `&mut dyn ReflectEnum`，生命周期不是 `'static`。
pub trait Reflect {
    /// 静态元数据。实现由 `#[derive(Reflect)]` 生成，或手写。
    ///
    /// 名字刻意**不叫** `meta`：`ReflectMeta::meta()` 是关联函数，两者同名会让
    /// `Camera::meta()` 这种写法有歧义（E0034），而它恰恰是最常用的写法。
    fn type_meta(&self) -> &'static TypeMeta;

    /// 读字段。字段不存在返回 `None`（不是错误 —— 调用方通常要区分
    /// "没有这个字段"和"字段存在但取不到值"）。
    fn get(&self, field: &str) -> Option<ValueRef<'_>>;

    /// 写字段。只读字段必须返回 `None`。
    fn get_mut(&mut self, field: &str) -> Option<ValueRefMut<'_>>;

    /// 带诊断信息的读（`get_path` 用它拼错误消息）。
    fn get_checked(&self, field: &str) -> Result<ValueRef<'_>, ReflectError> {
        self.get(field).ok_or_else(|| ReflectError::NoSuchField {
            type_name: self.type_meta().name,
            field: field.to_string(),
        })
    }

    fn get_mut_checked(&mut self, field: &str) -> Result<ValueRefMut<'_>, ReflectError> {
        // 先把元数据取出来（结果是 `'static`，不占用对 self 的借用），
        // 否则下面 `self.get_mut` 的可变借用会与 `self.meta()` 的不可变借用重叠。
        let meta = self.type_meta();
        let declared = meta.field(field).is_some();
        let type_name = meta.name;
        match self.get_mut(field) {
            Some(slot) => Ok(slot),
            // 区分"没有这个字段"与"有但只读"：后者对使用者是更准确的诊断。
            None if declared => Err(ReflectError::ReadOnly(field.to_string())),
            None => Err(ReflectError::NoSuchField {
                type_name,
                field: field.to_string(),
            }),
        }
    }

    /// 用 JSON 值整体替换一个字段。
    ///
    /// 与 `get_mut` 的标量写入互补：它允许 `Option` / 结构体 / 枚举 / 列表整体
    /// 被替换。实现必须在反序列化成功后才写回，避免半截值污染原数据。
    fn set_json(&mut self, field: &str, value: Value) -> Result<(), ReflectError> {
        let _ = (field, value);
        Err(ReflectError::UnsupportedJson(self.type_meta().name))
    }
}

/// 可按下标访问、且能增删元素的列表。
///
/// `insert_default` / `remove` 之所以是必需方法而不是可选，是因为列表编辑器
/// （Inspector 的 `Vec<LodLevel>` 那种）没有它们就只是个只读表格。
pub trait ReflectList {
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn get(&self, index: usize) -> Option<ValueRef<'_>>;

    fn get_mut(&mut self, index: usize) -> Option<ValueRefMut<'_>>;

    /// 在下标处插入一个默认值元素。越界返回 `false`。
    fn insert_default(&mut self, index: usize) -> bool;

    /// 删除下标处的元素。越界返回 `false`。
    fn remove(&mut self, index: usize) -> bool;

    /// 把下标处的元素替换成同类型的默认值（"重置这一项"）。
    fn reset(&mut self, index: usize) -> bool {
        if index >= self.len() {
            return false;
        }
        self.remove(index) && self.insert_default(index)
    }

    /// 用 JSON 值整体替换一个元素。
    fn set_json(&mut self, index: usize, value: Value) -> Result<(), ReflectError> {
        let _ = (index, value);
        Err(ReflectError::UnsupportedJson("ReflectList"))
    }
}

/// 带载荷枚举的读写。
///
/// **切变体会丢掉原载荷**（重置为默认）—— 这条语义是刻意的：不同变体的载荷
/// 类型可能完全不同（`CollisionShape::Cuboid` 是 `[f32; 3]`、`Ball` 是 `f32`），
/// 保留旧值没有意义，静默解释成新类型会造出不可能的状态。
///
/// # 载荷为什么是"按字段名"而不是一个 `ValueRef`
///
/// 载荷不是独立的 Rust 值，它住在枚举变体里 —— 没有一块连续内存可以假装成
/// 一个结构体借出去。所以载荷走**与 [`Reflect`] 同形**的三个方法：一张字段表
/// 加按名字读写。编辑器因此能用同一套控件画 `PhysicsBody` 的字段和
/// `CollisionShape::Cuboid.half_extents` 的字段。
pub trait ReflectEnum {
    /// 当前变体名（**寻址名**，等于 Rust 变体名）。
    fn variant(&self) -> &'static str;

    /// 切到另一个变体（载荷重置为默认）。变体不存在返回 `false`，状态不变。
    fn set_variant(&mut self, name: &str) -> bool;

    /// 所有可选变体名（编辑器画下拉框用），按声明顺序。
    fn variant_names(&self) -> &'static [&'static str];

    /// 当前变体的载荷字段表；无载荷变体返回 `None`。
    fn payload_meta(&self) -> Option<&'static TypeMeta> {
        None
    }

    /// 读当前变体的一个载荷字段。
    fn payload_get(&self, _field: &str) -> Option<ValueRef<'_>> {
        None
    }

    /// 写当前变体的一个载荷字段。
    fn payload_get_mut(&mut self, _field: &str) -> Option<ValueRefMut<'_>> {
        None
    }

    /// 用 JSON 值整体替换当前枚举值。
    fn set_json(&mut self, value: Value) -> Result<(), ReflectError> {
        let _ = value;
        Err(ReflectError::UnsupportedJson("ReflectEnum"))
    }

    /// 用 JSON 值替换当前变体的一个载荷字段。
    fn payload_set_json(&mut self, field: &str, value: Value) -> Result<(), ReflectError> {
        let _ = (field, value);
        Err(ReflectError::UnsupportedJson("ReflectEnum payload"))
    }
}

/// "能变成一个反射值"的类型。
///
/// # 为什么要这层
///
/// derive 生成的 `get` 需要对字段做统一转换。如果按字段的**语法类型**分别生成
/// 转换代码（`f32` → `ValueRef::F32`、`Camera` → `ValueRef::Struct`、`Option<T>` →
/// 展开成 `match`），宏要递归处理每种组合，很容易写漏。
///
/// 引入这一层后，derive 对每个字段只生成一句 `self.field.as_value()`，组合交给
/// trait 解决：标量、结构体、`Option<T>`、`Vec<T>` 各自实现一次即可。
///
/// **不能对 `T: Reflect` 做 blanket impl**：标量也要实现本 trait，而
/// `impl ReflectItem for f32` 与 `impl<T: Reflect> ReflectItem for T` 在
/// 一致性检查下视为潜在重叠（本 crate 将来完全可能给 `f32` 加 `Reflect`）。
/// 所以结构体的 `ReflectItem` 由 derive 一并生成。
pub trait ReflectItem {
    fn as_value(&self) -> ValueRef<'_>;
    fn as_value_mut(&mut self) -> ValueRefMut<'_>;
}

/// 按字符串键访问的映射容器。
///
/// 覆盖两类真实数据：动画参数表（`BTreeMap<String, f32>`）与插件参数表
/// （`FieldValues`）。键名就是路径的一段：`parameters.speed` / `fields.health`。
pub trait ReflectMap {
    fn get(&self, key: &str) -> Option<ValueRef<'_>>;

    fn get_mut(&mut self, key: &str) -> Option<ValueRefMut<'_>>;

    /// 用 JSON 值替换一个已有键的值。键不存在返回 `NoSuchField`。
    fn set_json(&mut self, key: &str, value: Value) -> Result<(), ReflectError> {
        let _ = (key, value);
        Err(ReflectError::UnsupportedJson("ReflectMap"))
    }
}
