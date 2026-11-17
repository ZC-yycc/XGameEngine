//! 粒子模拟（CPU、确定性）：发射 / 积分 / 寿命 → [`ParticleQuad`]。
//!
//! 与旧端 `xgame-particle` 同一套数学：SplitMix64 随机流（种子只来自组件）、
//! 连续发射的小数累加、球面均匀 / 圆锥采样、半隐式阻尼、世界空间积分。
//! 同一个种子 + 同一串 dt ⇒ 逐位相同的四边形序列 —— 截图基线可复现的地基。
//!
//! 渲染消费的是 [`ParticleQuad`]（世界空间中心 + 世界单位尺寸 + 线性颜色 +
//! 混合方式），模拟层不依赖相机：换个视角不需要重算模拟。

use std::collections::BTreeMap;

use xgc_base::{EntityId, EntityKey};
use xgc_world::{
    EmissionShape, GlobalTransform, ParticleConfig, ParticleEmitter, ParticleQuad, World,
};

/// SplitMix64：便宜、雪崩、可复现；算法永远不换（换 = 所有既有粒子表现失效）。
#[derive(Clone, Debug)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0xA076_1D64_78BD_642F)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// `[0, 1)`（取高 24 位：f32 尾数正好 24 位）。
    fn unit(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) / ((1u32 << 24) as f32)
    }

    /// `[min, max]`（区间反了也合法）。
    fn range(&mut self, min: f32, max: f32) -> f32 {
        let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
        lo + (hi - lo) * self.unit()
    }

    fn symmetric(&mut self, half: f32) -> f32 {
        self.range(-half, half)
    }

    /// 球面均匀方向。
    fn unit_direction(&mut self) -> [f32; 3] {
        let z = self.unit() * 2.0 - 1.0;
        let phi = self.unit() * std::f32::consts::TAU;
        let r = (1.0 - z * z).max(0.0).sqrt();
        [r * phi.cos(), z, r * phi.sin()]
    }

    /// 以 `axis` 为轴、半角 `spread` 的圆锥内均匀方向（`spread <= 0` 不消耗随机数）。
    fn cone(&mut self, axis: [f32; 3], spread: f32) -> [f32; 3] {
        if spread >= std::f32::consts::PI {
            return self.unit_direction();
        }
        if spread <= 0.0 {
            return axis;
        }
        let cos_max = spread.cos();
        let cos_t = 1.0 - self.unit() * (1.0 - cos_max);
        let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
        let phi = self.unit() * std::f32::consts::TAU;
        let (u, v) = orthonormal_basis(axis);
        let direction = [
            u[0] * (sin_t * phi.cos()) + v[0] * (sin_t * phi.sin()) + axis[0] * cos_t,
            u[1] * (sin_t * phi.cos()) + v[1] * (sin_t * phi.sin()) + axis[1] * cos_t,
            u[2] * (sin_t * phi.cos()) + v[2] * (sin_t * phi.sin()) + axis[2] * cos_t,
        ];
        normalize_or_zero(direction)
    }
}

fn normalize_or_zero(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if !len.is_finite() || len < 1e-9 {
        [0.0; 3]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// 给单位向量凑一组正交基（`u × v = axis`）。
fn orthonormal_basis(axis: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let helper = if axis[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let u = normalize_or_zero(cross(axis, helper));
    let v = cross(axis, u);
    (u, v)
}

fn rotate(quaternion: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let [qx, qy, qz, qw] = quaternion;
    let tv = cross([qx, qy, qz], v);
    let t = [2.0 * tv[0], 2.0 * tv[1], 2.0 * tv[2]];
    let u = cross([qx, qy, qz], t);
    [
        v[0] + qw * t[0] + u[0],
        v[1] + qw * t[1] + u[1],
        v[2] + qw * t[2] + u[2],
    ]
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// 一颗活着的粒子。只有模拟会碰它。
#[derive(Debug, Clone, Copy)]
struct Particle {
    position: [f32; 3],
    velocity: [f32; 3],
    age: f32,
    life: f32,
}

#[derive(Default)]
struct EmitterState {
    particles: Vec<Particle>,
    rng: Option<Rng>,
    /// 上次使用的种子：作者改种子 = 换一批随机数（保留已出生的粒子）。
    seed: u64,
    /// 连续发射的小数余量。
    accum: f32,
    /// 发射窗口内已用时长（只在 `duration > 0` 时推进）。
    elapsed: f32,
    /// 解析后的完整配置；组件未变时不重复解析 RON。
    config: ParticleConfig,
    config_source: Option<ParticleEmitter>,
}

impl EmitterState {
    fn resolved(&mut self, emitter: &ParticleEmitter) -> ParticleConfig {
        if self.config_source.as_ref() != Some(emitter) {
            self.config = emitter.resolved();
            self.config_source = Some(emitter.clone());
        }
        self.config
    }

    /// 本帧发射 + 积分 + 淘汰，并把活着的粒子写成四边形。
    fn step(
        &mut self,
        config: &ParticleConfig,
        world: &GlobalTransform,
        dt: f32,
        out: &mut Vec<ParticleQuad>,
    ) {
        if self.seed != config.seed || self.rng.is_none() {
            self.seed = config.seed;
            self.rng = Some(Rng::new(config.seed));
        }
        // -- 发射窗口（duration / looping）--
        if config.duration > 0.0 {
            self.elapsed += dt;
            while self.elapsed >= config.duration {
                if config.looping {
                    self.elapsed -= config.duration;
                    // 新一轮从头开始：小数余量清零，否则每轮都多一颗。
                    self.accum = 0.0;
                } else {
                    self.elapsed = config.duration;
                    break;
                }
            }
        }
        let within_window = config.duration <= 0.0 || self.elapsed < config.duration;
        let continuous = config.emitting && within_window && config.rate > 0.0;
        let max = config.max_particles as usize;
        let axis = rotate(
            world.rotation,
            xgc_world::ParticleConfig::direction_vec(config),
        );
        let spread_rad = config.spread.to_radians();

        if continuous {
            self.accum += config.rate * dt;
            // 单帧最多补 256 颗：rate 手滑写成 1e5 时不能一帧锁死。
            let mut budget = 256usize;
            while self.accum >= 1.0 && budget > 0 {
                if self.particles.len() >= max {
                    // 满了：欠账丢掉（不攒），否则一旦有空位会瞬间灌满。
                    self.accum = 0.0;
                    break;
                }
                self.accum -= 1.0;
                self.spawn(config, world, axis, spread_rad);
                budget -= 1;
            }
            if budget == 0 {
                self.accum = 0.999;
            }
        } else if !within_window {
            self.accum = 0.0;
        }

        // -- 积分与淘汰（保序：输出顺序因此可复现）--
        // 半隐式阻尼 `1 / (1 + k*dt)` 在任意 dt 下稳定，显式写法在 k*dt > 1 会翻号。
        let damp = 1.0 / (1.0 + config.drag.max(0.0) * dt);
        self.particles.retain_mut(|particle| {
            particle.age += dt;
            if particle.age >= particle.life {
                return false;
            }
            for axis in 0..3 {
                particle.velocity[axis] =
                    (particle.velocity[axis] + config.gravity[axis] * dt) * damp;
                particle.position[axis] += particle.velocity[axis] * dt;
            }
            true
        });

        for particle in &self.particles {
            let t = (particle.age / particle.life).clamp(0.0, 1.0);
            out.push(ParticleQuad {
                position: particle.position,
                size: lerp(config.start_size, config.end_size, t).max(0.0),
                color: [
                    lerp(config.start_color[0], config.end_color[0], t),
                    lerp(config.start_color[1], config.end_color[1], t),
                    lerp(config.start_color[2], config.end_color[2], t),
                    lerp(config.start_color[3], config.end_color[3], t),
                ],
                blend: config.blend,
            });
        }
    }

    /// 生一颗粒子。
    fn spawn(
        &mut self,
        config: &ParticleConfig,
        world: &GlobalTransform,
        axis: [f32; 3],
        spread_rad: f32,
    ) {
        let Some(rng) = self.rng.as_mut() else {
            return;
        };
        let offset = match config.shape {
            EmissionShape::Point => [0.0; 3],
            EmissionShape::Sphere { radius } => {
                // 半径按 `cbrt(u)` 取，球内体积均匀（直接乘 u 会挤在中心）。
                let direction = rng.unit_direction();
                let scale = radius.max(0.0) * rng.unit().cbrt();
                [
                    direction[0] * scale,
                    direction[1] * scale,
                    direction[2] * scale,
                ]
            }
            EmissionShape::Box { half_extents } => [
                rng.symmetric(half_extents[0]),
                rng.symmetric(half_extents[1]),
                rng.symmetric(half_extents[2]),
            ],
        };
        let rotated = rotate(world.rotation, offset);
        let position = [
            world.position[0] + rotated[0],
            world.position[1] + rotated[1],
            world.position[2] + rotated[2],
        ];
        let direction = rng.cone(axis, spread_rad);
        let speed = rng.range(config.speed[0], config.speed[1]);
        let life = rng.range(config.lifetime[0], config.lifetime[1]);
        self.particles.push(Particle {
            position,
            velocity: [
                direction[0] * speed,
                direction[1] * speed,
                direction[2] * speed,
            ],
            age: 0.0,
            // 寿命不能是 0：`age / life` 会变成 NaN。
            life: life.max(1.0e-4),
        });
    }
}

/// 粒子系统：按实体 id 分桶保存运行态，每帧输出四边形。
#[derive(Default)]
pub struct ParticleSystem {
    emitters: BTreeMap<EntityId, EmitterState>,
    quads: Vec<ParticleQuad>,
    /// 本帧实体清单：复用容量，且只存可复制的句柄 / 变换，不克隆配置字符串。
    scratch: Vec<(EntityId, EntityKey, GlobalTransform)>,
    stats: ParticleStats,
}

/// 一帧统计（诊断 / 测试）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParticleStats {
    /// 本帧四边形数。
    pub alive: usize,
    /// 有发射器的实体数。
    pub emitters: usize,
    /// 发射器声明的上界之和。
    pub capacity: usize,
}

impl ParticleSystem {
    pub fn new() -> Self {
        Self::default()
    }

    /// 推进一帧（`dt` 秒），刷新 [`Self::quads`]。
    pub fn update(&mut self, dt: f32, world: &World) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        // 卡顿保护：巨大 dt 会让粒子瞬移穿墙、也会一次补出上限数量。
        let dt = dt.min(0.25);
        self.quads.clear();
        self.scratch.clear();
        world.for_each2::<ParticleEmitter, GlobalTransform>(
            |key, _emitter, transform: &GlobalTransform| {
                if let Some(id) = world.entity(key) {
                    self.scratch.push((id, key, *transform));
                }
            },
        );
        // archetype 顺序不稳定：按实体 id 排序保证逐位可复现。
        self.scratch.sort_by_key(|(id, _, _)| id.raw());
        let mut capacity = 0usize;
        for &(id, key, transform) in &self.scratch {
            let Some(emitter) = world.get::<ParticleEmitter>(key) else {
                continue;
            };
            let state = self.emitters.entry(id).or_default();
            let config = state.resolved(emitter);
            capacity = capacity.saturating_add(config.max_particles as usize);
            state.step(&config, &transform, dt, &mut self.quads);
        }
        // 实体被删 / 发射器被摘：整桶丢掉（没有实体锚点的粒子 = 泄漏）。
        let live = &self.scratch;
        self.emitters.retain(|id, _| {
            live.binary_search_by_key(&id.raw(), |(id, _, _)| id.raw())
                .is_ok()
        });
        self.stats = ParticleStats {
            alive: self.quads.len(),
            emitters: self.scratch.len(),
            capacity,
        };
    }

    /// 本帧的四边形序列。
    pub fn quads(&self) -> &[ParticleQuad] {
        &self.quads
    }

    pub fn stats(&self) -> ParticleStats {
        self.stats
    }

    /// 某个实体当前活着的粒子数（测试 / 诊断）。
    pub fn count_for(&self, id: EntityId) -> usize {
        self.emitters
            .get(&id)
            .map_or(0, |state| state.particles.len())
    }

    pub fn clear(&mut self) {
        self.emitters.clear();
        self.quads.clear();
        self.scratch.clear();
        self.stats = ParticleStats::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgc_world::{Transform, World};

    fn world_with(config: ParticleConfig) -> (World, EntityId) {
        let mut world = World::new();
        let emitter = ParticleEmitter {
            emitting: config.emitting,
            rate: config.rate,
            max_particles: config.max_particles,
            lifetime: config.lifetime,
            speed: config.speed,
            gravity: config.gravity,
            start_color: config.start_color,
            seed: config.seed,
            ..ParticleEmitter::default()
        };
        let key = world.spawn((Transform::default(), GlobalTransform::default(), emitter));
        let id = world.entity(key).expect("实体 id");
        (world, id)
    }

    #[test]
    fn same_seed_is_bit_identical() {
        let (a_world, _) = world_with(ParticleConfig::default());
        let (b_world, _) = world_with(ParticleConfig::default());
        let mut a = ParticleSystem::new();
        let mut b = ParticleSystem::new();
        for _ in 0..40 {
            a.update(1.0 / 60.0, &a_world);
            b.update(1.0 / 60.0, &b_world);
        }
        assert!(!a.quads().is_empty(), "应该有粒子，否则这条测试没有意义");
        assert_eq!(a.quads(), b.quads(), "同种子 + 同 dt 序列必须逐位一致");
        assert_eq!(a.stats().emitters, 1);
        assert_eq!(b.stats().emitters, 1);
    }

    #[test]
    fn rate_controls_spawn_count() {
        let config = ParticleConfig {
            rate: 60.0,
            lifetime: [10.0, 10.0],
            max_particles: 1000,
            ..ParticleConfig::default()
        };
        let (world, id) = world_with(config);
        let mut system = ParticleSystem::new();
        for _ in 0..30 {
            system.update(1.0 / 60.0, &world);
        }
        // 60/s × 0.5s ≈ 30 颗（累加器取整误差 ±1）。
        let count = system.count_for(id);
        assert!(
            (29..=31).contains(&count),
            "粒数应约等于 rate×时间：{count}"
        );
    }

    #[test]
    fn max_particles_is_a_hard_cap() {
        let config = ParticleConfig {
            rate: 1000.0,
            lifetime: [10.0, 10.0],
            max_particles: 8,
            ..ParticleConfig::default()
        };
        let (world, id) = world_with(config);
        let mut system = ParticleSystem::new();
        for _ in 0..10 {
            system.update(1.0 / 60.0, &world);
        }
        assert_eq!(system.count_for(id), 8);
    }
}
