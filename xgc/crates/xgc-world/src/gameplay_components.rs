//! 玩法侧数据组件：脚本引用 / 音频源。这里只存作者数据，运行时由脚本 /
//! 音频系统消费。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

/// 脚本引用：项目相对路径；M1 只迁移 / 存盘，Lua 运行时后续接。
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Script {
    pub path: String,
}

impl Component for Script {
    const ID: ComponentId = ComponentId(0x0301);
    const NAME: &'static str = "script.lua";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "脚本路径；单轨只认 .lua"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("path", FieldType::String, Value::String(String::new()))
                .doc("项目相对路径"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "path" => Some(Value::String(self.path.clone())),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "path" => {
                self.path = value
                    .as_string()
                    .ok_or_else(|| {
                        format!("script.lua.path 期望 string，得到 {}", value.type_name())
                    })?
                    .to_string();
                Ok(())
            }
            _ => Err(format!("script.lua 没有字段 {field}")),
        }
    }
}

/// 音频源：剪辑 + 播放参数；总线 / 距离衰减等完整参数后续扩展。
#[derive(Clone, Debug, PartialEq)]
pub struct AudioSource {
    pub clip: String,
    pub volume: f32,
    pub pitch: f32,
    pub looping: bool,
    pub autoplay: bool,
    pub spatial: bool,
}

impl Default for AudioSource {
    fn default() -> Self {
        Self {
            clip: String::new(),
            volume: 1.0,
            pitch: 1.0,
            looping: false,
            autoplay: false,
            spatial: true,
        }
    }
}

impl Component for AudioSource {
    const ID: ComponentId = ComponentId(0x0302);
    const NAME: &'static str = "audio.source";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "音频源：剪辑 + 音量 / 速度 / 循环 / 自动播放 / 空间化"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("clip", FieldType::String, Value::String(String::new()))
                .doc("音频路径"),
            FieldDescriptor::new("volume", FieldType::F32, Value::F64(1.0)).doc("线性音量 0..1"),
            FieldDescriptor::new("pitch", FieldType::F32, Value::F64(1.0)).doc("播放速率 / 音高"),
            FieldDescriptor::new("looping", FieldType::Bool, Value::Bool(false)).doc("是否循环"),
            FieldDescriptor::new("autoplay", FieldType::Bool, Value::Bool(false))
                .doc("进场景自动播放"),
            FieldDescriptor::new("spatial", FieldType::Bool, Value::Bool(true))
                .doc("是否 3D 空间化"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "clip" => Some(Value::String(self.clip.clone())),
            "volume" => Some(Value::F64(f64::from(self.volume))),
            "pitch" => Some(Value::F64(f64::from(self.pitch))),
            "looping" => Some(Value::Bool(self.looping)),
            "autoplay" => Some(Value::Bool(self.autoplay)),
            "spatial" => Some(Value::Bool(self.spatial)),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "clip" => {
                self.clip = value
                    .as_string()
                    .ok_or_else(|| {
                        format!("audio.source.clip 期望 string，得到 {}", value.type_name())
                    })?
                    .to_string();
                Ok(())
            }
            "volume" => {
                self.volume = value.as_f32().ok_or_else(|| {
                    format!("audio.source.volume 期望 f32，得到 {}", value.type_name())
                })?;
                Ok(())
            }
            "pitch" => {
                self.pitch = value.as_f32().ok_or_else(|| {
                    format!("audio.source.pitch 期望 f32，得到 {}", value.type_name())
                })?;
                Ok(())
            }
            "looping" => {
                self.looping = value.as_bool().ok_or_else(|| {
                    format!("audio.source.looping 期望 bool，得到 {}", value.type_name())
                })?;
                Ok(())
            }
            "autoplay" => {
                self.autoplay = value.as_bool().ok_or_else(|| {
                    format!(
                        "audio.source.autoplay 期望 bool，得到 {}",
                        value.type_name()
                    )
                })?;
                Ok(())
            }
            "spatial" => {
                self.spatial = value.as_bool().ok_or_else(|| {
                    format!("audio.source.spatial 期望 bool，得到 {}", value.type_name())
                })?;
                Ok(())
            }
            _ => Err(format!("audio.source 没有字段 {field}")),
        }
    }
}

/// 音频监听者：`active` 时按 Transform 位置计算距离衰减。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioListener {
    pub active: bool,
}

impl Default for AudioListener {
    fn default() -> Self {
        Self { active: true }
    }
}

impl Component for AudioListener {
    const ID: ComponentId = ComponentId(0x0303);
    const NAME: &'static str = "audio.listener";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "音频监听者：active 时按 Transform 位置计算距离衰减"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![FieldDescriptor::new(
            "active",
            FieldType::Bool,
            Value::Bool(true),
        )]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        (field == "active").then_some(Value::Bool(self.active))
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        if field != "active" {
            return Err(format!("audio.listener 没有字段 {field}"));
        }
        self.active = value.as_bool().ok_or_else(|| {
            format!(
                "audio.listener.active 期望 bool，得到 {}",
                value.type_name()
            )
        })?;
        Ok(())
    }
}

/// 运行时音频增益：由 AudioMixerSystem 写入。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioGain {
    pub gain: f32,
}

impl Default for AudioGain {
    fn default() -> Self {
        Self { gain: 1.0 }
    }
}

impl Component for AudioGain {
    const ID: ComponentId = ComponentId(0x0304);
    const NAME: &'static str = "audio.gain";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "运行时音频增益：AudioMixerSystem 写入，0..4"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![FieldDescriptor::new(
            "gain",
            FieldType::F32,
            Value::F64(1.0),
        )]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        (field == "gain").then_some(Value::F64(f64::from(self.gain)))
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        if field != "gain" {
            return Err(format!("audio.gain 没有字段 {field}"));
        }
        self.gain = value
            .as_f32()
            .ok_or_else(|| format!("audio.gain.gain 期望 f32，得到 {}", value.type_name()))?;
        Ok(())
    }
}
