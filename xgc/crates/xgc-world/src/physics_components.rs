//! 物理侧数据组件：只存作者参数，solver / 后端在后续接。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

/// 物理体参数。`kind` 与 `shape` 用稳定字符串，避免把后端枚举写进 schema。
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicsBody {
    pub kind: String,
    pub shape: String,
    pub half_extents: [f32; 3],
    pub radius: f32,
    pub half_height: f32,
    pub friction: f32,
    pub restitution: f32,
    pub mass: f32,
    pub gravity_scale: f32,
    pub is_sensor: bool,
}

impl Default for PhysicsBody {
    fn default() -> Self {
        Self {
            kind: "dynamic".to_string(),
            shape: "auto".to_string(),
            half_extents: [0.5; 3],
            radius: 0.5,
            half_height: 0.5,
            friction: 0.5,
            restitution: 0.0,
            mass: 0.0,
            gravity_scale: 1.0,
            is_sensor: false,
        }
    }
}

/// `PhysicsBody.kind` 的解析结果。
///
/// 建体、回写、地面夹持、AABB 分离都必须走这一处判据：此前各处分别用
/// `match kind { "static" ... _ => Dynamic }` 和 `kind == "dynamic"` 两种写法，
/// 拼错的字符串（比如 `"Dynamic"`）会一边建出动态体、另一边不回写，实体静默冻住。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind {
    Static,
    Kinematic,
    Dynamic,
}

impl BodyKind {
    /// 未知字符串一律按 `Dynamic` 处理（与旧端 `body_from_scene` 的兜底一致）。
    pub fn parse(kind: &str) -> Self {
        match kind {
            "static" => BodyKind::Static,
            "kinematic" => BodyKind::Kinematic,
            _ => BodyKind::Dynamic,
        }
    }
}

impl PhysicsBody {
    pub fn kind(&self) -> BodyKind {
        BodyKind::parse(&self.kind)
    }

    pub fn is_dynamic(&self) -> bool {
        self.kind() == BodyKind::Dynamic
    }
}

impl Component for PhysicsBody {
    const ID: ComponentId = ComponentId(0x0401);
    const NAME: &'static str = "physics.body";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "物理体参数：类型 / 形状 / 摩擦 / 弹性 / 质量 / 重力缩放 / 传感器"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new(
                "kind",
                FieldType::String,
                Value::String("dynamic".to_string()),
            )
            .doc("static / dynamic / kinematic"),
            FieldDescriptor::new(
                "shape",
                FieldType::String,
                Value::String("auto".to_string()),
            )
            .doc("auto / cuboid / ball / capsule / cylinder"),
            FieldDescriptor::new("half_extents", FieldType::Vec3, Value::Vec3([0.5; 3]))
                .unit("m")
                .doc("cuboid 半边长"),
            FieldDescriptor::new("radius", FieldType::F32, Value::F64(0.5))
                .unit("m")
                .doc("ball / capsule / cylinder 半径"),
            FieldDescriptor::new("half_height", FieldType::F32, Value::F64(0.5))
                .unit("m")
                .doc("capsule / cylinder 半高"),
            FieldDescriptor::new("friction", FieldType::F32, Value::F64(0.5)).doc("库仑摩擦"),
            FieldDescriptor::new("restitution", FieldType::F32, Value::F64(0.0)).doc("弹性 0..1"),
            FieldDescriptor::new("mass", FieldType::F32, Value::F64(0.0))
                .unit("kg")
                .doc("质量；0 = 按密度推算"),
            FieldDescriptor::new("gravity_scale", FieldType::F32, Value::F64(1.0)).doc("重力缩放"),
            FieldDescriptor::new("is_sensor", FieldType::Bool, Value::Bool(false))
                .doc("传感器 / 触发器"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "kind" => Some(Value::String(self.kind.clone())),
            "shape" => Some(Value::String(self.shape.clone())),
            "half_extents" => Some(Value::Vec3([
                f64::from(self.half_extents[0]),
                f64::from(self.half_extents[1]),
                f64::from(self.half_extents[2]),
            ])),
            "radius" => Some(Value::F64(f64::from(self.radius))),
            "half_height" => Some(Value::F64(f64::from(self.half_height))),
            "friction" => Some(Value::F64(f64::from(self.friction))),
            "restitution" => Some(Value::F64(f64::from(self.restitution))),
            "mass" => Some(Value::F64(f64::from(self.mass))),
            "gravity_scale" => Some(Value::F64(f64::from(self.gravity_scale))),
            "is_sensor" => Some(Value::Bool(self.is_sensor)),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "kind" => {
                self.kind = value
                    .as_string()
                    .ok_or_else(|| {
                        format!("physics.body.kind 期望 string，得到 {}", value.type_name())
                    })?
                    .to_string();
                Ok(())
            }
            "shape" => {
                self.shape = value
                    .as_string()
                    .ok_or_else(|| {
                        format!("physics.body.shape 期望 string，得到 {}", value.type_name())
                    })?
                    .to_string();
                Ok(())
            }
            "half_extents" => {
                self.half_extents = value.as_vec3().ok_or_else(|| {
                    format!(
                        "physics.body.half_extents 期望 vec3，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "radius" => {
                self.radius = value.as_f32().ok_or_else(|| {
                    format!("physics.body.radius 期望 f32，得到 {}", value.type_name())
                })?;
                Ok(())
            }
            "half_height" => {
                self.half_height = value.as_f32().ok_or_else(|| {
                    format!(
                        "physics.body.half_height 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "friction" => {
                self.friction = value.as_f32().ok_or_else(|| {
                    format!("physics.body.friction 期望 f32，得到 {}", value.type_name())
                })?;
                Ok(())
            }
            "restitution" => {
                self.restitution = value.as_f32().ok_or_else(|| {
                    format!(
                        "physics.body.restitution 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "mass" => {
                self.mass = value.as_f32().ok_or_else(|| {
                    format!("physics.body.mass 期望 f32，得到 {}", value.type_name())
                })?;
                Ok(())
            }
            "gravity_scale" => {
                self.gravity_scale = value.as_f32().ok_or_else(|| {
                    format!(
                        "physics.body.gravity_scale 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "is_sensor" => {
                self.is_sensor = value.as_bool().ok_or_else(|| {
                    format!(
                        "physics.body.is_sensor 期望 bool，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            _ => Err(format!("physics.body 没有字段 {field}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_kind_parsing_follows_the_old_engine_fallback() {
        assert_eq!(BodyKind::parse("static"), BodyKind::Static);
        assert_eq!(BodyKind::parse("kinematic"), BodyKind::Kinematic);
        assert_eq!(BodyKind::parse("dynamic"), BodyKind::Dynamic);
        // 旧端 `body_from_scene` 的兜底：不认识的一律当动态体，别让拼错的字符串
        // 变成「建了刚体但不回写」的半托管状态。
        assert_eq!(BodyKind::parse("Dynamic"), BodyKind::Dynamic);
        assert_eq!(BodyKind::parse(""), BodyKind::Dynamic);
    }

    #[test]
    fn physics_body_exposes_the_shared_kind_judgement() {
        let mut body = PhysicsBody::default();
        assert!(body.is_dynamic());
        body.kind = "static".to_string();
        assert!(!body.is_dynamic());
        assert_eq!(body.kind(), BodyKind::Static);
    }
}
