//! 玩法侧数据组件：脚本引用 / 音频源。这里只存作者数据，运行时由脚本 /
//! 音频系统消费。

use xgc_base::EntityKey;
use xgc_reflect::{AssetKind, Component, ComponentId, FieldDescriptor, FieldType, Value};

use crate::{GlobalTransform, Transform, World};

/// 声源所属的三个音量总线。主音量单独作用于混音结果。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioBus {
    #[default]
    Sfx,
    Bgm,
    Voice,
}

impl AudioBus {
    pub const ALL: [Self; 3] = [Self::Sfx, Self::Bgm, Self::Voice];

    pub fn index(self) -> usize {
        match self {
            Self::Sfx => 0,
            Self::Bgm => 1,
            Self::Voice => 2,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        [
            (Self::Sfx, &["sfx", "s"][..]),
            (Self::Bgm, &["bgm", "music", "m"][..]),
            (Self::Voice, &["voice", "v"][..]),
        ]
        .into_iter()
        .find_map(|(bus, aliases)| {
            aliases
                .iter()
                .any(|alias| name.eq_ignore_ascii_case(alias))
                .then_some(bus)
        })
    }
}

/// 脚本和宿主之间的一次播放请求，属于播放会话。
#[derive(Clone, Debug, PartialEq)]
pub struct PlaySound {
    pub clip: String,
    pub volume: f32,
    pub pitch: f32,
    pub bus: AudioBus,
    pub position: Option<[f32; 3]>,
    pub looping: bool,
}

impl PlaySound {
    pub fn new(clip: impl Into<String>) -> Self {
        Self {
            clip: clip.into(),
            volume: 1.0,
            pitch: 1.0,
            bus: AudioBus::default(),
            position: None,
            looping: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AudioCommand {
    Play(PlaySound),
    StopClip(String),
    StopAll,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListenerPose {
    pub position: [f32; 3],
    pub forward: [f32; 3],
}

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
                .asset(AssetKind::Script)
                .doc("项目相对路径或 GUID"),
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
    pub bus: String,
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
            bus: "sfx".to_string(),
        }
    }
}

impl AudioSource {
    pub fn play_sound(&self, world_position: [f32; 3]) -> PlaySound {
        PlaySound {
            clip: self.clip.clone(),
            volume: self.volume,
            pitch: self.pitch,
            bus: AudioBus::from_name(&self.bus).unwrap_or_default(),
            position: self.spatial.then_some(world_position),
            looping: self.looping,
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
                .asset(AssetKind::Audio)
                .doc("音频路径"),
            FieldDescriptor::new("volume", FieldType::F32, Value::F64(1.0)).doc("线性音量 0..1"),
            FieldDescriptor::new("pitch", FieldType::F32, Value::F64(1.0)).doc("播放速率 / 音高"),
            FieldDescriptor::new("looping", FieldType::Bool, Value::Bool(false)).doc("是否循环"),
            FieldDescriptor::new("autoplay", FieldType::Bool, Value::Bool(false))
                .doc("进场景自动播放"),
            FieldDescriptor::new("spatial", FieldType::Bool, Value::Bool(true))
                .doc("是否 3D 空间化"),
            FieldDescriptor::new("bus", FieldType::String, Value::String("sfx".into()))
                .doc("总线：master / sfx / bgm / voice"),
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
            "bus" => Some(Value::String(self.bus.clone())),
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
            "bus" => {
                let bus = value.as_string().ok_or_else(|| {
                    format!("audio.source.bus 期望 string，得到 {}", value.type_name())
                })?;
                if !matches!(bus, "master" | "sfx" | "bgm" | "voice") {
                    return Err(format!("audio.source.bus 未知总线：{bus}"));
                }
                self.bus = bus.to_string();
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

/// 全局音频总线音量；没有该组件时所有总线按 1.0。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioMixer {
    pub master: f32,
    pub sfx: f32,
    pub bgm: f32,
    pub voice: f32,
}

impl Default for AudioMixer {
    fn default() -> Self {
        Self {
            master: 1.0,
            sfx: 1.0,
            bgm: 1.0,
            voice: 1.0,
        }
    }
}

impl AudioMixer {
    pub fn bus(&self, bus: &str) -> f32 {
        // 作者组件的 schema 仍允许直接取主音量，Lua 总线名字走 AudioBus。
        let value = match bus {
            "master" => self.master,
            "bgm" => self.bgm,
            "voice" => self.voice,
            _ => self.sfx,
        };
        value.clamp(0.0, 1.0)
    }

    pub fn bus_volume(&self, bus: AudioBus) -> f32 {
        audio_volume(match bus {
            AudioBus::Sfx => self.sfx,
            AudioBus::Bgm => self.bgm,
            AudioBus::Voice => self.voice,
        })
    }

    pub fn set_bus_volume(&mut self, bus: AudioBus, gain: f32) {
        let volume = match bus {
            AudioBus::Sfx => &mut self.sfx,
            AudioBus::Bgm => &mut self.bgm,
            AudioBus::Voice => &mut self.voice,
        };
        *volume = audio_volume(gain);
    }
}

/// 让非有限值静音，避免一个声源污染整块 PCM。
pub fn audio_volume(gain: f32) -> f32 {
    if gain.is_finite() {
        gain.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

impl World {
    pub fn audio_emitter_position(&self, key: EntityKey) -> [f32; 3] {
        self.get::<GlobalTransform>(key)
            .map(|global| global.position)
            .or_else(|| self.get::<Transform>(key).map(|local| local.position))
            .unwrap_or([0.0; 3])
    }

    pub fn audio_mixer(&self) -> AudioMixer {
        let mut mixer = AudioMixer::default();
        self.for_each::<AudioMixer>(|_, config| mixer = *config);
        mixer
    }

    pub fn audio_listener_position(&self) -> Option<[f32; 3]> {
        let mut position = None;
        self.for_each::<AudioListener>(|key, config| {
            if config.active && position.is_none() {
                position = self
                    .get::<Transform>(key)
                    .map(|transform| transform.position);
            }
        });
        position
    }
}

impl Component for AudioMixer {
    const ID: ComponentId = ComponentId(0x0305);
    const NAME: &'static str = "audio.mixer";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "音频总线：master / sfx / bgm / voice，0..1"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("master", FieldType::F32, Value::F64(1.0)),
            FieldDescriptor::new("sfx", FieldType::F32, Value::F64(1.0)),
            FieldDescriptor::new("bgm", FieldType::F32, Value::F64(1.0)),
            FieldDescriptor::new("voice", FieldType::F32, Value::F64(1.0)),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        let value = match field {
            "master" => self.master,
            "sfx" => self.sfx,
            "bgm" => self.bgm,
            "voice" => self.voice,
            _ => return None,
        };
        Some(Value::F64(f64::from(value)))
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        let value = value
            .as_f32()
            .ok_or_else(|| format!("audio.mixer.{field} 期望 f32，得到 {}", value.type_name()))?;
        match field {
            "master" => self.master = value.clamp(0.0, 1.0),
            "sfx" => self.sfx = value.clamp(0.0, 1.0),
            "bgm" => self.bgm = value.clamp(0.0, 1.0),
            "voice" => self.voice = value.clamp(0.0, 1.0),
            _ => return Err(format!("audio.mixer 没有字段 {field}")),
        }
        Ok(())
    }
}
