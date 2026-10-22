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
//! 单剪辑播放 + 换剪辑时的**交叉淡入** + 声明式**动画状态机**（参数阈值驱动的
//! 剪辑切换，见 `xgame_core::AnimMachine`）+ **叠加（additive）混合**（增量 =
//! 叠加剪辑当前帧 − 其第 0 帧参考姿势，见 `compute_bones_additive`）+ **混合层栈**
//! （多层 Additive/Override × 骨骼遮罩，见 [`compute_bones_composed`]），LINEAR/STEP
//! 插值（CUBICSPLINE 读入时降采样成线性）。旋转用 slerp，循环用取模、非循环钳在末帧。

use std::collections::HashMap;
use std::time::Instant;

use xgame_core::{AnimLayerMode, Entity, EntityId};
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
    /// 节点名（glTF `name`，可能为空串）。骨骼遮罩（`AnimLayer::mask`）按名字
    /// 匹配子树根 —— 匿名节点只能靠有名字的祖先进名单。
    pub name: String,
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

/// 局部姿态 → 蒙皮矩阵（`globals[joint] × inverse_bind`），四条 compute_bones 共用的尾巴。
fn bones_from_pose(nodes: &[AnimNode], pose: &[LocalPose], skin: &Skin) -> Vec<Mat4> {
    let globals = global_matrices(nodes, pose);
    skin.joints
        .iter()
        .zip(skin.inverse_bind.iter())
        .map(|(&j, &ibm)| globals.get(j as usize).copied().unwrap_or(Mat4::IDENTITY) * ibm)
        .collect()
}

/// 把 `pose_b` 按 `alpha` 混进 `pose_a`（就地，交叉淡入共用）。
fn blend_into(pose_a: &mut [LocalPose], pose_b: &[LocalPose], alpha: f32) {
    for (a, b) in pose_a.iter_mut().zip(pose_b) {
        a.translation = a.translation.lerp(b.translation, alpha);
        a.rotation = a.rotation.slerp(b.rotation, alpha);
        a.scale = a.scale.lerp(b.scale, alpha);
    }
}

/// 参与 [`compute_bones_composed`] 的一层 —— 名字已在调用方解析成剪辑引用的
/// 运行时输入（管线拿 `mesh.clip()` 的结果填进来）。
pub struct LayerInput<'a> {
    /// 这层播的剪辑（不在模型里的层调用方直接不放进栈）。
    pub clip: &'a AnimClip,
    /// 这层的当前时间（已按循环 / 钳位归一）。
    pub t: f32,
    /// 权重 ∈ [0,1]（越界引擎侧钳位）。
    pub weight: f32,
    /// 模式：`Additive` = 加增量（减第 0 帧参考），`Override` = 向这层姿势 lerp。
    pub mode: AnimLayerMode,
    /// 骨骼遮罩子树根名单；空 = 整副骨架。
    pub mask: &'a [String],
}

/// 节点 `i` 是否在遮罩范围内：自身或**任一祖先**的名字命中名单。
///
/// 空名单 = 全覆盖（没配遮罩不设卡）。glTF 匿名节点（空名）不参与匹配 —— 只能靠
/// 有名字的祖先进名单。环护栏与 `global_matrices` 同款：畸形成环的资产最多沿父链
/// 走一遍节点数就停，不死循环。
fn mask_covers(nodes: &[AnimNode], i: usize, mask: &[String]) -> bool {
    if mask.is_empty() {
        return true;
    }
    let mut cur = i;
    for _ in 0..=nodes.len() {
        if cur >= nodes.len() {
            break;
        }
        let node = &nodes[cur];
        if !node.name.is_empty() && mask.iter().any(|m| m == &node.name) {
            return true;
        }
        match node.parent {
            Some(p) => cur = p as usize,
            None => break,
        }
    }
    false
}

/// 单节点 Additive：`b += w × (a − r)`；旋转增量在**该节点局部空间**按权重向单位
/// 四元数 slerp 收 —— w=0 恒为原姿势、w=1 全额、中间值走最短路径（对四元数分量
/// 直接 lerp 会走反向长弧）。
fn blend_additive(b: &mut LocalPose, a: &LocalPose, r: &LocalPose, w: f32) {
    b.translation += (a.translation - r.translation) * w;
    b.scale += (a.scale - r.scale) * w;
    let delta = r.rotation.inverse() * a.rotation;
    b.rotation = b.rotation.slerp(b.rotation * delta, w);
}

/// 单节点 Override：向该层姿势 lerp / slerp —— w=1 完全接管、w=0 纹丝不动。
fn blend_override(b: &mut LocalPose, layer: &LocalPose, w: f32) {
    b.translation = b.translation.lerp(layer.translation, w);
    b.rotation = b.rotation.slerp(layer.rotation, w);
    b.scale = b.scale.lerp(layer.scale, w);
}

/// 一步到位：基础层（可带交叉淡入）+ 层栈（Additive/Override × 骨骼遮罩）→ 蒙皮矩阵。
///
/// 求值次序：基础姿态（单剪辑；淡入中 = 旧剪辑按 `alpha` 混向新剪辑）→ 按数组
/// 顺序逐层合到"当前已合成的姿势"上（后层盖前层），每层只作用于**遮罩命中**的
/// 节点。本族其余 `compute_bones*` 全是它的薄封装 —— 管线只走这一条路，老测试
/// 原样当回归网。
///
/// 零权重 / 空剪辑的层直接跳过：没配层的实体与没这功能时**逐位一致**，不付采样代价。
pub fn compute_bones_composed(
    nodes: &[AnimNode],
    skin: &Skin,
    base: (&AnimClip, f32),
    fade: Option<(&AnimClip, f32, f32)>,
    layers: &[LayerInput<'_>],
) -> Vec<Mat4> {
    let (base_clip, base_t) = base;
    let mut pose = default_pose(nodes);
    match fade {
        // 淡入先采旧剪辑、再按 alpha 混向新剪辑 —— 与历史 `compute_bones_blended`
        // 同一拍的采样/混合顺序，逐位一致（顺序换了会引入不同的浮点舍入）。
        Some((old_clip, old_t, alpha)) => {
            let alpha = alpha.clamp(0.0, 1.0);
            sample_clip_into(old_clip, old_t, &mut pose);
            let mut new_pose = default_pose(nodes);
            sample_clip_into(base_clip, base_t, &mut new_pose);
            blend_into(&mut pose, &new_pose, alpha);
        }
        None => sample_clip_into(base_clip, base_t, &mut pose),
    }
    for layer in layers {
        let w = layer.weight.clamp(0.0, 1.0);
        if w <= 0.0 || layer.clip.duration <= 0.0 {
            continue;
        }
        match layer.mode {
            AnimLayerMode::Additive => {
                let mut add_pose = default_pose(nodes);
                sample_clip_into(layer.clip, layer.t, &mut add_pose);
                // 参考姿势 = 本层剪辑第 0 帧：没轨道的节点两份都停在绑定姿态 →
                // 增量恒 0，叠加层"没动的骨头"不干扰基础层。
                let mut ref_pose = default_pose(nodes);
                sample_clip_into(layer.clip, 0.0, &mut ref_pose);
                for i in 0..nodes.len() {
                    if mask_covers(nodes, i, layer.mask) {
                        blend_additive(&mut pose[i], &add_pose[i], &ref_pose[i], w);
                    }
                }
            }
            AnimLayerMode::Override => {
                let mut layer_pose = default_pose(nodes);
                sample_clip_into(layer.clip, layer.t, &mut layer_pose);
                for i in 0..nodes.len() {
                    if mask_covers(nodes, i, layer.mask) {
                        blend_override(&mut pose[i], &layer_pose[i], w);
                    }
                }
            }
        }
    }
    bones_from_pose(nodes, &pose, skin)
}

/// 采样一个剪辑并算出某张蒙皮的**蒙皮矩阵**（`globals[joint] × inverse_bind`）。
///
/// 顶点着色器拿它做 4 影响混合：`skinned = Σ wₖ · boneₖ · position`。
/// 骨架根节点的祖先（glTF 场景根）也走同一套连乘 —— 模型文件自带的世界原点
/// 偏移因此被正确吃进矩阵，而不是在运行时"看起来歪了"再打补丁。
pub fn compute_bones(nodes: &[AnimNode], clip: &AnimClip, skin: &Skin, t: f32) -> Vec<Mat4> {
    compute_bones_composed(nodes, skin, (clip, t), None, &[])
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
    compute_bones_composed(
        nodes,
        skin,
        (clip_b, t_b),
        Some((clip_a, t_a, alpha)),
        &[],
    )
}

/// 把叠加层增量按 `weight` 加进基础**局部姿态**（additive blending 的核心一步）。
///
/// 增量 = `add_pose − ref_pose`（ref 是这层的参考姿势/"零增量"帧 —— 约定为
/// 叠加剪辑自己的**第 0 帧**，由调用方采样后传进来）：
///
/// - 平移 / 缩放：线性差 —— `base + w × (add − ref)`；
/// - 旋转：局部空间增量 `ref⁻¹ · add`，按权重向单位四元数 slerp 收 ——
///   `base · slerp(I, ref⁻¹·add, w)`。w=0 恒为原姿势、w=1 全额增量，
///   中间值是一条**最短路径**的插值（直接对四元数分量做 lerp 会走反向长弧）。
///
/// 纯函数，不猜时间/循环语义 —— 两份叠加姿态由调用方采样好。
pub fn apply_additive_into(
    base: &mut [LocalPose],
    add_pose: &[LocalPose],
    ref_pose: &[LocalPose],
    weight: f32,
) {
    let w = weight.clamp(0.0, 1.0);
    for (b, (a, r)) in base.iter_mut().zip(add_pose.iter().zip(ref_pose)) {
        blend_additive(b, a, r, w);
    }
}

/// 基础剪辑 + 叠加层 → 蒙皮矩阵（叠加混合的一步到位封装 = [`compute_bones_composed`]
/// 的"单层 Additive、无遮罩"退化；零权重与纯 `compute_bones` 逐位一致）。
pub fn compute_bones_additive(
    nodes: &[AnimNode],
    clip: &AnimClip,
    t: f32,
    skin: &Skin,
    add_clip: &AnimClip,
    add_t: f32,
    weight: f32,
) -> Vec<Mat4> {
    compute_bones_composed(
        nodes,
        skin,
        (clip, t),
        None,
        &[LayerInput {
            clip: add_clip,
            t: add_t,
            weight,
            mode: AnimLayerMode::Additive,
            mask: &[],
        }],
    )
}

/// 交叉淡入中 + 叠加层：先按 `alpha` 混合基础层，再把叠加增量加到**混合后**的
/// 姿态上（叠加层与基础层的切换正交 —— 基础层淡入淡出时它照叠不误）。
///
/// 参数多但每个都直呼其名（历史签名；管线已改走 [`compute_bones_composed`]，
/// 这里保留给直调方与回归测试）—— 与 `ScenePipeline::render` 同样的取舍。
#[allow(clippy::too_many_arguments)]
pub fn compute_bones_blended_additive(
    nodes: &[AnimNode],
    clip_a: &AnimClip,
    t_a: f32,
    clip_b: &AnimClip,
    t_b: f32,
    alpha: f32,
    skin: &Skin,
    add_clip: &AnimClip,
    add_t: f32,
    weight: f32,
) -> Vec<Mat4> {
    compute_bones_composed(
        nodes,
        skin,
        (clip_b, t_b),
        Some((clip_a, t_a, alpha)),
        &[LayerInput {
            clip: add_clip,
            t: add_t,
            weight,
            mode: AnimLayerMode::Additive,
            mask: &[],
        }],
    )
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

/// 叠加层的解析结果，给渲染侧采样用（[`AnimationDriver::additive`]）。
///
/// 渲染侧拿 `clip` 找到剪辑、按 `t` / `looping` 采样，减去它**第 0 帧**（参考
/// 姿势）得到增量，再按 `weight` 加到基础局部姿态上（见 [`compute_bones_additive`]）。
pub struct Additive<'a> {
    /// 叠加层剪辑名（渲染侧拿去 `mesh.clip()`）。
    pub clip: &'a str,
    /// 叠加层的当前时间。
    pub t: f32,
    /// 叠加层的循环方式。
    pub looping: bool,
    /// 权重 ∈ [0,1]（tick 里已钳位）。
    pub weight: f32,
}

/// 层栈某一层的解析时钟（[`AnimationDriver::layers`] 按**配置顺序**返回切片）。
///
/// 与配置**按 name 对齐**同步：Inspector 重排层各保各的进度、删层即 prune、
/// 换本层剪辑只归零它自己的时钟（硬切，与叠加层同语义）。`mode` / `mask` /
/// `weight` 都由 tick 跟配置对齐（越界权重钳位），渲染侧只问驱动、不回头读组件。
#[derive(Clone, Debug)]
pub struct LayerClock {
    /// 层名（脚本 `set_anim_layer_weight` 按名查找的就是它）。
    pub name: String,
    /// 本层剪辑名（空 = 未解析，渲染侧跳过这层）。
    pub clip: String,
    /// 本层当前时间。
    pub t: f32,
    /// 本层播放速度。
    pub speed: f32,
    /// 本层循环方式。
    pub looping: bool,
    /// 权重 ∈ [0,1]（tick 里已钳位）。
    pub weight: f32,
    /// 求值模式（Additive / Override）。
    pub mode: AnimLayerMode,
    /// 骨骼遮罩子树根名单（空 = 整副骨架）。
    pub mask: Vec<String>,
}

/// 进行中的交叉淡入，给渲染侧采样用（[`AnimationDriver::fade_from`]）。
///
/// 旧剪辑按**它自己**的时钟与循环方式采样 —— Walk→Run 淡出的那半边仍是 Walk
/// 的步态，不会被 Run 的循环方式带偏。
pub struct FadeFrom<'a> {
    /// 旧剪辑名（渲染侧拿去 `mesh.clip()`）。
    pub clip: &'a str,
    /// 旧剪辑的当前时间。
    pub t: f32,
    /// 旧剪辑的循环方式（切换时刻定格）。
    pub looping: bool,
    /// 混合权重 ∈ [0,1]：0 = 纯旧剪辑，1 = 淡入完成。
    pub alpha: f32,
}

/// 交叉淡入的**来源**剪辑（换剪辑时把旧剪辑连同它的时钟一起记下来）。
struct Prev {
    clip: String,
    t: f32,
    /// 切换那一刻旧剪辑的播放速度与循环方式 —— 淡出的那半边按**旧状态自己**的
    /// 速度/循环走。单剪辑模式下与主剪辑同值（切换时刻定格，不跟随事后调速）；
    /// 机器模式下 Walk→Run 时旧 Walk 仍按 Walk 的速度淡出，不会突然被 Run 带快。
    speed: f32,
    looping: bool,
    /// 已淡入的秒数。
    fade_t: f32,
    /// 当前混合权重 ∈ [0,1]：0 = 纯旧剪辑，1 = 淡入完成。tick 里算好存下，
    /// 渲染侧查询时不再需要 Animator 配置。
    alpha: f32,
}

struct Entry {
    /// 状态机当前状态名（单剪辑模式恒为空串）。换状态 = 换剪辑，走同一条淡入路径。
    state: String,
    clip: String,
    t: f32,
    /// 本帧解析后的播放速度 / 循环方式（机器模式下由状态表覆盖 Animator 字段）。
    speed: f32,
    looping: bool,
    /// `None` = 没有淡入（未切换 / 已完成 / 硬切）。
    prev: Option<Prev>,
    /// 叠加层的解析结果：独立剪辑名 + 独立时钟 + 权重（配置摘掉时清成空剪辑）。
    /// 与基础层正交 —— 状态机换状态不影响它，它换剪辑也不触发基础层淡入。
    additive_clip: String,
    additive_t: f32,
    additive_speed: f32,
    additive_looping: bool,
    additive_weight: f32,
    /// 层栈（`Animator.layers`）逐层时钟，保持配置顺序；tick 按 name 对齐同步。
    layer_clocks: Vec<LayerClock>,
}

/// 每个实体的动画播放时钟（clip + 已累计时间）。
///
/// 时钟放在**调用方**（编辑器的 `SceneRenderer`、游戏的 `GameApp`）而不是
/// `ScenePipeline`：管线是纯绘制，`render()` 每帧被调用与否还受指纹门控 ——
/// 时钟跟着门控走会出现"不出帧 → 时间不走 → 指纹不变 → 更不出帧"的死锁。
/// 由外层在**每帧 update** 里 tick，时间永远在推进；渲染只读时间。
pub struct AnimationDriver {
    entries: HashMap<EntityId, Entry>,
    /// `tick_wall` 用：上一次推进的时刻。
    last_tick: Option<Instant>,
    /// `tick_wall` 的时间缩放（编辑器 Play 态 = 运行时 `time_scale`；
    /// 编辑态恒 1.0）。`tick`（游戏侧）不吃它 —— 那边的 dt 已经缩放过了。
    wall_time_scale: f32,
}

impl Default for AnimationDriver {
    /// 手写而不是 derive：`f32` 的默认值是 **0.0**，derive 出来的驱动会
    /// 让编辑器的预览动画永远停在第一帧，而且不报任何错。
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            last_tick: None,
            wall_time_scale: 1.0,
        }
    }
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
    /// - 有状态机时先**求值一步转移**（校验当前状态还在表内 → 查一条命中的转移），
    ///   得到本帧的 (状态, 剪辑, 速度, 循环)；没有状态机就取 Animator 的
    ///   clip/speed/looping。求值每 tick 都做，暂停中改参数也立刻换姿势
    ///   （与单剪辑模式"暂停中改 clip 硬切"同一语义）。
    /// - 剪辑名变了 → 时间归零（换剪辑从头播）。淡入时长 > 0 且当时正在播放 →
    ///   记下旧剪辑（连同它的时钟、速度、循环方式）做交叉淡入，见
    ///   [`fade_from`](Self::fade_from)；暂停中切换 / `fade_time = 0` → 硬切。
    ///   换状态与换剪辑走同一条路径 —— 状态机的转移就是一次换剪辑。
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
            // -- 1. 解析本帧目标播放 --
            //
            // 机器模式：当前状态必须还在表里（配置被 RON 改坏 → 退回 entry），
            // 然后求值一步转移（每 tick 最多走一条边，不链跳）。
            let (state, clip, speed, looping) = match animator.active_machine() {
                None => (
                    String::new(),
                    animator.clip.clone(),
                    animator.speed,
                    animator.looping,
                ),
                Some(m) => {
                    let current = self
                        .entries
                        .get(&entity.id)
                        .map(|e| e.state.as_str())
                        .filter(|s| m.state(s).is_some())
                        .unwrap_or(m.entry.as_str());
                    let next = m.transition_for(current, &m.parameters).unwrap_or(current);
                    // next 只能是 current（已校验在表内）或转移表里校验过的目标
                    // （transition_for 跳过缺失状态）—— 必查得到。
                    let Some(s) = m.state(next) else {
                        continue;
                    };
                    (next.to_owned(), s.clip.clone(), s.speed, s.looping)
                }
            };
            // -- 2. 条目（首建直接落解析值，不触发假淡入）--
            let entry = self
                .entries
                .entry(entity.id)
                .or_insert_with(|| Entry {
                    state: state.clone(),
                    clip: clip.clone(),
                    t: 0.0,
                    speed,
                    looping,
                    prev: None,
                    additive_clip: String::new(),
                    additive_t: 0.0,
                    additive_speed: 1.0,
                    additive_looping: true,
                    additive_weight: 0.0,
                    layer_clocks: Vec::new(),
                });
            // -- 3. 换剪辑 → 交叉淡入 --
            if entry.clip != clip {
                entry.prev = if animator.fade_time > 0.0 && animator.is_active() {
                    Some(Prev {
                        clip: entry.clip.clone(),
                        t: entry.t,
                        // 旧剪辑的速度/循环在**切换时刻**定格：淡出的那半边按它
                        // 自己的参数走，不被新状态带偏。
                        speed: entry.speed,
                        looping: entry.looping,
                        fade_t: 0.0,
                        alpha: 0.0,
                    })
                } else {
                    None
                };
                entry.clip = clip;
                entry.t = 0.0;
            }
            // -- 4. 速度/循环/状态刷新（必须在换剪辑比较之后：Prev 记的是旧值）--
            entry.state = state;
            entry.speed = speed;
            entry.looping = looping;
            // -- 4.5 叠加层刷新：独立剪辑名 → 独立时钟 --
            //
            // 与基础层正交：这里换叠加剪辑只归零**它自己**的时钟（硬切 —— 想平滑
            // 就脚本把 weight 拉 0 再拉回），不动基础层的淡入状态。
            match &animator.additive {
                Some(add) if !add.clip.is_empty() => {
                    if entry.additive_clip != add.clip {
                        entry.additive_clip = add.clip.clone();
                        entry.additive_t = 0.0;
                    }
                    entry.additive_speed = add.speed;
                    entry.additive_looping = add.looping;
                    // 权重钳在 [0,1]：RON 里写 3.0 不该让狐狸抖成筛子。
                    entry.additive_weight = add.weight.clamp(0.0, 1.0);
                }
                _ => {
                    // 摘掉叠加层（或 clip 写空）→ 清干净，时钟归零。
                    entry.additive_clip.clear();
                    entry.additive_t = 0.0;
                }
            }
            // -- 4.6 层栈同步：按 name 对齐旧时钟（重排 / 删层各保各的进度）--
            //
            // 换本层剪辑只归零这一层自己的时钟（硬切，与叠加层同语义）；权重钳
            // [0,1]；mode / mask 跟配置走。没配层的实体这里只是把 vec 清空。
            let old = std::mem::take(&mut entry.layer_clocks);
            entry.layer_clocks = animator
                .layers
                .iter()
                .map(|cfg| {
                    let mut clock = old
                        .iter()
                        .find(|c| c.name == cfg.name)
                        .cloned()
                        .unwrap_or_else(|| LayerClock {
                            name: cfg.name.clone(),
                            clip: String::new(),
                            t: 0.0,
                            speed: cfg.speed,
                            looping: cfg.looping,
                            weight: 0.0,
                            mode: cfg.mode,
                            mask: Vec::new(),
                        });
                    if clock.clip != cfg.clip {
                        clock.clip = cfg.clip.clone();
                        clock.t = 0.0;
                    }
                    clock.speed = cfg.speed;
                    clock.looping = cfg.looping;
                    clock.weight = cfg.weight.clamp(0.0, 1.0);
                    clock.mode = cfg.mode;
                    if clock.mask != cfg.mask {
                        clock.mask = cfg.mask.clone();
                    }
                    clock
                })
                .collect();
            // -- 5. 推进时钟 --
            if animator.is_active() {
                // 负速在 v1 是"静止"而不是倒放（与单剪辑语义一致）：状态表里
                // 写了负速度 → 该状态定格，但转移照常求值，机器不会卡死。
                entry.t += dt * entry.speed.max(0.0);
                // 叠加层时钟与基础层共用同一个闸门（v1 边界，AnimAdditive 文档）。
                if !entry.additive_clip.is_empty() {
                    entry.additive_t += dt * entry.additive_speed.max(0.0);
                }
                // 层栈同闸门；空剪辑的层不推进（时间停 0，解析时被跳过）。
                for clock in &mut entry.layer_clocks {
                    if !clock.clip.is_empty() {
                        clock.t += dt * clock.speed.max(0.0);
                    }
                }
                let mut fade_done = false;
                if let Some(prev) = entry.prev.as_mut() {
                    // 旧剪辑的时钟也继续走 —— 淡出的那半边若定格，混合出来的
                    // 姿势会在过渡中"粘住"一下，看起来像卡帧。
                    prev.t += dt * prev.speed;
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
        // 先夹墙钟（窗口被拖动几秒会一次补上整段），再乘缩放 ——
        // 顺序反过来会让 100 倍速把"拖动窗口"放大成一次长跳。
        self.tick(dt.min(0.1) * self.wall_time_scale, entities);
    }

    /// 设置 `tick_wall` 的时间缩放（编辑器 Play 态每帧从运行时同步）。
    ///
    /// 非有限值忽略、负数钳到 0：与 `GameClock::set_time_scale` 同一套规矩，
    /// 免得两个入口对"坏值"给出不同结果。
    pub fn set_wall_time_scale(&mut self, scale: f32) {
        if !scale.is_finite() {
            return;
        }
        self.wall_time_scale = scale.clamp(0.0, xgame_core::MAX_TIME_SCALE);
    }

    /// 当前 `tick_wall` 的时间缩放（测试与诊断用）。
    pub fn wall_time_scale(&self) -> f32 {
        self.wall_time_scale
    }

    /// 某实体当前的播放时间（未被 tick 过的实体没有条目）。
    pub fn time(&self, id: EntityId) -> Option<f32> {
        self.entries.get(&id).map(|e| e.t)
    }

    /// 某实体本帧的**解析结果**：`(剪辑名, 时间, 循环方式)`。
    ///
    /// 机器模式下剪辑/速度/循环来自状态表（不是 `Animator` 的字段）—— 渲染侧
    /// 一律问驱动要，别再回头读组件配置，否则换状态不出效。
    pub fn playback(&self, id: EntityId) -> Option<(&str, f32, bool)> {
        self.entries
            .get(&id)
            .map(|e| (e.clip.as_str(), e.t, e.looping))
    }

    /// 某实体状态机的**当前状态名**（单剪辑模式恒 `None`）。
    ///
    /// 编辑器 Inspector 显示"现在站在哪个状态"用。
    pub fn state(&self, id: EntityId) -> Option<&str> {
        self.entries
            .get(&id)
            .map(|e| e.state.as_str())
            .filter(|s| !s.is_empty())
    }

    /// 某实体的**叠加层**解析结果（结构体 [`Additive`]）；没配 / clip 写空 → `None`。
    ///
    /// 渲染侧拿它调 [`compute_bones_additive`]：增量 = 叠加剪辑当前帧 − 其第 0 帧
    /// （参考姿势），按 weight 加到基础姿势上。
    pub fn additive(&self, id: EntityId) -> Option<Additive<'_>> {
        let e = self.entries.get(&id)?;
        if e.additive_clip.is_empty() {
            return None;
        }
        Some(Additive {
            clip: e.additive_clip.as_str(),
            t: e.additive_t,
            looping: e.additive_looping,
            weight: e.additive_weight,
        })
    }

    /// 某实体**层栈**的解析时钟（[`LayerClock`] 列表，配置顺序；没配层 / 没条目
    /// → 空切片）。
    ///
    /// 渲染侧逐层取 `clip` → `mesh.clip()` 解析，喂 [`compute_bones_composed`]；
    /// 剪辑不在模型里的层直接跳过（这层不生效，不画错）。
    pub fn layers(&self, id: EntityId) -> &[LayerClock] {
        self.entries
            .get(&id)
            .map(|e| e.layer_clocks.as_slice())
            .unwrap_or(&[])
    }

    /// 某实体正在进行的交叉淡入（结构体 [`FadeFrom`]）。
    ///
    /// `alpha = 0` 还是纯旧剪辑，`1` 淡入完成（此时条目已被清掉，返回 `None`）。
    /// 渲染侧拿它调 [`compute_bones_blended`]：旧剪辑在旧时钟、按旧剪辑自己的
    /// 速度与循环方式采样；新剪辑用 [`playback`](Self::playback) 的时钟。
    pub fn fade_from(&self, id: EntityId) -> Option<FadeFrom<'_>> {
        let p = self.entries.get(&id)?.prev.as_ref()?;
        Some(FadeFrom {
            clip: p.clip.as_str(),
            t: p.t,
            looping: p.looping,
            alpha: p.alpha,
        })
    }

    /// 当前所有时钟的**内容指纹**：进编辑器的场景指纹用。
    ///
    /// 有动画在播 → 每帧指纹变 → 每帧真渲染（否则指纹门控会把动画冻结）；
    /// 没有时钟 → 恒为 0，静态场景照旧"没动就不画"。按 ID 排序后混合，
    /// 与 `HashMap` 的遍历顺序无关 —— 同样的内容必须给同样的指纹。
    ///
    /// **三个时钟都进**：机器模式下基础层可以冻结在某个速度 0 的状态里而
    /// 叠加层 / 层栈照常推进（is_active 看的是"还有没有活状态"）—— 只哈希基础
    /// 时钟会让它们在冻结基础层上"演给瞎子看"（画面不刷新）。
    pub fn signature(&self) -> u64 {
        let mut pairs: Vec<(u64, u64)> = self
            .entries
            .iter()
            .map(|(id, e)| {
                let mut packed =
                    ((e.t.to_bits() as u64) << 32) | e.additive_t.to_bits() as u64;
                // 层栈时钟按配置顺序折进来（splitmix64 是双射，任一层在走都会变）。
                for l in &e.layer_clocks {
                    packed = mix(packed, l.t.to_bits() as u64);
                }
                (id.raw(), packed)
            })
            .collect();
        pairs.sort_unstable();
        let mut h = 0u64;
        for (id, bits) in pairs {
            h = mix(h, id);
            h = mix(h, bits);
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
            name: String::new(),
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
            name: String::new(),
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
                name: String::new(),
                translation: Vec3::new(1.0, 0.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            AnimNode {
                parent: Some(0),
                name: String::new(),
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
                name: String::new(),
                translation: Vec3::new(0.0, 5.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            AnimNode {
                parent: None,
                name: String::new(),
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
            AnimNode { parent: Some(1), name: String::new(), translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE },
            AnimNode { parent: Some(0), name: String::new(), translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE },
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
                name: String::new(),
                translation: Vec3::new(5.0, 1.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            AnimNode {
                parent: Some(0),
                name: String::new(),
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
        let f = d.fade_from(id).expect("换剪辑后应处于淡入中");
        assert_eq!(f.clip, "Walk", "应从旧剪辑淡出");
        assert!(
            (f.t - 0.5).abs() < 1e-6,
            "旧剪辑时钟应继续走（0.4+0.1），实得 {}",
            f.t
        );
        assert!((f.alpha - 0.4).abs() < 1e-6, "alpha 应是 0.1/0.25，实得 {}", f.alpha);
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
        // 先把数值抄出来 —— FadeFrom 借用着驱动，挡不住下一次 tick 的可变借用。
        let (prev_t1, alpha1) = {
            let f = d.fade_from(e.id).unwrap();
            (f.t, f.alpha)
        };

        e.animator.as_mut().unwrap().autoplay = false;
        d.tick(5.0, std::slice::from_ref(&e));
        let (prev_t2, alpha2) = {
            let f = d.fade_from(e.id).expect("暂停不该把淡入清掉");
            (f.t, f.alpha)
        };
        assert!((prev_t2 - prev_t1).abs() < 1e-6, "暂停中旧剪辑时钟该冻结");
        assert!((alpha2 - alpha1).abs() < 1e-6, "暂停中淡入进度该冻结");
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
        let f = d.fade_from(e.id).expect("双切后仍应处于淡入中");
        assert_eq!(f.clip, "Run", "第二次切换应从当前主剪辑（Run）淡出");
        assert!(
            (f.alpha - 0.2).abs() < 1e-6,
            "淡入进度应重新起算（0.05/0.25），实得 {}",
            f.alpha
        );
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

    // -- 状态机 --

    use xgame_core::{
        AnimAdditive, AnimCondition, AnimLayer, AnimLayerMode, AnimMachine, AnimState,
        AnimTransition,
    };

    /// Idle(Survey, 速度 1) ⇄ Walk(速度 1) ⇄ Run(速度 2)：
    /// Idle→Walk 需 ≥0.5、回 Idle 需 <0.3（滞回），Walk→Run 需 ≥2.0、回 <1.8。
    /// Walk 的剪辑故意取**半速**，用来验证速度覆盖。
    fn locomotion_machine() -> AnimMachine {
        AnimMachine {
            entry: "Idle".into(),
            parameters: std::collections::BTreeMap::from([("speed".to_string(), 0.0)]),
            states: vec![
                AnimState {
                    name: "Idle".into(),
                    clip: "Survey".into(),
                    speed: 1.0,
                    looping: true,
                },
                AnimState {
                    name: "Walk".into(),
                    clip: "Walk".into(),
                    speed: 0.5,
                    looping: false,
                },
                AnimState {
                    name: "Run".into(),
                    clip: "Run".into(),
                    speed: 2.0,
                    looping: true,
                },
            ],
            transitions: vec![
                AnimTransition {
                    from: "Idle".into(),
                    to: "Walk".into(),
                    when: AnimCondition::AtLeast {
                        param: "speed".into(),
                        value: 0.5,
                    },
                },
                AnimTransition {
                    from: "Walk".into(),
                    to: "Idle".into(),
                    when: AnimCondition::Below {
                        param: "speed".into(),
                        value: 0.3,
                    },
                },
                AnimTransition {
                    from: "Walk".into(),
                    to: "Run".into(),
                    when: AnimCondition::AtLeast {
                        param: "speed".into(),
                        value: 2.0,
                    },
                },
                AnimTransition {
                    from: "Run".into(),
                    to: "Walk".into(),
                    when: AnimCondition::Below {
                        param: "speed".into(),
                        value: 1.8,
                    },
                },
            ],
        }
    }

    fn entity_with_machine() -> Entity {
        let mut a = Animator::new("ignored");
        a.machine = Some(locomotion_machine());
        Entity::new("Fox").with_animator(a)
    }

    /// 首帧：entry 状态起步、播 entry 状态的剪辑（不是 Animator.clip ——
    /// 机器模式下它已被状态表覆盖）；状态名可查。
    #[test]
    fn driver_starts_at_entry_state_and_plays_its_clip() {
        let mut d = AnimationDriver::new();
        let e = entity_with_machine();
        d.tick(0.1, std::slice::from_ref(&e));

        assert_eq!(d.state(e.id), Some("Idle"), "应停在 entry 状态");
        let (clip, t, looping) = d.playback(e.id).expect("有 animator 就该有解析结果");
        assert_eq!(clip, "Survey", "entry 状态的剪辑才是入口（Animator.clip 被覆盖）");
        assert!((t - 0.1).abs() < 1e-6, "按状态速度推进");
        assert!(looping, "循环方式也来自状态表");
    }

    /// 参数跨阈值 → 求值一步转移 → 当帧换剪辑并交叉淡入（复用第二片的淡入机制）。
    #[test]
    fn driver_transitions_on_param_and_fades_to_new_state() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_machine();
        d.tick(0.3, std::slice::from_ref(&e));

        e.animator.as_mut().unwrap().machine.as_mut().unwrap()
            .parameters
            .insert("speed".into(), 0.6);
        d.tick(0.1, std::slice::from_ref(&e));

        assert_eq!(d.state(e.id), Some("Walk"), "speed ≥ 0.5 应转 Walk");
        let (clip, t, looping) = d.playback(e.id).unwrap();
        assert_eq!(clip, "Walk", "换状态 = 换剪辑");
        // 转移与新剪辑推进在同一 tick：0.1 × Walk 速度 0.5 = 0.05。
        assert!((t - 0.05).abs() < 1e-6, "新剪辑应从 0 起播并当帧推进，实得 {t}");
        assert!(!looping, "新状态是非循环，循环方式也跟着换");
        let f = d.fade_from(e.id).expect("换状态应交叉淡入（复用换剪辑机制）");
        assert_eq!(f.clip, "Survey", "应从旧状态的剪辑淡出");
        assert!(f.looping, "旧剪辑按**它自己**的循环方式淡出");
    }

    /// 状态表的 speed 覆盖 Animator.speed：Walk 状态半速 → 时钟按 dt×0.5 走。
    #[test]
    fn driver_uses_state_speed_and_looping_over_animator_fields() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_machine();
        // 直接从 Walk 起步（半速、非循环）；参数停在滞回区间 0.3~0.5 内，
        // 保证首帧不会被转移带走。
        e.animator.as_mut().unwrap().machine.as_mut().unwrap().entry = "Walk".into();
        e.animator.as_mut().unwrap().machine.as_mut().unwrap()
            .parameters
            .insert("speed".into(), 0.4);
        e.animator.as_mut().unwrap().speed = 7.0; // 单剪辑速度，必须被无视

        d.tick(0.4, std::slice::from_ref(&e));
        let (_, t, looping) = d.playback(e.id).unwrap();
        assert!((t - 0.2).abs() < 1e-6, "应按状态速度 0.5×0.4=0.2 走，实得 {t}");
        assert!(!looping, "非循环状态应报非循环");
    }

    /// entry 状态冻住（速度 0）但转移仍要被求值 —— 否则冻结状态永远出不来。
    #[test]
    fn driver_evaluates_transitions_even_from_frozen_state() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_machine();
        // 把 Idle 冻住（speed 0），但机器还有活状态 → is_active 仍为真。
        e.animator.as_mut().unwrap().machine.as_mut().unwrap().states[0].speed = 0.0;
        d.tick(0.5, std::slice::from_ref(&e));
        let (_, t, _) = d.playback(e.id).unwrap();
        assert!(t.abs() < 1e-6, "冻结状态时钟不该走");

        e.animator.as_mut().unwrap().machine.as_mut().unwrap()
            .parameters
            .insert("speed".into(), 0.6);
        d.tick(0.1, std::slice::from_ref(&e));
        assert_eq!(d.state(e.id), Some("Walk"), "冻结状态里的转移求值必须活着");
    }

    /// 单剪辑模式行为逐位不变：playback 就是 Animator 字段，state 恒 None。
    #[test]
    fn driver_single_clip_playback_mirrors_animator_fields() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_animator("Walk");
        e.animator.as_mut().unwrap().looping = false;
        d.tick(0.2, std::slice::from_ref(&e));

        let (clip, t, looping) = d.playback(e.id).unwrap();
        assert_eq!(clip, "Walk");
        assert!((t - 0.2).abs() < 1e-6);
        assert!(!looping, "单剪辑的 looping 原样透出");
        assert_eq!(d.state(e.id), None, "单剪辑模式没有状态名");
    }

    /// 坏机器（entry 指不到状态）→ 退回单剪辑解析：Animator.clip 照旧生效。
    #[test]
    fn driver_unusable_machine_falls_back_to_animator_clip() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_animator("Walk");
        e.animator.as_mut().unwrap().machine = Some(AnimMachine {
            entry: "Ghost".into(),
            states: vec![AnimState {
                name: "Idle".into(),
                clip: "Survey".into(),
                speed: 1.0,
                looping: true,
            }],
            ..Default::default()
        });
        d.tick(0.2, std::slice::from_ref(&e));
        let (clip, t, _) = d.playback(e.id).unwrap();
        assert_eq!(clip, "Walk", "坏机器应退回单剪辑的 clip");
        assert!((t - 0.2).abs() < 1e-6, "单剪辑时钟照走");
        assert_eq!(d.state(e.id), None, "坏机器不算状态机，没有状态名");
    }

    // -- 叠加混合 --

    /// 叠加增量 = 相对**第 0 帧**的差，按权重线性加入基础平移。
    /// move_x_clip：0s=(0,0,0) → 1s=(2,0,0)，所以 t=0.5 的增量是 (1,0,0)。
    #[test]
    fn additive_applies_frame_zero_relative_translation_delta() {
        let nodes = vec![root_node()];
        let clip = move_x_clip();
        let mut add = default_pose(&nodes);
        sample_clip_into(&clip, 0.5, &mut add); // (1,0,0)
        let mut r = default_pose(&nodes);
        sample_clip_into(&clip, 0.0, &mut r); // 参考 = 第 0 帧 (0,0,0)

        let base_t = Vec3::new(0.0, 5.0, 0.0);
        let at = |w: f32| {
            let mut p = default_pose(&nodes);
            p[0].translation = base_t;
            apply_additive_into(&mut p, &add, &r, w);
            p[0].translation
        };
        assert!((at(1.0) - Vec3::new(1.0, 5.0, 0.0)).length() < 1e-6, "全额增量");
        assert!((at(0.5) - Vec3::new(0.5, 5.0, 0.0)).length() < 1e-6, "半权");
        assert!((at(0.0) - base_t).length() < 1e-6, "零权重纹丝不动");
    }

    /// 停在参考帧上的叠加层增量恒为 0 —— "没动的骨头"不干扰基础姿势。
    #[test]
    fn additive_at_reference_frame_is_a_no_op() {
        let nodes = vec![root_node()];
        let clip = move_x_clip();
        let mut add = default_pose(&nodes);
        sample_clip_into(&clip, 0.0, &mut add);
        let mut base = default_pose(&nodes);
        base[0].translation = Vec3::new(9.0, 9.0, 9.0);
        let before = base[0];
        let r = add.clone();
        apply_additive_into(&mut base, &add, &r, 1.0);
        assert!(
            (base[0].translation - before.translation).length() < 1e-6,
            "参考帧上的叠加必须是零增量"
        );
        assert!(
            base[0].rotation.angle_between(before.rotation) < 1e-5,
            "旋转同理"
        );
    }

    /// 旋转增量走最短路径、按权重缩放：90° 叠加层全额 = 90°、半权 = 45°。
    #[test]
    fn additive_rotation_composes_delta_by_weight() {
        let nodes = vec![root_node()];
        let mut add = default_pose(&nodes);
        add[0].rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let r = default_pose(&nodes); // 参考 = 单位旋转

        for (w, want) in [
            (1.0f32, std::f32::consts::FRAC_PI_2),
            (0.5, std::f32::consts::FRAC_PI_4),
        ] {
            let mut base = default_pose(&nodes);
            apply_additive_into(&mut base, &add, &r, w);
            let angle = base[0].rotation.angle_between(Quat::IDENTITY);
            assert!((angle - want).abs() < 1e-3, "w={w} 应转 {want} rad，实得 {angle}");
        }
    }

    /// 零权重的 `compute_bones_additive` 与纯 `compute_bones` **逐位一致** ——
    /// 没配叠加层的实体不该为它付任何代价（也不引入浮点噪声）。
    #[test]
    fn zero_weight_additive_is_bit_identical_to_plain() {
        let nodes = vec![root_node()];
        let clip = move_x_clip();
        let add = move_y_clip();
        let skin = Skin {
            joints: vec![0],
            inverse_bind: vec![Mat4::IDENTITY],
        };
        let plain = compute_bones(&nodes, &clip, &skin, 0.5);
        let zero = compute_bones_additive(&nodes, &clip, 0.5, &skin, &add, 0.5, 0.0);
        assert_eq!(plain, zero, "零权重必须逐位一致");
    }

    /// 淡入 × 叠加组合：alpha=0（纯旧基础）+ 叠加 == 对旧基础直接叠加。
    #[test]
    fn blended_additive_reduces_to_additive_on_pure_base() {
        let nodes = vec![root_node()];
        let old = move_x_clip(); // 旧基础
        let new = move_y_clip();
        let add = move_x_clip();
        let skin = Skin {
            joints: vec![0],
            inverse_bind: vec![Mat4::IDENTITY],
        };
        let expected = compute_bones_additive(&nodes, &old, 0.5, &skin, &add, 0.5, 1.0);
        let combined =
            compute_bones_blended_additive(&nodes, &old, 0.5, &new, 0.5, 0.0, &skin, &add, 0.5, 1.0);
        assert_eq!(expected, combined, "alpha=0 应退化成'纯旧基础 + 叠加'");
    }

    fn entity_with_additive() -> Entity {
        let mut a = Animator::new("Walk");
        a.additive = Some(AnimAdditive {
            clip: "Run".into(),
            weight: 0.6,
            speed: 2.0,
            looping: true,
        });
        Entity::new("Fox").with_animator(a)
    }

    /// 叠加层有自己的时钟与速度；权重钳位在 [0,1]；摘掉配置 → None；基础层不受影响。
    #[test]
    fn driver_advances_additive_clock_with_own_speed() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_additive();
        d.tick(0.5, std::slice::from_ref(&e));
        let a = d.additive(e.id).expect("配了叠加层就该有解析结果");
        assert_eq!(a.clip, "Run");
        assert!((a.t - 1.0).abs() < 1e-6, "按叠加层自己的速度 2.0 推进，实得 {}", a.t);
        assert!((a.weight - 0.6).abs() < 1e-6);
        let (clip, t, _) = d.playback(e.id).unwrap();
        assert_eq!(clip, "Walk", "基础层不受叠加层影响");
        assert!((t - 0.5).abs() < 1e-6);

        e.animator.as_mut().unwrap().additive.as_mut().unwrap().weight = 7.0;
        d.tick(0.0, std::slice::from_ref(&e));
        assert!((d.additive(e.id).unwrap().weight - 1.0).abs() < 1e-6, "权重应钳在 1.0");

        e.animator.as_mut().unwrap().additive = None;
        d.tick(0.1, std::slice::from_ref(&e));
        assert!(d.additive(e.id).is_none(), "摘掉叠加层后应无解析结果");
    }

    /// 换叠加剪辑：只归零**它自己**的时钟（硬切），基础层照走、不触发淡入。
    #[test]
    fn driver_additive_clip_switch_resets_only_its_own_clock() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_additive();
        d.tick(0.4, std::slice::from_ref(&e));
        e.animator.as_mut().unwrap().additive.as_mut().unwrap().clip = "Survey".into();
        d.tick(0.1, std::slice::from_ref(&e));

        let a = d.additive(e.id).unwrap();
        assert_eq!(a.clip, "Survey");
        assert!((a.t - 0.2).abs() < 1e-6, "换叠加剪辑应从 0 重播（0.1×2.0），实得 {}", a.t);
        assert!(d.fade_from(e.id).is_none(), "叠加层切换不该触发基础层淡入");
        let (_, t, _) = d.playback(e.id).unwrap();
        assert!((t - 0.5).abs() < 1e-6, "基础层时钟照走");
    }

    /// 冻结基础层（机器的当前状态速度 0、别的状态还活着）+ 活叠加层：
    /// 叠加时钟继续推进且指纹跟着变 —— 否则叠加层"演给瞎子看"（画面不刷新）。
    #[test]
    fn driver_frozen_base_still_advances_additive_and_signature() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_machine();
        e.animator.as_mut().unwrap().machine.as_mut().unwrap().states[0].speed = 0.0;
        e.animator.as_mut().unwrap().additive = Some(AnimAdditive {
            clip: "Run".into(),
            ..Default::default()
        });

        d.tick(0.5, std::slice::from_ref(&e));
        let (_, base_t, _) = d.playback(e.id).unwrap();
        assert!(base_t.abs() < 1e-6, "冻结状态基础时钟不该走");
        assert!((d.additive(e.id).unwrap().t - 0.5).abs() < 1e-6, "叠加层时钟要继续走");
        let sig1 = d.signature();
        d.tick(0.1, std::slice::from_ref(&e));
        assert_ne!(sig1, d.signature(), "叠加层在走 → 指纹必须变，否则画面冻结");
    }

    // -- 层栈与骨骼遮罩 --

    /// 两节骨架：Root("Root") → Child("Child")，都停在绑定姿态。
    fn root_child_nodes() -> Vec<AnimNode> {
        vec![
            AnimNode {
                parent: None,
                name: "Root".into(),
                translation: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            AnimNode {
                parent: Some(0),
                name: "Child".into(),
                translation: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        ]
    }

    /// 单轨剪辑：`node` 的平移 0→1 秒从原点走到 `end`。
    fn translate_clip(name: &str, node: u32, end: Vec3) -> AnimClip {
        AnimClip {
            name: name.into(),
            duration: 1.0,
            tracks: vec![Track {
                node,
                path: TrackPath::Translation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![0.0, 0.0, 0.0, end.x, end.y, end.z],
            }],
        }
    }

    /// 每个关节都是单位逆绑定（关节矩阵 = 全局矩阵）的小蒙皮。
    fn flat_skin(joints: u32) -> Skin {
        Skin {
            joints: (0..joints).collect(),
            inverse_bind: vec![Mat4::IDENTITY; joints as usize],
        }
    }

    /// 遮罩 = 子树根名单：命中自身或祖先；空名单全覆盖；匿名节点不与空串互认；
    /// 畸形成环不死循环。
    #[test]
    fn mask_covers_subtree_roots_and_their_descendants() {
        let nodes = root_child_nodes();
        assert!(mask_covers(&nodes, 0, &[]), "空名单 = 整副骨架");
        assert!(mask_covers(&nodes, 1, &["Child".into()]), "命中自身");
        assert!(!mask_covers(&nodes, 0, &["Child".into()]), "只列子 → 父不被覆盖");
        assert!(mask_covers(&nodes, 1, &["Root".into()]), "祖先命中 → 子孙一并覆盖");
        assert!(!mask_covers(&nodes, 1, &["Nope".into()]), "没命中不覆盖");

        // 匿名节点（名字空串）不能靠空名单项互认进来。
        let anon = vec![root_node()];
        assert!(!mask_covers(&anon, 0, &["".into()]), "匿名节点不与空串名单项互认");

        // 畸形：A→B→A 成环 —— 沿父链能命中就返回，没命中也要收尾而不是挂起。
        let cycle = vec![
            AnimNode {
                parent: Some(1),
                name: "A".into(),
                translation: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            AnimNode {
                parent: Some(0),
                name: "B".into(),
                translation: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        ];
        assert!(mask_covers(&cycle, 0, &["B".into()]), "环里沿父链也要能命中");
        assert!(
            !mask_covers(&cycle, 0, &["Nope".into()]),
            "成环没命中也要返回（不死循环）"
        );
    }

    /// Override × 遮罩端到端：只有名单内的骨被接管 —— w=1 完全等于该层姿势、
    /// w=0.5 折半、遮罩没命中则整层不生效；名单外的骨保持基础姿势。
    #[test]
    fn override_layer_takes_over_only_masked_bones() {
        let nodes = root_child_nodes();
        let base = move_x_clip(); // 根 0→2（t=1 → x=2）
        let upper = translate_clip("upper", 1, Vec3::new(0.0, 4.0, 0.0));
        let skin = flat_skin(2);
        let mask_child: Vec<String> = vec!["Child".into()];
        let mask_no: Vec<String> = vec!["Nope".into()];

        let composed = |weight: f32, mask: &[String]| {
            compute_bones_composed(
                &nodes,
                &skin,
                (&base, 1.0),
                None,
                &[LayerInput {
                    clip: &upper,
                    t: 1.0,
                    weight,
                    mode: AnimLayerMode::Override,
                    mask,
                }],
            )
        };

        let full = composed(1.0, &mask_child);
        assert!(
            (full[0].w_axis.truncate() - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-6,
            "名单外的根保持基础姿势"
        );
        assert!(
            (full[1].w_axis.truncate() - Vec3::new(2.0, 4.0, 0.0)).length() < 1e-6,
            "w=1 时子 = 根位移 + 该层姿势"
        );

        let half = composed(0.5, &mask_child);
        assert!(
            (half[1].w_axis.truncate() - Vec3::new(2.0, 2.0, 0.0)).length() < 1e-6,
            "w=0.5 应折半"
        );

        let miss = composed(1.0, &mask_no);
        assert!(
            (miss[1].w_axis.truncate() - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-6,
            "遮罩没命中 → 层不生效"
        );
    }

    /// Additive 层同样走遮罩：增量（当前帧 − 第 0 帧）只加在名单内的骨上。
    #[test]
    fn additive_layer_in_stack_respects_mask() {
        let nodes = root_child_nodes();
        let base = move_x_clip(); // t=0 → 根在原点
        let inc = translate_clip("inc", 1, Vec3::new(0.0, 4.0, 0.0));
        let skin = flat_skin(2);
        let mask_child: Vec<String> = vec!["Child".into()];
        let mask_no: Vec<String> = vec!["Nope".into()];

        let on = compute_bones_composed(
            &nodes,
            &skin,
            (&base, 0.0),
            None,
            &[LayerInput {
                clip: &inc,
                t: 1.0,
                weight: 1.0,
                mode: AnimLayerMode::Additive,
                mask: &mask_child,
            }],
        );
        assert!(
            (on[1].w_axis.truncate() - Vec3::new(0.0, 4.0, 0.0)).length() < 1e-6,
            "名单内的骨吃到增量"
        );
        assert!(
            on[0].w_axis.truncate().length() < 1e-6,
            "名单外的骨不动"
        );

        let off = compute_bones_composed(
            &nodes,
            &skin,
            (&base, 0.0),
            None,
            &[LayerInput {
                clip: &inc,
                t: 1.0,
                weight: 1.0,
                mode: AnimLayerMode::Additive,
                mask: &mask_no,
            }],
        );
        assert!(
            off.iter().all(|b| b.w_axis.truncate().length() < 1e-6),
            "遮罩没命中 → 整副骨架不动"
        );
    }

    /// 栈按数组顺序合到"当前已合成的姿势"上：后层盖前层。
    #[test]
    fn later_layers_compose_on_top() {
        let nodes = root_child_nodes();
        let base = move_x_clip();
        let first = translate_clip("first", 1, Vec3::new(0.0, 4.0, 0.0));
        let second = translate_clip("second", 1, Vec3::new(0.0, 1.0, 0.0));
        let skin = flat_skin(2);
        let mask: Vec<String> = vec!["Child".into()];

        let bones = compute_bones_composed(
            &nodes,
            &skin,
            (&base, 0.0),
            None,
            &[
                LayerInput {
                    clip: &first,
                    t: 1.0,
                    weight: 1.0,
                    mode: AnimLayerMode::Override,
                    mask: &mask,
                },
                LayerInput {
                    clip: &second,
                    t: 1.0,
                    weight: 1.0,
                    mode: AnimLayerMode::Override,
                    mask: &mask,
                },
            ],
        );
        assert!(
            (bones[1].w_axis.truncate() - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-6,
            "后层 w=1 应完全盖掉前层"
        );
    }

    fn entity_with_layers() -> Entity {
        let mut a = Animator::new("Walk");
        a.layers = vec![
            AnimLayer {
                name: "upper".into(),
                clip: "Survey".into(),
                weight: 0.6,
                ..Default::default()
            },
            AnimLayer {
                name: "legs".into(),
                clip: "Run".into(),
                speed: 2.0,
                ..Default::default()
            },
        ];
        Entity::new("Fox").with_animator(a)
    }

    /// 层时钟按 name 对齐同步：重排不丢进度、新层从 0 起播、删层即 prune、
    /// 换本层剪辑只归零那一层、权重越界钳位。
    #[test]
    fn driver_layer_clocks_sync_by_name() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_layers();
        d.tick(0.5, std::slice::from_ref(&e));
        {
            let layers = d.layers(e.id);
            assert_eq!(layers.len(), 2, "配置两层就该有两个时钟");
            assert_eq!(layers[0].name, "upper");
            assert!((layers[0].t - 0.5).abs() < 1e-6, "upper 按速度 1 推进");
            assert!((layers[0].weight - 0.6).abs() < 1e-6);
            assert_eq!(layers[1].clip, "Run");
            assert!((layers[1].t - 1.0).abs() < 1e-6, "legs 按速度 2 推进，实得 {}", layers[1].t);
        }

        // 重排 + 插入新层：各层按名字对齐，进度延续（不按位置错位）。
        let anim = e.animator.as_mut().unwrap();
        let upper = anim.layers[0].clone();
        let legs = anim.layers[1].clone();
        anim.layers = vec![
            AnimLayer {
                name: "new".into(),
                clip: "Walk".into(),
                ..Default::default()
            },
            legs,
            upper,
        ];
        d.tick(0.1, std::slice::from_ref(&e));
        {
            let layers = d.layers(e.id);
            assert_eq!(layers[0].name, "new");
            assert!((layers[0].t - 0.1).abs() < 1e-6, "新层从 0 起播");
            assert_eq!(layers[1].name, "legs");
            assert!(
                (layers[1].t - 1.2).abs() < 1e-6,
                "重排后 legs 进度延续（1.0 + 0.1×2），实得 {}",
                layers[1].t
            );
            assert_eq!(layers[2].name, "upper");
            assert!((layers[2].t - 0.6).abs() < 1e-6, "重排后 upper 进度延续");
        }

        // 换本层剪辑：只有它归零重播，别的层照走。
        e.animator.as_mut().unwrap().layers[2].clip = "Run".into();
        d.tick(0.1, std::slice::from_ref(&e));
        assert!(
            (d.layers(e.id)[2].t - 0.1).abs() < 1e-6,
            "换剪辑归零重播"
        );
        assert!(
            (d.layers(e.id)[1].t - 1.4).abs() < 1e-6,
            "别的层不受影响"
        );

        // 删层 → prune；权重越界 → 钳 [0,1]（dt=0 只同步不推进）。
        e.animator.as_mut().unwrap().layers.remove(0);
        e.animator.as_mut().unwrap().layers[1].weight = 9.0;
        d.tick(0.0, std::slice::from_ref(&e));
        assert_eq!(d.layers(e.id).len(), 2, "删掉的层不再出现，剩 legs + upper");
        assert_eq!(d.layers(e.id)[1].name, "upper");
        assert!((d.layers(e.id)[1].weight - 1.0).abs() < 1e-6, "权重钳在 1.0");
    }

    /// 冻结基础层（当前状态速度 0、别的状态还活）+ 活层栈：层时钟照走且指纹
    /// 跟着变 —— 不然层栈在冻结基础层上"演给瞎子看"（画面不刷新）。
    #[test]
    fn driver_layer_stack_advances_under_frozen_base_and_moves_signature() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_machine();
        e.animator.as_mut().unwrap().machine.as_mut().unwrap().states[0].speed = 0.0;
        e.animator.as_mut().unwrap().layers = vec![AnimLayer {
            name: "upper".into(),
            clip: "Survey".into(),
            ..Default::default()
        }];

        d.tick(0.4, std::slice::from_ref(&e));
        assert!(
            d.playback(e.id).unwrap().1.abs() < 1e-6,
            "冻结状态的基础时钟不该走"
        );
        assert!(
            (d.layers(e.id)[0].t - 0.4).abs() < 1e-6,
            "层时钟要继续走"
        );
        let sig1 = d.signature();
        d.tick(0.1, std::slice::from_ref(&e));
        assert_ne!(sig1, d.signature(), "层时钟在走 → 指纹必须变，否则画面冻结");
    }

    /// `tick_wall` 的时间缩放：编辑器 Play 态从运行时同步，脚本 `set_time_scale(0)`
    /// 暂停时预览动画必须一起停 —— 否则 Play 里"物理冻住、角色却在原地跑步"。
    ///
    /// 顺带钉住 `Default`：`f32` 的默认值是 0.0，若 derive 掉这个字段，编辑器的
    /// 预览动画会永远停在第一帧，且不报任何错。
    #[test]
    fn wall_time_scale_zero_freezes_the_preview_clock() {
        let driver_default = AnimationDriver::new();
        assert_eq!(driver_default.wall_time_scale(), 1.0, "默认必须是实时");

        let mut e = Entity::new("actor");
        e.animator = Some(Animator {
            clip: "Walk".into(),
            ..Animator::default()
        });

        let mut d = AnimationDriver::new();
        d.set_wall_time_scale(0.0);
        d.tick_wall(std::slice::from_ref(&e));
        std::thread::sleep(std::time::Duration::from_millis(5));
        d.tick_wall(std::slice::from_ref(&e));
        assert_eq!(d.time(e.id), Some(0.0), "缩放 0 时墙钟推不动预览动画");

        // 恢复实时：同样的墙钟间隔必须重新推动时钟（证明上一步不是"没 tick"）。
        d.set_wall_time_scale(1.0);
        std::thread::sleep(std::time::Duration::from_millis(5));
        d.tick_wall(std::slice::from_ref(&e));
        assert!(
            d.time(e.id).is_some_and(|t| t > 0.0),
            "恢复实时后时钟应继续走"
        );
    }
}
