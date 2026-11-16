//! 斜坡地面：X 轴方向线性抬升，用于角色贴地与台阶之外的连续坡面。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slope {
    pub origin: [f32; 3],
    pub size: [f32; 2],
    pub rise: f32,
}

impl Default for Slope {
    fn default() -> Self {
        Self {
            origin: [0.0; 3],
            size: [10.0, 10.0],
            rise: 0.0,
        }
    }
}

impl Slope {
    /// 斜坡在 (x, z) 处的高度；不在坡面内返回 None。
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        if self.size[0] <= 0.0 || self.size[1] <= 0.0 {
            return None;
        }
        let local_x = x - self.origin[0];
        let local_z = z - self.origin[2];
        if local_x < 0.0 || local_z < 0.0 || local_x > self.size[0] || local_z > self.size[1] {
            return None;
        }
        Some(self.origin[1] + self.rise * (local_x / self.size[0]))
    }
}

impl Component for Slope {
    const ID: ComponentId = ComponentId(0x0403);
    const NAME: &'static str = "physics.slope";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "斜坡地面：沿 +X 线性抬升 rise"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("origin", FieldType::Vec3, Value::Vec3([0.0; 3])),
            FieldDescriptor::new("size", FieldType::Vec2, Value::Vec2([10.0, 10.0]))
                .doc("X / Z 方向覆盖范围"),
            FieldDescriptor::new("rise", FieldType::F32, Value::F64(0.0)).doc("X 方向总抬升"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "origin" => Some(Value::Vec3([
                f64::from(self.origin[0]),
                f64::from(self.origin[1]),
                f64::from(self.origin[2]),
            ])),
            "size" => Some(Value::Vec2([
                f64::from(self.size[0]),
                f64::from(self.size[1]),
            ])),
            "rise" => Some(Value::F64(f64::from(self.rise))),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "origin" => {
                self.origin = value.as_vec3().ok_or_else(|| {
                    format!("physics.slope.origin 期望 vec3，得到 {}", value.type_name())
                })?;
            }
            "size" => {
                self.size = value.as_vec2().ok_or_else(|| {
                    format!("physics.slope.size 期望 vec2，得到 {}", value.type_name())
                })?;
            }
            "rise" => {
                self.rise = value.as_f32().ok_or_else(|| {
                    format!("physics.slope.rise 期望 f32，得到 {}", value.type_name())
                })?;
            }
            _ => return Err(format!("physics.slope 没有字段 {field}")),
        }
        Ok(())
    }
}
