//! 粒子发射器组件：最小发射参数。

use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldType, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct ParticleEmitter {
    pub emitting: bool,
    pub rate: f32,
    pub max_particles: u32,
    pub lifetime: [f32; 2],
    pub speed: [f32; 2],
    pub gravity: [f32; 3],
    pub start_color: [f32; 4],
    pub seed: u64,
}

impl Default for ParticleEmitter {
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

impl Component for ParticleEmitter {
    const ID: ComponentId = ComponentId(0x0701);
    const NAME: &'static str = "particle.emitter";

    fn default_value() -> Self {
        Self::default()
    }

    fn doc() -> &'static str {
        "粒子发射器：发射率 / 上限 / 寿命 / 速度 / 重力 / 颜色 / 种子"
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("emitting", FieldType::Bool, Value::Bool(true)),
            FieldDescriptor::new("rate", FieldType::F32, Value::F64(60.0)),
            FieldDescriptor::new("max_particles", FieldType::U32, Value::I64(512)),
            FieldDescriptor::new("lifetime", FieldType::Vec2, Value::Vec2([0.6, 1.2])),
            FieldDescriptor::new("speed", FieldType::Vec2, Value::Vec2([1.0, 2.2])),
            FieldDescriptor::new("gravity", FieldType::Vec3, Value::Vec3([0.0, -1.2, 0.0])),
            FieldDescriptor::new(
                "start_color",
                FieldType::Vec4,
                Value::Vec4([1.0, 0.55, 0.12, 0.9]),
            ),
            FieldDescriptor::new("seed", FieldType::I64, Value::I64(0)),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "emitting" => Some(Value::Bool(self.emitting)),
            "rate" => Some(Value::F64(f64::from(self.rate))),
            "max_particles" => Some(Value::I64(i64::from(self.max_particles))),
            "lifetime" => Some(Value::Vec2(vec2_f64(self.lifetime))),
            "speed" => Some(Value::Vec2(vec2_f64(self.speed))),
            "gravity" => Some(Value::Vec3(vec3_f64(self.gravity))),
            "start_color" => Some(Value::Vec4(vec4_f64(self.start_color))),
            "seed" => Some(Value::I64(self.seed as i64)),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "emitting" => {
                self.emitting = value.as_bool().ok_or_else(|| mismatch(field, value))?;
            }
            "rate" => {
                self.rate = value.as_f32().ok_or_else(|| mismatch(field, value))?;
            }
            "max_particles" => {
                let raw = match value {
                    Value::I64(value) => *value,
                    _ => return Err(mismatch(field, value)),
                };
                self.max_particles = u32::try_from(raw)
                    .map_err(|_| format!("particle.emitter.max_particles 超出 u32：{raw}"))?;
            }
            "lifetime" => self.lifetime = vec2(value, field)?,
            "speed" => self.speed = vec2(value, field)?,
            "gravity" => {
                let values = value.as_vec3().ok_or_else(|| mismatch(field, value))?;
                self.gravity = values;
            }
            "start_color" => {
                let values = value.as_vec4().ok_or_else(|| mismatch(field, value))?;
                self.start_color = values;
            }
            "seed" => {
                let raw = match value {
                    Value::I64(value) => *value,
                    _ => return Err(mismatch(field, value)),
                };
                self.seed = u64::try_from(raw)
                    .map_err(|_| format!("particle.emitter.seed 不能为负：{raw}"))?;
            }
            _ => return Err(format!("particle.emitter 没有字段 {field}")),
        }
        Ok(())
    }
}

fn mismatch(field: &str, value: &Value) -> String {
    format!(
        "particle.emitter.{field} 类型不匹配，得到 {}",
        value.type_name()
    )
}

fn vec2(value: &Value, field: &str) -> Result<[f32; 2], String> {
    let values = value.as_vec2().ok_or_else(|| mismatch(field, value))?;
    Ok(values)
}

fn vec2_f64(value: [f32; 2]) -> [f64; 2] {
    [f64::from(value[0]), f64::from(value[1])]
}

fn vec3_f64(value: [f32; 3]) -> [f64; 3] {
    [
        f64::from(value[0]),
        f64::from(value[1]),
        f64::from(value[2]),
    ]
}

fn vec4_f64(value: [f32; 4]) -> [f64; 4] {
    [
        f64::from(value[0]),
        f64::from(value[1]),
        f64::from(value[2]),
        f64::from(value[3]),
    ]
}
