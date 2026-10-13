//! 动画剪辑的数据结构、**纯函数采样**与播放时钟。
//!
//! # 分层
//!
//! - `Animator`（`xgame_core`）是**配置**（播什么、多快、循不循环），随场景落盘；
//! - 本模块是**运行时**：[`AnimClip`] 等类型住在渲染资产
//!   [`GltfMesh`](crate::gltf_loader::GltfMesh) 里（glTF 数据本身就是渲染资产），
//!   [`AnimationDriver`] 持有每个实体的播放时钟；
//! - 蒙皮矩阵（关节局部姿态 → 层级全局矩阵 × 逆绑定矩阵）由 [`compute_bones`]
//!   一次性算完，渲染管线直接拿去上传。
//!
//! 采样侧全部是**纯函数**（时间 → 矩阵），没有隐藏状态 —— 单测直接喂时间断言
//! 数值，不需要 GPU、不需要真实帧循环。
//!
//! # v1 范围
//!
//! 单剪辑播放 + 换剪辑时的**交叉淡入**（无状态机）、LINEAR/STEP 插值（CUBICSPLINE
//! 读入时降采样成线性）。旋转用 slerp，循环用取模、非循环钳在末帧。

use std::collections::HashMap;
use std::time::Instant;

use xgame_core::{Entity, EntityId};
use xgame_math::glam::{Mat4, Quat, Vec3};

// ---------------------------------------------------------------------------
// 数据类型（由 glTF 加载器填充，见 `gltf_loader::parse_animation_data`）
// ---------------------------------------------------------------------------

/// glTF 场景图里的一个节点（默认局部姿态 + 父引用）。
///
/// 骨骼层级完全活在**模型资产内部**，不映射成场景实体：一个 24 骨的角色
/// 一旦变成 24 个 `Entity`，层级面板会被灌满、每帧世界矩阵解算要多跑一遍、
/// 而用户根本不会去编辑单根骨头。骨架是美术资产，不是游戏设计对象。
#[derive(Clone, Debug)]
pub struct AnimNode {
    /// 父节点下标。`None` = 根。
    pub parent: Option<u32>,
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl AnimNode {
    pub fn local_matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

/// 一张蒙皮：关节（节点下标）+ 逆绑定矩阵。
///
/// `joints[k]` 与 `inverse_bind[k]` 一一对应，都按**蒙皮自己的顺序**排 ——
/// 顶点属性里的 `JOINTS_0` 索引指的就是这个顺序（不是节点下标）。
#[derive(Clone, Debug)]
pub struct Skin {
    pub joints: Vec<u32>,
    pub inverse_bind: Vec<Mat4>,
}

/// 关键帧轨道瞄准的变换通道。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackPath {
    Translation,
    Rotation,
    Scale,
}

impl TrackPath {
    /// 每个关键帧的分量数（位移/缩放 3，旋转 4）。
    fn stride(self) -> usize {
        match self {
            TrackPath::Translation | TrackPath::Scale => 3,
            TrackPath::Rotation => 4,
        }
    }
}

/// 关键帧插值方式。
///
/// glTF 的 `CUBICSPLINE` 在**读入时**降采样成 LINEAR（取输出数组的中间那份值，
/// 切线丢弃），运行时只保留两种形态 —— 分支越少，采样这个每帧必走的路径越稳。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    Linear,
    Step,
}

/// 一条关键帧轨道：`node` 节点的某个通道随时间变化。
#[derive(Clone, Debug)]
pub struct Track {
    /// 目标节点下标（[`AnimNode`] 数组内）。
    pub node: u32,
    pub path: TrackPath,
    pub interpolation: Interpolation,
    /// 严格递增的时间（秒）。
    pub times: Vec<f32>,
    /// 关键帧值，`stride()` 个分量连续排布：`times.len() * stride` 长。
    pub values: Vec<f32>,
}

/// 一段动画剪辑（glTF `animations[]`）。
#[derive(Clone, Debug)]
pub struct AnimClip {
    /// 剪辑名（glTF `name`，可能为空串）。
    pub name: String,
    /// 时长（秒）= 最后一个关键帧时间。
    pub duration: f32,
    pub tracks: Vec<Track>,
}

/// 一帧的**局部**姿态（每个节点一份）。
#[derive(Clone, Copy, Debug)]
pub struct LocalPose {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

// ---------------------------------------------------------------------------
// 采样（纯函数）
// ---------------------------------------------------------------------------

/// 由节点默认值铺出一份姿态。
pub fn default_pose(nodes: &[AnimNode]) -> Vec<LocalPose> {
    nodes
        .iter()
        .map(|n| LocalPose {
            translation: n.translation,
            rotation: n.rotation,
            scale: n.scale,
        })
        .collect()
}

/// 时间 → 关键帧段：返回 `(左下标, 右下标, 段内比例 u∈[0,1])`。
///
/// 段外一律**钳住**：`t` 早于首帧给 `(0,0,0)`，晚于末帧给 `(last,last,0)` ——
/// 采样结果就是首/末关键帧的值，由 [`clip_time`] 负责循环/钳位，这里不重复做。
fn key_segment(times: &[f32], t: f32) -> (usize, usize, f32) {
    if times.is_empty() {
        return (0, 0, 0.0);
    }
    if t <= times[0] {
        return (0, 0, 0.0);
    }
    let last = times.len() - 1;
    if t >= times[last] {
        return (last, last, 0.0);
    }
    // 第一个 `times[i] > t` 的位置就是右端；左端是它前一个。
    let i1 = times.partition_point(|&x| x <= t);
    let i0 = i1 - 1;
    let span = times[i1] - times[i0];
    let u = if span > 0.0 { (t - times[i0]) / span } else { 0.0 };
    (i0, i1, u)
}

/// 把 `clip` 在时刻 `t` 的采样结果**覆盖写进** `pose`（按轨道逐节点）。
///
/// `pose` 应由 [`default_pose`] 铺好：轨道没瞄到的节点保持默认姿态（绑定姿态）。
/// 每条轨道独立覆盖，同一节点的 translation/rotation 轨道互不干扰。
pub fn sample_clip_into(clip: &AnimClip, t: f32, pose: &mut [LocalPose]) {
    for track in &clip.tracks {
        let idx = track.node as usize;
        if idx >= pose.len() || track.times.is_empty() {
            continue;
        }
        let (i0, i1, mut u) = key_segment(&track.times, t);
        if track.interpolation == Interpolation::Step {
            u = 0.0;
        }
        let s = track.path.stride();
        // 越界读保护：坏资产少几个分量时宁可跳过这条轨道，也不要 panic。
        if (i1 + 1) * s > track.values.len() {
            continue;
        }
        match track.path {
            TrackPath::Translation => {
                let a = Vec3::from_slice(&track.values[i0 * s..i0 * s + 3]);
                let b = Vec3::from_slice(&track.values[i1 * s..i1 * s + 3]);
                pose[idx].translation = a.lerp(b, u);
            }
            TrackPath::Scale => {
                let a = Vec3::from_slice(&track.values[i0 * s..i0 * s + 3]);
                let b = Vec3::from_slice(&track.values[i1 * s..i1 * s + 3]);
                pose[idx].scale = a.lerp(b, u);
            }
            TrackPath::Rotation => {
                let a = Quat::from_slice(&track.values[i0 * s..i0 * s + 4]);
                let b = Quat::from_slice(&track.values[i1 * s..i1 * s + 4]);
                pose[idx].rotation = a.slerp(b, u);
            }
        }
    }
}

/// 局部姿态 → 各节点**全局**矩阵（沿父链连乘）。
///
/// 父下标不保证小于子下标（glTF 只保证是树），所以用带备忘的递归现算。
/// `depth > nodes.len()` 时按根处理：畸形资产（成环）时父链必然绕圈，
/// 宁可把它画错，也不要在加载/渲染路径上栈溢出。
fn global_of(
    i: usize,
    nodes: &[AnimNode],
    pose: &[LocalPose],
    memo: &mut [Option<Mat4>],
    depth: usize,
) -> Mat4 {
    if let Some(m) = memo[i] {
        return m;
    }
    let local = Mat4::from_scale_rotation_translation(
        pose[i].scale,
        pose[i].rotation,
        pose[i].translation,
    );
    let global = match nodes[i].parent {
        Some(p) if depth <= nodes.len() && (p as usize) < nodes.len() && p as usize != i => {
            global_of(p as usize, nodes, pose, memo, depth + 1) * local
        }
        _ => local,
    };
    memo[i] = Some(global);
    global
}

/// 局部姿态 → 各节点全局矩阵（与 [`AnimNode`] 同序）。
pub fn global_matrices(nodes: &[AnimNode], pose: &[LocalPose]) -> Vec<Mat4> {
    let mut memo = vec![None; nodes.len()];
    (0..nodes.len())
        .map(|i| global_of(i, nodes, pose, &mut memo, 0))
        .collect()
}

/// 采样一个剪辑并算出某张蒙皮的**蒙皮矩阵**（`globals[joint] × inverse_bind`）。
///
/// 顶点着色器拿它做 4 影响混合：`skinned = Σ wₖ · boneₖ · position`。
/// 骨架根节点的祖先（glTF 场景根）也走同一套连乘 —— 模型文件自带的世界原点
/// 偏移因此被正确吃进矩阵，而不是在运行时"看起来歪了"再打补丁。
pub fn compute_bones(nodes: &[AnimNode], clip: &AnimClip, skin: &Skin, t: f32) -> Vec<Mat4> {
    let mut pose = default_pose(nodes);
    sample_clip_into(clip, t, &mut pose);
    let globals = global_matrices(nodes, &pose);
    skin.joints
        .iter()
        .zip(skin.inverse_bind.iter())
        .map(|(&j, &ibm)| globals.get(j as usize).copied().unwrap_or(Mat4::IDENTITY) * ibm)
        .collect()
}

/// 两份剪辑在各自时刻采样后按 `alpha` 混合，再算蒙皮矩阵（交叉淡入用）。
///
/// 混合发生在**局部姿态**（平移 lerp、旋转 slerp、缩放 lerp）而不是全局矩阵上
/// —— 矩阵逐分量插值会把旋转混成剪切，模型在过渡中会"缩一下"。
/// `alpha = 0` 纯 `clip_a`，`alpha = 1` 纯 `clip_b`。
pub fn compute_bones_blended(
    nodes: &[AnimNode],
    clip_a: &AnimClip,
    t_a: f32,
    clip_b: &AnimClip,
    t_b: f32,
    alpha: f32,
    skin: &Skin,
) -> Vec<Mat4> {
    let alpha = alpha.clamp(0.0, 1.0);
    let mut pose_a = default_pose(nodes);
    sample_clip_into(clip_a, t_a, &mut pose_a);
    let mut pose_b = default_pose(nodes);
    sample_clip_into(clip_b, t_b, &mut pose_b);
    let pose: Vec<LocalPose> = pose_a
        .iter()
        .zip(&pose_b)
        .map(|(a, b)| LocalPose {
            translation: a.translation.lerp(b.translation, alpha),
            rotation: a.rotation.slerp(b.rotation, alpha),
            scale: a.scale.lerp(b.scale, alpha),
        })
        .collect();
    let globals = global_matrices(nodes, &pose);
    skin.joints
        .iter()
        .zip(skin.inverse_bind.iter())
        .map(|(&j, &ibm)| globals.get(j as usize).copied().unwrap_or(Mat4::IDENTITY) * ibm)
        .collect()
}

/// 播放时间归一：循环取模、非循环钳在末帧。
///
/// 非循环停在 `duration`（= 末关键帧）而不是 `duration - ε`：钳位后采样命中
/// 最后一个关键帧，画面停在末姿势；若取模回 0 会"播完瞬间弹回第一帧"。
pub fn clip_time(t: f32, duration: f32, looping: bool) -> f32 {
    if !t.is_finite() || duration <= 0.0 {
        return 0.0;
    }
    let t = t.max(0.0);
    if looping {
        t % duration
    } else {
        t.min(duration)
    }
}

// ---------------------------------------------------------------------------
// 播放时钟
// ---------------------------------------------------------------------------

/// 交叉淡入的**来源**剪辑（换剪辑时把旧剪辑连同它的时钟一起记下来）。
struct Prev {
    clip: String,
    t: f32,
    /// 已淡入的秒数。
    fade_t: f32,
    /// 当前混合权重 ∈ [0,1]：0 = 纯旧剪辑，1 = 淡入完成。tick 里算好存下，
    /// 渲染侧查询时不再需要 Animator 配置。
    alpha: f32,
}

struct Entry {
    clip: String,
    t: f32,
    /// `None` = 没有淡入（未切换 / 已完成 / 硬切）。
    prev: Option<Prev>,
}

/// 每个实体的动画播放时钟（clip + 已累计时间）。
///
/// 时钟放在**调用方**（编辑器的 `SceneRenderer`、游戏的 `GameApp`）而不是
/// `ScenePipeline`：管线是纯绘制，`render()` 每帧被调用与否还受指纹门控 ——
/// 时钟跟着门控走会出现"不出帧 → 时间不走 → 指纹不变 → 更不出帧"的死锁。
/// 由外层在**每帧 update** 里 tick，时间永远在推进；渲染只读时间。
#[derive(Default)]
pub struct AnimationDriver {
    entries: HashMap<EntityId, Entry>,
    /// `tick_wall` 用：上一次推进的时刻。
    last_tick: Option<Instant>,
}

impl AnimationDriver {
    pub fn new() -> Self {
        Self::default()
    }

    /// 推进时钟（调用方给 dt —— 游戏循环有自己的帧时钟）。
    ///
    /// - 只为**带 [`Animator`][xgame_core::Animator] 的实体**建条目；
    ///   `autoplay = false` 也建（时间停在 0），于是首帧能画出剪辑第 0 帧的姿势，
    ///   而不是退回绑定姿态 —— "摆好第一帧再播"是正常的制作流程。
    /// - 剪辑名变了 → 时间归零（换剪辑从头播）。淡入时长 > 0 且当时正在播放 →
    ///   记下旧剪辑（连同它的时钟）做交叉淡入，见 [`fade_from`](Self::fade_from)；
    ///   暂停中切换 / `fade_time = 0` → 硬切。
    /// - 实体被删 / 动画组件被摘掉 → 条目清理，防泄漏。
    ///
    /// 淡入中途再次换剪辑：旧的淡入被丢弃，从**当前主剪辑**重新淡入新剪辑
    /// （v1 取舍：0.25 秒内连切两刀本来就是编辑期误操作居多，不值得为此
    /// 维护多层混合栈；代价是双切瞬间有一帧小跳变）。
    pub fn tick(&mut self, dt: f32, entities: &[Entity]) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        for entity in entities {
            let Some(animator) = &entity.animator else {
                continue;
            };
            let entry = self
                .entries
                .entry(entity.id)
                .or_insert_with(|| Entry {
                    clip: animator.clip.clone(),
                    t: 0.0,
                    prev: None,
                });
            if entry.clip != animator.clip {
                entry.prev = if animator.fade_time > 0.0 && animator.is_active() {
                    Some(Prev {
                        clip: entry.clip.clone(),
                        t: entry.t,
                        fade_t: 0.0,
                        alpha: 0.0,
                    })
                } else {
                    None
                };
                entry.clip = animator.clip.clone();
                entry.t = 0.0;
            }
            if animator.is_active() {
                entry.t += dt * animator.speed;
                let mut fade_done = false;
                if let Some(prev) = entry.prev.as_mut() {
                    // 旧剪辑的时钟也继续走 —— 淡出的那半边若定格，混合出来的
                    // 姿势会在过渡中"粘住"一下，看起来像卡帧。
                    prev.t += dt * animator.speed;
                    prev.fade_t += dt;
                    prev.alpha = if animator.fade_time > 0.0 {
                        (prev.fade_t / animator.fade_time).min(1.0)
                    } else {
                        1.0
                    };
                    fade_done = animator.fade_time <= 0.0 || prev.fade_t >= animator.fade_time;
                }
                if fade_done {
                    entry.prev = None;
                }
            }
        }
        let live: std::collections::HashSet<EntityId> = entities
            .iter()
            .filter(|e| e.animator.is_some())
            .map(|e| e.id)
            .collect();
        self.entries.retain(|id, _| live.contains(id));
    }

    /// 用**墙钟**推进（编辑器编辑模式没有自己的帧时钟，用这个）。
    ///
    /// 首次调用只记时刻不推进；之后 `dt` 钳在 100ms 内 —— 断点/挂起回来的
    /// 那一帧若按真实间隔走，动画会瞬移一大段。
    pub fn tick_wall(&mut self, entities: &[Entity]) {
        let now = Instant::now();
        let dt = self
            .last_tick
            .map(|prev| now.saturating_duration_since(prev).as_secs_f32())
            .unwrap_or(0.0);
        self.last_tick = Some(now);
        self.tick(dt.min(0.1), entities);
    }

    /// 某实体当前的播放时间（未被 tick 过的实体没有条目）。
    pub fn time(&self, id: EntityId) -> Option<f32> {
        self.entries.get(&id).map(|e| e.t)
    }

    /// 某实体正在进行的交叉淡入：`(旧剪辑名, 旧剪辑时间, 混合权重 alpha)`。
    ///
    /// `alpha = 0` 还是纯旧剪辑，`1` 淡入完成（此时条目已被清掉，返回 `None`）。
    /// 渲染侧拿它调 [`compute_bones_blended`]：旧剪辑在旧时钟、新剪辑用
    /// [`time`](Self::time) 的时钟。
    pub fn fade_from(&self, id: EntityId) -> Option<(&str, f32, f32)> {
        self.entries
            .get(&id)?
            .prev
            .as_ref()
            .map(|p| (p.clip.as_str(), p.t, p.alpha))
    }

    /// 当前所有时钟的**内容指纹**：进编辑器的场景指纹用。
    ///
    /// 有动画在播 → 每帧指纹变 → 每帧真渲染（否则指纹门控会把动画冻结）；
    /// 没有时钟 → 恒为 0，静态场景照旧"没动就不画"。按 ID 排序后混合，
    /// 与 `HashMap` 的遍历顺序无关 —— 同样的内容必须给同样的指纹。
    pub fn signature(&self) -> u64 {
        let mut pairs: Vec<(u64, u32)> = self
            .entries
            .iter()
            .map(|(id, e)| (id.raw(), e.t.to_bits()))
            .collect();
        pairs.sort_unstable();
        let mut h = 0u64;
        for (id, bits) in pairs {
            h = mix(h, id);
            h = mix(h, bits as u64);
        }
        h
    }

    /// 有没有任何时钟条目。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 清空所有时钟（换场景 / 重启播放时用）。
    pub fn clear(&mut self) {
        self.entries.clear();
        self.last_tick = None;
    }
}

/// splitmix64 收尾混合（与编辑器 `mix_bits` 同款：只要稳定 + 雪崩，不求抗碰撞）。
fn mix(hash: u64, value: u64) -> u64 {
    let mut x = hash ^ value.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    x
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use xgame_core::Animator;

    /// 一条平移轨道：0→1 秒从 (0,0,0) 走到 (2,0,0)，挂在节点 0。
    fn move_x_clip() -> AnimClip {
        AnimClip {
            name: "Walk".into(),
            duration: 1.0,
            tracks: vec![Track {
                node: 0,
                path: TrackPath::Translation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![0.0, 0.0, 0.0, 2.0, 0.0, 0.0],
            }],
        }
    }

    /// 0→1 秒从 (0,0,0) 走到 (0,4,0)，挂在节点 0 —— 与 [`move_x_clip`] 不同轴，
    /// 混合结果的两个分量能分别断言。
    fn move_y_clip() -> AnimClip {
        AnimClip {
            name: "Run".into(),
            duration: 1.0,
            tracks: vec![Track {
                node: 0,
                path: TrackPath::Translation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![0.0, 0.0, 0.0, 0.0, 4.0, 0.0],
            }],
        }
    }

    fn root_node() -> AnimNode {
        AnimNode {
            parent: None,
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }

    // -- key_segment / 插值 --

    #[test]
    fn sampling_clamps_before_first_and_after_last_key() {
        let times = [0.5, 1.0];
        assert_eq!(key_segment(&times, 0.0), (0, 0, 0.0), "早于首帧钳在首帧");
        assert_eq!(key_segment(&times, 9.0), (1, 1, 0.0), "晚于末帧钳在末帧");
    }

    #[test]
    fn linear_interpolation_hits_the_midpoint() {
        let clip = move_x_clip();
        let mut pose = vec![LocalPose {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }];
        sample_clip_into(&clip, 0.5, &mut pose);
        assert!(
            (pose[0].translation.x - 1.0).abs() < 1e-6,
            "0→2 的中点应当是 1，实得 {}",
            pose[0].translation.x
        );
    }

    #[test]
    fn step_interpolation_holds_the_previous_key() {
        let clip = AnimClip {
            name: "S".into(),
            duration: 1.0,
            tracks: vec![Track {
                node: 0,
                path: TrackPath::Translation,
                interpolation: Interpolation::Step,
                times: vec![0.0, 1.0],
                values: vec![0.0, 0.0, 0.0, 2.0, 0.0, 0.0],
            }],
        };
        let mut pose = default_pose(&[root_node()]);
        sample_clip_into(&clip, 0.99, &mut pose);
        assert!(
            pose[0].translation.x.abs() < 1e-6,
            "STEP 插值在段内应当保持前一个关键帧，实得 {}",
            pose[0].translation.x
        );
    }

    #[test]
    fn rotation_slerps_between_keys() {
        let clip = AnimClip {
            name: "R".into(),
            duration: 1.0,
            tracks: vec![Track {
                node: 0,
                path: TrackPath::Rotation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: {
                    let mut v = Quat::IDENTITY.to_array().to_vec();
                    v.extend(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2).to_array());
                    v
                },
            }],
        };
        let mut pose = default_pose(&[root_node()]);
        sample_clip_into(&clip, 0.5, &mut pose);
        let angle = pose[0].rotation.angle_between(Quat::IDENTITY);
        // 中点应是绕 Y 转 45° = π/4。
        assert!(
            (angle - std::f32::consts::FRAC_PI_4).abs() < 1e-3,
            "旋转中点应当是 45°，实得 {angle} rad"
        );
        // 方向也要对：绕 +Y 的右手旋转把 +X 转向 −Z（右手法则）。
        let rotated = pose[0].rotation * Vec3::X;
        assert!(rotated.z < 0.0, "绕 +Y 转 45° 后 +X 应有 −z 分量，实得 {rotated:?}");
    }

    #[test]
    fn a_track_only_overwrites_its_own_channel() {
        // 节点 0 有平移轨道；默认旋转/缩放必须原样保留。
        let clip = move_x_clip();
        let nodes = vec![AnimNode {
            parent: None,
            translation: Vec3::new(9.0, 9.0, 9.0),
            rotation: Quat::from_rotation_z(0.3),
            scale: Vec3::splat(2.0),
        }];
        let mut pose = default_pose(&nodes);
        sample_clip_into(&clip, 1.0, &mut pose);
        assert!((pose[0].translation.x - 2.0).abs() < 1e-6, "平移轨道应生效");
        assert!(
            pose[0].rotation.angle_between(Quat::from_rotation_z(0.3)) < 1e-5,
            "没被瞄准的旋转通道必须保持默认"
        );
        assert!((pose[0].scale.x - 2.0).abs() < 1e-6, "没被瞄准的缩放通道必须保持默认");
    }

    #[test]
    fn malformed_track_is_skipped_not_panicked() {
        let clip = AnimClip {
            name: "bad".into(),
            duration: 1.0,
            tracks: vec![Track {
                node: 0,
                path: TrackPath::Rotation, // 需要 4 分量，却只给 2 个
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![0.0, 0.0],
            }],
        };
        let mut pose = default_pose(&[root_node()]);
        sample_clip_into(&clip, 0.5, &mut pose); // 不 panic 即通过
    }

    // -- 层级 --

    #[test]
    fn globals_compose_parent_and_child() {
        let nodes = vec![
            AnimNode {
                parent: None,
                translation: Vec3::new(1.0, 0.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            AnimNode {
                parent: Some(0),
                translation: Vec3::new(0.0, 2.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        ];
        let pose = default_pose(&nodes);
        let g = global_matrices(&nodes, &pose);
        let child_pos = g[1].w_axis.truncate();
        assert!(
            (child_pos - Vec3::new(1.0, 2.0, 0.0)).length() < 1e-6,
            "子节点全局位置应是父位移+子位移，实得 {child_pos:?}"
        );
    }

    /// 父下标大于子下标（glTF 不保证有序）也必须正确 —— 递归备忘不依赖顺序。
    #[test]
    fn globals_handle_parent_with_higher_index() {
        let nodes = vec![
            AnimNode {
                parent: Some(1), // 子在前、父在后
                translation: Vec3::new(0.0, 5.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            AnimNode {
                parent: None,
                translation: Vec3::new(3.0, 0.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        ];
        let pose = default_pose(&nodes);
        let g = global_matrices(&nodes, &pose);
        let pos = g[0].w_axis.truncate();
        assert!((pos - Vec3::new(3.0, 5.0, 0.0)).length() < 1e-6, "实得 {pos:?}");
    }

    /// 成环的畸形资产不能栈溢出（超深按根处理）。
    #[test]
    fn cyclic_parents_degrade_instead_of_overflowing() {
        let nodes = vec![
            AnimNode { parent: Some(1), translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE },
            AnimNode { parent: Some(0), translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE },
        ];
        let pose = default_pose(&nodes);
        let g = global_matrices(&nodes, &pose);
        assert!(g.iter().all(|m| m.w_axis.is_finite()), "成环也必须给出有限矩阵");
    }

    // -- 蒙皮矩阵 --

    /// 绑定姿态下蒙皮矩阵必须是**单位阵**：IBM 本就是绑定全局矩阵的逆，
    /// `bind_global × bind_global⁻¹ = I`。这是"不播动画时模型不散架"的根基。
    #[test]
    fn bind_pose_yields_identity_bones() {
        let nodes = vec![
            AnimNode {
                parent: None,
                translation: Vec3::new(5.0, 1.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            AnimNode {
                parent: Some(0),
                translation: Vec3::new(0.0, 2.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        ];
        let pose = default_pose(&nodes);
        let g = global_matrices(&nodes, &pose);
        let skin = Skin {
            joints: vec![1],
            inverse_bind: vec![g[1].inverse()],
        };
        let bones = compute_bones(&nodes, &move_x_clip(), &skin, 0.0);
        assert_eq!(bones.len(), 1);
        // glam 的 `to_cols_array()` 是平铺的 [f32;16]（列主序）：[0] = 第0列第0行。
        let dev = (bones[0] - Mat4::IDENTITY).to_cols_array()[0].abs();
        assert!(dev < 1e-5, "绑定姿态的骨矩阵应当是单位阵，偏差 {dev}");
    }

    /// 交叉淡入的混合在**局部姿态**上做：两段位移的中点必须是逐分量的中点，
    /// 且 alpha 端点要退回纯剪辑（钳位保护坏数据）。
    #[test]
    fn blended_bones_mix_poses_and_clamp_alpha() {
        let nodes = vec![root_node()];
        let skin = Skin {
            joints: vec![0],
            inverse_bind: vec![Mat4::IDENTITY],
        };
        let a = move_x_clip(); // t=1 → (2,0,0)
        let b = move_y_clip(); // t=1 → (0,4,0)
        let at = |alpha: f32| compute_bones_blended(&nodes, &a, 1.0, &b, 1.0, alpha, &skin)[0]
            .w_axis
            .truncate();

        let mid = at(0.5);
        assert!(
            (mid - Vec3::new(1.0, 2.0, 0.0)).length() < 1e-5,
            "alpha=0.5 应是两姿势的中点，实得 {mid:?}"
        );
        let only_a = at(0.0);
        assert!((only_a - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-5, "alpha=0 应是纯旧剪辑");
        let only_b = at(1.0);
        assert!((only_b - Vec3::new(0.0, 4.0, 0.0)).length() < 1e-5, "alpha=1 应是纯新剪辑");
        let clamped = at(9.0);
        assert!((clamped - only_b).length() < 1e-5, "alpha 越界应钳在纯新剪辑");
    }

    // -- 时间归一 --

    #[test]
    fn clip_time_loops_wraps_and_oneshot_clamps() {
        assert!((clip_time(1.5, 1.0, true) - 0.5).abs() < 1e-6, "循环应取模");
        assert!((clip_time(9.0, 1.0, true) - 0.0).abs() < 1e-6, "整圈应绕回 0");
        assert!((clip_time(9.0, 1.0, false) - 1.0).abs() < 1e-6, "非循环应钳在末帧");
        assert_eq!(clip_time(f32::NAN, 1.0, true), 0.0, "NaN 不该扩散进采样");
        assert_eq!(clip_time(-1.0, 1.0, false), 0.0, "负时间钳在 0");
        assert_eq!(clip_time(0.5, 0.0, true), 0.0, "零时长剪辑恒为 0（不产生 NaN）");
    }

    // -- 播放时钟 --

    fn entity_with_animator(clip: &str) -> Entity {
        Entity::new("A").with_animator(Animator::new(clip))
    }

    #[test]
    fn driver_advances_active_clocks_and_restarts_on_clip_change() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_animator("Walk");
        let id = e.id;

        d.tick(0.5, std::slice::from_ref(&e));
        assert!((d.time(id).unwrap() - 0.5).abs() < 1e-6, "应当按 dt 推进");

        d.tick(0.5, std::slice::from_ref(&e));
        assert!((d.time(id).unwrap() - 1.0).abs() < 1e-6, "应当累加");

        // 换剪辑 → 从头播。
        e.animator.as_mut().unwrap().clip = "Run".into();
        d.tick(0.25, std::slice::from_ref(&e));
        assert!((d.time(id).unwrap() - 0.25).abs() < 1e-6, "换剪辑应归零重走");
    }

    #[test]
    fn driver_freezes_paused_clocks_but_keeps_the_entry() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_animator("Walk");
        d.tick(0.5, std::slice::from_ref(&e));

        e.animator.as_mut().unwrap().autoplay = false;
        d.tick(5.0, std::slice::from_ref(&e));
        assert!(
            (d.time(e.id).unwrap() - 0.5).abs() < 1e-6,
            "暂停后时间不该继续走"
        );
        assert!(d.time(e.id).is_some(), "条目要留着 —— 画面冻结在当前帧而不是弹回绑定姿态");
    }

    #[test]
    fn driver_prunes_removed_entities() {
        let mut d = AnimationDriver::new();
        let e = entity_with_animator("Walk");
        d.tick(0.1, std::slice::from_ref(&e));
        assert!(!d.is_empty());
        d.tick(0.1, &[]); // 实体没了
        assert!(d.is_empty(), "摘掉动画组件/删除实体后条目必须清理");
    }

    // -- 交叉淡入 --

    #[test]
    fn driver_fades_from_old_clip_on_switch_and_completes() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_animator("Walk");
        let id = e.id;
        d.tick(0.4, std::slice::from_ref(&e));
        assert!(d.fade_from(id).is_none(), "没切换过就不该有淡入");

        e.animator.as_mut().unwrap().clip = "Run".into(); // 默认 fade_time = 0.25s
        d.tick(0.1, std::slice::from_ref(&e));
        let (clip, prev_t, alpha) = d.fade_from(id).expect("换剪辑后应处于淡入中");
        assert_eq!(clip, "Walk", "应从旧剪辑淡出");
        assert!(
            (prev_t - 0.5).abs() < 1e-6,
            "旧剪辑时钟应继续走（0.4+0.1），实得 {prev_t}"
        );
        assert!((alpha - 0.4).abs() < 1e-6, "alpha 应是 0.1/0.25，实得 {alpha}");
        assert!((d.time(id).unwrap() - 0.1).abs() < 1e-6, "新剪辑从 0 起播");

        d.tick(0.2, std::slice::from_ref(&e)); // 累计 0.3 ≥ 0.25
        assert!(d.fade_from(id).is_none(), "淡入完成后应清掉旧剪辑");
        assert!((d.time(id).unwrap() - 0.3).abs() < 1e-6, "主时钟不受淡入影响");
    }

    #[test]
    fn driver_hard_switches_when_fade_time_is_zero() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_animator("Walk");
        e.animator.as_mut().unwrap().fade_time = 0.0;
        d.tick(0.4, std::slice::from_ref(&e));
        e.animator.as_mut().unwrap().clip = "Run".into();
        d.tick(0.1, std::slice::from_ref(&e));
        assert!(d.fade_from(e.id).is_none(), "fade_time = 0 应硬切");
        assert!((d.time(e.id).unwrap() - 0.1).abs() < 1e-6, "硬切后新剪辑从 0 起播");
    }

    #[test]
    fn driver_does_not_fade_when_switching_while_paused() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_animator("Walk");
        d.tick(0.4, std::slice::from_ref(&e));

        e.animator.as_mut().unwrap().autoplay = false;
        e.animator.as_mut().unwrap().clip = "Run".into();
        d.tick(0.1, std::slice::from_ref(&e));
        assert!(
            d.fade_from(e.id).is_none(),
            "暂停中切换不该淡入（没有'正在播的姿势'可淡出）"
        );
        assert!((d.time(e.id).unwrap()).abs() < 1e-6, "切换照旧把新剪辑时钟归零");
    }

    #[test]
    fn driver_freezes_the_fade_while_paused() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_animator("Walk");
        d.tick(0.4, std::slice::from_ref(&e));
        e.animator.as_mut().unwrap().clip = "Run".into();
        d.tick(0.1, std::slice::from_ref(&e));
        let (_, prev_t, alpha) = d.fade_from(e.id).unwrap();

        e.animator.as_mut().unwrap().autoplay = false;
        d.tick(5.0, std::slice::from_ref(&e));
        let (_, prev_t2, alpha2) = d.fade_from(e.id).expect("暂停不该把淡入清掉");
        assert!((prev_t2 - prev_t).abs() < 1e-6, "暂停中旧剪辑时钟该冻结");
        assert!((alpha2 - alpha).abs() < 1e-6, "暂停中淡入进度该冻结");
        assert!((d.time(e.id).unwrap() - 0.1).abs() < 1e-6, "暂停中主时钟也该冻结");
    }

    /// 淡入中途连切两刀（0.25s 内 Walk→Run→Survey）：丢弃旧淡入，从当前主剪辑
    /// 重新开始 —— 不维护多层混合栈，见 `tick` 的文档。
    #[test]
    fn driver_double_switch_restarts_fade_from_current_clip() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_animator("Walk");
        d.tick(0.4, std::slice::from_ref(&e));
        e.animator.as_mut().unwrap().clip = "Run".into();
        d.tick(0.1, std::slice::from_ref(&e));

        e.animator.as_mut().unwrap().clip = "Survey".into();
        d.tick(0.05, std::slice::from_ref(&e));
        let (clip, _, alpha) = d.fade_from(e.id).expect("双切后仍应处于淡入中");
        assert_eq!(clip, "Run", "第二次切换应从当前主剪辑（Run）淡出");
        assert!((alpha - 0.2).abs() < 1e-6, "淡入进度应重新起算（0.05/0.25），实得 {alpha}");
    }

    #[test]
    fn driver_signature_tracks_time_and_goes_zero_when_idle() {
        let mut d = AnimationDriver::new();
        assert_eq!(d.signature(), 0, "没有时钟时指纹应恒为 0（静态帧不重画）");

        let e = entity_with_animator("Walk");
        d.tick(0.1, std::slice::from_ref(&e));
        let s1 = d.signature();
        d.tick(0.1, std::slice::from_ref(&e));
        let s2 = d.signature();
        assert_ne!(s1, s2, "时间在走 → 指纹必须变（否则门控会冻结动画）");
        assert_ne!(s1, 0);

        // 冻结后指纹必须**稳定**（暂停不该引发逐帧重画）。
        let mut paused = entity_with_animator("Walk");
        paused.animator.as_mut().unwrap().autoplay = false;
        let mut d2 = AnimationDriver::new();
        d2.tick(0.1, std::slice::from_ref(&paused));
        let a = d2.signature();
        d2.tick(0.1, std::slice::from_ref(&paused));
        assert_eq!(a, d2.signature(), "暂停中的时钟不该让指纹抖动");
    }
}
