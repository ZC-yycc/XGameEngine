//! 粒子模拟：**纯 CPU、确定性的**粒子系统。
//!
//! ```text
//! core::ParticleEmitter  ──(声明)──►  本 crate（发射 / 积分 / 寿命）
//!                                        │
//!                                        ▼
//!                                   SpatialQuad 序列
//!                                        │
//!                          xgame-render（billboard 实例化管线）
//! ```
//!
//! # 三条不可动摇的约定
//!
//! **1. 确定性来自"种子 + dt 序列"，不来自时间戳。**
//!
//! 每个发射器一把 [`Rng`]（种子就是组件里的 `seed`，**不混入实体 id**），
//! 所有随机数只从这一把流里取。同一个种子 + 同一串 dt ⇒ 逐位相同的四边形序列
//! —— 无头运行（固定 dt）与截图逐字节一致的地基就在这里。绝不要引入
//! `rand::thread_rng()` / `SystemTime` / 哈希序（`HashMap` 的迭代顺序）：
//! 前两者让结果不可复现，后者让每次运行的发射器顺序都不同。
//!
//! "种子不混实体 id"是个刻意的取舍：混进去能让两堆并排的篝火不照镜子，但那
//! 依赖实体 id —— 运行时新建实体的 id 来自进程内计数器，每次运行都不同，
//! 于是同一个场景两次无头运行的粒子轨迹会分叉。要变化就**显式写不同的 seed**，
//! 让"复现"永远成立。
//!
//! **2. 单个粒子不进实体。**
//!
//! 位置 / 速度 / 年龄住在 [`ParticleSystem`] 里，按实体 id 分桶。实体上只有
//! 一份 [`ParticleEmitter`] 声明。理由：粒子是每帧全变的运行态，塞进
//! `Entity` 会让场景 diff、撤销快照、脏检查全部失真。
//!
//! **3. 一帧一次分配都不该有（稳态）。**
//!
//! 桶是 `HashMap<EntityId, EmitterState>`，桶里的 `Vec<Particle>` 跨帧复用
//! （`retain_mut` 淘汰，不清空容量）；输出四边形缓冲 [quad] 也是复用字段。
//! 粒子全死光之后每帧只剩一次"扫实体表看有没有发射器"的 O(n)，与音频那条
//! 快路径同一套思路。
//!
//! # 坐标与朝向
//!
//! 出生点在**实体世界变换**下：发射体积（球 / 盒）按实体旋转摆，发射方向
//! 也按实体旋转 —— 于是"把发射器父节点转个弯，火苗就朝那边喷"是免费的。
//! 出生之后粒子就活在**世界空间**，实体再动也不会拖着已经飞出去的粒子走
//! （与 Unity 的 `Simulation Space = World` 一致）。
//!
//! 实体缩放**不参与**：粒子的尺寸是绝对世界单位（`start_size` / `end_size`），
//! 缩放父节点只影响出生点位置。理由：粒子是视觉特效，尺寸随父级缩放会让
//! "同一个火苗挂在不同大小的物体上大小不一"，比统一绝对尺寸更难用。
//!
//! # 生命周期
//!
//! 实体被删除 / 摘掉发射器 ⇒ 它的粒子**立即消失**（`retain` 掉整个桶）。
//! 这是刻意的：没有实体做锚点的粒子属于"泄漏"，而爆炸类特效本来就该用
//! `duration` 一次性发射器表达，而不是靠"删掉实体让粒子继续飞"。

use std::collections::HashMap;

use xgame_math::glam::{Quat, Vec3};
use xgame_core::{EntityId, ParticleBlend, ParticleEmitter, Scene, Transform};

// ---------------------------------------------------------------------------
// 随机数
// ---------------------------------------------------------------------------

/// 分裂混合（SplitMix64）。自己写而不引 `rand`：
///
/// 这里只要"便宜 + 雪崩 + 可复现"，`rand` 的 trait 体系与版本兼容成本对这个
/// 用途太重；而且自己的实现**永远不会换算法** —— 换算法等于换掉所有既有
/// 场景的粒子表现（截图基线会集体失效）。
#[derive(Debug, Clone)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // 种子 0 在 SplitMix 里完全能用，但 0 常来自"没填"，混一个常量
        // 让"默认配置"也有像样的分布。
        Self(seed ^ 0xA076_1D64_78BD_642F)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// `[0, 1)`。
    fn unit(&mut self) -> f32 {
        // 取高 24 位：f32 的尾数正好 24 位，除完落在 [0, 1)。
        ((self.next_u64() >> 40) as f32) / ((1u32 << 24) as f32)
    }

    /// `[min, max]`（区间反了也能给出合法结果）。
    fn range(&mut self, min: f32, max: f32) -> f32 {
        let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
        lo + (hi - lo) * self.unit()
    }

    /// `[-half, half]`。
    fn symmetric(&mut self, half: f32) -> f32 {
        self.range(-half, half)
    }

    /// 球面均匀方向。
    fn unit_direction(&mut self) -> Vec3 {
        let z = self.unit() * 2.0 - 1.0;
        let phi = self.unit() * std::f32::consts::TAU;
        let r = (1.0 - z * z).max(0.0).sqrt();
        Vec3::new(r * phi.cos(), z, r * phi.sin())
    }

    /// 以 `axis` 为轴、半角 `spread`（弧度）的圆锥内均匀方向。
    ///
    /// `spread >= π` 退化成全球面（均匀）。`spread <= 0` 时返回轴本身 ——
    /// 不消耗随机数（同一颗粒子的其它随机量因此不会移位，改 `spread`
    /// 不会让整批粒子的寿命都变）。
    fn cone(&mut self, axis: Vec3, spread: f32) -> Vec3 {
        if spread >= std::f32::consts::PI {
            return self.unit_direction();
        }
        if spread <= 0.0 {
            return axis;
        }
        // 圆帽上按面积均匀：cos 在 [cos(spread), 1] 上均匀。
        let cos_max = spread.cos();
        let cos_t = 1.0 - self.unit() * (1.0 - cos_max);
        let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
        let phi = self.unit() * std::f32::consts::TAU;
        let (u, v) = orthonormal_basis(axis);
        (u * (sin_t * phi.cos()) + v * (sin_t * phi.sin()) + axis * cos_t).normalize_or_zero()
    }
}

/// 给一个单位向量凑一组正交基（`u × v = axis`）。
fn orthonormal_basis(axis: Vec3) -> (Vec3, Vec3) {
    // 挑一个与 axis 不平行的参考轴：拿"最不像 axis"的那根世界轴。
    let helper = if axis.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let u = axis.cross(helper).normalize_or_zero();
    let v = axis.cross(u);
    (u, v)
}

// ---------------------------------------------------------------------------
// 单个粒子与输出格式
// ---------------------------------------------------------------------------

/// 一颗活着的粒子。**只有模拟会碰它**（渲染读的是 [`ParticleQuad`]）。
#[derive(Debug, Clone, Copy)]
struct Particle {
    position: Vec3,
    velocity: Vec3,
    age: f32,
    life: f32,
}

/// 渲染需要的一个 billboard 四边形（一次 draw 的一个实例）。
///
/// 尺寸是**世界单位**（正方形半边长），朝向由渲染侧按相机现算 —— 模拟层
/// 不知道相机在哪，这是刻意的：粒子模拟不该依赖相机，否则"换个视角就重算
/// 一遍模拟"。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParticleQuad {
    /// 世界空间中心。
    pub position: [f32; 3],
    /// 正方形半边长（世界单位）。
    pub size: f32,
    /// 线性空间 RGBA（RGB 允许 > 1）。
    pub color: [f32; 4],
    /// 混合方式（决定进哪个批次）。
    pub blend: ParticleBlend,
}

// ---------------------------------------------------------------------------
// 每个发射器的运行态
// ---------------------------------------------------------------------------

struct EmitterState {
    particles: Vec<Particle>,
    rng: Rng,
    /// 上一次使用的种子（作者改种子 = 要换一批随机数，见 `reseed_if`）。
    seed: u64,
    /// 连续发射的小数余量：`rate * dt` 常常不是整数，攒够 1 才生一颗。
    accum: f32,
    /// 当前发射窗口内的已用时长（只在 `duration > 0` 时推进）。
    elapsed: f32,
    /// 本发射器的粒子上限快照（`stats.capacity` 就是各桶这个数之和）。
    capacity: usize,
}

impl EmitterState {
    fn new(seed: u64) -> Self {
        Self {
            particles: Vec::new(),
            rng: Rng::new(seed),
            seed,
            accum: 0.0,
            elapsed: 0.0,
            capacity: 0,
        }
    }

    /// 种子变了就重建随机流（并保留已出生的粒子）。
    ///
    /// 只在种子真的变了时重建 —— 每帧无条件重建会让粒子流永远停在第 0 步，
    /// 表现是"所有粒子一模一样"。
    fn reseed_if(&mut self, seed: u64) {
        if self.seed != seed {
            self.seed = seed;
            self.rng = Rng::new(seed);
        }
    }
}

// ---------------------------------------------------------------------------
// 系统
// ---------------------------------------------------------------------------

/// 一帧的粒子统计（状态栏 / 无头报告 / 测试断言用）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParticleStats {
    /// 活着的粒子总数。
    pub alive: usize,
    /// 参与模拟的发射器数。
    pub emitters: usize,
    /// 所有发射器的 `max_particles` 之和（"撞顶"的诊断读数：
    /// `alive == capacity` 说明新粒子已经被上限挡在门外）。
    pub capacity: usize,
}

/// 粒子系统：按实体分桶的确定性 CPU 模拟。
#[derive(Default)]
pub struct ParticleSystem {
    emitters: HashMap<EntityId, EmitterState>,
    /// 本帧的四边形（跨帧复用容量）。
    quads: Vec<ParticleQuad>,
    /// 只在本帧确实有东西在动时推进的时钟（见 [`Self::signature`]）。
    time: f32,
    /// 清理已消失发射器时的复用暂存（避免每帧分配）。
    scratch: Vec<EntityId>,
    stats: ParticleStats,
    /// 上一帧是否"有东西在动"（宿主据此决定要不要继续排帧）。
    animating: bool,
}

impl ParticleSystem {
    pub fn new() -> Self {
        Self::default()
    }

    /// 推进一帧。返回本帧统计。
    ///
    /// 需要 `&mut Scene` 的唯一原因是 **burst 请求位**：脚本把数字写进
    /// `Entity::particle.burst`，这里消费掉并清零（一次性语义）。其余都是只读。
    pub fn update(&mut self, dt: f32, scene: &mut Scene) -> ParticleStats {
        if !dt.is_finite() || dt <= 0.0 {
            // dt = 0（暂停 / 同一时刻重复调用）不该有任何副作用。
            return self.stats;
        }
        // 卡顿保护：窗口被拖动几秒后 dt 可能上千毫秒，一次巨大的 dt 会让粒子
        // 瞬移穿墙、也让 `rate * dt` 一次补出上限数量的粒子。与时钟那边
        // `World` 收到的 dt 已经夹过一次，这里再兜一层底（模拟层不该假设调用方）。
        let dt = dt.min(0.25);

        let mut animating = false;
        let count = scene.entities.len();
        self.quads.clear();

        for index in 0..count {
            if scene.entities[index].particle.is_none() {
                continue;
            }
            // 世界变换先取（返回所有权，可变借用尚未开始）。
            let world = scene.world_transform(index);
            let entity = &mut scene.entities[index];
            let id = entity.id;
            let Some(emitter) = entity.particle.as_mut() else {
                continue;
            };
            let state = self
                .emitters
                .entry(id)
                .or_insert_with(|| EmitterState::new(emitter.seed));
            state.reseed_if(emitter.seed);
            state.step(emitter, &world, dt, &mut animating);
            state.push_quads(emitter, &mut self.quads);
        }

        // 实体被删 / 发射器被摘：整桶丢掉（理由见模块文档）。
        self.scratch.clear();
        for entity in &scene.entities {
            if entity.particle.is_some() {
                self.scratch.push(entity.id);
            }
        }
        self.emitters
            .retain(|id, _| self.scratch.contains(id));

        let alive = self.quads.len();
        self.animating = animating;
        if animating {
            self.time += dt;
        }
        self.stats = ParticleStats {
            alive,
            emitters: self.emitters.len(),
            capacity: self.emitters.values().map(|s| s.capacity).sum(),
        };
        self.stats
    }

    /// 本帧的四边形序列（发射顺序；渲染侧自己按混合方式分组、按深度排序）。
    pub fn quads(&self) -> &[ParticleQuad] {
        &self.quads
    }

    /// 最近一次 [`Self::update`] 的统计。
    pub fn stats(&self) -> ParticleStats {
        self.stats
    }

    /// 某个实体当前活着的粒子数（测试 / 诊断）。
    pub fn count_for(&self, id: EntityId) -> usize {
        self.emitters.get(&id).map_or(0, |s| s.particles.len())
    }

    /// 时钟：**只在"有东西在动"时前进**。
    pub fn time(&self) -> f32 {
        self.time
    }

    /// 上一帧是否有粒子活着或正在发射（宿主据此决定要不要继续排帧）。
    ///
    /// 与 [`Self::signature`] 是两件事：签名回答"画面变了吗"，这个回答
    /// "还要不要继续出帧"。编辑器两个都要用 —— 前者做静帧跳过，后者做
    /// 帧调度（否则视角不动、粒子在飞时会被当成"空闲"停下来）。
    pub fn animating(&self) -> bool {
        self.animating
    }

    /// 渲染指纹：宿主用它判断"这一帧要不要重画"。
    ///
    /// 设计约束（照抄 `scene_renderer` 的指纹门控要求）：
    ///
    /// - **静止就必须稳定** —— 没有任何发射器 / 发射器全关且粒子已清空时，
    ///   `time` 不前进，签名恒定，编辑器静态帧照旧整帧跳过；
    /// - **在动就必须每帧都变** —— 只要有粒子活着或正在发射，`time` 每帧
    ///   前进，签名必变，粒子不会被"静帧不重画"冻住。
    ///
    /// 刻意**不**哈希每个粒子的位置：那是 O(n) 且没必要 —— 时钟前进本身就
    /// 已经表达了"这一帧跟上一帧不一样"。
    pub fn signature(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for value in [
            self.stats.alive as u64,
            self.stats.emitters as u64,
            self.time.to_bits() as u64,
        ] {
            hash ^= value;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// 清空所有粒子与发射器状态（切场景 / 停止播放）。
    pub fn clear(&mut self) {
        self.emitters.clear();
        self.quads.clear();
        self.stats = ParticleStats::default();
        self.animating = false;
    }

    /// 丢掉某个实体的粒子（实体销毁时调用；`update` 的 retain 也会兜住）。
    pub fn drop_entity(&mut self, id: EntityId) {
        self.emitters.remove(&id);
    }
}

impl EmitterState {
    /// 发射 + 积分 + 淘汰。
    fn step(
        &mut self,
        emitter: &mut ParticleEmitter,
        world: &Transform,
        dt: f32,
        animating: &mut bool,
    ) {
        // -- 发射窗口（duration / looping）--
        if emitter.duration > 0.0 {
            self.elapsed += dt;
            while self.elapsed >= emitter.duration {
                if emitter.looping {
                    self.elapsed -= emitter.duration;
                    // 新一轮从头开始：小数余量也清零，否则"每一轮都多一颗"。
                    self.accum = 0.0;
                } else {
                    self.elapsed = emitter.duration;
                    break;
                }
            }
        }
        let within_window = emitter.duration <= 0.0 || self.elapsed < emitter.duration;
        let continuous = emitter.emitting && within_window && emitter.rate > 0.0;
        if continuous {
            *animating = true;
        }

        let max = emitter.max_particles as usize;
        // 上限写进状态：统计的 capacity 就是各桶这个数之和。放在这里（而不是
        // 每帧从 emitter 现取）是因为 stats 要在 update 末统一算，那时 emitter
        // 的借用已经散了。
        self.capacity = max;
        let origin = world.position;
        let rotation = world.rotation;
        let axis = rotation * emitter.direction_vec();
        let spread_rad = emitter.spread.to_radians();

        // -- 连续发射 --
        if continuous {
            self.accum += emitter.rate * dt;
            // 单帧最多补 256 颗：`rate` 手滑写成 1e5 时不能一帧锁死。
            let mut budget = 256usize;
            while self.accum >= 1.0 && budget > 0 {
                if self.particles.len() >= max {
                    // 满了：欠账丢掉（不攒），否则一旦有空位会瞬间灌满。
                    self.accum = 0.0;
                    break;
                }
                self.accum -= 1.0;
                self.spawn(emitter, origin, rotation, axis, spread_rad);
                budget -= 1;
            }
            if budget == 0 {
                self.accum = 0.999;
            }
        } else if !within_window {
            self.accum = 0.0;
        }

        // -- 脚本 burst（一次性：消费后清零）--
        if emitter.burst > 0 {
            let requested = emitter.burst;
            emitter.burst = 0;
            *animating = true;
            for _ in 0..requested {
                if self.particles.len() >= max {
                    break;
                }
                self.spawn(emitter, origin, rotation, axis, spread_rad);
            }
        }

        // -- 积分与淘汰（保序：`retain_mut` 稳定，输出顺序因此可复现）--
        let gravity = Vec3::from(emitter.gravity);
        // 半隐式阻尼：`1 / (1 + k*dt)` 在任意 dt 下都稳定（显式 `v *= 1-k*dt`
        // 在 `k*dt > 1` 时会翻号 —— 一次卡顿就能让整团粒子反向飞出）。
        let damp = 1.0 / (1.0 + emitter.drag.max(0.0) * dt);
        self.particles.retain_mut(|particle| {
            particle.age += dt;
            if particle.age >= particle.life {
                return false;
            }
            particle.velocity = (particle.velocity + gravity * dt) * damp;
            particle.position += particle.velocity * dt;
            true
        });

        if !self.particles.is_empty() {
            *animating = true;
        }
    }

    /// 生一颗粒子。
    fn spawn(
        &mut self,
        emitter: &ParticleEmitter,
        origin: Vec3,
        rotation: Quat,
        axis: Vec3,
        spread_rad: f32,
    ) {
        let offset = match emitter.shape {
            xgame_core::EmissionShape::Point => Vec3::ZERO,
            xgame_core::EmissionShape::Sphere { radius } => {
                // 半径按 `cbrt(u)` 取，球内体积均匀（直接乘 `u` 会挤在中心）。
                self.rng.unit_direction() * (radius.max(0.0) * self.rng.unit().cbrt())
            }
            xgame_core::EmissionShape::Box { half_extents } => Vec3::new(
                self.rng.symmetric(half_extents[0]),
                self.rng.symmetric(half_extents[1]),
                self.rng.symmetric(half_extents[2]),
            ),
        };
        let position = origin + rotation * offset;
        let direction = self.rng.cone(axis, spread_rad);
        let speed = self.rng.range(emitter.speed[0], emitter.speed[1]);
        let life = self.rng.range(emitter.lifetime[0], emitter.lifetime[1]);
        self.particles.push(Particle {
            position,
            velocity: direction * speed,
            age: 0.0,
            // 寿命不能是 0：`age / life` 会变成 NaN，四边形尺寸随之为 NaN。
            life: life.max(1.0e-4),
        });
    }

    /// 把活着的粒子转成本帧的四边形。
    fn push_quads(&self, emitter: &ParticleEmitter, out: &mut Vec<ParticleQuad>) {
        for particle in &self.particles {
            let t = (particle.age / particle.life).clamp(0.0, 1.0);
            out.push(ParticleQuad {
                position: particle.position.to_array(),
                size: lerp(emitter.start_size, emitter.end_size, t).max(0.0),
                color: [
                    lerp(emitter.start_color[0], emitter.end_color[0], t),
                    lerp(emitter.start_color[1], emitter.end_color[1], t),
                    lerp(emitter.start_color[2], emitter.end_color[2], t),
                    lerp(emitter.start_color[3], emitter.end_color[3], t),
                ],
                blend: emitter.blend,
            });
        }
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use xgame_core::EmissionShape;

    /// 造一个只有发射器的场景，返回 `(scene, id)`。
    fn scene_with(emitter: ParticleEmitter) -> (Scene, EntityId) {
        let mut scene = Scene::new("Particles");
        let id = scene.add_entity(xgame_core::Entity::new("Emitter").with_particle(emitter));
        (scene, id)
    }

    fn step_many(system: &mut ParticleSystem, scene: &mut Scene, frames: usize, dt: f32) {
        for _ in 0..frames {
            system.update(dt, scene);
        }
    }

    #[test]
    fn same_seed_is_bit_identical() {
        let emitter = ParticleEmitter::sparks();
        let (mut a_scene, _) = scene_with(emitter.clone());
        let (mut b_scene, _) = scene_with(emitter);
        let mut a = ParticleSystem::new();
        let mut b = ParticleSystem::new();
        step_many(&mut a, &mut a_scene, 40, 1.0 / 60.0);
        step_many(&mut b, &mut b_scene, 40, 1.0 / 60.0);
        assert!(!a.quads().is_empty(), "应该有粒子，否则这条测试没有意义");
        assert_eq!(a.quads(), b.quads(), "同种子 + 同 dt 序列必须逐位一致");
        assert_eq!(a.signature(), b.signature());
    }

    #[test]
    fn different_seed_diverges() {
        let mut emitter = ParticleEmitter::sparks();
        emitter.seed = 1;
        let (mut a_scene, _) = scene_with(emitter.clone());
        emitter.seed = 2;
        let (mut b_scene, _) = scene_with(emitter);
        let mut a = ParticleSystem::new();
        let mut b = ParticleSystem::new();
        step_many(&mut a, &mut a_scene, 20, 1.0 / 60.0);
        step_many(&mut b, &mut b_scene, 20, 1.0 / 60.0);
        assert_ne!(a.quads(), b.quads());
    }

    #[test]
    fn rate_accumulates_across_frames() {
        let mut emitter = ParticleEmitter::sparks();
        emitter.seed = 0;
        emitter.rate = 10.0;
        emitter.max_particles = 10_000;
        emitter.spread = 0.0;
        emitter.gravity = [0.0; 3];
        emitter.drag = 0.0;
        emitter.lifetime = [100.0, 100.0];
        let (mut scene, id) = scene_with(emitter);
        let mut system = ParticleSystem::new();
        // 10 颗/秒 × 0.1 秒 = 每帧 1 颗，10 帧正好 10 颗。
        step_many(&mut system, &mut scene, 10, 0.1);
        assert_eq!(system.count_for(id), 10);
    }

    #[test]
    fn particles_die_after_lifetime() {
        let mut emitter = ParticleEmitter::sparks();
        emitter.rate = 1000.0;
        emitter.lifetime = [0.5, 0.5];
        emitter.max_particles = 10_000;
        let (mut scene, _) = scene_with(emitter);
        let mut system = ParticleSystem::new();
        // 只让它在头 0.1 秒发射，再关掉。
        step_many(&mut system, &mut scene, 1, 0.1);
        if let Some(p) = scene.entities[0].particle.as_mut() {
            p.emitting = false;
        }
        let born = system.stats().alive;
        assert!(born > 0);
        // 走满 0.6 秒后全部到期。
        step_many(&mut system, &mut scene, 6, 0.1);
        assert_eq!(system.stats().alive, 0, "过了寿命必须一颗不剩");
        assert!(system.quads().is_empty());
    }

    #[test]
    fn max_particles_is_a_hard_cap() {
        let mut emitter = ParticleEmitter::sparks();
        emitter.rate = 10_000.0;
        emitter.lifetime = [10.0, 10.0];
        emitter.max_particles = 37;
        let (mut scene, _) = scene_with(emitter);
        let mut system = ParticleSystem::new();
        step_many(&mut system, &mut scene, 30, 1.0 / 60.0);
        assert_eq!(system.stats().alive, 37, "上限是硬顶");
    }

    #[test]
    fn gravity_moves_particles_down() {
        let mut emitter = ParticleEmitter::sparks();
        emitter.rate = 0.0;
        emitter.burst = 1;
        emitter.speed = [0.0, 0.0];
        emitter.gravity = [0.0, -10.0, 0.0];
        emitter.drag = 0.0;
        emitter.lifetime = [10.0, 10.0];
        emitter.spread = 0.0;
        emitter.shape = EmissionShape::Point;
        let (mut scene, _) = scene_with(emitter);
        let mut system = ParticleSystem::new();
        system.update(0.1, &mut scene); // 出生 + 第一步积分
        let y0 = system.quads()[0].position[1];
        system.update(0.1, &mut scene);
        let y1 = system.quads()[0].position[1];
        assert!(y1 < y0, "重力必须把它往下拉: {y0} -> {y1}");
    }

    #[test]
    fn burst_fires_once_and_clears_the_request() {
        let mut emitter = ParticleEmitter::sparks();
        emitter.rate = 0.0;
        emitter.burst = 5;
        emitter.lifetime = [10.0, 10.0];
        let (mut scene, id) = scene_with(emitter);
        let mut system = ParticleSystem::new();
        system.update(1.0 / 60.0, &mut scene);
        assert_eq!(system.count_for(id), 5);
        assert_eq!(
            scene.entities[0].particle.as_ref().unwrap().burst,
            0,
            "burst 是一次性请求，消费后必须清零"
        );
        system.update(1.0 / 60.0, &mut scene);
        assert_eq!(system.count_for(id), 5, "第二帧不该再喷一批");
    }

    #[test]
    fn zero_spread_keeps_the_axis() {
        let mut emitter = ParticleEmitter::sparks();
        emitter.rate = 0.0;
        emitter.burst = 20;
        emitter.spread = 0.0;
        emitter.speed = [1.0, 1.0];
        emitter.gravity = [0.0; 3];
        emitter.drag = 0.0;
        emitter.lifetime = [10.0, 10.0];
        emitter.shape = EmissionShape::Point;
        let (mut scene, id) = scene_with(emitter);
        let mut system = ParticleSystem::new();
        system.update(1.0 / 60.0, &mut scene);
        assert_eq!(system.count_for(id), 20);
        for quad in system.quads() {
            // 只要有一丁点横向偏移就说明方向锥没守住 0°。
            assert!(quad.position[0].abs() < 1.0e-5, "{:?}", quad.position);
            assert!(quad.position[2].abs() < 1.0e-5, "{:?}", quad.position);
            assert!(quad.position[1] > 0.0, "{:?}", quad.position);
        }
    }

    #[test]
    fn entity_rotation_turns_the_cone() {
        let mut emitter = ParticleEmitter::sparks();
        emitter.rate = 0.0;
        emitter.burst = 4;
        emitter.spread = 0.0;
        emitter.speed = [2.0, 2.0];
        emitter.gravity = [0.0; 3];
        emitter.drag = 0.0;
        emitter.lifetime = [10.0, 10.0];
        emitter.shape = EmissionShape::Point;
        // 朝向 +X（默认 +Y 绕 -Z 转 90°）。
        let mut scene = Scene::new("Rotated");
        let mut entity = xgame_core::Entity::new("E").with_particle(emitter);
        entity.transform.rotation = Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2);
        scene.add_entity(entity);
        let mut system = ParticleSystem::new();
        system.update(1.0 / 60.0, &mut scene);
        for quad in system.quads() {
            assert!(quad.position[0] > 0.0, "应该朝 +X 飞: {:?}", quad.position);
        }
    }

    #[test]
    fn removing_the_entity_drops_its_particles() {
        let mut emitter = ParticleEmitter::sparks();
        emitter.lifetime = [10.0, 10.0];
        let (mut scene, id) = scene_with(emitter);
        let mut system = ParticleSystem::new();
        step_many(&mut system, &mut scene, 5, 1.0 / 60.0);
        assert!(system.stats().alive > 0);
        scene.remove_entity(id);
        system.update(1.0 / 60.0, &mut scene);
        assert_eq!(system.stats().alive, 0);
        assert_eq!(system.stats().emitters, 0);
    }

    #[test]
    fn one_shot_emitter_stops_after_duration() {
        let mut emitter = ParticleEmitter::sparks();
        emitter.lifetime = [10.0, 10.0];
        emitter.duration = 0.2;
        emitter.looping = false;
        emitter.rate = 100.0;
        emitter.max_particles = 10_000;
        let (mut scene, id) = scene_with(emitter);
        let mut system = ParticleSystem::new();
        step_many(&mut system, &mut scene, 4, 0.1); // 0.4 秒 > 0.2
        let after_window = system.count_for(id);
        step_many(&mut system, &mut scene, 4, 0.1);
        assert_eq!(
            system.count_for(id),
            after_window,
            "过了 duration 且不循环就必须停止发射"
        );
    }

    #[test]
    fn signature_is_stable_when_idle_and_moves_while_alive() {
        // 没有发射器：签名恒定（编辑器静态帧照旧跳过）。
        let mut scene = Scene::new("Empty");
        scene.add_entity(xgame_core::Entity::new("Nothing"));
        let mut system = ParticleSystem::new();
        system.update(1.0 / 60.0, &mut scene);
        let idle = system.signature();
        system.update(1.0 / 60.0, &mut scene);
        assert_eq!(system.signature(), idle);

        // 有粒子在飞：每帧都得变。
        let (mut scene, _) = scene_with(ParticleEmitter::sparks());
        let mut system = ParticleSystem::new();
        system.update(1.0 / 60.0, &mut scene);
        let a = system.signature();
        system.update(1.0 / 60.0, &mut scene);
        assert_ne!(system.signature(), a);
    }

    #[test]
    fn disabled_emitter_without_particles_is_idle() {
        let mut emitter = ParticleEmitter::sparks();
        emitter.emitting = false;
        emitter.rate = 0.0;
        let (mut scene, _) = scene_with(emitter);
        let mut system = ParticleSystem::new();
        system.update(1.0 / 60.0, &mut scene);
        let idle = system.signature();
        system.update(1.0 / 60.0, &mut scene);
        assert_eq!(system.signature(), idle, "关着且没粒子时不能每帧都要求重画");
    }

    #[test]
    fn size_and_color_follow_life_ratio() {
        let mut emitter = ParticleEmitter::sparks();
        emitter.rate = 0.0;
        emitter.burst = 1;
        emitter.speed = [0.0, 0.0];
        emitter.gravity = [0.0; 3];
        emitter.drag = 0.0;
        emitter.lifetime = [1.0, 1.0];
        emitter.start_size = 1.0;
        emitter.end_size = 0.0;
        emitter.start_color = [1.0, 1.0, 1.0, 1.0];
        emitter.end_color = [0.0, 0.0, 0.0, 0.0];
        let (mut scene, _) = scene_with(emitter);
        let mut system = ParticleSystem::new();
        // dt 会被夹到 0.25（卡顿保护），所以走两步凑出 age = 0.5。
        system.update(0.25, &mut scene);
        system.update(0.25, &mut scene);
        let quad = system.quads()[0];
        assert!((quad.size - 0.5).abs() < 0.01, "size={}", quad.size);
        assert!((quad.color[3] - 0.5).abs() < 0.01, "alpha={}", quad.color[3]);
    }
}
