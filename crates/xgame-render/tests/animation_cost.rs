//! 微基准：一帧动画采样的 CPU 成本（三种插值分档）。
//!
//! 默认 `#[ignore]`（绝对值随机器浮动，不该进 CI 门禁）。手动跑：
//!
//! ```sh
//! cargo test -p xgame-render --release --test animation_cost -- --ignored --nocapture
//! ```
//!
//! # 比什么
//!
//! 28 骨 × 3 通道 = 84 条轨道的角色，**每帧**都要把剪辑采样成局部姿态。
//! 三条插值路径的读法不同：
//!
//! | 档位 | 每条轨道每帧读 | 段内计算 |
//! |---|---|---|
//! | 线性 | 1 份值 | 1 次 lerp（旋转 slerp） |
//! | 阶梯 | 1 份值 | 无（直接取左帧） |
//! | 三次样条 | 3 份（入切线 / 值 / 出切线） | 4 个基函数 + 4 次乘加，旋转再归一化 |
//!
//! 这张表就是"保真换成本"的定价：样条贵多少、值不值得开（答案在
//! `Docs/决策记录.md` §五十一 —— 切线是内容，丢了补不回来）。
//!
//! # 口径
//!
//! 每档跑 `ROUNDS` 轮、每轮 `SAMPLES` 次采样取**最快一轮**（最快轮最接近
//! "没有别的进程在抢核"的状态）。只量 [`sample_clip_into`]，
//! 不含蒙皮矩阵解算（`compute_bones` 与插值无关，另行计入渲染段）。
//!
//! ⚠ 与 `update_throughput` 同款警告：**跨版本比较必须同机背靠背**。

use std::hint::black_box;
use std::time::Instant;

use xgame_math::glam::{Quat, Vec3};
use xgame_render::animation::{
    default_pose, sample_clip_into, AnimClip, AnimNode, Interpolation, Track, TrackPath,
};

/// 骨骼数（对齐示例角色：Fox 24 骨，这里取 28 更保守）。
const BONES: usize = 28;
/// 每条轨道的关键帧数。
const KEYS: usize = 16;
/// 每档的重复轮数。
const ROUNDS: usize = 7;
/// 每轮的采样次数。
const SAMPLES: usize = 2000;

fn nodes() -> Vec<AnimNode> {
    (0..BONES)
        .map(|i| AnimNode {
            parent: (i > 0).then_some(0),
            name: String::new(),
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        })
        .collect()
}

/// 造一段 28 骨 × 3 通道的剪辑。
///
/// 值曲线取正弦（相邻帧确实不同，避免编译器把 lerp 折叠成常量）；
/// 旋转用半角公式构造单位四元数；切线取值的导数近似 —— 与真实导出的
/// 数据同量级，不会因为切线全 0 把样条测成线性。
fn clip(interpolation: Interpolation) -> AnimClip {
    let slots = if interpolation == Interpolation::CubicSpline {
        3
    } else {
        1
    };
    let paths = [
        TrackPath::Translation,
        TrackPath::Rotation,
        TrackPath::Scale,
    ];
    let mut tracks = Vec::with_capacity(BONES * paths.len());
    for bone in 0..BONES as u32 {
        for &path in &paths {
            let comps = if path == TrackPath::Rotation { 4 } else { 3 };
            let mut times = Vec::with_capacity(KEYS);
            let mut values = Vec::with_capacity(KEYS * comps * slots);
            for k in 0..KEYS {
                let f = k as f32;
                times.push(f * 0.0625);
                let sample = |c: usize| -> f32 {
                    let phase = (bone as f32 + c as f32) * 0.37;
                    match path {
                        TrackPath::Rotation => {
                            // 绕 Z 轴的小角度旋转：(0, 0, sin(θ/2), cos(θ/2))
                            let theta = (f * 0.4 + phase).sin() * 0.5;
                            match c {
                                2 => (theta * 0.5).sin(),
                                3 => (theta * 0.5).cos(),
                                _ => 0.0,
                            }
                        }
                        _ => (f * 0.4 + phase).sin(),
                    }
                };
                for slot in 0..slots {
                    // 0 = 入切线、1 = 值、2 = 出切线；切线取 "值 × 0.4" 量级。
                    let scale = if slot == 1 { 1.0 } else { 0.4 };
                    for c in 0..comps {
                        values.push(sample(c) * scale);
                    }
                }
            }
            tracks.push(Track {
                node: bone,
                path,
                interpolation,
                times,
                values,
            });
        }
    }
    AnimClip {
        name: "bench".into(),
        duration: 1.0,
        tracks,
    }
}

/// 采样 `SAMPLES` 次的最快一轮，返回**单次采样**的纳秒数。
fn measure(clip: &AnimClip, nodes: &[AnimNode]) -> f64 {
    let mut best = f64::MAX;
    let mut pose = default_pose(nodes);
    for _ in 0..ROUNDS {
        let start = Instant::now();
        for i in 0..SAMPLES {
            // 时间扫过整段：每段、每帧边界都走到。
            let t = (i as f32 / SAMPLES as f32) * 0.99;
            sample_clip_into(black_box(clip), t, &mut pose);
        }
        let per_sample = start.elapsed().as_secs_f64() * 1e9 / SAMPLES as f64;
        best = best.min(per_sample);
    }
    // 防止整个循环被判成无用计算（写进 pose 有副作用，这里再兜一层）。
    black_box(&pose);
    best
}

#[test]
#[ignore = "基准：cargo test -p xgame-render --release --test animation_cost -- --ignored --nocapture"]
fn animation_sampling_cost() {
    let nodes = nodes();
    let linear = measure(&clip(Interpolation::Linear), &nodes);
    let step = measure(&clip(Interpolation::Step), &nodes);
    let cubic = measure(&clip(Interpolation::CubicSpline), &nodes);

    println!(
        "轨道 {} 条 / 每轨 {} 帧 ── 单次采样：线性 {:.0} ns · 阶梯 {:.0} ns · 样条 {:.0} ns（样条 = {:.2}× 线性）",
        BONES * 3,
        KEYS,
        linear,
        step,
        cubic,
        cubic / linear
    );
}
