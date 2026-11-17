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
    /// 完整配置的 RON 快照（旧场景参数远多于上面的展示字段，见 `ParticleConfig`）。
    pub config: String,
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
            config: String::new(),
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
            FieldDescriptor::new("config", FieldType::String, Value::String(String::new()))
                .doc("完整粒子配置的 RON 快照（旧场景迁移而来）"),
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
            "config" => Some(Value::String(self.config.clone())),
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
            "config" => {
                self.config = match value {
                    Value::String(value) => value.clone(),
                    _ => return Err(mismatch(field, value)),
                };
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

// ---------------------------------------------------------------------------
// 与旧端粒子配置对齐的完整快照（模拟 / 渲染共用）
// ---------------------------------------------------------------------------

/// 粒子混合方式（旧端 `ParticleBlend`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ParticleBlend {
    /// 直通 alpha（`src*a + dst*(1-a)`）：烟、尘。
    #[default]
    Alpha,
    /// 加色（`src*a + dst`）：火焰、火花。
    Additive,
}

/// 发射体积（旧端 `EmissionShape`）：只决定出生位置，方向归 `direction` / `spread`。
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum EmissionShape {
    /// 实体原点一个点。
    #[default]
    Point,
    /// 球内均匀分布。
    Sphere { radius: f32 },
    /// 盒内均匀分布（半边长）。
    Box { half_extents: [f32; 3] },
}

/// 旧端 `ParticleEmitter` 的完整配置快照。
///
/// `particle.emitter.config` 存它的 RON 序列化文本：旧场景的粒子参数远多于
/// 组件里的展示字段（尺寸 / 颜色插值 / 发射形状 / 混合方式…），一比一摊成
/// 组件字段会把 schema 撑爆；模拟与渲染需要的完整语义放这一份快照里，
/// 组件上的标量字段仍保留（编辑器 / Agent / 脚本要能直接读改常用项）。
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ParticleConfig {
    pub emitting: bool,
    pub rate: f32,
    pub max_particles: u32,
    pub duration: f32,
    pub looping: bool,
    pub lifetime: [f32; 2],
    pub speed: [f32; 2],
    pub direction: [f32; 3],
    pub spread: f32,
    pub shape: EmissionShape,
    pub gravity: [f32; 3],
    pub drag: f32,
    pub start_size: f32,
    pub end_size: f32,
    pub start_color: [f32; 4],
    pub end_color: [f32; 4],
    pub blend: ParticleBlend,
    pub seed: u64,
}

impl Default for ParticleConfig {
    fn default() -> Self {
        Self {
            emitting: true,
            rate: 60.0,
            max_particles: 512,
            duration: 0.0,
            looping: true,
            lifetime: [0.6, 1.2],
            speed: [1.0, 2.2],
            direction: [0.0, 1.0, 0.0],
            spread: 18.0,
            shape: EmissionShape::Sphere { radius: 0.15 },
            gravity: [0.0, -1.2, 0.0],
            drag: 0.6,
            start_size: 0.16,
            end_size: 0.03,
            start_color: [1.0, 0.55, 0.12, 0.9],
            end_color: [0.35, 0.05, 0.0, 0.0],
            blend: ParticleBlend::Additive,
            seed: 0,
        }
    }
}

/// 同时存活粒子的硬上限：与旧端一致，挡住手写配置造成的无界增长。
pub const MAX_PARTICLES_LIMIT: u32 = 200_000;

impl ParticleConfig {
    /// 加载 / 运行边界清洗：非有限值不准进入模拟与 GPU，范围与旧端一致。
    pub fn sanitized(mut self) -> Self {
        self.rate = sane(self.rate, 0.0, 1.0e5, 0.0);
        self.max_particles = self.max_particles.min(MAX_PARTICLES_LIMIT);
        self.duration = sane(self.duration, 0.0, 1.0e5, 0.0);
        self.lifetime = sane_range(self.lifetime, 0.001, 1.0e4, [0.6, 1.2]);
        self.speed = sane_range(self.speed, 0.0, 1.0e4, [1.0, 2.2]);
        self.direction = [
            sane(self.direction[0], -1.0e4, 1.0e4, 0.0),
            sane(self.direction[1], -1.0e4, 1.0e4, 0.0),
            sane(self.direction[2], -1.0e4, 1.0e4, 0.0),
        ];
        if self.direction == [0.0; 3] {
            self.direction = [0.0, 1.0, 0.0];
        }
        self.spread = sane(self.spread, 0.0, 180.0, 18.0);
        self.shape = match self.shape {
            EmissionShape::Point => EmissionShape::Point,
            EmissionShape::Sphere { radius } => EmissionShape::Sphere {
                radius: sane(radius, 0.0, 1.0e4, 0.0),
            },
            EmissionShape::Box { half_extents } => EmissionShape::Box {
                half_extents: [
                    sane(half_extents[0], 0.0, 1.0e4, 0.0),
                    sane(half_extents[1], 0.0, 1.0e4, 0.0),
                    sane(half_extents[2], 0.0, 1.0e4, 0.0),
                ],
            },
        };
        self.gravity = [
            sane(self.gravity[0], -1.0e4, 1.0e4, 0.0),
            sane(self.gravity[1], -1.0e4, 1.0e4, 0.0),
            sane(self.gravity[2], -1.0e4, 1.0e4, 0.0),
        ];
        self.drag = sane(self.drag, 0.0, 1.0e3, 0.6);
        self.start_size = sane(self.start_size, 0.0, 1.0e4, 0.16);
        self.end_size = sane(self.end_size, 0.0, 1.0e4, 0.03);
        self.start_color = sane_color(self.start_color, [1.0, 0.55, 0.12, 0.9]);
        self.end_color = sane_color(self.end_color, [0.35, 0.05, 0.0, 0.0]);
        self
    }

    /// 单位化的发射方向；零向量退化成 +Y（与旧端 `direction_vec` 一致）。
    pub fn direction_vec(&self) -> [f32; 3] {
        let [x, y, z] = self.direction;
        let len = (x * x + y * y + z * z).sqrt();
        if !len.is_finite() || len < 1e-6 {
            [0.0, 1.0, 0.0]
        } else {
            [x / len, y / len, z / len]
        }
    }
}

fn sane(value: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(lo, hi)
    } else {
        fallback
    }
}

fn sane_range(range: [f32; 2], lo: f32, hi: f32, fallback: [f32; 2]) -> [f32; 2] {
    let min = sane(range[0], lo, hi, fallback[0]);
    let max = sane(range[1], lo, hi, fallback[1]);
    if min <= max {
        [min, max]
    } else {
        [max, min]
    }
}

fn sane_color(color: [f32; 4], fallback: [f32; 4]) -> [f32; 4] {
    [
        sane(color[0], 0.0, 64.0, fallback[0]),
        sane(color[1], 0.0, 64.0, fallback[1]),
        sane(color[2], 0.0, 64.0, fallback[2]),
        sane(color[3], 0.0, 1.0, fallback[3]),
    ]
}

/// 一帧里一个 billboard 四边形（模拟输出 → 渲染输入）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParticleQuad {
    /// 世界空间中心。
    pub position: [f32; 3],
    /// 正方形半边长（世界单位）。
    pub size: f32,
    /// 线性空间 RGBA（RGB 可以 > 1，加色火焰靠它过曝）。
    pub color: [f32; 4],
    pub blend: ParticleBlend,
}

impl ParticleEmitter {
    /// 完整配置：`config` 有 RON 快照就用它，否则从组件上的标量字段拼一份。
    pub fn resolved(&self) -> ParticleConfig {
        if !self.config.trim().is_empty() {
            if let Ok(mut config) = ron::from_str::<ParticleConfig>(&self.config) {
                // 常用字段仍是 Agent / Inspector 可直接编辑的真相；RON 快照只补齐
                // schema 没摊开的形状 / 尺寸 / 阻力 / 终止颜色等参数。
                config.emitting = self.emitting;
                config.rate = self.rate;
                config.max_particles = self.max_particles;
                config.lifetime = self.lifetime;
                config.speed = self.speed;
                config.gravity = self.gravity;
                config.start_color = self.start_color;
                config.seed = self.seed;
                return config.sanitized();
            }
        }
        ParticleConfig {
            emitting: self.emitting,
            rate: self.rate,
            max_particles: self.max_particles,
            lifetime: self.lifetime,
            speed: self.speed,
            gravity: self.gravity,
            start_color: self.start_color,
            seed: self.seed,
            ..ParticleConfig::default()
        }
        .sanitized()
    }

    /// 完整配置的 RON 文本（迁移 / 编辑器保存用）。
    pub fn config_ron(config: &ParticleConfig) -> String {
        ron::ser::to_string(&config.sanitized()).expect("ParticleConfig 必须可序列化")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_sanitizes_untrusted_config() {
        let raw = ParticleConfig {
            rate: f32::NAN,
            max_particles: u32::MAX,
            lifetime: [5.0, -2.0],
            direction: [f32::INFINITY, 0.0, 0.0],
            shape: EmissionShape::Box {
                half_extents: [f32::NAN, -1.0, 20_000.0],
            },
            start_color: [f32::NAN, -1.0, 100.0, 2.0],
            ..ParticleConfig::default()
        };
        let emitter = ParticleEmitter {
            rate: raw.rate,
            max_particles: raw.max_particles,
            lifetime: raw.lifetime,
            start_color: raw.start_color,
            config: ron::ser::to_string(&raw).unwrap(),
            ..ParticleEmitter::default()
        };
        let actual = emitter.resolved();
        assert_eq!(actual.rate, 0.0);
        assert_eq!(actual.max_particles, MAX_PARTICLES_LIMIT);
        assert_eq!(actual.lifetime, [0.001, 5.0]);
        assert_eq!(actual.direction, [0.0, 1.0, 0.0]);
        assert_eq!(
            actual.shape,
            EmissionShape::Box {
                half_extents: [0.0, 0.0, 10_000.0]
            }
        );
        assert_eq!(actual.start_color, [1.0, 0.0, 64.0, 1.0]);
    }
}
