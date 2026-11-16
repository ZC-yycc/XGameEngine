//! Value 的读取 / 类型名 / 状态哈希辅助。

use super::Value;

impl Value {
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
        }
    }
}
