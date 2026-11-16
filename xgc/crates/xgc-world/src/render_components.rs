//! 渲染侧内建组件：网格引用 / 最小材质 / 相机。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

/// 内建：可渲染网格引用；模型路径是项目相对路径 / builtin URI。
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MeshRenderer {
    pub model: String,
}

impl Component for MeshRenderer {
    const ID: ComponentId = ComponentId(0x0201);
    const NAME: &'static str = "render.mesh";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "网格引用；渲染层按 model 路径解析资产"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("model", FieldType::String, Value::String(String::new()))
                .doc("模型路径 / builtin URI"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "model" => Some(Value::String(self.model.clone())),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "model" => {
                self.model = value
                    .as_string()
                    .ok_or_else(|| {
                        format!("render.mesh.model 期望 string，得到 {}", value.type_name())
                    })?
                    .to_string();
                Ok(())
            }
            _ => Err(format!("render.mesh 没有字段 {field}")),
        }
    }
}

/// 内建：最小材质参数；完整 PBR 资产后续按需扩展。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    pub base_color: [f32; 4],
    pub roughness: f32,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            base_color: [1.0; 4],
            roughness: 0.5,
        }
    }
}

impl Component for Material {
    const ID: ComponentId = ComponentId(0x0202);
    const NAME: &'static str = "render.material";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "最小材质：基色 + 粗糙度；完整 PBR 资产后续扩展"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("base_color", FieldType::Vec4, Value::Vec4([1.0; 4]))
                .doc("线性基色 RGBA"),
            FieldDescriptor::new("roughness", FieldType::F32, Value::F64(0.5)).doc("粗糙度 0..1"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "base_color" => Some(Value::Vec4([
                f64::from(self.base_color[0]),
                f64::from(self.base_color[1]),
                f64::from(self.base_color[2]),
                f64::from(self.base_color[3]),
            ])),
            "roughness" => Some(Value::F64(f64::from(self.roughness))),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "base_color" => {
                self.base_color = value.as_vec4().ok_or_else(|| {
                    format!(
                        "render.material.base_color 期望 vec4，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "roughness" => {
                self.roughness = value.as_f32().ok_or_else(|| {
                    format!(
                        "render.material.roughness 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            _ => Err(format!("render.material 没有字段 {field}")),
        }
    }
}

/// 内建：相机参数；active 决定渲染图选哪台相机。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub fov_y_deg: f32,
    pub near: f32,
    pub far: f32,
    pub active: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            fov_y_deg: 60.0,
            near: 0.1,
            far: 1000.0,
            active: true,
        }
    }
}

impl Component for Camera {
    const ID: ComponentId = ComponentId(0x0203);
    const NAME: &'static str = "render.camera";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "透视相机参数；同一时刻只认第一台 active 相机"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("fov_y_deg", FieldType::F32, Value::F64(60.0))
                .unit("deg")
                .doc("垂直视场角"),
            FieldDescriptor::new("near", FieldType::F32, Value::F64(0.1))
                .unit("m")
                .doc("近裁剪面"),
            FieldDescriptor::new("far", FieldType::F32, Value::F64(1000.0))
                .unit("m")
                .doc("远裁剪面"),
            FieldDescriptor::new("active", FieldType::Bool, Value::Bool(true))
                .doc("是否作为主相机"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "fov_y_deg" => Some(Value::F64(f64::from(self.fov_y_deg))),
            "near" => Some(Value::F64(f64::from(self.near))),
            "far" => Some(Value::F64(f64::from(self.far))),
            "active" => Some(Value::Bool(self.active)),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "fov_y_deg" => {
                self.fov_y_deg = value.as_f32().ok_or_else(|| {
                    format!(
                        "render.camera.fov_y_deg 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "near" => {
                self.near = value.as_f32().ok_or_else(|| {
                    format!("render.camera.near 期望 f32，得到 {}", value.type_name())
                })?;
                Ok(())
            }
            "far" => {
                self.far = value.as_f32().ok_or_else(|| {
                    format!("render.camera.far 期望 f32，得到 {}", value.type_name())
                })?;
                Ok(())
            }
            "active" => {
                self.active = value.as_bool().ok_or_else(|| {
                    format!("render.camera.active 期望 bool，得到 {}", value.type_name())
                })?;
                Ok(())
            }
            _ => Err(format!("render.camera 没有字段 {field}")),
        }
    }
}
