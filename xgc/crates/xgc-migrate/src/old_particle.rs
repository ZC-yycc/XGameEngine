//! 旧 `ParticleEmitter` RON → `particle.emitter` 组件数据。
//!
//! 旧端的粒子参数远多于组件上的展示字段（发射形状 / 尺寸与颜色插值 / 阻力 /
//! 混合方式…）：这里把完整配置解析成 [`ParticleConfig`]，再以 RON 快照写进
//! 组件的 `config` 字段；常用项同时摊到标量字段，编辑器 / Agent 能直接读改。

use xgc_world::{Component, ComponentData, ParticleConfig, ParticleEmitter, Value};

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(default)]
pub(crate) struct OldParticle {
    emitting: bool,
    rate: f32,
    max_particles: u32,
    duration: f32,
    looping: bool,
    lifetime: [f32; 2],
    speed: [f32; 2],
    direction: [f32; 3],
    spread: f32,
    shape: xgc_world::EmissionShape,
    gravity: [f32; 3],
    drag: f32,
    start_size: f32,
    end_size: f32,
    start_color: [f32; 4],
    end_color: [f32; 4],
    blend: xgc_world::ParticleBlend,
    seed: u64,
}

impl Default for OldParticle {
    fn default() -> Self {
        let config = ParticleConfig::default();
        Self {
            emitting: config.emitting,
            rate: config.rate,
            max_particles: config.max_particles,
            duration: config.duration,
            looping: config.looping,
            lifetime: config.lifetime,
            speed: config.speed,
            direction: config.direction,
            spread: config.spread,
            shape: config.shape,
            gravity: config.gravity,
            drag: config.drag,
            start_size: config.start_size,
            end_size: config.end_size,
            start_color: config.start_color,
            end_color: config.end_color,
            blend: config.blend,
            seed: config.seed,
        }
    }
}

impl OldParticle {
    fn config(&self) -> ParticleConfig {
        ParticleConfig {
            emitting: self.emitting,
            rate: self.rate,
            max_particles: self.max_particles,
            duration: self.duration,
            looping: self.looping,
            lifetime: self.lifetime,
            speed: self.speed,
            direction: self.direction,
            spread: self.spread,
            shape: self.shape,
            gravity: self.gravity,
            drag: self.drag,
            start_size: self.start_size,
            end_size: self.end_size,
            start_color: self.start_color,
            end_color: self.end_color,
            blend: self.blend,
            seed: self.seed,
        }
    }
}

pub fn convert(old: &OldParticle) -> ComponentData {
    // 旧 ProjectHandle 在加载边界做同一轮清洗；迁移器直接读 RON，也必须补上。
    let config = old.config().sanitized();
    ComponentData {
        component: ParticleEmitter::ID,
        fields: vec![
            ("emitting".to_string(), Value::Bool(config.emitting)),
            ("rate".to_string(), Value::F64(f64::from(config.rate))),
            (
                "max_particles".to_string(),
                Value::I64(i64::from(config.max_particles)),
            ),
            ("lifetime".to_string(), Value::Vec2(vec2(config.lifetime))),
            ("speed".to_string(), Value::Vec2(vec2(config.speed))),
            ("gravity".to_string(), Value::Vec3(vec3(config.gravity))),
            (
                "start_color".to_string(),
                Value::Vec4(vec4(config.start_color)),
            ),
            (
                "seed".to_string(),
                Value::I64(i64::try_from(config.seed).unwrap_or(i64::MAX)),
            ),
            (
                "config".to_string(),
                Value::String(ParticleEmitter::config_ron(&config)),
            ),
        ],
    }
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
