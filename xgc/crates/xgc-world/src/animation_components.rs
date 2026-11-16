//! 动画侧内建组件：最小 Animator 状态。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct Animator {
    pub clip: String,
    pub speed: f32,
    pub looping: bool,
    pub autoplay: bool,
    pub fade_time: f32,
    pub duration: f32,
    pub time: f32,
    pub base_position: [f32; 3],
    pub base_rotation: [f32; 4],
    pub base_scale: [f32; 3],
    pub blend_from: String,
    pub blend_elapsed: f32,
    pub blend_duration: f32,
}

impl Default for Animator {
    fn default() -> Self {
        Self {
            clip: String::new(),
            speed: 1.0,
            looping: true,
            autoplay: true,
            fade_time: 0.0,
            duration: 0.0,
            time: 0.0,
            base_position: [0.0; 3],
            base_rotation: [0.0, 0.0, 0.0, 1.0],
            base_scale: [1.0; 3],
            blend_from: String::new(),
            blend_elapsed: 0.0,
            blend_duration: 0.0,
        }
    }
}

impl Component for Animator {
    const ID: ComponentId = ComponentId(0x0501);
    const NAME: &'static str = "animation.animator";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "最小动画状态：剪辑 / 速度 / 循环 / 自动播放 / 淡入时长"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("clip", FieldType::String, Value::String(String::new()))
                .doc("glTF 动画剪辑名；空 = 绑定姿势"),
            FieldDescriptor::new("speed", FieldType::F32, Value::F64(1.0)).doc("播放速度倍率"),
            FieldDescriptor::new("looping", FieldType::Bool, Value::Bool(true)).doc("是否循环"),
            FieldDescriptor::new("autoplay", FieldType::Bool, Value::Bool(true))
                .doc("是否自动开始"),
            FieldDescriptor::new("fade_time", FieldType::F32, Value::F64(0.0))
                .doc("切剪辑淡入时长（秒）"),
            FieldDescriptor::new("duration", FieldType::F32, Value::F64(0.0))
                .doc("剪辑时长（秒）；0 = 未知，不循环截断"),
            FieldDescriptor::new("time", FieldType::F32, Value::F64(0.0)).doc("当前播放时间（秒）"),
            FieldDescriptor::new(
                "base_position",
                FieldType::Vec3,
                Value::Vec3([0.0, 0.0, 0.0]),
            )
            .doc("动画位移的基准位置；采样结果 = base + 关键帧偏移"),
            FieldDescriptor::new(
                "base_rotation",
                FieldType::Vec4,
                Value::Vec4([0.0, 0.0, 0.0, 1.0]),
            )
            .doc("动画旋转的基准四元数；采样结果 = base * 关键帧旋转"),
            FieldDescriptor::new("base_scale", FieldType::Vec3, Value::Vec3([1.0; 3]))
                .doc("动画缩放的基准；采样结果 = base * 关键帧缩放"),
            FieldDescriptor::new(
                "blend_from",
                FieldType::String,
                Value::String(String::new()),
            )
            .doc("转场来源剪辑；空 = 无转场"),
            FieldDescriptor::new("blend_elapsed", FieldType::F32, Value::F64(0.0))
                .doc("转场已进行时间（秒）"),
            FieldDescriptor::new("blend_duration", FieldType::F32, Value::F64(0.0))
                .doc("转场总时长（秒）；0 = 硬切"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "clip" => Some(Value::String(self.clip.clone())),
            "speed" => Some(Value::F64(f64::from(self.speed))),
            "looping" => Some(Value::Bool(self.looping)),
            "autoplay" => Some(Value::Bool(self.autoplay)),
            "fade_time" => Some(Value::F64(f64::from(self.fade_time))),
            "duration" => Some(Value::F64(f64::from(self.duration))),
            "time" => Some(Value::F64(f64::from(self.time))),
            "base_position" => Some(Value::Vec3([
                f64::from(self.base_position[0]),
                f64::from(self.base_position[1]),
                f64::from(self.base_position[2]),
            ])),
            "base_rotation" => Some(Value::Vec4([
                f64::from(self.base_rotation[0]),
                f64::from(self.base_rotation[1]),
                f64::from(self.base_rotation[2]),
                f64::from(self.base_rotation[3]),
            ])),
            "blend_from" => Some(Value::String(self.blend_from.clone())),
            "blend_elapsed" => Some(Value::F64(f64::from(self.blend_elapsed))),
            "blend_duration" => Some(Value::F64(f64::from(self.blend_duration))),
            "base_scale" => Some(Value::Vec3([
                f64::from(self.base_scale[0]),
                f64::from(self.base_scale[1]),
                f64::from(self.base_scale[2]),
            ])),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "clip" => {
                self.clip = value
                    .as_string()
                    .ok_or_else(|| {
                        format!(
                            "animation.animator.clip 期望 string，得到 {}",
                            value.type_name()
                        )
                    })?
                    .to_string();
                Ok(())
            }
            "speed" => {
                self.speed = value.as_f32().ok_or_else(|| {
                    format!(
                        "animation.animator.speed 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "looping" => {
                self.looping = value.as_bool().ok_or_else(|| {
                    format!(
                        "animation.animator.looping 期望 bool，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "autoplay" => {
                self.autoplay = value.as_bool().ok_or_else(|| {
                    format!(
                        "animation.animator.autoplay 期望 bool，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "fade_time" => {
                self.fade_time = value.as_f32().ok_or_else(|| {
                    format!(
                        "animation.animator.fade_time 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "duration" => {
                self.duration = value.as_f32().ok_or_else(|| {
                    format!(
                        "animation.animator.duration 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "time" => {
                self.time = value.as_f32().ok_or_else(|| {
                    format!(
                        "animation.animator.time 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "base_position" => {
                self.base_position = value.as_vec3().ok_or_else(|| {
                    format!(
                        "animation.animator.base_position 期望 vec3，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "base_rotation" => {
                self.base_rotation = match value {
                    Value::Vec4(value) => [
                        value[0] as f32,
                        value[1] as f32,
                        value[2] as f32,
                        value[3] as f32,
                    ],
                    other => {
                        return Err(format!(
                            "animation.animator.base_rotation 期望 vec4，得到 {}",
                            other.type_name()
                        ));
                    }
                };
                Ok(())
            }
            "blend_from" => {
                self.blend_from = match value {
                    Value::String(value) => value.clone(),
                    other => {
                        return Err(format!(
                            "animation.animator.blend_from 期望 string，得到 {}",
                            other.type_name()
                        ));
                    }
                };
                Ok(())
            }
            "blend_elapsed" => {
                self.blend_elapsed = value.as_f32().ok_or_else(|| {
                    format!(
                        "animation.animator.blend_elapsed 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "blend_duration" => {
                self.blend_duration = value.as_f32().ok_or_else(|| {
                    format!(
                        "animation.animator.blend_duration 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "base_scale" => {
                self.base_scale = value.as_vec3().ok_or_else(|| {
                    format!(
                        "animation.animator.base_scale 期望 vec3，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            _ => Err(format!("animation.animator 没有字段 {field}")),
        }
    }
}
