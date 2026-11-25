//! 动画侧内建组件：最小 Animator 状态。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

use crate::AnimEvent;

#[derive(Clone, Debug, PartialEq)]
pub struct Animator {
    pub clip: String,
    /// 作者写下的动画事件表；空 = 没有事件。
    ///
    /// 它是配置而不是运行时状态：非空时进 `WorldData`，但
    /// `World::animation_events` 的瞬态队列不进快照 / `state_hash`。
    pub events: Vec<AnimEvent>,
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
    /// 转场来源剪辑的播放时间（秒）；淡出那半边按旧剪辑自己的时钟走。
    pub blend_from_time: f32,
    /// 转场来源剪辑的播放速度（切换时刻定格，不跟随新状态调速）。
    pub blend_from_speed: f32,
    /// 转场来源剪辑的循环方式（切换时刻定格）。
    pub blend_from_looping: bool,
    /// 叠加层剪辑名（glTF 剪辑）；空 = 没有叠加层。
    pub additive_clip: String,
    /// 叠加层权重 ∈ [0,1]：脚本 `set_anim_additive_weight` 每帧写。
    pub additive_weight: f32,
    /// 叠加层播放速度倍率（与基础层时钟相互独立）。
    pub additive_speed: f32,
    /// 叠加层是否循环。
    pub additive_looping: bool,
    /// 叠加层播放时间（秒）；由 `AnimatorSystem` 按 `additive_speed` 推进。
    pub additive_time: f32,
    /// 根运动配置 RON：`(node: "b_Hip_01", axes: (true, false, true))`；空 = 不做根运动。
    ///
    /// 解析 / 语义在 `xgc_world::root_motion`；放字符串是因为它只在迁入时写一次，
    /// 运行时读出来做位移，没有逐帧改字段的需求。
    pub root_motion: String,
}

impl Default for Animator {
    fn default() -> Self {
        Self {
            clip: String::new(),
            events: Vec::new(),
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
            blend_from_time: 0.0,
            blend_from_speed: 1.0,
            blend_from_looping: true,
            additive_clip: String::new(),
            additive_weight: 0.0,
            additive_speed: 1.0,
            additive_looping: true,
            additive_time: 0.0,
            root_motion: String::new(),
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
        "最小动画状态：剪辑 / 速度 / 循环 / 自动播放 / 淡入时长 / 叠加层"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("clip", FieldType::String, Value::String(String::new()))
                .doc("glTF 动画剪辑名；空 = 绑定姿势"),
            AnimEvent::list_field(),
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
            FieldDescriptor::new("blend_from_time", FieldType::F32, Value::F64(0.0))
                .doc("转场来源剪辑的播放时间（秒），按旧剪辑自己的时钟推进"),
            FieldDescriptor::new("blend_from_speed", FieldType::F32, Value::F64(1.0))
                .doc("转场来源剪辑的播放速度（切换时刻定格）"),
            FieldDescriptor::new("blend_from_looping", FieldType::Bool, Value::Bool(true))
                .doc("转场来源剪辑是否循环（切换时刻定格）"),
            FieldDescriptor::new(
                "additive_clip",
                FieldType::String,
                Value::String(String::new()),
            )
            .doc("叠加层剪辑名；增量 = 当前帧 − 该剪辑第 0 帧参考姿势"),
            FieldDescriptor::new("additive_weight", FieldType::F32, Value::F64(0.0))
                .doc("叠加层权重 ∈ [0,1]；0 = 不叠加"),
            FieldDescriptor::new("additive_speed", FieldType::F32, Value::F64(1.0))
                .doc("叠加层播放速度倍率（独立时钟）"),
            FieldDescriptor::new("additive_looping", FieldType::Bool, Value::Bool(true))
                .doc("叠加层是否循环"),
            FieldDescriptor::new("additive_time", FieldType::F32, Value::F64(0.0))
                .doc("叠加层播放时间（秒），由 AnimatorSystem 推进"),
            FieldDescriptor::new(
                "root_motion",
                FieldType::String,
                Value::String(String::new()),
            )
            .doc("根运动配置 RON（node + axes）；空 = 不做根运动"),
        ]
    }

    /// `events` 是作者配置，不是运行时状态：保留在 `fields()`（落盘 / schema /
    /// 校验都要它），但不进 `state_hash`，否则给任意 Animator 加一行事件都会
    /// 让验收参考哈希整体漂移。
    fn state_fields() -> Vec<FieldDescriptor> {
        Self::fields()
            .into_iter()
            .filter(|field| field.name != "events")
            .collect()
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "clip" => Some(Value::String(self.clip.clone())),
            "events" => Some(Value::List(
                self.events.iter().map(AnimEvent::to_value).collect(),
            )),
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
            "additive_clip" => Some(Value::String(self.additive_clip.clone())),
            "additive_weight" => Some(Value::F64(f64::from(self.additive_weight))),
            "additive_speed" => Some(Value::F64(f64::from(self.additive_speed))),
            "additive_looping" => Some(Value::Bool(self.additive_looping)),
            "additive_time" => Some(Value::F64(f64::from(self.additive_time))),
            "root_motion" => Some(Value::String(self.root_motion.clone())),
            "blend_from" => Some(Value::String(self.blend_from.clone())),
            "blend_elapsed" => Some(Value::F64(f64::from(self.blend_elapsed))),
            "blend_duration" => Some(Value::F64(f64::from(self.blend_duration))),
            "blend_from_time" => Some(Value::F64(f64::from(self.blend_from_time))),
            "blend_from_speed" => Some(Value::F64(f64::from(self.blend_from_speed))),
            "blend_from_looping" => Some(Value::Bool(self.blend_from_looping)),
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
            "events" => {
                let Value::List(events) = value else {
                    return Err("animation.animator.events 期望事件列表".to_string());
                };
                self.events = events
                    .iter()
                    .enumerate()
                    .map(|(index, event)| {
                        AnimEvent::from_value(event)
                            .map_err(|error| format!("animation.animator.events[{index}]: {error}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
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
            "additive_clip" => {
                self.additive_clip = value
                    .as_string()
                    .ok_or_else(|| {
                        format!(
                            "animation.animator.additive_clip 期望 string，得到 {}",
                            value.type_name()
                        )
                    })?
                    .to_string();
                Ok(())
            }
            "additive_weight" => {
                self.additive_weight = value.as_f32().ok_or_else(|| {
                    format!(
                        "animation.animator.additive_weight 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "additive_speed" => {
                self.additive_speed = value.as_f32().ok_or_else(|| {
                    format!(
                        "animation.animator.additive_speed 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "additive_looping" => {
                self.additive_looping = value.as_bool().ok_or_else(|| {
                    format!(
                        "animation.animator.additive_looping 期望 bool，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "additive_time" => {
                self.additive_time = value.as_f32().ok_or_else(|| {
                    format!(
                        "animation.animator.additive_time 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
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
            "blend_from_time" => {
                self.blend_from_time = value.as_f32().ok_or_else(|| {
                    format!(
                        "animation.animator.blend_from_time 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "blend_from_speed" => {
                self.blend_from_speed = value.as_f32().ok_or_else(|| {
                    format!(
                        "animation.animator.blend_from_speed 期望 f32，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "blend_from_looping" => {
                self.blend_from_looping = value.as_bool().ok_or_else(|| {
                    format!(
                        "animation.animator.blend_from_looping 期望 bool，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "root_motion" => {
                self.root_motion = match value {
                    Value::String(value) => value.clone(),
                    other => {
                        return Err(format!(
                            "animation.animator.root_motion 期望 string，得到 {}",
                            other.type_name()
                        ));
                    }
                };
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
