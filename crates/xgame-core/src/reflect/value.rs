//! 反射值：**借用式**的字段访问载体。
//!
//! # 为什么是借用而不是值树
//!
//! 另一种做法是 `get()` 返回一棵 `Value` 树（像 serde_json::Value）。那样每读一次
//! 字段就分配一次，Inspector 展开一个 `Animator` 可能几百次分配。这里全部返回
//! `&'a T`，读字段**零分配**。
//!
//! 代价：`ValueRefMut` 同时只能存在一个（不能同时拿两个 `&mut` 到不同字段）。
//! 编辑器的实际用法是"读全部 → 画 → 改一个 → 重读全部"，不违反这条。

use std::fmt;

use glam::{Quat, Vec2, Vec3, Vec4};
use serde::de::DeserializeOwned;

use super::meta::ScalarKind;
use super::{Reflect, ReflectEnum, ReflectList, ReflectMap};

/// 严格反序列化一个 JSON 值：未知嵌套字段直接报错，不静默丢弃。
///
/// 普通 `serde_json::from_value` 会忽略未声明字段；组件补丁必须与文件边界同级，
/// 否则智能体写错字段名会"看似成功、实际丢数据"。
pub fn from_json_strict<T: DeserializeOwned>(value: serde_json::Value) -> Result<T, ReflectError> {
    let type_name = std::any::type_name::<T>();
    let text = serde_json::to_string(&value).map_err(|error| ReflectError::InvalidValue {
        type_name,
        message: error.to_string(),
    })?;
    let mut deserializer = serde_json::Deserializer::from_str(&text);
    let mut ignored = Vec::new();
    let parsed = serde_ignored::deserialize(&mut deserializer, |path| ignored.push(path.to_string()))
        .map_err(|error| ReflectError::InvalidValue {
            type_name,
            message: error.to_string(),
        })?;
    deserializer.end().map_err(|error| ReflectError::InvalidValue {
        type_name,
        message: error.to_string(),
    })?;
    if ignored.is_empty() {
        Ok(parsed)
    } else {
        Err(ReflectError::InvalidValue {
            type_name,
            message: format!("不认识的字段: {}", ignored.join(", ")),
        })
    }
}

/// 反射操作的错误。全部是**可诊断**的，调用方（编辑器 / MCP）负责转成人话。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ReflectError {
    #[error("路径为空")]
    EmptyPath,
    #[error("路径语法错误：`{0}`（只支持 `字段.子字段[下标]`，字段名不能含 `.` 或 `[`）")]
    BadPath(String),
    #[error("找不到字段 `{field}`（类型 `{type_name}`）")]
    NoSuchField {
        type_name: &'static str,
        field: String,
    },
    #[error("下标 {index} 越界（长度 {len}）")]
    IndexOutOfRange { index: usize, len: usize },
    #[error("路径末尾是 `{0}`，还要继续往下一层走")]
    NotTraversable(&'static str),
    #[error("类型不匹配：目标是 `{expected}`，写入的是 `{got}`")]
    TypeMismatch {
        expected: &'static str,
        got: &'static str,
    },
    #[error("字段 `{0}` 是只读的")]
    ReadOnly(String),
    #[error("`{0}` 是 None，没有可读的值")]
    NoneValue(&'static str),
    #[error("无法把 JSON 值写入 `{type_name}`：{message}")]
    InvalidValue {
        type_name: &'static str,
        message: String,
    },
    #[error("反射容器不支持这种 JSON 写入：{0}")]
    UnsupportedJson(&'static str),
}

/// 写入用的标量值（拥有所有权，因为调用方通常是从 UI 控件拿到的临时值）。
#[derive(Debug, Clone, PartialEq)]
pub enum ScalarValue {
    F32(f32),
    F64(f64),
    I32(i32),
    I64(i64),
    U32(u32),
    U64(u64),
    Bool(bool),
    Str(String),
    Vec2(Vec2),
    Vec3(Vec3),
    Vec4(Vec4),
    Quat(Quat),
}

impl ScalarValue {
    pub fn kind(&self) -> ScalarKind {
        match self {
            Self::F32(_) => ScalarKind::F32,
            Self::F64(_) => ScalarKind::F64,
            Self::I32(_) => ScalarKind::I32,
            Self::I64(_) => ScalarKind::I64,
            Self::U32(_) => ScalarKind::U32,
            Self::U64(_) => ScalarKind::U64,
            Self::Bool(_) => ScalarKind::Bool,
            Self::Str(_) => ScalarKind::String,
            Self::Vec2(_) => ScalarKind::Vec2,
            Self::Vec3(_) => ScalarKind::Vec3,
            Self::Vec4(_) => ScalarKind::Vec4,
            Self::Quat(_) => ScalarKind::Quat,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Self::F32(v) => Some(*v),
            Self::F64(v) => Some(*v as f32),
            Self::I32(v) => Some(*v as f32),
            Self::I64(v) => Some(*v as f32),
            Self::U32(v) => Some(*v as f32),
            Self::U64(v) => Some(*v as f32),
            _ => None,
        }
    }
}

/// 只读的一次字段访问结果。
pub enum ValueRef<'a> {
    F32(&'a f32),
    F64(&'a f64),
    I32(&'a i32),
    I64(&'a i64),
    U32(&'a u32),
    U64(&'a u64),
    Bool(&'a bool),
    Str(&'a str),
    Vec2(&'a Vec2),
    Vec3(&'a Vec3),
    Vec4(&'a Vec4),
    Quat(&'a Quat),
    Struct(&'a dyn Reflect),
    Enum(&'a dyn ReflectEnum),
    List(&'a dyn ReflectList),
    Map(&'a dyn ReflectMap),
    /// `Option::None` —— 不是"错误"，是"这里现在是空的"。
    None,
}

impl fmt::Debug for ValueRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::F32(v) => write!(f, "F32({v})"),
            Self::F64(v) => write!(f, "F64({v})"),
            Self::I32(v) => write!(f, "I32({v})"),
            Self::I64(v) => write!(f, "I64({v})"),
            Self::U32(v) => write!(f, "U32({v})"),
            Self::U64(v) => write!(f, "U64({v})"),
            Self::Bool(v) => write!(f, "Bool({v})"),
            Self::Str(v) => write!(f, "Str({v:?})"),
            Self::Vec2(v) => write!(f, "Vec2({v:?})"),
            Self::Vec3(v) => write!(f, "Vec3({v:?})"),
            Self::Vec4(v) => write!(f, "Vec4({v:?})"),
            Self::Quat(v) => write!(f, "Quat({v:?})"),
            Self::Struct(v) => write!(f, "Struct({})", v.type_meta().name),
            Self::Enum(v) => write!(f, "Enum({})", v.variant()),
            Self::List(v) => write!(f, "List(len={})", v.len()),
            Self::Map(_) => write!(f, "Map"),
            Self::None => write!(f, "None"),
        }
    }
}

impl<'a> ValueRef<'a> {
    /// 标量类别；结构 / 枚举 / 列表 / None 返回 `None`。
    pub fn scalar_kind(&self) -> Option<ScalarKind> {
        Some(match self {
            Self::F32(_) => ScalarKind::F32,
            Self::F64(_) => ScalarKind::F64,
            Self::I32(_) => ScalarKind::I32,
            Self::I64(_) => ScalarKind::I64,
            Self::U32(_) => ScalarKind::U32,
            Self::U64(_) => ScalarKind::U64,
            Self::Bool(_) => ScalarKind::Bool,
            Self::Str(_) => ScalarKind::String,
            Self::Vec2(_) => ScalarKind::Vec2,
            Self::Vec3(_) => ScalarKind::Vec3,
            Self::Vec4(_) => ScalarKind::Vec4,
            Self::Quat(_) => ScalarKind::Quat,
            _ => return None,
        })
    }

    pub fn is_scalar(&self) -> bool {
        self.scalar_kind().is_some()
    }

    /// 取 `f32`。整型自动提升 —— 编辑器的数值框统一按 `f32` 编辑。
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Self::F32(v) => Some(**v),
            Self::F64(v) => Some(**v as f32),
            Self::I32(v) => Some(**v as f32),
            Self::I64(v) => Some(**v as f32),
            Self::U32(v) => Some(**v as f32),
            Self::U64(v) => Some(**v as f32),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(**v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&'a str> {
        match self {
            Self::Str(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_vec3(&self) -> Option<&'a Vec3> {
        match self {
            Self::Vec3(v) => Some(v),
            _ => None,
        }
    }

    /// 转成 owning 的 [`ScalarValue`]（列表编辑器 / MCP 输出用）。
    pub fn to_scalar(&self) -> Option<ScalarValue> {
        Some(match self {
            Self::F32(v) => ScalarValue::F32(**v),
            Self::F64(v) => ScalarValue::F64(**v),
            Self::I32(v) => ScalarValue::I32(**v),
            Self::I64(v) => ScalarValue::I64(**v),
            Self::U32(v) => ScalarValue::U32(**v),
            Self::U64(v) => ScalarValue::U64(**v),
            Self::Bool(v) => ScalarValue::Bool(**v),
            Self::Str(v) => ScalarValue::Str((*v).to_string()),
            Self::Vec2(v) => ScalarValue::Vec2(**v),
            Self::Vec3(v) => ScalarValue::Vec3(**v),
            Self::Vec4(v) => ScalarValue::Vec4(**v),
            Self::Quat(v) => ScalarValue::Quat(**v),
            _ => return None,
        })
    }

    /// 一行可读文本（控制台 / MCP 文本输出用，不用于界面排版）。
    pub fn to_display_string(&self) -> String {
        match self {
            Self::F32(v) => format!("{v}"),
            Self::F64(v) => format!("{v}"),
            Self::I32(v) => format!("{v}"),
            Self::I64(v) => format!("{v}"),
            Self::U32(v) => format!("{v}"),
            Self::U64(v) => format!("{v}"),
            Self::Bool(v) => format!("{v}"),
            Self::Str(v) => (*v).to_string(),
            Self::Vec2(v) => format!("({}, {})", v.x, v.y),
            Self::Vec3(v) => format!("({}, {}, {})", v.x, v.y, v.z),
            Self::Vec4(v) => format!("({}, {}, {}, {})", v.x, v.y, v.z, v.w),
            Self::Quat(v) => format!("({}, {}, {}, {})", v.x, v.y, v.z, v.w),
            Self::Struct(v) => format!("<{}>", v.type_meta().name),
            Self::Enum(v) => format!("<{}::{}>", "enum", v.variant()),
            Self::List(v) => format!("<list len={}>", v.len()),
            Self::Map(_) => "<map>".to_string(),
            Self::None => "None".to_string(),
        }
    }
}

/// 可写的一次字段访问结果。
pub enum ValueRefMut<'a> {
    F32(&'a mut f32),
    F64(&'a mut f64),
    I32(&'a mut i32),
    I64(&'a mut i64),
    U32(&'a mut u32),
    U64(&'a mut u64),
    Bool(&'a mut bool),
    Str(&'a mut String),
    Vec2(&'a mut Vec2),
    Vec3(&'a mut Vec3),
    Vec4(&'a mut Vec4),
    Quat(&'a mut Quat),
    Struct(&'a mut dyn Reflect),
    Enum(&'a mut dyn ReflectEnum),
    List(&'a mut dyn ReflectList),
    Map(&'a mut dyn ReflectMap),
    None,
}

impl fmt::Debug for ValueRefMut<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Struct(v) => write!(f, "StructMut({})", v.type_meta().name),
            Self::Enum(v) => write!(f, "EnumMut({})", v.variant()),
            Self::List(v) => write!(f, "ListMut(len={})", v.len()),
            Self::None => write!(f, "None"),
            other => write!(f, "ScalarMut({})", other.scalar_kind().map_or("?", ScalarKind::name)),
        }
    }
}

impl<'a> ValueRefMut<'a> {
    pub fn scalar_kind(&self) -> Option<ScalarKind> {
        Some(match self {
            Self::F32(_) => ScalarKind::F32,
            Self::F64(_) => ScalarKind::F64,
            Self::I32(_) => ScalarKind::I32,
            Self::I64(_) => ScalarKind::I64,
            Self::U32(_) => ScalarKind::U32,
            Self::U64(_) => ScalarKind::U64,
            Self::Bool(_) => ScalarKind::Bool,
            Self::Str(_) => ScalarKind::String,
            Self::Vec2(_) => ScalarKind::Vec2,
            Self::Vec3(_) => ScalarKind::Vec3,
            Self::Vec4(_) => ScalarKind::Vec4,
            Self::Quat(_) => ScalarKind::Quat,
            _ => return None,
        })
    }

    /// 写入一个标量。**类型必须完全一致** —— 不做隐式转换。
    ///
    /// 为什么不允许 `F32` 写进 `I32` 字段：整数与浮点的舍入语义不同，
    /// 静默转换会让"我输了 3.7 结果存成 3"变成难查的 bug。UI 侧自己负责
    /// 按声明的 `ScalarKind` 产出正确的 `ScalarValue`。
    pub fn set(&mut self, value: &ScalarValue) -> Result<(), ReflectError> {
        let expected = match self.scalar_kind() {
            Some(kind) => kind,
            None => {
                return Err(ReflectError::TypeMismatch {
                    expected: "scalar",
                    got: value.kind().name(),
                })
            }
        };
        let got = value.kind();
        if expected != got {
            return Err(ReflectError::TypeMismatch {
                expected: expected.name(),
                got: got.name(),
            });
        }
        match (self, value) {
            (Self::F32(slot), ScalarValue::F32(v)) => **slot = *v,
            (Self::F64(slot), ScalarValue::F64(v)) => **slot = *v,
            (Self::I32(slot), ScalarValue::I32(v)) => **slot = *v,
            (Self::I64(slot), ScalarValue::I64(v)) => **slot = *v,
            (Self::U32(slot), ScalarValue::U32(v)) => **slot = *v,
            (Self::U64(slot), ScalarValue::U64(v)) => **slot = *v,
            (Self::Bool(slot), ScalarValue::Bool(v)) => **slot = *v,
            (Self::Str(slot), ScalarValue::Str(v)) => **slot = v.clone(),
            (Self::Vec2(slot), ScalarValue::Vec2(v)) => **slot = *v,
            (Self::Vec3(slot), ScalarValue::Vec3(v)) => **slot = *v,
            (Self::Vec4(slot), ScalarValue::Vec4(v)) => **slot = *v,
            (Self::Quat(slot), ScalarValue::Quat(v)) => **slot = *v,
            // 前面的类型检查已经覆盖了所有组合，这里只是让编译器闭嘴。
            _ => unreachable!("scalar_kind 与 set 的分支必须一一对应"),
        }
        Ok(())
    }

    /// 只读视图（给"改之前想知道现在是多少"的场合）。
    ///
    /// 返回的引用借用的是 `&self`，所以生命周期短于 `ValueRefMut` 自身的 `'a` ——
    /// 这正是想要的：视图不能活过这次借用。
    pub fn as_ref(&self) -> ValueRef<'_> {
        match self {
            Self::F32(v) => ValueRef::F32(v),
            Self::F64(v) => ValueRef::F64(v),
            Self::I32(v) => ValueRef::I32(v),
            Self::I64(v) => ValueRef::I64(v),
            Self::U32(v) => ValueRef::U32(v),
            Self::U64(v) => ValueRef::U64(v),
            Self::Bool(v) => ValueRef::Bool(v),
            Self::Str(v) => ValueRef::Str(v.as_str()),
            Self::Vec2(v) => ValueRef::Vec2(v),
            Self::Vec3(v) => ValueRef::Vec3(v),
            Self::Vec4(v) => ValueRef::Vec4(v),
            Self::Quat(v) => ValueRef::Quat(v),
            Self::Struct(v) => ValueRef::Struct(&**v),
            Self::Enum(v) => ValueRef::Enum(&**v),
            Self::List(v) => ValueRef::List(&**v),
            Self::Map(v) => ValueRef::Map(&**v),
            Self::None => ValueRef::None,
        }
    }
}
