//! XGC 内建组件：M1 只放最小集合，够跑命令 / 查询 / schema 测试。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

/// 内建：局部变换。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

impl Component for Transform {
    const ID: ComponentId = ComponentId(0x0101);
    const NAME: &'static str = "core.transform";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "局部变换：相对父节点的平移 / 旋转 / 缩放"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("position", FieldType::Vec3, Value::Vec3([0.0; 3]))
                .unit("m")
                .doc("相对父节点的平移"),
            FieldDescriptor::new(
                "rotation",
                FieldType::Quat,
                Value::Quat([0.0, 0.0, 0.0, 1.0]),
            )
            .doc("单位四元数"),
            FieldDescriptor::new("scale", FieldType::Vec3, Value::Vec3([1.0; 3]))
                .unit("x")
                .doc("逐轴缩放"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "position" => Some(Value::Vec3([
                f64::from(self.position[0]),
                f64::from(self.position[1]),
                f64::from(self.position[2]),
            ])),
            "rotation" => Some(Value::Quat([
                f64::from(self.rotation[0]),
                f64::from(self.rotation[1]),
                f64::from(self.rotation[2]),
                f64::from(self.rotation[3]),
            ])),
            "scale" => Some(Value::Vec3([
                f64::from(self.scale[0]),
                f64::from(self.scale[1]),
                f64::from(self.scale[2]),
            ])),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "position" => {
                self.position = value.as_vec3().ok_or_else(|| {
                    format!(
                        "core.transform.position 期望 vec3，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "rotation" => {
                self.rotation = value.as_quat().ok_or_else(|| {
                    format!(
                        "core.transform.rotation 期望 quat，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "scale" => {
                self.scale = value.as_vec3().ok_or_else(|| {
                    format!("core.transform.scale 期望 vec3，得到 {}", value.type_name())
                })?;
                Ok(())
            }
            _ => Err(format!("core.transform 没有字段 {field}")),
        }
    }
}

/// 内建：速度。
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Velocity {
    pub linear: [f32; 3],
}

impl Component for Velocity {
    const ID: ComponentId = ComponentId(0x0102);
    const NAME: &'static str = "core.velocity";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "线速度"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("linear", FieldType::Vec3, Value::Vec3([0.0; 3]))
                .unit("m/s")
                .doc("世界空间线速度"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "linear" => Some(Value::Vec3([
                f64::from(self.linear[0]),
                f64::from(self.linear[1]),
                f64::from(self.linear[2]),
            ])),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "linear" => {
                self.linear = value.as_vec3().ok_or_else(|| {
                    format!("core.velocity.linear 期望 vec3，得到 {}", value.type_name())
                })?;
                Ok(())
            }
            _ => Err(format!("core.velocity 没有字段 {field}")),
        }
    }
}

/// 内建：名字（验证 String 组件的 clone / drop 与 schema）。
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Name {
    pub value: String,
}

impl Component for Name {
    const ID: ComponentId = ComponentId(0x0103);
    const NAME: &'static str = "core.name";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "显示名；不是身份，身份只认 EntityId"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("value", FieldType::String, Value::String(String::new()))
                .doc("显示名"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "value" => Some(Value::String(self.value.clone())),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "value" => {
                self.value = value
                    .as_string()
                    .ok_or_else(|| {
                        format!("core.name.value 期望 string，得到 {}", value.type_name())
                    })?
                    .to_string();
                Ok(())
            }
            _ => Err(format!("core.name 没有字段 {field}")),
        }
    }
}
