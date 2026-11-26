//! 复用基础状态机的反射定义与字段校验，供分层动画使用。

use xgc_reflect::{Record, ValueProjection, ValueWriter, Variant};

use super::{
    enum_parts, list_field, number, number_field, parameters_field, read_list, read_parameters,
    structure, text, text_field, unit_branch, unit_value, Component, ComponentId, FieldDescriptor,
    FieldType, StateMachine, Value,
};
use crate::{AnimLayer, AnimationLayers, LayerMode};

impl AnimationLayers {
    /// 脚本的帧内视图也使用与存储相同的字段值。
    pub fn layers_value(&self) -> Value {
        self.layers.as_slice().projected_value()
    }
}

impl Component for AnimationLayers {
    const ID: ComponentId = ComponentId(0x0506);
    const NAME: &'static str = "animation.layers";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "在基础姿势与叠加层之后依次混合剪辑、遮罩和分层状态机"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![list_field("layers", AnimLayer::descriptor()).doc("栈内顺序即混合顺序")]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        (field == "layers").then(|| self.layers_value())
    }

    fn hash_field_into(&self, field: &str, hash: &mut u64) {
        if field == "layers" {
            self.layers.as_slice().hash_projected(hash);
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        if field != "layers" {
            return Err(format!("animation.layers 没有字段 {field}"));
        }
        self.layers = read_list(value, "layers", AnimLayer::from_value)?;
        Ok(())
    }
}

impl ValueProjection for AnimLayer {
    fn project(&self, writer: &mut ValueWriter<'_>) {
        let empty = Record([]);
        let mode = Variant(
            match self.mode {
                LayerMode::Additive => "Additive",
                LayerMode::Override => "Override",
            },
            &empty,
        );
        let machine = match self.machine.as_ref() {
            Some(machine) => Variant("Enabled", machine),
            None => Variant("Disabled", &empty),
        };
        writer.record([
            ("name", &self.name),
            ("mode", &mode),
            ("clip", &self.clip),
            ("weight", &self.weight),
            ("speed", &self.speed),
            ("looping", &self.looping),
            ("fade_time", &self.fade_time),
            ("mask", &self.mask),
            ("machine", &machine),
            ("state", &self.state),
            ("time", &self.time),
            ("parameters", &self.parameters),
            ("fade_from", &self.fade_from),
            ("fade_from_time", &self.fade_from_time),
            ("fade_from_speed", &self.fade_from_speed),
            ("fade_from_looping", &self.fade_from_looping),
            ("fade_elapsed", &self.fade_elapsed),
            ("fade_duration", &self.fade_duration),
        ]);
    }
}

impl AnimLayer {
    fn descriptor() -> FieldDescriptor {
        const BLEND_STATE: &str = "Blend State";
        let layer = Self::default();
        let mut machine = StateMachine::descriptor();
        machine.name = "Enabled";
        FieldDescriptor::new(
            "layer",
            FieldType::Struct(vec![
                text_field("name").doc("脚本按名选择第一层"),
                FieldDescriptor::new(
                    "mode",
                    FieldType::Enum(vec![unit_branch("Additive"), unit_branch("Override")]),
                    unit_value("Additive"),
                ),
                text_field("clip").doc("层机器启用时由当前状态选择剪辑"),
                number_field("weight", layer.weight).doc("混合时钳制到 [0, 1]"),
                number_field("speed", layer.speed).doc("播放速度倍数"),
                bool_field("looping", layer.looping),
                number_field("fade_time", layer.fade_time).unit("s"),
                list_field("mask", text_field("bone")).doc("骨骼子树根名单；空名单覆盖全身"),
                FieldDescriptor::new(
                    "machine",
                    FieldType::Enum(vec![unit_branch("Disabled"), machine]),
                    unit_value("Disabled"),
                ),
                text_field("state").doc("当前状态名；空表示从机器入口开始"),
                number_field("time", layer.time).unit("s"),
                parameters_field(),
                text_field("fade_from").group(BLEND_STATE).read_only(),
                number_field("fade_from_time", layer.fade_from_time)
                    .unit("s")
                    .group(BLEND_STATE)
                    .read_only(),
                number_field("fade_from_speed", layer.fade_from_speed)
                    .group(BLEND_STATE)
                    .read_only(),
                bool_field("fade_from_looping", layer.fade_from_looping)
                    .group(BLEND_STATE)
                    .read_only(),
                number_field("fade_elapsed", layer.fade_elapsed)
                    .unit("s")
                    .group(BLEND_STATE)
                    .read_only(),
                number_field("fade_duration", layer.fade_duration)
                    .unit("s")
                    .group(BLEND_STATE)
                    .read_only(),
            ]),
            layer.projected_value(),
        )
    }

    fn from_value(value: &Value) -> Result<Self, String> {
        let [name, mode, clip, weight, speed, looping, fade_time, mask, machine, state, time, parameters, fade_from, fade_from_time, fade_from_speed, fade_from_looping, fade_elapsed, fade_duration] =
            structure(
                value,
                [
                    "name",
                    "mode",
                    "clip",
                    "weight",
                    "speed",
                    "looping",
                    "fade_time",
                    "mask",
                    "machine",
                    "state",
                    "time",
                    "parameters",
                    "fade_from",
                    "fade_from_time",
                    "fade_from_speed",
                    "fade_from_looping",
                    "fade_elapsed",
                    "fade_duration",
                ],
            )?;
        let (mode, payload) = enum_parts(mode, "mode")?;
        structure(payload, [])?;
        let mode = match mode {
            "Additive" => LayerMode::Additive,
            "Override" => LayerMode::Override,
            variant => return Err(format!("未知的 mode 分支：{variant}")),
        };
        let machine = match enum_parts(machine, "machine")? {
            ("Disabled", payload) => {
                structure(payload, [])?;
                None
            }
            ("Enabled", payload) => Some(StateMachine::from_value(payload)?),
            (variant, _) => return Err(format!("未知的 machine 分支：{variant}")),
        };
        Ok(Self {
            name: text(name, "name")?,
            mode,
            clip: text(clip, "clip")?,
            weight: number(weight, "weight")?,
            speed: number(speed, "speed")?,
            looping: looping.as_bool().ok_or("looping 必须是布尔值")?,
            fade_time: number(fade_time, "fade_time")?,
            mask: read_list(mask, "mask", |bone| text(bone, "bone"))?,
            machine,
            state: text(state, "state")?,
            time: number(time, "time")?,
            parameters: read_parameters(parameters)?,
            fade_from: text(fade_from, "fade_from")?,
            fade_from_time: number(fade_from_time, "fade_from_time")?,
            fade_from_speed: number(fade_from_speed, "fade_from_speed")?,
            fade_from_looping: fade_from_looping
                .as_bool()
                .ok_or("fade_from_looping 必须是布尔值")?,
            fade_elapsed: number(fade_elapsed, "fade_elapsed")?,
            fade_duration: number(fade_duration, "fade_duration")?,
        })
    }
}

fn bool_field(name: &'static str, value: bool) -> FieldDescriptor {
    FieldDescriptor::new(name, FieldType::Bool, Value::Bool(value))
}
