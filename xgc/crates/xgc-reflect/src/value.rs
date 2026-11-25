//! Value 的读取 / 类型名 / 状态哈希辅助。

use std::collections::BTreeMap;

use super::{FieldType, Value};

impl Value {
    /// 结构体必须包含且仅包含声明的字段；默认值用于构造新值。
    pub fn fits(&self, ty: &FieldType) -> bool {
        match (self, ty) {
            (Self::Bool(_), FieldType::Bool) => true,
            (Self::I64(_), FieldType::I64) => true,
            (Self::I64(value), FieldType::U32) => (0..=u32::MAX as i64).contains(value),
            (Self::F64(_), FieldType::F32 | FieldType::F64) => true,
            (Self::String(_), FieldType::String) => true,
            (Self::Vec2(_), FieldType::Vec2) => true,
            (Self::Vec3(_), FieldType::Vec3) => true,
            (Self::Vec4(_), FieldType::Vec4) => true,
            (Self::Quat(_), FieldType::Quat) => true,
            (Self::Entity(_), FieldType::Entity) => true,
            (Self::List(values), FieldType::List(element)) => {
                values.iter().all(|value| value.fits(&element.ty))
            }
            (Self::Struct(values), FieldType::Struct(fields)) => struct_fits(
                values,
                fields,
                |field| field.name,
                |field, value| value.fits(&field.ty),
            ),
            (Self::Map(values), FieldType::Map(element)) => {
                values.values().all(|value| value.fits(&element.ty))
            }
            (Self::Enum { variant, payload }, FieldType::Enum(variants)) => variants
                .iter()
                .find(|branch| branch.name == variant)
                .is_some_and(|branch| payload.fits(&branch.ty)),
            _ => false,
        }
    }

    pub fn kind(&self) -> &'static str {
        self.type_name()
    }

    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Bool(_) => "bool",
            Self::I64(_) => "i64",
            Self::F64(_) => "f64",
            Self::String(_) => "string",
            Self::Vec2(_) => "vec2",
            Self::Vec3(_) => "vec3",
            Self::Vec4(_) => "vec4",
            Self::Quat(_) => "quat",
            Self::Entity(_) => "entity",
            Self::List(_) => "list",
            Self::Struct(_) => "struct",
            Self::Map(_) => "map",
            Self::Enum { .. } => "enum",
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Self::F64(value) => Some(*value as f32),
            _ => None,
        }
    }

    pub fn as_vec2(&self) -> Option<[f32; 2]> {
        match self {
            Self::Vec2(value) => Some([value[0] as f32, value[1] as f32]),
            _ => None,
        }
    }

    pub fn as_vec3(&self) -> Option<[f32; 3]> {
        match self {
            Self::Vec3(value) => Some([value[0] as f32, value[1] as f32, value[2] as f32]),
            _ => None,
        }
    }

    pub fn as_quat(&self) -> Option<[f32; 4]> {
        match self {
            Self::Quat(value) => Some([
                value[0] as f32,
                value[1] as f32,
                value[2] as f32,
                value[3] as f32,
            ]),
            _ => None,
        }
    }

    pub fn as_vec4(&self) -> Option<[f32; 4]> {
        match self {
            Self::Vec4(value) => Some([
                value[0] as f32,
                value[1] as f32,
                value[2] as f32,
                value[3] as f32,
            ]),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    /// 状态哈希用的稳定混合；只求同输入同输出，不做密码学。
    pub fn hash_into(&self, hash: &mut u64) {
        fn mix(hash: &mut u64, value: u64) {
            *hash ^= value;
            *hash = hash.wrapping_mul(0x100000001b3);
        }

        match self {
            Self::Bool(value) => mix(hash, u64::from(*value)),
            Self::I64(value) => mix(hash, *value as u64),
            Self::F64(value) => mix(hash, value.to_bits()),
            Self::String(value) => {
                for byte in value.bytes() {
                    mix(hash, u64::from(byte));
                }
            }
            Self::Vec2(value) => {
                mix(hash, value[0].to_bits());
                mix(hash, value[1].to_bits());
            }
            Self::Vec3(value) => {
                mix(hash, value[0].to_bits());
                mix(hash, value[1].to_bits());
                mix(hash, value[2].to_bits());
            }
            Self::Vec4(value) | Self::Quat(value) => {
                mix(hash, value[0].to_bits());
                mix(hash, value[1].to_bits());
                mix(hash, value[2].to_bits());
                mix(hash, value[3].to_bits());
            }
            Self::Entity(value) => mix(hash, *value),
            Self::List(values) => {
                mix(hash, u64::from(b'['));
                mix(hash, values.len() as u64);
                for value in values {
                    mix_child(hash, value);
                }
            }
            Self::Struct(fields) => mix_fields(hash, b'{', fields),
            Self::Map(fields) => mix_fields(hash, b'm', fields),
            Self::Enum { variant, payload } => {
                mix(hash, u64::from(b'e'));
                mix_name(hash, variant);
                mix_child(hash, payload);
            }
        }

        fn mix_fields(hash: &mut u64, tag: u8, fields: &BTreeMap<String, Value>) {
            mix(hash, u64::from(tag));
            mix(hash, fields.len() as u64);
            for (name, value) in fields {
                mix_name(hash, name);
                mix_child(hash, value);
            }
        }

        fn mix_name(hash: &mut u64, name: &str) {
            mix(hash, name.len() as u64);
            for byte in name.bytes() {
                mix(hash, u64::from(byte));
            }
        }

        // 递归节点的边界与类型都要混入，否则异型列表可能混成同一串数值。
        fn mix_child(hash: &mut u64, value: &Value) {
            for byte in value.kind().bytes() {
                mix(hash, u64::from(byte));
            }
            if let Value::String(text) = value {
                mix(hash, text.len() as u64);
            }
            value.hash_into(hash);
        }
    }
}

pub(super) fn struct_fits<F>(
    values: &BTreeMap<String, Value>,
    fields: &[F],
    name: impl Fn(&F) -> &str,
    fits: impl Fn(&F, &Value) -> bool,
) -> bool {
    fields.len() == values.len()
        && values.iter().all(|(key, value)| {
            fields
                .iter()
                .find(|field| name(field) == key)
                .is_some_and(|field| fits(field, value))
        })
}
