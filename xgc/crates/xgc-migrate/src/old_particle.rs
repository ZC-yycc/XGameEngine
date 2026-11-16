//! 旧 `ParticleEmitter` RON → `particle.emitter` 组件数据。

use serde::Deserialize;
use xgc_world::{Component, ComponentData, ParticleEmitter, Value};

#[derive(Debug, Deserialize)]
#[serde(default)]
struct OldParticle {
    emitting: bool,
    rate: f32,
    max_particles: u32,
    lifetime: [f32; 2],
    speed: [f32; 2],
    gravity: [f32; 3],
    start_color: [f32; 4],
    seed: u64,
}

impl Default for OldParticle {
    fn default() -> Self {
        Self {
            emitting: true,
            rate: 60.0,
            max_particles: 512,
            lifetime: [0.6, 1.2],
            speed: [1.0, 2.2],
            gravity: [0.0, -1.2, 0.0],
            start_color: [1.0, 0.55, 0.12, 0.9],
            seed: 0,
        }
    }
}

pub fn convert(value: ron::Value) -> Result<ComponentData, String> {
    let old: OldParticle = value
        .into_rust()
        .map_err(|err| format!("particle.emitter 解析失败：{err}"))?;
    Ok(ComponentData {
        component: ParticleEmitter::ID,
        fields: vec![
            ("emitting".to_string(), Value::Bool(old.emitting)),
            ("rate".to_string(), Value::F64(f64::from(old.rate))),
            (
                "max_particles".to_string(),
                Value::I64(i64::from(old.max_particles)),
            ),
            ("lifetime".to_string(), Value::Vec2(vec2(old.lifetime))),
            ("speed".to_string(), Value::Vec2(vec2(old.speed))),
            ("gravity".to_string(), Value::Vec3(vec3(old.gravity))),
            (
                "start_color".to_string(),
                Value::Vec4(vec4(old.start_color)),
            ),
            (
                "seed".to_string(),
                Value::I64(i64::try_from(old.seed).unwrap_or(i64::MAX)),
            ),
        ],
    })
}

fn vec2(value: [f32; 2]) -> [f64; 2] {
    [f64::from(value[0]), f64::from(value[1])]
}

fn vec3(value: [f32; 3]) -> [f64; 3] {
    [
        f64::from(value[0]),
        f64::from(value[1]),
        f64::from(value[2]),
    ]
}

fn vec4(value: [f32; 4]) -> [f64; 4] {
    [
        f64::from(value[0]),
        f64::from(value[1]),
        f64::from(value[2]),
        f64::from(value[3]),
    ]
}
