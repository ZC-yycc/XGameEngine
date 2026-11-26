//! XGC 内建组件：M1 只放最小集合，够跑命令 / 查询 / schema 测试。

use xgc_base::EntityId;
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
#[derive(Clone, Debug, PartialEq, Default, Component)]
#[component(
    id = 0x0103,
    name = "core.name",
    doc = "显示名；不是身份，身份只认 EntityId"
)]
pub struct Name {
    #[field(doc = "显示名")]
    pub value: String,
}

/// 内建：父节点引用；层级系统据此算 `GlobalTransform`。
#[derive(Clone, Copy, Debug, PartialEq, Component)]
#[component(
    id = 0x0104,
    name = "core.parent",
    doc = "父实体；只认 EntityId，不认运行时句柄"
)]
pub struct Parent {
    #[field(doc = "父实体 ID")]
    pub entity: EntityId,
}

impl Default for Parent {
    fn default() -> Self {
        Self {
            entity: EntityId::from_raw(0),
        }
    }
}

/// 内建：世界空间最终变换；层级系统每帧写它，渲染只读它。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobalTransform {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl Default for GlobalTransform {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

impl GlobalTransform {
    pub fn from_local(local: &Transform) -> Self {
        Self {
            position: local.position,
            rotation: local.rotation,
            scale: local.scale,
        }
    }

    /// 父全局变换 ∘ 本地变换。旋转按四元数复合，缩放逐轴相乘。
    pub fn compose(parent: &Self, local: &Transform) -> Self {
        let scaled = [
            local.position[0] * parent.scale[0],
            local.position[1] * parent.scale[1],
            local.position[2] * parent.scale[2],
        ];
        let rotated = quat_rotate(parent.rotation, scaled);
        Self {
            position: [
                parent.position[0] + rotated[0],
                parent.position[1] + rotated[1],
                parent.position[2] + rotated[2],
            ],
            rotation: normalize_quat(quat_mul(parent.rotation, local.rotation)),
            scale: [
                parent.scale[0] * local.scale[0],
                parent.scale[1] * local.scale[1],
                parent.scale[2] * local.scale[2],
            ],
        }
    }
}

impl Component for GlobalTransform {
    const ID: ComponentId = ComponentId(0x0105);
    const NAME: &'static str = "core.global_transform";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "世界空间最终变换；由父链合成，不应由编辑器直接写"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("position", FieldType::Vec3, Value::Vec3([0.0; 3]))
                .unit("m")
                .doc("世界空间平移"),
            FieldDescriptor::new(
                "rotation",
                FieldType::Quat,
                Value::Quat([0.0, 0.0, 0.0, 1.0]),
            )
            .doc("世界空间单位四元数"),
            FieldDescriptor::new("scale", FieldType::Vec3, Value::Vec3([1.0; 3]))
                .unit("x")
                .doc("世界空间逐轴缩放"),
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
                let values = value.as_vec3().ok_or_else(|| {
                    format!(
                        "core.global_transform.position 期望 vec3，得到 {}",
                        value.type_name()
                    )
                })?;
                self.position = values;
                Ok(())
            }
            "rotation" => {
                let values = value.as_quat().ok_or_else(|| {
                    format!(
                        "core.global_transform.rotation 期望 quat，得到 {}",
                        value.type_name()
                    )
                })?;
                self.rotation = values;
                Ok(())
            }
            "scale" => {
                let values = value.as_vec3().ok_or_else(|| {
                    format!(
                        "core.global_transform.scale 期望 vec3，得到 {}",
                        value.type_name()
                    )
                })?;
                self.scale = values;
                Ok(())
            }
            _ => Err(format!("core.global_transform 没有字段 {field}")),
        }
    }
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

pub(crate) fn quat_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let [qx, qy, qz, qw] = q;
    let uv = [
        qy * v[2] - qz * v[1],
        qz * v[0] - qx * v[2],
        qx * v[1] - qy * v[0],
    ];
    let uuv = [
        qy * uv[2] - qz * uv[1],
        qz * uv[0] - qx * uv[2],
        qx * uv[1] - qy * uv[0],
    ];
    [
        v[0] + 2.0 * (qw * uv[0] + uuv[0]),
        v[1] + 2.0 * (qw * uv[1] + uuv[1]),
        v[2] + 2.0 * (qw * uv[2] + uuv[2]),
    ]
}

fn normalize_quat(q: [f32; 4]) -> [f32; 4] {
    let length = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if length <= f32::EPSILON {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        [q[0] / length, q[1] / length, q[2] / length, q[3] / length]
    }
}
