//! v2 动画解码器：沿用 Animator 的序列化形态，拦截状态机参数表中的重复键。
//!
//! 基础机器和每个分层机器都能携带参数；只保护顶层机器会漏掉层里的配置。

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::animation::{
    AnimAdditive, AnimEvent, AnimLayer, AnimLayerMode, AnimMachine, AnimState, AnimTransition,
    Animator, RootMotion,
};

#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct StrictAnimator(pub Animator);

#[derive(Default)]
struct UniqueParameters(BTreeMap<String, f32>);

impl<'de> Deserialize<'de> for UniqueParameters {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Unique;

        impl<'de> Visitor<'de> for Unique {
            type Value = UniqueParameters;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("参数名不重复的动画参数表")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut params = BTreeMap::new();
                while let Some((name, value)) = map.next_entry()? {
                    if params.insert(name, value).is_some() {
                        return Err(serde::de::Error::custom("v2 动画参数里有重复键"));
                    }
                }
                Ok(UniqueParameters(params))
            }
        }

        deserializer.deserialize_map(Unique)
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct MachineWire {
    entry: String,
    parameters: UniqueParameters,
    states: Vec<AnimState>,
    transitions: Vec<AnimTransition>,
}

impl From<MachineWire> for AnimMachine {
    fn from(wire: MachineWire) -> Self {
        Self {
            entry: wire.entry,
            parameters: wire.parameters.0,
            states: wire.states,
            transitions: wire.transitions,
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct LayerWire {
    name: String,
    mode: AnimLayerMode,
    clip: String,
    weight: f32,
    speed: f32,
    looping: bool,
    fade_time: f32,
    machine: Option<MachineWire>,
    mask: Vec<String>,
}

impl Default for LayerWire {
    fn default() -> Self {
        let layer = AnimLayer::default();
        Self {
            name: layer.name,
            mode: layer.mode,
            clip: layer.clip,
            weight: layer.weight,
            speed: layer.speed,
            looping: layer.looping,
            fade_time: layer.fade_time,
            machine: None,
            mask: layer.mask,
        }
    }
}

impl From<LayerWire> for AnimLayer {
    fn from(wire: LayerWire) -> Self {
        Self {
            name: wire.name,
            mode: wire.mode,
            clip: wire.clip,
            weight: wire.weight,
            speed: wire.speed,
            looping: wire.looping,
            fade_time: wire.fade_time,
            machine: wire.machine.map(Into::into),
            mask: wire.mask,
        }
    }
}

#[derive(Deserialize)]
#[serde(default)]
struct AnimatorWire {
    clip: String,
    speed: f32,
    looping: bool,
    autoplay: bool,
    fade_time: f32,
    machine: Option<MachineWire>,
    additive: Option<AnimAdditive>,
    layers: Vec<LayerWire>,
    root_motion: Option<RootMotion>,
    events: Vec<AnimEvent>,
}

impl Default for AnimatorWire {
    fn default() -> Self {
        let animator = Animator::default();
        Self {
            clip: animator.clip,
            speed: animator.speed,
            looping: animator.looping,
            autoplay: animator.autoplay,
            fade_time: animator.fade_time,
            machine: None,
            additive: animator.additive,
            layers: Vec::new(),
            root_motion: animator.root_motion,
            events: animator.events,
        }
    }
}

impl<'de> Deserialize<'de> for StrictAnimator {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = AnimatorWire::deserialize(deserializer)?;
        Ok(Self(Animator {
            clip: wire.clip,
            speed: wire.speed,
            looping: wire.looping,
            autoplay: wire.autoplay,
            fade_time: wire.fade_time,
            machine: wire.machine.map(Into::into),
            additive: wire.additive,
            layers: wire.layers.into_iter().map(Into::into).collect(),
            root_motion: wire.root_motion,
            events: wire.events,
        }))
    }
}
