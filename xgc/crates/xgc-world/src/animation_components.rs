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
            _ => Err(format!("animation.animator 没有字段 {field}")),
        }
    }
}
