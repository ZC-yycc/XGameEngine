//! 旧 Animator → `animation.animator` 组件数据（含从 Transform 取得的基础姿态）。

use xgc_world::{Animator, Component, ComponentData, Value};

pub struct AnimatorInput {
    pub clip: String,
    pub speed: f32,
    pub looping: bool,
    pub autoplay: bool,
    pub fade_time: f32,
    pub position: [f64; 3],
    pub rotation: [f64; 4],
    pub scale: [f64; 3],
}

pub fn animator_component(input: AnimatorInput) -> ComponentData {
    ComponentData {
        component: Animator::ID,
        fields: vec![
            ("clip".to_string(), Value::String(input.clip)),
            ("speed".to_string(), Value::F64(f64::from(input.speed))),
            ("looping".to_string(), Value::Bool(input.looping)),
            ("autoplay".to_string(), Value::Bool(input.autoplay)),
            (
                "fade_time".to_string(),
                Value::F64(f64::from(input.fade_time)),
            ),
            ("base_position".to_string(), Value::Vec3(input.position)),
            ("base_rotation".to_string(), Value::Vec4(input.rotation)),
            ("base_scale".to_string(), Value::Vec3(input.scale)),
        ],
    }
}
