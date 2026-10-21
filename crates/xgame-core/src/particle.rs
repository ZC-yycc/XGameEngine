//! 粒子发射器：**纯数据层**（声明式配置，不含任何模拟或 GPU 代码）。
//!
//! # 分层
//!
//! ```text
//! core（本模块：ParticleEmitter 声明）
//!   ↑
//! xgame-particle（CPU 模拟：发射 / 积分 / 寿命 → ParticleQuad）
//!   ↑                        ↑
//! runtime::World          xgame-render（billboard 实例化管线）
//! ```
//!
//! 放在 core 的理由与 `AudioSource` / `Animator` 完全一致：**组件是场景数据**，
//! 要能 RON 序列化、能进撤销快照、能被脚本读改。模拟与绘制是上层各自的事，
//! 谁也不该为了序列化去依赖另一个。
//!
//! # 为什么不给粒子留"运行时状态"字段
//!
//! 单个粒子的位置 / 速度 / 年龄是真·运行时数据，量大且每帧全变，**不进实体**：
//! 它们住在 `xgame_particle::ParticleSystem` 里（按实体 id 分桶）。
//! 实体上只留一个 [`ParticleEmitter::burst`] 请求位 —— 脚本"炸一下"需要一条
//! 从脚本到模拟层的通道，而命令束（`AudioPort` 那种）对单个整数来说太重。
//! 该字段 `#[serde(skip)]`，不落盘、不进场景 diff。
//!
//! # 与 `ui` 的对照
//!
//! `UiElement` 的 `state` 也走 `#[serde(skip)]`，理由相同：运行态就地挂在组件上
//! 比另开一张 `HashMap<EntityId, _>` 少一份同步义务，也不会在实体复制 / 删除时
//! 留下悬垂条目。

use serde::{Deserialize, Serialize};

/// 粒子混合方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParticleBlend {
    /// 直通 alpha 混合（`src*a + dst*(1-a)`）：烟雾、尘土、落叶。
    Alpha,
    /// 加色混合（`src*a + dst`）：火焰、火花、能量 —— 亮部叠起来自然过曝。
    Additive,
}

impl Default for ParticleBlend {
    fn default() -> Self {
        Self::Alpha
    }
}

/// 发射体积。决定**出生位置**在实体局部空间里的分布。
///
/// 方向不由形状决定，见 [`ParticleEmitter::direction`] 与
/// [`ParticleEmitter::spread`] —— 两者正交，形状只管"从哪儿冒出来"。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum EmissionShape {
    /// 一个点（实体原点）。
    Point,
    /// 实心球内均匀分布（半径 = `radius`）。
    Sphere {
        /// 球半径（世界单位）。
        radius: f32,
    },
    /// 实心盒内均匀分布（半边长）。
    Box {
        /// 三个半边长。
        half_extents: [f32; 3],
    },
}

impl Default for EmissionShape {
    fn default() -> Self {
        Self::Point
    }
}

impl EmissionShape {
    /// 把越界参数钳回合法范围（加载边界与 Inspector 共用）。
    fn sanitized(self) -> Self {
        match self {
            Self::Point => Self::Point,
            Self::Sphere { radius } => Self::Sphere {
                radius: sane(radius, 0.0, 1.0e4, 0.0),
            },
            Self::Box { half_extents } => Self::Box {
                half_extents: [
                    sane(half_extents[0], 0.0, 1.0e4, 0.0),
                    sane(half_extents[1], 0.0, 1.0e4, 0.0),
                    sane(half_extents[2], 0.0, 1.0e4, 0.0),
                ],
            },
        }
    }
}

/// 实体上的粒子发射器组件。
///
/// `None`（实体没挂）= 不发射任何粒子，零成本（模拟层连一个桶都不建）。
///
/// # 关键语义
///
/// - **发射时长**：[`duration`](Self::duration) > 0 时只在开头这么多秒里发射；
///   `looping` 为真则到点重来。`duration <= 0` = 一直发射（默认）。
/// - **粒子生命周期**：出生即独立，实体移动 / 消亡不追着粒子跑（世界空间，
///   与 Unity 的 `Simulation Space = World` 一致）。发射**位置**每帧按实体
///   当前世界变换取，所以"边飞边喷"是自然的。
/// - **上限**：[`max_particles`](Self::max_particles) 是硬顶，满了就不再生成
///   （先死的先腾位置）—— 没有它，一个手滑的 `rate` 就能把帧时间吃光。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ParticleEmitter {
    /// 是否持续发射。脚本 `particle_set_emitting(false)` 关掉之后，
    /// 已经出生的粒子照常走完寿命（不是"清屏"）。
    pub emitting: bool,
    /// 每秒发射数（`0` = 只有 burst 才出粒子）。
    pub rate: f32,
    /// 同时存在的粒子上限。
    pub max_particles: u32,
    /// 发射时长（秒）。`<= 0` = 无限。
    pub duration: f32,
    /// 到时后是否重来（只在 `duration > 0` 时有意义）。
    pub looping: bool,

    /// 寿命区间 `[最短, 最长]`（秒）。
    pub lifetime: [f32; 2],
    /// 初速度区间 `[最慢, 最快]`（世界单位/秒）。
    pub speed: [f32; 2],
    /// 发射方向（**实体局部空间**，会随实体朝向转）。
    pub direction: [f32; 3],
    /// 方向锥的半角（度）。`0` = 笔直一条线，`180` = 全向球。
    pub spread: f32,
    /// 发射体积。
    pub shape: EmissionShape,

    /// 粒子自身受到的重力（世界空间，`[0, -9.81, 0]` = 地球）。
    ///
    /// **用发射器自己的重力而不是取自物理世界**：火焰要往上飘、烟要慢慢散，
    /// 与场景全局重力无关；而且粒子是纯视觉，不该被物理世界的参数牵着走。
    pub gravity: [f32; 3],
    /// 线性阻尼（每秒衰减比例，`0` = 无阻尼）。
    pub drag: f32,

    /// 出生尺寸（世界单位，四边形的半边长）。
    pub start_size: f32,
    /// 死亡尺寸。
    pub end_size: f32,
    /// 出生颜色（**线性空间** RGBA；与材质 / UI 同一套颜色约定）。
    ///
    /// 分量允许 > 1：加色粒子写 2.0 就真的更亮，配 Bloom 会自然晕开。
    pub start_color: [f32; 4],
    /// 死亡颜色。
    pub end_color: [f32; 4],
    /// 混合方式。
    pub blend: ParticleBlend,

    /// 随机种子。同一个种子 + 同一段 dt 序列 ⇒ 逐位相同的结果
    /// （无头截图与回归测试依赖这一条）。
    pub seed: u64,

    /// **运行时**：待处理的 burst 请求（脚本"炸一下"用）。
    ///
    /// `#[serde(skip)]`：不落盘。模拟层消费后清零，所以它天然是"一次性"的。
    #[serde(skip)]
    pub burst: u32,
}

impl Default for ParticleEmitter {
    fn default() -> Self {
        // 默认值刻意是一份**能直接看见东西**的火焰：新挂一个发射器立即可用，
        // 作者再按需要调。默认全零的话，挂上去什么都没发生，更像坏了。
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
            burst: 0,
        }
    }
}

/// 同时存在的粒子上限的硬上限（防止手改 RON 写成天文数字）。
pub const MAX_PARTICLES_LIMIT: u32 = 200_000;

impl ParticleEmitter {
    /// 火焰预设（默认值就是它，这里只是给"逐字段想改一个"的调用方一个入口）。
    pub fn fire() -> Self {
        Self::default()
    }

    /// 烟尘预设：灰色、Alpha 混合、慢、飘。
    pub fn smoke() -> Self {
        Self {
            rate: 24.0,
            lifetime: [1.6, 2.8],
            speed: [0.3, 0.8],
            spread: 25.0,
            shape: EmissionShape::Sphere { radius: 0.25 },
            gravity: [0.0, 0.35, 0.0],
            drag: 1.1,
            start_size: 0.22,
            end_size: 0.75,
            start_color: [0.32, 0.32, 0.34, 0.55],
            end_color: [0.18, 0.18, 0.2, 0.0],
            blend: ParticleBlend::Alpha,
            ..Self::default()
        }
    }

    /// 火花预设：小、快、强重力、加色。
    pub fn sparks() -> Self {
        Self {
            rate: 90.0,
            lifetime: [0.45, 0.9],
            speed: [3.0, 6.0],
            spread: 35.0,
            gravity: [0.0, -9.81, 0.0],
            drag: 0.2,
            start_size: 0.05,
            end_size: 0.01,
            start_color: [1.5, 0.9, 0.35, 1.0],
            end_color: [1.0, 0.25, 0.0, 0.0],
            blend: ParticleBlend::Additive,
            ..Self::default()
        }
    }

    /// 一次爆发（不持续发射）。`count` 个粒子在同一帧出生。
    pub fn burst_of(count: u32) -> Self {
        Self {
            emitting: false,
            rate: 0.0,
            burst: count,
            ..Self::default()
        }
    }

    /// 把越界 / 非有限的值钳到合法范围，返回是否真的动过。
    ///
    /// 两个调用点：`ProjectHandle::load_scene` 的加载边界（手写 RON 里的 `nan`
    /// 不该进 GPU）与 Inspector 的拖拽输入。返回 `bool` 让调用方能记一条告警。
    pub fn sanitized(&mut self) -> bool {
        let before = self.clone();
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
        // 全零方向没有"锥"可言，退回 +Y（与默认值一致）而不是让粒子原地不动。
        if self.direction.iter().all(|v| *v == 0.0) {
            self.direction = [0.0, 1.0, 0.0];
        }
        self.spread = sane(self.spread, 0.0, 180.0, 18.0);
        self.shape = self.shape.sanitized();
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
        before != *self
    }

    /// 只比**落盘内容**（忽略 [`burst`](Self::burst)，它是运行态）。
    ///
    /// `Entity::eq` 用它而不是 `==`：脚本 `particle_burst()` 写进去的那几个数字
    /// 不该让编辑器显示"未保存"。与 `UiElement::data_eq` 同一套理由。
    pub fn data_eq(&self, other: &Self) -> bool {
        // 结构体很小（< 120 B），复制两份比逐字段写 20 行比较更不容易漏字段 ——
        // 漏一个字段的后果是"改了没反应"，比多拷几十字节糟得多。
        let mut a = self.clone();
        let mut b = other.clone();
        a.burst = 0;
        b.burst = 0;
        a == b
    }

    /// 有效发射方向（归一化后转成 `glam` 向量；退化时退回 +Y）。
    pub fn direction_vec(&self) -> glam::Vec3 {
        let v = glam::Vec3::from(self.direction);
        if v.length_squared() < 1.0e-12 {
            glam::Vec3::Y
        } else {
            v.normalize()
        }
    }
}

/// 钳制到 `[lo, hi]`；非有限值退回 `fallback`。
fn sane(value: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if !value.is_finite() {
        return fallback;
    }
    value.clamp(lo, hi)
}

/// 钳制一个 `[min, max]` 区间：两个分量各自钳好，再保证 `min <= max`。
fn sane_range(range: [f32; 2], lo: f32, hi: f32, fallback: [f32; 2]) -> [f32; 2] {
    let min = sane(range[0], lo, hi, fallback[0]);
    let max = sane(range[1], lo, hi, fallback[1]);
    if min <= max {
        [min, max]
    } else {
        [max, min]
    }
}

/// 颜色钳制：RGB 允许到 64（HDR 高亮配 Bloom），alpha 收到 `[0, 1]`。
fn sane_color(color: [f32; 4], fallback: [f32; 4]) -> [f32; 4] {
    [
        sane(color[0], 0.0, 64.0, fallback[0]),
        sane(color[1], 0.0, 64.0, fallback[1]),
        sane(color[2], 0.0, 64.0, fallback[2]),
        sane(color[3], 0.0, 1.0, fallback[3]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_visible_fire_not_a_no_op() {
        let e = ParticleEmitter::default();
        assert!(e.emitting && e.rate > 0.0 && e.max_particles > 0);
        assert_eq!(e.blend, ParticleBlend::Additive);
        assert!(e.start_color[3] > 0.0, "默认得有可见的不透明度");
    }

    #[test]
    fn ron_roundtrip_keeps_every_field() {
        let mut e = ParticleEmitter::smoke();
        e.seed = 42;
        e.burst = 7;
        let text = ron::to_string(&e).unwrap();
        let back: ParticleEmitter = ron::from_str(&text).unwrap();
        assert_eq!(back.seed, 42);
        assert_eq!(back.blend, ParticleBlend::Alpha);
        assert_eq!(back.shape, e.shape);
        // burst 是运行态：不落盘，回来必须是 0。
        assert_eq!(back.burst, 0, "burst 必须被 serde(skip) 挡住");
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // 只写一个字段的旧 / 手写配置也要能读。
        let e: ParticleEmitter = ron::from_str("(rate: 12.0)").unwrap();
        assert_eq!(e.rate, 12.0);
        assert_eq!(e.lifetime, ParticleEmitter::default().lifetime);
        assert!(e.emitting);
    }

    #[test]
    fn sanitized_clamps_nan_and_orders_ranges() {
        let mut e = ParticleEmitter {
            rate: f32::NAN,
            max_particles: u32::MAX,
            lifetime: [3.0, 1.0],
            speed: [-5.0, f32::INFINITY],
            spread: 400.0,
            drag: -1.0,
            start_size: f32::NAN,
            end_color: [1.0e9, -2.0, 1.0, 9.0],
            shape: EmissionShape::Sphere { radius: f32::NAN },
            ..ParticleEmitter::default()
        };
        assert!(e.sanitized(), "确实改了东西就得返回 true");
        assert_eq!(e.rate, 0.0);
        assert_eq!(e.max_particles, MAX_PARTICLES_LIMIT);
        assert_eq!(e.lifetime, [1.0, 3.0], "区间必须被排序");
        // 非有限值退回**默认值**（而不是钳到上界）：`inf` 几乎总是手改坏了，
        // 猜"作者想要极快"不如回到一份确定可用的配置。
        assert_eq!(e.speed, [0.0, 2.2]);
        assert_eq!(e.spread, 180.0);
        assert_eq!(e.drag, 0.0);
        assert!(e.start_size.is_finite());
        assert_eq!(e.end_color, [64.0, 0.0, 1.0, 1.0]);
        assert_eq!(e.shape, EmissionShape::Sphere { radius: 0.0 });
        // 再钳一次不该再变（幂等）。
        assert!(!e.sanitized());
    }

    #[test]
    fn zero_direction_falls_back_to_up() {
        let mut e = ParticleEmitter {
            direction: [0.0, 0.0, 0.0],
            ..ParticleEmitter::default()
        };
        e.sanitized();
        assert_eq!(e.direction, [0.0, 1.0, 0.0]);
        assert_eq!(e.direction_vec(), glam::Vec3::Y);
    }

    #[test]
    fn burst_preset_does_not_emit_continuously() {
        let e = ParticleEmitter::burst_of(25);
        assert!(!e.emitting);
        assert_eq!(e.rate, 0.0);
        assert_eq!(e.burst, 25);
    }
}
