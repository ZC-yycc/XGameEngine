//! 动画剪辑的数据结构、**纯函数采样**与播放时钟。
//!
//! # 分层
//!
//! - [`Animator`](crate::Animator) 是**配置**（播什么、多快、循不循环），随场景落盘；
//! - 本模块是 CPU 共享层：[`AnimClip`] / [`AnimationAsset`] 与
//!   [`AnimationDriver`] 同时给 runtime 和 render 使用；
//! - glTF 仍只由 `xgame-render` 的加载器解析，它把纯 CPU 数据交给这里定义的类型；
//! - 蒙皮矩阵（关节局部姿态 → 层级全局矩阵 × 逆绑定矩阵）由 [`compute_bones`]
//!   一次性算完，渲染管线直接拿去上传。
//!
//! 采样侧全部是**纯函数**（时间 → 矩阵），没有隐藏状态 —— 单测直接喂时间断言
//! 数值，不需要 GPU、不需要真实帧循环。
//!
//! # v1 范围
//!
//! 单剪辑播放 + 换剪辑时的**交叉淡入** + 声明式**动画状态机**（参数阈值驱动的
//! 剪辑切换，见 [`AnimMachine`](crate::AnimMachine)）+ **叠加（additive）混合**（增量 =
//! 叠加剪辑当前帧 − 其第 0 帧参考姿势，见 `compute_bones_additive`）+ **混合层栈**
//! （多层 Additive/Override × 骨骼遮罩，见 [`compute_bones_composed`]），三种插值
//! （LINEAR / STEP / **CUBICSPLINE Hermite 样条**）。旋转用 slerp，循环用取模、
//! 非循环钳在末帧。

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use glam::{Mat4, Quat, Vec3};

use crate::{AnimLayerMode, Entity, EntityId};

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
    /// 形变目标权重（glTF `KHR_animation_pointer` 之前的标准写法：
    /// channel 指向**节点**，输出是"每帧一组权重"，分量数 = 该网格的目标数）。
    ///
    /// 它不进 [`LocalPose`]：权重是网格的属性，不是节点的姿态。采样入口是
    /// [`sample_morph_weights`]。
    MorphWeights,
}

impl TrackPath {
    /// 每个关键帧的分量数（位移/缩放 3，旋转 4）。
    ///
    /// `MorphWeights` 的 stride 是**逐轨道**的（= 目标数），返回 0 只是
    /// 占位；请用 [`Track::stride`]。
    fn stride(self) -> usize {
        match self {
            TrackPath::Translation | TrackPath::Scale => 3,
            TrackPath::Rotation => 4,
            TrackPath::MorphWeights => 0,
        }
    }
}

/// 关键帧插值方式（glTF 的三种原样保留）。
///
/// # 为什么 `CubicSpline` 不再降采样
///
/// 早先版本在读入时把 `CUBICSPLINE` 取中间那份值降成 LINEAR、切线直接丢弃，
/// 理由是"运行时分支越少越稳"。代价是**内容失真且不可逆**：Blender / Maya
/// 默认导出的就是三次样条，降成线性后加减速被抹平 —— 挥砍变成匀速平移、
/// 起跳没有蓄力。切线一旦在读入时扔掉就再也补不回来，所以现在完整保留，
/// 采样端走 Hermite（见 [`hermite_basis`]）。
///
/// 线性 / 阶梯两条路径的标志位分支与从前逐位一致（同样的 `lerp` / 取左帧），
/// 新增的只是第三种枚举值，不是给热路径加一层判断。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    Linear,
    Step,
    /// 每关键帧三份值：`(入切线, 值, 出切线)`，切线单位是**每秒**。
    CubicSpline,
}

impl Interpolation {
    /// 每关键帧在 [`Track::values`] 里占几"组"值：样条三组，其余一组。
    ///
    /// 布局常量只在这里定义一次 —— 采样、加载、测试全都读它，
    /// 免得出现"某处以为只有一份值"的错位。
    #[inline]
    pub fn slots(self) -> usize {
        match self {
            Interpolation::CubicSpline => 3,
            Interpolation::Linear | Interpolation::Step => 1,
        }
    }

    /// 本帧三组值里"值"是哪一组（样条是中间那份，其余只有一份）。
    #[inline]
    fn value_slot(self) -> usize {
        match self {
            Interpolation::CubicSpline => 1,
            Interpolation::Linear | Interpolation::Step => 0,
        }
    }
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
    /// 关键帧值，每帧 [`Track::frame_len`] 个浮点紧凑排布 —— 总长
    /// `times.len() × frame_len()`。
    ///
    /// 线性 / 阶梯每帧一组值（`stride()` 个分量）；CUBICSPLINE 每帧三组
    /// （入切线 / 值 / 出切线），切线单位是**每秒**。
    pub values: Vec<f32>,
}

impl Track {
    /// 每关键帧的**值**分量数（不含样条的入 / 出切线）。
    ///
    /// 变换通道是常量；`MorphWeights` 的 stride 是"目标数"，由
    /// "浮点总数 ÷ (关键帧数 × 每帧组数)"现算。
    pub fn stride(&self) -> usize {
        match self.path {
            // 关键帧数 × 每帧组数 = 0（空轨道）时 `checked_div` 给 None → 0。
            TrackPath::MorphWeights => self
                .values
                .len()
                .checked_div(self.times.len() * self.interpolation.slots())
                .unwrap_or(0),
            _ => self.path.stride(),
        }
    }

    /// 关键帧 `k` 在 [`Track::values`] 里的起始浮点下标（样条时=入切线）。
    #[inline]
    pub fn frame_start(&self, k: usize) -> usize {
        k * self.stride() * self.interpolation.slots()
    }

    /// 关键帧 `k` 的**值**起始下标（样条跳过入切线）。
    #[inline]
    pub fn value_start(&self, k: usize) -> usize {
        self.frame_start(k) + self.interpolation.value_slot() * self.stride()
    }

    /// 每关键帧占的浮点数（= `stride() × 组数`）。
    ///
    /// `values.len() == times.len() × frame_len()` 是"轨道形状自洽"的唯一判据。
    #[inline]
    pub fn frame_len(&self) -> usize {
        self.stride() * self.interpolation.slots()
    }

    /// 采样前的一次性形状校验：坏资产（少分量 / 长度不自洽）宁可跳过这条轨道，
    /// 也不要在每帧必走的路径上 panic。
    ///
    /// `i0 <= i1` 时只要 `(i1+1)` 帧完整，两帧的入 / 值 / 出三份就都读得到。
    fn readable(&self, i1: usize) -> bool {
        let stride = self.stride();
        stride > 0 && (i1 + 1) * self.frame_len() <= self.values.len()
    }
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
    let u = if span > 0.0 {
        (t - times[i0]) / span
    } else {
        0.0
    };
    (i0, i1, u)
}

/// Hermite 基函数：段内比例 `u∈[0,1]` → `(h00, h10, h01, h11)`。
///
/// 三次样条在 glTF 里就是这个形式（规范附录 C）：
/// `p(u) = h00·p0 + h10·m0 + h01·p1 + h11·m1`，其中 `m0/m1` 是**已乘段长**的切线。
#[inline]
fn hermite_basis(u: f32) -> (f32, f32, f32, f32) {
    let u2 = u * u;
    let u3 = u2 * u;
    (
        2.0 * u3 - 3.0 * u2 + 1.0,
        u3 - 2.0 * u2 + u,
        -2.0 * u3 + 3.0 * u2,
        u3 - u2,
    )
}

/// 平移 / 缩放通道求值：`i0 → i1` 段、段内比例 `u`、段长 `dt`（秒）。
///
/// 三种插值都在这里分派 —— 调用方只说"这条轨道这段时间的值"，
/// 不会出现"某条通道忘了走样条"这种漏分支。
fn eval_vec3(track: &Track, i0: usize, i1: usize, u: f32, dt: f32) -> Vec3 {
    let s = track.stride();
    let p0 = Vec3::from_slice(&track.values[track.value_start(i0)..][..3]);
    match track.interpolation {
        Interpolation::Step => p0,
        Interpolation::Linear => {
            let p1 = Vec3::from_slice(&track.values[track.value_start(i1)..][..3]);
            p0.lerp(p1, u)
        }
        Interpolation::CubicSpline => {
            let p1 = Vec3::from_slice(&track.values[track.value_start(i1)..][..3]);
            // 切线单位是"每秒"，乘段长换成本段量纲（漏乘会让长段欠冲）。
            let m0 = Vec3::from_slice(&track.values[track.frame_start(i0) + 2 * s..][..3]) * dt;
            let m1 = Vec3::from_slice(&track.values[track.frame_start(i1)..][..3]) * dt;
            let (h00, h10, h01, h11) = hermite_basis(u);
            p0 * h00 + m0 * h10 + p1 * h01 + m1 * h11
        }
    }
}

/// 旋转通道求值。
///
/// 样条结果**必须归一化**：逐分量 Hermite 出来的四元数不在单位球上，
/// 直接合成矩阵等于给蒙皮乘了一个随动画变化的缩放（模型会越动越大）。
fn eval_quat(track: &Track, i0: usize, i1: usize, u: f32, dt: f32) -> Quat {
    let s = track.stride();
    let q0 = Quat::from_slice(&track.values[track.value_start(i0)..][..4]);
    match track.interpolation {
        Interpolation::Step => q0,
        Interpolation::Linear => {
            let q1 = Quat::from_slice(&track.values[track.value_start(i1)..][..4]);
            q0.slerp(q1, u)
        }
        Interpolation::CubicSpline => {
            let q1 = Quat::from_slice(&track.values[track.value_start(i1)..][..4]);
            let m0 = Quat::from_slice(&track.values[track.frame_start(i0) + 2 * s..][..4]) * dt;
            let m1 = Quat::from_slice(&track.values[track.frame_start(i1)..][..4]) * dt;
            let (h00, h10, h01, h11) = hermite_basis(u);
            let raw = q0 * h00 + m0 * h10 + q1 * h01 + m1 * h11;
            // 极端切线能把长度压到 0 附近（手工改过的曲线 / 坏资产）：
            // 那时归一化会吐 NaN，退回 slerp 至少方向是对的。
            if raw.length_squared() > 1e-12 {
                raw.normalize()
            } else {
                q0.slerp(q1, u)
            }
        }
    }
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
        // 权重轨道不是姿态：它改的是网格顶点，由 `sample_morph_weights` 消费。
        if track.path == TrackPath::MorphWeights {
            continue;
        }
        let (i0, i1, u) = key_segment(&track.times, t);
        // 越界读保护：坏资产少几个分量时宁可跳过这条轨道，也不要 panic。
        if !track.readable(i1) {
            continue;
        }
        let dt = track.times[i1] - track.times[i0];
        match track.path {
            TrackPath::Translation => {
                pose[idx].translation = eval_vec3(track, i0, i1, u, dt);
            }
            TrackPath::Scale => {
                pose[idx].scale = eval_vec3(track, i0, i1, u, dt);
            }
            TrackPath::Rotation => {
                pose[idx].rotation = eval_quat(track, i0, i1, u, dt);
            }
            // 上面已经 `continue` 掉了；列出来是为了让"新增通道必须在这里
            // 做一次决断"变成编译错误，而不是被 `_ =>` 静默吞掉。
            TrackPath::MorphWeights => {}
        }
    }
}

/// 采样剪辑里的形变权重轨道，写进 `out`（长度 = 目标数）。
///
/// # 为什么只看第一条权重轨道
///
/// glTF 把权重轨道挂在**节点**上，一个角色理论上可以有多个带 morph 的网格、
/// 各自一条轨道。当前引擎一个实体 = 一个模型 = 一套可见网格，多网格角色
/// （头 / 身体分开）的权重轨道会取到第一条 —— 这是明确的 v1 边界，
/// 写在 `PLAN.md` 的"还没做"里，不假装支持。
///
/// 剪辑没有权重轨道时返回 `false`（`out` 不动）：调用方据此退回
/// "实体权重 / 资产默认"，而不是把所有权重清零。
pub fn sample_morph_weights(clip: &AnimClip, t: f32, out: &mut Vec<f32>) -> bool {
    let Some(track) = clip
        .tracks
        .iter()
        .find(|track| track.path == TrackPath::MorphWeights && !track.times.is_empty())
    else {
        return false;
    };
    let count = track.stride();
    // 形状必须严丝合缝：morph 的 stride 是现算的，长度对不上说明资产脏，
    // 硬读会串到相邻目标的分量上（画出来是一张扭曲的脸）。
    if count == 0 || track.values.len() != track.times.len() * track.frame_len() {
        return false;
    }
    let (i0, i1, u) = key_segment(&track.times, t);
    let dt = track.times[i1] - track.times[i0];
    out.clear();
    out.reserve(count);
    match track.interpolation {
        Interpolation::Step | Interpolation::Linear => {
            let a = track.value_start(i0);
            let b = track.value_start(i1);
            let u = if track.interpolation == Interpolation::Step {
                0.0
            } else {
                u
            };
            for c in 0..count {
                let va = track.values[a + c];
                out.push(va + (track.values[b + c] - va) * u);
            }
        }
        Interpolation::CubicSpline => {
            let a = track.value_start(i0);
            let b = track.value_start(i1);
            let out0 = track.frame_start(i0) + 2 * count;
            let in1 = track.frame_start(i1);
            let (h00, h10, h01, h11) = hermite_basis(u);
            for c in 0..count {
                let p0 = track.values[a + c];
                let p1 = track.values[b + c];
                let m0 = track.values[out0 + c] * dt;
                let m1 = track.values[in1 + c] * dt;
                out.push(p0 * h00 + m0 * h10 + p1 * h01 + m1 * h11);
            }
        }
    }
    true
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
    let local =
        Mat4::from_scale_rotation_translation(pose[i].scale, pose[i].rotation, pose[i].translation);
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
    /// 本层状态机正在交叉淡入时的旧剪辑、旧时间与混合权重。
    ///
    /// `None` 是历史直接剪辑层；那条路径不多采样一次，结果逐位不变。
    pub fade: Option<(&'a AnimClip, f32, f32)>,
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
    compute_bones_composed_with_root_motion(nodes, skin, base, fade, layers, None)
}

/// 与 [`compute_bones_composed`] 相同，但会从**基础姿势**里剥掉指定节点的根运动轴。
///
/// 剥离发生在基础层（含交叉淡入）采样之后、附加层合成之前：runtime 已把这些轴的
/// 每帧增量交给实体 / 角色控制器，蒙皮若再保留一次就会双倍前进；additive 与 layers
/// 则按约定仍属于骨架姿势，不能被一刀清掉。节点名不命中时与普通合成逐位一致。
pub fn compute_bones_composed_with_root_motion(
    nodes: &[AnimNode],
    skin: &Skin,
    base: (&AnimClip, f32),
    fade: Option<(&AnimClip, f32, f32)>,
    layers: &[LayerInput<'_>],
    root_motion: Option<(&str, [bool; 3])>,
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
    if let Some((name, axes)) = root_motion {
        if let Some(index) = (!name.is_empty())
            .then(|| nodes.iter().position(|node| node.name == name))
            .flatten()
        {
            let bind = nodes[index].translation;
            if axes[0] {
                pose[index].translation.x = bind.x;
            }
            if axes[1] {
                pose[index].translation.y = bind.y;
            }
            if axes[2] {
                pose[index].translation.z = bind.z;
            }
        }
    }
    for layer in layers {
        let w = layer.weight.clamp(0.0, 1.0);
        if w <= 0.0 || layer.clip.duration <= 0.0 {
            continue;
        }
        match layer.mode {
            AnimLayerMode::Additive => {
                let mut add_pose = default_pose(nodes);
                sample_clip_with_fade(nodes, layer.clip, layer.t, layer.fade, &mut add_pose);
                // 参考姿势 = 本层剪辑第 0 帧：没轨道的节点两份都停在绑定姿态 →
                // 增量恒 0，叠加层"没动的骨头"不干扰基础层。
                let mut ref_pose = default_pose(nodes);
                // 状态机淡入时，参考姿势也必须按同一 alpha 从旧剪辑第 0 帧混到
                // 新剪辑第 0 帧。只混当前姿势会让参考点在切换帧硬跳，Additive
                // 层即使 alpha=0 也会突然吃到新剪辑的增量基准。
                let ref_fade = layer.fade.map(|(old, _, alpha)| (old, 0.0, alpha));
                sample_clip_with_fade(nodes, layer.clip, 0.0, ref_fade, &mut ref_pose);
                for i in 0..nodes.len() {
                    if mask_covers(nodes, i, layer.mask) {
                        blend_additive(&mut pose[i], &add_pose[i], &ref_pose[i], w);
                    }
                }
            }
            AnimLayerMode::Override => {
                let mut layer_pose = default_pose(nodes);
                sample_clip_with_fade(nodes, layer.clip, layer.t, layer.fade, &mut layer_pose);
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

/// 一个模型里与动画有关的纯 CPU 数据。
///
/// 它不含顶点缓冲、纹理或 glTF 解析器，因此 runtime 可以在普通无头运行中加载、
/// 采样根运动，而不需要初始化 GPU。渲染模型持有同一组类型，不再维护第二份格式。
#[derive(Clone, Debug, Default)]
pub struct AnimationAsset {
    pub nodes: Vec<AnimNode>,
    pub clips: Vec<AnimClip>,
}

impl AnimationAsset {
    /// 按 glTF 动画名查剪辑。
    pub fn clip(&self, name: &str) -> Option<&AnimClip> {
        self.clips.iter().find(|clip| clip.name == name)
    }

    /// 按显式 glTF 节点名查下标。重名时取文件顺序里的第一个，与其它按名查询一致。
    pub fn node_index(&self, name: &str) -> Option<usize> {
        (!name.is_empty())
            .then(|| self.nodes.iter().position(|node| node.name == name))
            .flatten()
    }
}

/// runtime 的动画资产注入点（项目相对模型路径 → 纯 CPU 动画数据）。
///
/// trait 住在 core，避免 runtime 反向依赖 render；真实实现仍由 render 提供，因而
/// glTF 只有一个解析器。测试可以注入内存资产，不需要磁盘、GPU 或 glTF。
pub trait AnimationLoader {
    fn load_animation(&self, model: &str) -> Result<AnimationAsset, String>;
}

impl<T: AnimationLoader + ?Sized> AnimationLoader for std::sync::Arc<T> {
    fn load_animation(&self, model: &str) -> Result<AnimationAsset, String> {
        (**self).load_animation(model)
    }
}

/// 采样一份当前剪辑；有淡入时先采旧剪辑，再向当前剪辑混合。
///
/// 层状态机和基础层共享同一条局部姿势混合规则（平移/缩放 lerp、旋转 slerp），
/// 但缓冲由调用方提供，便于 Additive 同时对“当前姿势”和“第 0 帧参考”复用。
fn sample_clip_with_fade(
    nodes: &[AnimNode],
    clip: &AnimClip,
    t: f32,
    fade: Option<(&AnimClip, f32, f32)>,
    pose: &mut [LocalPose],
) {
    match fade {
        Some((old_clip, old_t, alpha)) => {
            sample_clip_into(old_clip, old_t, pose);
            // `pose` 此刻含旧剪辑，不能拿它当新剪辑的默认姿势：两条剪辑未覆盖
            // 同一通道时，新侧必须从绑定姿势起算。
            let mut current = default_pose(nodes);
            sample_clip_into(clip, t, &mut current);
            blend_into(pose, &current, alpha.clamp(0.0, 1.0));
        }
        None => sample_clip_into(clip, t, pose),
    }
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
    compute_bones_composed(nodes, skin, (clip_b, t_b), Some((clip_a, t_a, alpha)), &[])
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
            fade: None,
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
            fade: None,
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

/// 一条剪辑在本帧走过的原始时间段（时间尚未取模 / 钳位）。
#[derive(Clone, Copy, Debug)]
pub struct MotionSpan<'a> {
    pub clip: &'a AnimClip,
    pub from: f32,
    pub to: f32,
    pub looping: bool,
}

/// 交叉淡入中旧剪辑的本帧时间段与权重区间。
#[derive(Clone, Copy, Debug)]
pub struct MotionFade<'a> {
    pub span: MotionSpan<'a>,
    /// 本帧开始时新剪辑的权重。
    pub alpha_from: f32,
    /// 本帧结束时新剪辑的权重。
    pub alpha_to: f32,
}

/// 从基础播放通道提取一帧根运动位移（模型局部空间）。
///
/// `node_name` 必须显式命中 glTF 节点；空名 / 缺节点 / 缺平移轨道都返回零。
/// additive 与 layers 不在参数里，因此天然不会偷偷参与根运动。
pub fn root_motion_delta(
    asset: &AnimationAsset,
    node_name: &str,
    axes: [bool; 3],
    current: MotionSpan<'_>,
    fade: Option<MotionFade<'_>>,
) -> Vec3 {
    let Some(node) = asset.node_index(node_name) else {
        return Vec3::ZERO;
    };
    let current_delta = motion_span_delta(&asset.nodes, node, current);
    let delta = match fade {
        Some(fade) => {
            // 混“本帧各自走了多远”，不能混两条剪辑的根骨绝对坐标：两份资产的
            // 制作原点可能不同，直接拿姿势端点相减会在切换瞬间把原点差当成位移。
            // alpha 在一帧内线性推进，因此区间平均值就是这一帧的平均权重。
            let alpha_from = finite_alpha(fade.alpha_from);
            let alpha_to = finite_alpha(fade.alpha_to);
            let alpha = (alpha_from + alpha_to) * 0.5;
            let old_delta = motion_span_delta(&asset.nodes, node, fade.span);
            old_delta * (1.0 - alpha) + current_delta * alpha
        }
        None => current_delta,
    };
    filter_motion_axes(delta, axes)
}

/// 一条剪辑在原始时间段内贡献的位移。
///
/// 循环剪辑不能先把两端各自取模再相减：跨过循环边界时，那会把“继续向前”算成
/// “瞬移回起点”。这里把每一整圈的 `末帧 - 首帧` 累进一条展开轨迹，再减两端；
/// 因而一帧跨多圈也仍然正确。非循环剪辑只把两端钳在首末帧，播完后自然为零。
fn motion_span_delta(nodes: &[AnimNode], node: usize, span: MotionSpan<'_>) -> Vec3 {
    if !span.from.is_finite()
        || !span.to.is_finite()
        || !span.clip.duration.is_finite()
        || span.clip.duration <= 0.0
    {
        return Vec3::ZERO;
    }
    if !span.looping {
        let from = sample_node_translation(
            nodes,
            span.clip,
            node,
            clip_time(span.from, span.clip.duration, false),
        );
        let to = sample_node_translation(
            nodes,
            span.clip,
            node,
            clip_time(span.to, span.clip.duration, false),
        );
        return to - from;
    }

    let unfolded = |raw_t: f32| {
        let t = raw_t.max(0.0);
        let cycles = (t / span.clip.duration).floor();
        let local_t = t % span.clip.duration;
        let first = sample_node_translation(nodes, span.clip, node, 0.0);
        let last = sample_node_translation(nodes, span.clip, node, span.clip.duration);
        let local = sample_node_translation(nodes, span.clip, node, local_t);
        (last - first) * cycles + (local - first)
    };
    unfolded(span.to) - unfolded(span.from)
}

#[inline]
fn finite_alpha(alpha: f32) -> f32 {
    if alpha.is_finite() {
        alpha.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn sample_node_translation(nodes: &[AnimNode], clip: &AnimClip, node: usize, t: f32) -> Vec3 {
    let Some(default) = nodes.get(node).map(|n| n.translation) else {
        return Vec3::ZERO;
    };
    let mut value = default;
    for track in &clip.tracks {
        if track.node as usize != node
            || track.path != TrackPath::Translation
            || track.times.is_empty()
        {
            continue;
        }
        let (i0, i1, u) = key_segment(&track.times, t);
        if track.readable(i1) {
            value = eval_vec3(track, i0, i1, u, track.times[i1] - track.times[i0]);
        }
    }
    value
}

fn filter_motion_axes(delta: Vec3, axes: [bool; 3]) -> Vec3 {
    Vec3::new(
        if axes[0] { delta.x } else { 0.0 },
        if axes[1] { delta.y } else { 0.0 },
        if axes[2] { delta.z } else { 0.0 },
    )
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
/// 直接层换本层剪辑只归零它自己的时钟（硬切，与叠加层同语义）；机器层换状态则
/// 保留旧剪辑 / 时钟做交叉淡入。`mode` / `mask` / `weight` 都由 tick 跟配置对齐
/// （越界权重钳位），渲染侧只问驱动、不回头读组件。
#[derive(Clone, Debug)]
pub struct LayerClock {
    /// 层名（脚本 `set_anim_layer_weight` 按名查找的就是它）。
    pub name: String,
    /// 本层机器的当前状态；直接剪辑模式恒为空串。
    state: String,
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
    /// 本层自己的交叉淡入；直接剪辑模式不建立它。
    prev: Option<Prev>,
}

impl LayerClock {
    /// 本层状态机的当前状态名（直接剪辑模式返回 `None`）。
    pub fn state(&self) -> Option<&str> {
        (!self.state.is_empty()).then_some(self.state.as_str())
    }

    /// 本层正在进行的交叉淡入。
    pub fn fade_from(&self) -> Option<FadeFrom<'_>> {
        let prev = self.prev.as_ref()?;
        Some(FadeFrom {
            clip: prev.clip.as_str(),
            t: prev.t,
            looping: prev.looping,
            alpha: prev.alpha,
        })
    }
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

/// 基础通道最近一帧的播放区间（根运动消费者用）。
#[derive(Clone, Copy, Debug)]
pub struct MotionPlayback<'a> {
    pub clip: &'a str,
    pub from: f32,
    pub to: f32,
    pub looping: bool,
    pub fade: Option<MotionFadePlayback<'a>>,
}

/// 交叉淡入旧侧在最近一帧的播放区间与新侧权重区间。
#[derive(Clone, Copy, Debug)]
pub struct MotionFadePlayback<'a> {
    pub clip: &'a str,
    pub from: f32,
    pub to: f32,
    pub looping: bool,
    pub alpha_from: f32,
    pub alpha_to: f32,
}

/// 交叉淡入的**来源**剪辑（换剪辑时把旧剪辑连同它的时钟一起记下来）。
#[derive(Clone, Debug)]
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
    /// 本帧推进前的旧剪辑时间 / 淡入权重。根运动必须拿“帧初→帧末”做增量，
    /// 只暴露帧末值会迫使 runtime 猜速度与转移发生时刻。
    frame_from: f32,
    alpha_from: f32,
    /// 这次淡入的**总时长**（秒）。切换那一刻定格：转移自己的 `duration` 与
    /// `Animator::fade_time` 取哪个，是在启动这次淡入时决定的，之后再改配置
    /// 不该把进行中的过渡一把拽完（那会表现为"改了一下参数，角色突然跳帧"）。
    duration: f32,
}

struct Entry {
    /// 状态机当前状态名（单剪辑模式恒为空串）。换状态 = 换剪辑，走同一条淡入路径。
    state: String,
    clip: String,
    t: f32,
    /// 本帧解析后的播放速度 / 循环方式（机器模式下由状态表覆盖 Animator 字段）。
    speed: f32,
    looping: bool,
    /// 本帧推进前的基础剪辑时间。
    frame_from: f32,
    /// `None` = 没有淡入（未切换 / 已完成 / 硬切）。
    prev: Option<Prev>,
    /// 淡入恰好在本帧结束时完成时，把旧侧保留到下一次 tick 开始。
    /// `fade_from()` 仍按历史语义立即返回 None，但根运动不会丢掉最后一帧的旧增量。
    frame_prev: Option<Prev>,
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

/// 把一层配置刷新进既有时钟。
///
/// 稳态只比较 / 原位改值，不复制字符串；状态、剪辑或遮罩真的变化时才复用各自
/// `String` / `Vec` 的既有容量。`existed=false` 表示新层，不能凭空建立淡入旧侧。
fn sync_layer_clock(clock: &mut LayerClock, cfg: &crate::AnimLayer, autoplay: bool, existed: bool) {
    let (state, clip, speed, looping, fade, machine_mode) = match cfg.active_machine() {
        None => ("", cfg.clip.as_str(), cfg.speed, cfg.looping, 0.0, false),
        Some(machine) => {
            let current = machine
                .state(&clock.state)
                .or_else(|| machine.state(&machine.entry));
            let Some(current) = current else {
                return;
            };
            let transition = machine.transition_from(&current.name, &machine.parameters);
            let next = transition
                .and_then(|transition| machine.state(&transition.to))
                .unwrap_or(current);
            let fade = transition
                .and_then(|transition| transition.duration)
                .filter(|duration| duration.is_finite())
                .map(|duration| duration.max(0.0))
                .unwrap_or(cfg.fade_time);
            (
                next.name.as_str(),
                next.clip.as_str(),
                next.speed,
                next.looping,
                fade,
                true,
            )
        }
    };

    if clock.clip != clip {
        // 只有机器层做交叉淡入；直接层保持历史“换 clip 硬切”。
        clock.prev = if existed
            && machine_mode
            && fade.is_finite()
            && fade > 0.0
            && autoplay
            && cfg.is_active()
        {
            Some(Prev {
                clip: clock.clip.clone(),
                t: clock.t,
                speed: clock.speed,
                looping: clock.looping,
                fade_t: 0.0,
                alpha: 0.0,
                frame_from: clock.t,
                alpha_from: 0.0,
                duration: fade,
            })
        } else {
            None
        };
        clock.clip.clear();
        clock.clip.push_str(clip);
        clock.t = 0.0;
    } else if !machine_mode {
        // 摘掉机器立即回到直接层；不能把机器时代未完的淡入拖过去。
        clock.prev = None;
    }
    if clock.state != state {
        clock.state.clear();
        clock.state.push_str(state);
    }
    clock.speed = speed;
    clock.looping = looping;
    clock.weight = cfg.weight.clamp(0.0, 1.0);
    clock.mode = cfg.mode;
    if clock.mask != cfg.mask {
        clock.mask.clone_from(&cfg.mask);
    }
}

/// 每个实体的动画播放时钟（clip + 已累计时间）。
///
/// 运行态时钟由 `xgame-runtime::World` 唯一持有；编辑态没有 `World::update`，所以
/// `SceneRenderer` 另有一只只服务 Edit 预览的墙钟。`ScenePipeline` 始终纯绘制：
/// `render()` 会受静帧指纹门控，若让管线自己推进会形成
/// “不出帧 → 时间不走 → 指纹不变 → 更不出帧”的死锁。
pub struct AnimationDriver {
    entries: HashMap<EntityId, Entry>,
    /// `tick` 的存活集合暂存。每帧清空内容但保留桶，稳态不重新分配。
    live_entries: HashSet<EntityId>,
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
            live_entries: HashSet::new(),
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
    /// - 只为**带 [`Animator`](crate::Animator) 的实体**建条目；
    ///   `autoplay = false` 也建（时间停在 0），于是首帧能画出剪辑第 0 帧的姿势，
    ///   而不是退回绑定姿态 —— "摆好第一帧再播"是正常的制作流程。
    /// - 有状态机时先**求值一步转移**（校验当前状态还在表内 → 查一条命中的转移），
    ///   得到本帧的 (状态, 剪辑, 速度, 循环)；没有状态机就取 Animator 的
    ///   clip/speed/looping。求值每 tick 都做，暂停中改参数也立刻换姿势
    ///   （与单剪辑模式"暂停中改 clip 硬切"同一语义）。
    /// - 剪辑名变了 → 时间归零（换剪辑从头播）。淡入时长 > 0 且**该通道**当时正在播放 →
    ///   记下旧剪辑（连同它的时钟、速度、循环方式）做交叉淡入，见
    ///   [`fade_from`](Self::fade_from)；暂停中切换 / `fade_time = 0` → 硬切。
    ///   换状态与换剪辑走同一条路径 —— 状态机的转移就是一次换剪辑。autoplay 是
    ///   总闸，但基础 / additive / 各层的活跃判据独立，不能互相唤醒冻结时钟。
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
            // autoplay 是所有通道共用的总闸，但“能不能推进”必须逐通道判断。
            // `Animator::is_active()` 的语义已经扩成“任一通道活着”：拿它控制
            // 基础淡入会让一条活跃上身层把 speed=0 的基础层也唤醒。
            let base_active = animator.autoplay && animator.base_is_active();
            let additive_active = animator.autoplay
                && animator
                    .additive
                    .as_ref()
                    .is_some_and(|layer| !layer.clip.is_empty() && layer.speed > 0.0);
            // -- 1. 解析本帧目标播放 --
            //
            // 机器模式：当前状态必须还在表里（配置被 RON 改坏 → 退回 entry），
            // 然后求值一步转移（每 tick 最多走一条边，不链跳）。
            let (state, clip, speed, looping, fade) = match animator.active_machine() {
                None => (
                    "",
                    animator.clip.as_str(),
                    animator.speed,
                    animator.looping,
                    animator.fade_time,
                ),
                Some(m) => {
                    // `current` 必须重新落回配置表里的状态引用，不能让下面的 `state`
                    // 借自 `self.entries`：条目马上要可变借用。顺带让稳态不复制名字。
                    let current = self
                        .entries
                        .get(&entity.id)
                        .and_then(|e| m.state(&e.state))
                        .or_else(|| m.state(&m.entry));
                    let Some(current) = current else {
                        continue;
                    };
                    let transition = m.transition_from(&current.name, &m.parameters);
                    let next = transition.and_then(|t| m.state(&t.to)).unwrap_or(current);
                    // 过渡时长逐条覆盖：转移自己写了的用它，没写退回全局。
                    let fade = transition
                        .and_then(|t| t.duration)
                        .filter(|d| d.is_finite())
                        .map(|d| d.max(0.0))
                        .unwrap_or(animator.fade_time);
                    (
                        next.name.as_str(),
                        next.clip.as_str(),
                        next.speed,
                        next.looping,
                        fade,
                    )
                }
            };
            // -- 2. 条目（首建直接落解析值，不触发假淡入）--
            let entry = self.entries.entry(entity.id).or_insert_with(|| Entry {
                state: state.to_owned(),
                clip: clip.to_owned(),
                t: 0.0,
                speed,
                looping,
                frame_from: 0.0,
                prev: None,
                frame_prev: None,
                additive_clip: String::new(),
                additive_t: 0.0,
                additive_speed: 1.0,
                additive_looping: true,
                additive_weight: 0.0,
                layer_clocks: Vec::new(),
            });
            // 上一帧仅供根运动读取的淡出快照到这里失效；本帧若又完成淡入，下面
            // 会把新的旧侧移进来。
            entry.frame_prev = None;
            // -- 3. 换剪辑 → 交叉淡入 --
            if entry.clip != clip {
                entry.prev = if fade > 0.0 && base_active {
                    Some(Prev {
                        clip: entry.clip.clone(),
                        t: entry.t,
                        // 旧剪辑的速度/循环在**切换时刻**定格：淡出的那半边按它
                        // 自己的参数走，不被新状态带偏。
                        speed: entry.speed,
                        looping: entry.looping,
                        fade_t: 0.0,
                        alpha: 0.0,
                        frame_from: entry.t,
                        alpha_from: 0.0,
                        duration: fade,
                    })
                } else {
                    None
                };
                entry.clip.clear();
                entry.clip.push_str(clip);
                entry.t = 0.0;
            }
            // -- 4. 速度/循环/状态刷新（必须在换剪辑比较之后：Prev 记的是旧值）--
            if entry.state != state {
                entry.state.clear();
                entry.state.push_str(state);
            }
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
            // 直接剪辑层换 clip 仍然硬切；配了 machine 的层则独立求值一步转移，
            // 用本层自己的状态 / 时钟 / 淡入。两台层机器不共享 current state，
            // 否则“上半身开火”会把“腿部跑步”的状态一起顶掉。
            let layer_layout_unchanged = entry.layer_clocks.len() == animator.layers.len()
                && entry
                    .layer_clocks
                    .iter()
                    .zip(&animator.layers)
                    .all(|(clock, cfg)| clock.name == cfg.name);
            if layer_layout_unchanged {
                // 稳态快路径：原位刷新，字符串与 Vec 容量全部复用。只有作者真的换
                // 状态 / 剪辑 / 遮罩时才允许分配。
                for (clock, cfg) in entry.layer_clocks.iter_mut().zip(&animator.layers) {
                    sync_layer_clock(clock, cfg, animator.autoplay, true);
                }
            } else {
                // 结构变化（增删 / 重排 / 改名）不是逐帧热路径：按名字认回旧钟，
                // 重建一次顺序，之后立即回到上面的零分配快路径。
                let old = std::mem::take(&mut entry.layer_clocks);
                let mut old = old;
                let mut rebuilt = Vec::with_capacity(animator.layers.len());
                for cfg in &animator.layers {
                    let previous = old
                        .iter()
                        .position(|clock| clock.name == cfg.name)
                        .map(|index| old.remove(index));
                    let existed = previous.is_some();
                    let mut clock = previous.unwrap_or_else(|| LayerClock {
                        name: cfg.name.clone(),
                        state: String::new(),
                        clip: String::new(),
                        t: 0.0,
                        speed: cfg.speed,
                        looping: cfg.looping,
                        weight: 0.0,
                        mode: cfg.mode,
                        mask: Vec::new(),
                        prev: None,
                    });
                    sync_layer_clock(&mut clock, cfg, animator.autoplay, existed);
                    rebuilt.push(clock);
                }
                entry.layer_clocks = rebuilt;
            }
            // -- 5. 推进时钟 --
            entry.frame_from = entry.t;
            if let Some(prev) = entry.prev.as_mut() {
                prev.frame_from = prev.t;
                prev.alpha_from = prev.alpha;
            }
            if base_active {
                // 负速在 v1 是"静止"而不是倒放（与单剪辑语义一致）：状态表里
                // 写了负速度 → 该状态定格，但转移照常求值，机器不会卡死。
                entry.t += dt * entry.speed.max(0.0);
                let mut fade_done = false;
                if let Some(prev) = entry.prev.as_mut() {
                    // 旧剪辑的时钟也继续走 —— 淡出的那半边若定格，混合出来的
                    // 姿势会在过渡中"粘住"一下，看起来像卡帧。
                    prev.t += dt * prev.speed;
                    prev.fade_t += dt;
                    // 除数用**这次过渡自己的**时长（切换时刻定格），不是当前的
                    // `animator.fade_time`：编辑期改全局淡入时长不该把进行中的
                    // 过渡一把拽完。
                    prev.alpha = if prev.duration > 0.0 {
                        (prev.fade_t / prev.duration).min(1.0)
                    } else {
                        1.0
                    };
                    fade_done = prev.duration <= 0.0 || prev.fade_t >= prev.duration;
                }
                if fade_done {
                    entry.frame_prev = entry.prev.take();
                }
            }
            // additive 只与基础层共享 autoplay，不共享“基础层是否能走”的判据。
            if additive_active {
                entry.additive_t += dt * entry.additive_speed.max(0.0);
            }
            // 每个混合层也独立判断：一个活跃层不能替另一个冻结层推进时钟或淡入。
            // `layer_clocks` 刚按配置顺序重建，zip 在这里是一一对应的。
            for (clock, cfg) in entry.layer_clocks.iter_mut().zip(&animator.layers) {
                if !animator.autoplay || !cfg.is_active() {
                    continue;
                }
                if !clock.clip.is_empty() {
                    clock.t += dt * clock.speed.max(0.0);
                }
                let mut layer_fade_done = false;
                if let Some(prev) = clock.prev.as_mut() {
                    prev.t += dt * prev.speed.max(0.0);
                    prev.fade_t += dt;
                    prev.alpha = if prev.duration > 0.0 {
                        (prev.fade_t / prev.duration).min(1.0)
                    } else {
                        1.0
                    };
                    layer_fade_done = prev.duration <= 0.0 || prev.fade_t >= prev.duration;
                }
                if layer_fade_done {
                    clock.prev = None;
                }
            }
        }
        self.live_entries.clear();
        self.live_entries.extend(
            entities
                .iter()
                .filter(|entity| entity.animator.is_some())
                .map(|entity| entity.id),
        );
        self.entries.retain(|id, _| self.live_entries.contains(id));
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
        self.wall_time_scale = scale.clamp(0.0, crate::MAX_TIME_SCALE);
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

    /// 某实体**指定混合层**状态机的当前状态名。
    ///
    /// 层名重名时与脚本 setter 同语义：按栈顺序取第一个。直接剪辑层、坏机器、
    /// 还没 tick 过或层名不存在都返回 `None`。
    pub fn layer_state(&self, id: EntityId, layer: &str) -> Option<&str> {
        self.entries
            .get(&id)?
            .layer_clocks
            .iter()
            .find(|clock| clock.name == layer)?
            .state()
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

    /// 某实体在**最近一次 tick** 里基础通道实际走过的时间段。
    ///
    /// 与 [`playback`](Self::playback) 的区别：后者只给帧末采样点；本函数同时给
    /// 帧初/帧末，并在交叉淡入最后一帧保留旧侧与 alpha 区间，供 runtime 提取
    /// 根运动。附加层与层栈刻意不在这里，它们不参与根运动。
    pub fn motion_frame(&self, id: EntityId) -> Option<MotionPlayback<'_>> {
        let entry = self.entries.get(&id)?;
        let prev = entry.prev.as_ref().or(entry.frame_prev.as_ref());
        Some(MotionPlayback {
            clip: entry.clip.as_str(),
            from: entry.frame_from,
            to: entry.t,
            looping: entry.looping,
            fade: prev.map(|prev| MotionFadePlayback {
                clip: prev.clip.as_str(),
                from: prev.frame_from,
                to: prev.t,
                looping: prev.looping,
                alpha_from: prev.alpha_from,
                alpha_to: prev.alpha,
            }),
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
                let mut packed = ((e.t.to_bits() as u64) << 32) | e.additive_t.to_bits() as u64;
                // 层栈时钟按配置顺序折进来（splitmix64 是双射，任一层在走都会变）。
                for l in &e.layer_clocks {
                    packed = mix(packed, l.t.to_bits() as u64);
                    if let Some(prev) = &l.prev {
                        packed = mix(packed, prev.alpha.to_bits() as u64);
                    }
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
        self.live_entries.clear();
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
    use crate::Animator;

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
        assert!(
            rotated.z < 0.0,
            "绕 +Y 转 45° 后 +X 应有 −z 分量，实得 {rotated:?}"
        );
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
        assert!(
            (pose[0].scale.x - 2.0).abs() < 1e-6,
            "没被瞄准的缩放通道必须保持默认"
        );
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
        assert!(
            (pos - Vec3::new(3.0, 5.0, 0.0)).length() < 1e-6,
            "实得 {pos:?}"
        );
    }

    /// 成环的畸形资产不能栈溢出（超深按根处理）。
    #[test]
    fn cyclic_parents_degrade_instead_of_overflowing() {
        let nodes = vec![
            AnimNode {
                parent: Some(1),
                name: String::new(),
                translation: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            AnimNode {
                parent: Some(0),
                name: String::new(),
                translation: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        ];
        let pose = default_pose(&nodes);
        let g = global_matrices(&nodes, &pose);
        assert!(
            g.iter().all(|m| m.w_axis.is_finite()),
            "成环也必须给出有限矩阵"
        );
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
        let at = |alpha: f32| {
            compute_bones_blended(&nodes, &a, 1.0, &b, 1.0, alpha, &skin)[0]
                .w_axis
                .truncate()
        };

        let mid = at(0.5);
        assert!(
            (mid - Vec3::new(1.0, 2.0, 0.0)).length() < 1e-5,
            "alpha=0.5 应是两姿势的中点，实得 {mid:?}"
        );
        let only_a = at(0.0);
        assert!(
            (only_a - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-5,
            "alpha=0 应是纯旧剪辑"
        );
        let only_b = at(1.0);
        assert!(
            (only_b - Vec3::new(0.0, 4.0, 0.0)).length() < 1e-5,
            "alpha=1 应是纯新剪辑"
        );
        let clamped = at(9.0);
        assert!(
            (clamped - only_b).length() < 1e-5,
            "alpha 越界应钳在纯新剪辑"
        );
    }

    // -- 时间归一 --

    #[test]
    fn clip_time_loops_wraps_and_oneshot_clamps() {
        assert!((clip_time(1.5, 1.0, true) - 0.5).abs() < 1e-6, "循环应取模");
        assert!(
            (clip_time(9.0, 1.0, true) - 0.0).abs() < 1e-6,
            "整圈应绕回 0"
        );
        assert!(
            (clip_time(9.0, 1.0, false) - 1.0).abs() < 1e-6,
            "非循环应钳在末帧"
        );
        assert_eq!(clip_time(f32::NAN, 1.0, true), 0.0, "NaN 不该扩散进采样");
        assert_eq!(clip_time(-1.0, 1.0, false), 0.0, "负时间钳在 0");
        assert_eq!(
            clip_time(0.5, 0.0, true),
            0.0,
            "零时长剪辑恒为 0（不产生 NaN）"
        );
    }

    fn motion_asset(clips: Vec<AnimClip>) -> AnimationAsset {
        AnimationAsset {
            nodes: vec![AnimNode {
                parent: None,
                name: "MotionRoot".into(),
                translation: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            }],
            clips,
        }
    }

    fn move_clip(name: &str, end: Vec3) -> AnimClip {
        AnimClip {
            name: name.into(),
            duration: 1.0,
            tracks: vec![Track {
                node: 0,
                path: TrackPath::Translation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![0.0, 0.0, 0.0, end.x, end.y, end.z],
            }],
        }
    }

    /// 普通推进按“本帧位移差”提取，并在最后一步做逐轴过滤。
    #[test]
    fn root_motion_extracts_frame_delta_and_filters_axes() {
        let asset = motion_asset(vec![move_clip("Walk", Vec3::new(4.0, 2.0, 6.0))]);
        let delta = root_motion_delta(
            &asset,
            "MotionRoot",
            [true, false, true],
            MotionSpan {
                clip: &asset.clips[0],
                from: 0.25,
                to: 0.75,
                looping: true,
            },
            None,
        );
        assert!((delta - Vec3::new(2.0, 0.0, 3.0)).length() < 1e-6);
    }

    /// 循环跨界不能拿“取模后的末点 − 起点”：0.75→1.25 应继续向前半圈，
    /// 不是从 75% 瞬移回 25%。这条在旧的两端直接相减算法上会得到 -2。
    #[test]
    fn root_motion_loop_wrap_keeps_moving_forward() {
        let asset = motion_asset(vec![move_clip("Walk", Vec3::new(4.0, 0.0, 0.0))]);
        let delta = root_motion_delta(
            &asset,
            "MotionRoot",
            [true; 3],
            MotionSpan {
                clip: &asset.clips[0],
                from: 0.75,
                to: 1.25,
                looping: true,
            },
            None,
        );
        assert!((delta.x - 2.0).abs() < 1e-6, "跨界应向前 2，实际 {delta:?}");
    }

    /// 卡顿帧可能一次跨过不止一圈，不能只特判“是否跨过一次边界”。
    #[test]
    fn root_motion_loop_span_counts_every_full_cycle() {
        let asset = motion_asset(vec![move_clip("Walk", Vec3::new(4.0, 0.0, 0.0))]);
        let delta = root_motion_delta(
            &asset,
            "MotionRoot",
            [true; 3],
            MotionSpan {
                clip: &asset.clips[0],
                from: 0.25,
                to: 2.75,
                looping: true,
            },
            None,
        );
        assert!(
            (delta.x - 10.0).abs() < 1e-6,
            "2.5 圈应前进 10，实际 {delta:?}"
        );
    }

    /// 非循环剪辑到末帧后不再重复吃末帧绝对坐标。
    #[test]
    fn root_motion_one_shot_stops_at_the_end() {
        let asset = motion_asset(vec![move_clip("Walk", Vec3::new(4.0, 0.0, 0.0))]);
        let span = |from, to| MotionSpan {
            clip: &asset.clips[0],
            from,
            to,
            looping: false,
        };
        assert!(
            (root_motion_delta(&asset, "MotionRoot", [true; 3], span(0.75, 1.25), None).x - 1.0)
                .abs()
                < 1e-6
        );
        assert_eq!(
            root_motion_delta(&asset, "MotionRoot", [true; 3], span(1.25, 2.0), None),
            Vec3::ZERO
        );
    }

    /// runtime 已消费的根位移必须从基础骨架姿势剥掉，否则实体和骨头各走一次。
    #[test]
    fn composed_pose_strips_consumed_root_motion_axes() {
        let nodes = vec![AnimNode {
            parent: None,
            name: "MotionRoot".into(),
            translation: Vec3::new(0.0, 3.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }];
        let skin = Skin {
            joints: vec![0],
            inverse_bind: vec![Mat4::IDENTITY],
        };
        let clip = move_clip("Walk", Vec3::new(4.0, 5.0, 6.0));
        let plain = compute_bones_composed(&nodes, &skin, (&clip, 0.5), None, &[])[0]
            .w_axis
            .truncate();
        let stripped = compute_bones_composed_with_root_motion(
            &nodes,
            &skin,
            (&clip, 0.5),
            None,
            &[],
            Some(("MotionRoot", [true, false, true])),
        )[0]
        .w_axis
        .truncate();
        assert_eq!(plain, Vec3::new(2.0, 2.5, 3.0));
        assert_eq!(
            stripped,
            Vec3::new(0.0, 2.5, 0.0),
            "只剥已交给 runtime 的 X/Z；Y 仍留在骨架姿势"
        );
    }

    /// 淡入混合的是两条剪辑各自的**本帧增量**，不是根骨绝对坐标。
    /// alpha 0.2→0.6 的帧取区间平均 0.4：旧 X 1m ×0.6，新 Z 2m ×0.4。
    #[test]
    fn root_motion_crossfade_blends_clip_deltas() {
        let asset = motion_asset(vec![
            move_clip("Walk", Vec3::new(2.0, 0.0, 0.0)),
            move_clip("Run", Vec3::new(0.0, 0.0, 4.0)),
        ]);
        let delta = root_motion_delta(
            &asset,
            "MotionRoot",
            [true; 3],
            MotionSpan {
                clip: &asset.clips[1],
                from: 0.0,
                to: 0.5,
                looping: true,
            },
            Some(MotionFade {
                span: MotionSpan {
                    clip: &asset.clips[0],
                    from: 0.25,
                    to: 0.75,
                    looping: true,
                },
                alpha_from: 0.2,
                alpha_to: 0.6,
            }),
        );
        assert!(
            (delta - Vec3::new(0.6, 0.0, 0.8)).length() < 1e-6,
            "应按区间平均权重混合两条增量，实际 {delta:?}"
        );
    }

    /// 显式节点名是硬门槛：不猜第一个根节点，也不把匿名包装根当运动根。
    #[test]
    fn root_motion_missing_node_is_zero() {
        let asset = motion_asset(vec![move_clip("Walk", Vec3::X)]);
        assert_eq!(
            root_motion_delta(
                &asset,
                "NoSuchNode",
                [true; 3],
                MotionSpan {
                    clip: &asset.clips[0],
                    from: 0.0,
                    to: 1.0,
                    looping: false,
                },
                None,
            ),
            Vec3::ZERO
        );
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
        assert!(
            (d.time(id).unwrap() - 0.25).abs() < 1e-6,
            "换剪辑应归零重走"
        );
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
        assert!(
            d.time(e.id).is_some(),
            "条目要留着 —— 画面冻结在当前帧而不是弹回绑定姿态"
        );
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
        assert!(
            (f.alpha - 0.4).abs() < 1e-6,
            "alpha 应是 0.1/0.25，实得 {}",
            f.alpha
        );
        assert!((d.time(id).unwrap() - 0.1).abs() < 1e-6, "新剪辑从 0 起播");

        d.tick(0.2, std::slice::from_ref(&e)); // 累计 0.3 ≥ 0.25
        assert!(d.fade_from(id).is_none(), "淡入完成后应清掉旧剪辑");
        assert!(
            (d.time(id).unwrap() - 0.3).abs() < 1e-6,
            "主时钟不受淡入影响"
        );
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
        assert!(
            (d.time(e.id).unwrap() - 0.1).abs() < 1e-6,
            "硬切后新剪辑从 0 起播"
        );
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
        assert!(
            (d.time(e.id).unwrap()).abs() < 1e-6,
            "切换照旧把新剪辑时钟归零"
        );
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
        assert!(
            (d.time(e.id).unwrap() - 0.1).abs() < 1e-6,
            "暂停中主时钟也该冻结"
        );
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

    use crate::{
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
                    duration: None,
                },
                AnimTransition {
                    from: "Walk".into(),
                    to: "Idle".into(),
                    when: AnimCondition::Below {
                        param: "speed".into(),
                        value: 0.3,
                    },
                    duration: None,
                },
                AnimTransition {
                    from: "Walk".into(),
                    to: "Run".into(),
                    when: AnimCondition::AtLeast {
                        param: "speed".into(),
                        value: 2.0,
                    },
                    duration: None,
                },
                AnimTransition {
                    from: "Run".into(),
                    to: "Walk".into(),
                    when: AnimCondition::Below {
                        param: "speed".into(),
                        value: 1.8,
                    },
                    duration: None,
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
        assert_eq!(
            clip, "Survey",
            "entry 状态的剪辑才是入口（Animator.clip 被覆盖）"
        );
        assert!((t - 0.1).abs() < 1e-6, "按状态速度推进");
        assert!(looping, "循环方式也来自状态表");
    }

    /// 参数跨阈值 → 求值一步转移 → 当帧换剪辑并交叉淡入（复用第二片的淡入机制）。
    #[test]
    fn driver_transitions_on_param_and_fades_to_new_state() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_machine();
        d.tick(0.3, std::slice::from_ref(&e));

        e.animator
            .as_mut()
            .unwrap()
            .machine
            .as_mut()
            .unwrap()
            .parameters
            .insert("speed".into(), 0.6);
        d.tick(0.1, std::slice::from_ref(&e));

        assert_eq!(d.state(e.id), Some("Walk"), "speed ≥ 0.5 应转 Walk");
        let (clip, t, looping) = d.playback(e.id).unwrap();
        assert_eq!(clip, "Walk", "换状态 = 换剪辑");
        // 转移与新剪辑推进在同一 tick：0.1 × Walk 速度 0.5 = 0.05。
        assert!(
            (t - 0.05).abs() < 1e-6,
            "新剪辑应从 0 起播并当帧推进，实得 {t}"
        );
        assert!(!looping, "新状态是非循环，循环方式也跟着换");
        let f = d
            .fade_from(e.id)
            .expect("换状态应交叉淡入（复用换剪辑机制）");
        assert_eq!(f.clip, "Survey", "应从旧状态的剪辑淡出");
        assert!(f.looping, "旧剪辑按**它自己**的循环方式淡出");
    }

    /// 转移自己写的过渡时长优先于 `Animator.fade_time`。
    ///
    /// 全局一个数不可能同时表达"走路→跑步慢慢混"和"受击→僵直立刻断"。
    #[test]
    fn transition_duration_overrides_the_global_fade() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_machine();
        {
            let a = e.animator.as_mut().unwrap();
            a.fade_time = 0.25; // 全局淡入
                                // Idle → Walk 这条自己要求 1.0 秒
            a.machine.as_mut().unwrap().transitions[0].duration = Some(1.0);
        }
        d.tick(0.3, std::slice::from_ref(&e));
        e.animator
            .as_mut()
            .unwrap()
            .machine
            .as_mut()
            .unwrap()
            .parameters
            .insert("speed".into(), 0.6);

        // 转移发生 + 已淡入 0.5 秒：进度必须是 0.5/1.0，而按全局 0.25 早该结束了。
        d.tick(0.5, std::slice::from_ref(&e));
        let f = d.fade_from(e.id).expect("1 秒的过渡在半秒时仍在进行");
        assert!(
            (f.alpha - 0.5).abs() < 1e-5,
            "淡入进度应按转移自己的 1.0 秒算，实得 {}",
            f.alpha
        );

        d.tick(0.5, std::slice::from_ref(&e));
        assert!(d.fade_from(e.id).is_none(), "满 1 秒后过渡必须结束");
    }

    /// 转移没写时长 → 退回 `Animator.fade_time`（老行为，逐位一致）。
    #[test]
    fn transition_without_duration_uses_the_global_fade() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_machine();
        e.animator.as_mut().unwrap().fade_time = 0.5;
        d.tick(0.3, std::slice::from_ref(&e));
        e.animator
            .as_mut()
            .unwrap()
            .machine
            .as_mut()
            .unwrap()
            .parameters
            .insert("speed".into(), 0.6);
        d.tick(0.25, std::slice::from_ref(&e));
        let f = d.fade_from(e.id).expect("进行中");
        assert!(
            (f.alpha - 0.5).abs() < 1e-5,
            "缺省时按全局 0.5 秒算，实得 {}",
            f.alpha
        );
    }

    /// `duration: 0` = 硬切：当帧换剪辑，没有交叉淡入。
    #[test]
    fn zero_transition_duration_is_a_hard_cut() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_machine();
        e.animator
            .as_mut()
            .unwrap()
            .machine
            .as_mut()
            .unwrap()
            .transitions[0]
            .duration = Some(0.0);
        d.tick(0.3, std::slice::from_ref(&e));
        e.animator
            .as_mut()
            .unwrap()
            .machine
            .as_mut()
            .unwrap()
            .parameters
            .insert("speed".into(), 0.6);
        d.tick(0.1, std::slice::from_ref(&e));
        assert_eq!(d.state(e.id), Some("Walk"));
        assert!(
            d.fade_from(e.id).is_none(),
            "duration = 0 是硬切，不该留下淡入状态"
        );
    }

    /// 状态表的 speed 覆盖 Animator.speed：Walk 状态半速 → 时钟按 dt×0.5 走。
    #[test]
    fn driver_uses_state_speed_and_looping_over_animator_fields() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_machine();
        // 直接从 Walk 起步（半速、非循环）；参数停在滞回区间 0.3~0.5 内，
        // 保证首帧不会被转移带走。
        e.animator.as_mut().unwrap().machine.as_mut().unwrap().entry = "Walk".into();
        e.animator
            .as_mut()
            .unwrap()
            .machine
            .as_mut()
            .unwrap()
            .parameters
            .insert("speed".into(), 0.4);
        e.animator.as_mut().unwrap().speed = 7.0; // 单剪辑速度，必须被无视

        d.tick(0.4, std::slice::from_ref(&e));
        let (_, t, looping) = d.playback(e.id).unwrap();
        assert!(
            (t - 0.2).abs() < 1e-6,
            "应按状态速度 0.5×0.4=0.2 走，实得 {t}"
        );
        assert!(!looping, "非循环状态应报非循环");
    }

    /// entry 状态冻住（速度 0）但转移仍要被求值 —— 否则冻结状态永远出不来。
    #[test]
    fn driver_evaluates_transitions_even_from_frozen_state() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_machine();
        // 把 Idle 冻住（speed 0），但机器还有活状态 → is_active 仍为真。
        e.animator
            .as_mut()
            .unwrap()
            .machine
            .as_mut()
            .unwrap()
            .states[0]
            .speed = 0.0;
        d.tick(0.5, std::slice::from_ref(&e));
        let (_, t, _) = d.playback(e.id).unwrap();
        assert!(t.abs() < 1e-6, "冻结状态时钟不该走");

        e.animator
            .as_mut()
            .unwrap()
            .machine
            .as_mut()
            .unwrap()
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
        assert!(
            (at(1.0) - Vec3::new(1.0, 5.0, 0.0)).length() < 1e-6,
            "全额增量"
        );
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
            assert!(
                (angle - want).abs() < 1e-3,
                "w={w} 应转 {want} rad，实得 {angle}"
            );
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
        let combined = compute_bones_blended_additive(
            &nodes, &old, 0.5, &new, 0.5, 0.0, &skin, &add, 0.5, 1.0,
        );
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
        assert!(
            (a.t - 1.0).abs() < 1e-6,
            "按叠加层自己的速度 2.0 推进，实得 {}",
            a.t
        );
        assert!((a.weight - 0.6).abs() < 1e-6);
        let (clip, t, _) = d.playback(e.id).unwrap();
        assert_eq!(clip, "Walk", "基础层不受叠加层影响");
        assert!((t - 0.5).abs() < 1e-6);

        e.animator
            .as_mut()
            .unwrap()
            .additive
            .as_mut()
            .unwrap()
            .weight = 7.0;
        d.tick(0.0, std::slice::from_ref(&e));
        assert!(
            (d.additive(e.id).unwrap().weight - 1.0).abs() < 1e-6,
            "权重应钳在 1.0"
        );

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
        assert!(
            (a.t - 0.2).abs() < 1e-6,
            "换叠加剪辑应从 0 重播（0.1×2.0），实得 {}",
            a.t
        );
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
        e.animator
            .as_mut()
            .unwrap()
            .machine
            .as_mut()
            .unwrap()
            .states[0]
            .speed = 0.0;
        e.animator.as_mut().unwrap().additive = Some(AnimAdditive {
            clip: "Run".into(),
            ..Default::default()
        });

        d.tick(0.5, std::slice::from_ref(&e));
        let (_, base_t, _) = d.playback(e.id).unwrap();
        assert!(base_t.abs() < 1e-6, "冻结状态基础时钟不该走");
        assert!(
            (d.additive(e.id).unwrap().t - 0.5).abs() < 1e-6,
            "叠加层时钟要继续走"
        );
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
        assert!(
            !mask_covers(&nodes, 0, &["Child".into()]),
            "只列子 → 父不被覆盖"
        );
        assert!(
            mask_covers(&nodes, 1, &["Root".into()]),
            "祖先命中 → 子孙一并覆盖"
        );
        assert!(!mask_covers(&nodes, 1, &["Nope".into()]), "没命中不覆盖");

        // 匿名节点（名字空串）不能靠空名单项互认进来。
        let anon = vec![root_node()];
        assert!(
            !mask_covers(&anon, 0, &["".into()]),
            "匿名节点不与空串名单项互认"
        );

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
        assert!(
            mask_covers(&cycle, 0, &["B".into()]),
            "环里沿父链也要能命中"
        );
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
                    fade: None,
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
                fade: None,
                weight: 1.0,
                mode: AnimLayerMode::Additive,
                mask: &mask_child,
            }],
        );
        assert!(
            (on[1].w_axis.truncate() - Vec3::new(0.0, 4.0, 0.0)).length() < 1e-6,
            "名单内的骨吃到增量"
        );
        assert!(on[0].w_axis.truncate().length() < 1e-6, "名单外的骨不动");

        let off = compute_bones_composed(
            &nodes,
            &skin,
            (&base, 0.0),
            None,
            &[LayerInput {
                clip: &inc,
                t: 1.0,
                fade: None,
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
                    fade: None,
                    weight: 1.0,
                    mode: AnimLayerMode::Override,
                    mask: &mask,
                },
                LayerInput {
                    clip: &second,
                    t: 1.0,
                    fade: None,
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

    #[test]
    fn driver_motion_frame_reports_exact_tick_interval() {
        let mut driver = AnimationDriver::new();
        let entity = entity_with_animator("Walk");
        driver.tick(0.25, std::slice::from_ref(&entity));
        let first = driver.motion_frame(entity.id).unwrap();
        assert!((first.from - 0.0).abs() < 1e-6);
        assert!((first.to - 0.25).abs() < 1e-6);

        driver.tick(0.5, std::slice::from_ref(&entity));
        let second = driver.motion_frame(entity.id).unwrap();
        assert!((second.from - 0.25).abs() < 1e-6);
        assert!((second.to - 0.75).abs() < 1e-6);
    }

    /// 淡入恰好在本帧结束时，渲染已可退到纯新剪辑，但根运动还要消费这一帧旧侧。
    #[test]
    fn driver_motion_frame_keeps_the_last_fade_interval() {
        let mut driver = AnimationDriver::new();
        let mut entity = entity_with_animator("Walk");
        driver.tick(0.1, std::slice::from_ref(&entity));
        entity.animator.as_mut().unwrap().clip = "Run".into();
        driver.tick(0.25, std::slice::from_ref(&entity));

        assert!(driver.fade_from(entity.id).is_none(), "姿势淡入应已完成");
        let fade = driver
            .motion_frame(entity.id)
            .unwrap()
            .fade
            .expect("根运动必须保留完成帧的旧侧");
        assert_eq!(fade.clip, "Walk");
        assert!((fade.from - 0.1).abs() < 1e-6);
        assert!((fade.to - 0.35).abs() < 1e-6);
        assert!((fade.alpha_from - 0.0).abs() < 1e-6);
        assert!((fade.alpha_to - 1.0).abs() < 1e-6);

        driver.tick(0.1, std::slice::from_ref(&entity));
        assert!(driver.motion_frame(entity.id).unwrap().fade.is_none());
    }

    /// 分层状态机的淡入必须真的进入姿势合成，而不是只让驱动报告一个 alpha。
    #[test]
    fn override_layer_fade_blends_old_and_new_layer_poses() {
        let nodes = root_child_nodes();
        let base = move_x_clip();
        let old = translate_clip("Aim", 1, Vec3::new(0.0, 4.0, 0.0));
        let new = translate_clip("Fire", 1, Vec3::new(0.0, 0.0, 6.0));
        let skin = flat_skin(2);
        let mask = vec!["Child".to_string()];

        let bones = compute_bones_composed(
            &nodes,
            &skin,
            (&base, 0.0),
            None,
            &[LayerInput {
                clip: &new,
                t: 1.0,
                fade: Some((&old, 1.0, 0.5)),
                weight: 1.0,
                mode: AnimLayerMode::Override,
                mask: &mask,
            }],
        );
        assert!(
            (bones[1].w_axis.truncate() - Vec3::new(0.0, 2.0, 3.0)).length() < 1e-6,
            "旧层姿势 (0,4,0) 与新层姿势 (0,0,6) 半混应为 (0,2,3)，实得 {:?}",
            bones[1].w_axis.truncate()
        );
        assert!(
            bones[0].w_axis.truncate().length() < 1e-6,
            "层淡入仍必须服从骨骼遮罩"
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

    fn layer_machine(name: &str, idle: &str, active: &str, param: &str) -> AnimLayer {
        AnimLayer {
            name: name.into(),
            mode: AnimLayerMode::Override,
            weight: 1.0,
            fade_time: 1.0,
            machine: Some(AnimMachine {
                entry: "Idle".into(),
                parameters: std::collections::BTreeMap::from([(param.to_string(), 0.0)]),
                states: vec![
                    AnimState {
                        name: "Idle".into(),
                        clip: idle.into(),
                        speed: 1.0,
                        looping: true,
                    },
                    AnimState {
                        name: "Active".into(),
                        clip: active.into(),
                        speed: 2.0,
                        looping: false,
                    },
                ],
                transitions: vec![AnimTransition {
                    from: "Idle".into(),
                    to: "Active".into(),
                    when: AnimCondition::AtLeast {
                        param: param.into(),
                        value: 0.5,
                    },
                    duration: Some(0.5),
                }],
            }),
            ..Default::default()
        }
    }

    /// 两个身体部位各跑一台机器：参数、状态、时钟、淡入都不能串台。
    #[test]
    fn layered_machines_keep_independent_state_clocks_and_fades() {
        let mut driver = AnimationDriver::new();
        let mut entity = entity_with_animator("Walk");
        let id = entity.id;
        {
            let animator = entity.animator.as_mut().unwrap();
            animator.layers = vec![
                layer_machine("upper", "Aim", "Fire", "fire"),
                layer_machine("face", "Calm", "Blink", "blink"),
            ];
        }

        driver.tick(0.25, std::slice::from_ref(&entity));
        assert_eq!(driver.layer_state(id, "upper"), Some("Idle"));
        assert_eq!(driver.layer_state(id, "face"), Some("Idle"));
        assert!((driver.layers(id)[0].t - 0.25).abs() < 1e-6);
        assert!((driver.layers(id)[1].t - 0.25).abs() < 1e-6);

        // 只推 upper.fire；face 的机器参数与状态保持原样。
        entity.animator.as_mut().unwrap().layers[0]
            .machine
            .as_mut()
            .unwrap()
            .parameters
            .insert("fire".into(), 1.0);
        driver.tick(0.25, std::slice::from_ref(&entity));

        assert_eq!(driver.layer_state(id, "upper"), Some("Active"));
        assert_eq!(driver.layer_state(id, "face"), Some("Idle"));
        assert_eq!(driver.layers(id)[0].clip, "Fire");
        assert_eq!(driver.layers(id)[1].clip, "Calm");
        assert!(
            (driver.layers(id)[0].t - 0.5).abs() < 1e-6,
            "Active 自己是 2x，转移当帧应推进 0.25×2"
        );
        assert!(
            (driver.layers(id)[1].t - 0.5).abs() < 1e-6,
            "face 继续按自己的 1x 时钟走"
        );
        let fade = driver.layers(id)[0].fade_from().expect("upper 应独立淡入");
        assert_eq!(fade.clip, "Aim");
        assert!((fade.alpha - 0.5).abs() < 1e-6, "逐条 0.5s 已走 0.25s");
        assert!(
            driver.layers(id)[1].fade_from().is_none(),
            "没转移的 face 不能被 upper 的淡入污染"
        );
        assert_eq!(
            driver.state(id),
            None,
            "基础层没有机器，层状态不能冒充基础状态"
        );

        driver.tick(0.25, std::slice::from_ref(&entity));
        assert!(
            driver.layers(id)[0].fade_from().is_none(),
            "本层逐条时长走满后只清自己的淡入"
        );
    }

    /// 基础剪辑冻结时，只要某个层机器能走，Animator 总闸仍应保持活跃。
    #[test]
    fn active_layer_machine_can_drive_a_frozen_base() {
        let mut driver = AnimationDriver::new();
        let mut entity = entity_with_animator("Bind");
        {
            let animator = entity.animator.as_mut().unwrap();
            animator.speed = 0.0;
            animator.layers = vec![layer_machine("upper", "Aim", "Fire", "fire")];
            assert!(animator.is_active(), "活层不能被冻结基础层关掉总闸");
        }
        driver.tick(0.2, std::slice::from_ref(&entity));
        assert_eq!(driver.time(entity.id), Some(0.0), "基础时钟保持冻结");
        assert!(
            (driver.layers(entity.id)[0].t - 0.2).abs() < 1e-6,
            "层机器时钟应独立推进"
        );
    }

    /// “任一通道活着”只用于宿主判断是否持续出帧，不能反过来唤醒冻结的基础层。
    #[test]
    fn active_layer_does_not_make_a_frozen_base_switch_fade() {
        let mut driver = AnimationDriver::new();
        let mut entity = entity_with_animator("Bind");
        {
            let animator = entity.animator.as_mut().unwrap();
            animator.speed = 0.0;
            animator.layers = vec![AnimLayer {
                name: "upper".into(),
                clip: "Aim".into(),
                speed: 1.0,
                ..Default::default()
            }];
        }
        driver.tick(0.2, std::slice::from_ref(&entity));
        entity.animator.as_mut().unwrap().clip = "OtherBind".into();
        driver.tick(0.1, std::slice::from_ref(&entity));

        assert!(
            driver.fade_from(entity.id).is_none(),
            "上身层在播不能让 speed=0 的基础层换剪辑时凭空开始淡入"
        );
        assert_eq!(driver.time(entity.id), Some(0.0), "基础时钟仍应冻结");
        assert!(
            (driver.layers(entity.id)[0].t - 0.3).abs() < 1e-6,
            "活跃层仍按自己的时钟推进"
        );
    }

    /// 淡入已经开始后若这一层整体冻结，活跃的基础层也不能替它推进淡入进度。
    #[test]
    fn active_base_does_not_advance_a_frozen_layer_fade() {
        let mut driver = AnimationDriver::new();
        let mut entity = entity_with_animator("Walk");
        entity.animator.as_mut().unwrap().layers =
            vec![layer_machine("upper", "Aim", "Fire", "fire")];
        driver.tick(0.1, std::slice::from_ref(&entity));
        {
            let machine = entity.animator.as_mut().unwrap().layers[0]
                .machine
                .as_mut()
                .unwrap();
            machine.parameters.insert("fire".into(), 1.0);
        }
        driver.tick(0.1, std::slice::from_ref(&entity));
        let before = driver.layers(entity.id)[0]
            .fade_from()
            .expect("转移应已启动")
            .alpha;

        for state in &mut entity.animator.as_mut().unwrap().layers[0]
            .machine
            .as_mut()
            .unwrap()
            .states
        {
            state.speed = 0.0;
        }
        driver.tick(0.2, std::slice::from_ref(&entity));
        let after = driver.layers(entity.id)[0]
            .fade_from()
            .expect("冻结应保留未完成淡入")
            .alpha;
        assert!(
            (after - before).abs() < 1e-6,
            "基础层在播不能替冻结的 upper 推淡入：{before} -> {after}"
        );
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
            assert!(
                (layers[1].t - 1.0).abs() < 1e-6,
                "legs 按速度 2 推进，实得 {}",
                layers[1].t
            );
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
        assert!((d.layers(e.id)[2].t - 0.1).abs() < 1e-6, "换剪辑归零重播");
        assert!((d.layers(e.id)[1].t - 1.4).abs() < 1e-6, "别的层不受影响");

        // 删层 → prune；权重越界 → 钳 [0,1]（dt=0 只同步不推进）。
        e.animator.as_mut().unwrap().layers.remove(0);
        e.animator.as_mut().unwrap().layers[1].weight = 9.0;
        d.tick(0.0, std::slice::from_ref(&e));
        assert_eq!(d.layers(e.id).len(), 2, "删掉的层不再出现，剩 legs + upper");
        assert_eq!(d.layers(e.id)[1].name, "upper");
        assert!(
            (d.layers(e.id)[1].weight - 1.0).abs() < 1e-6,
            "权重钳在 1.0"
        );
    }

    /// 冻结基础层（当前状态速度 0、别的状态还活）+ 活层栈：层时钟照走且指纹
    /// 跟着变 —— 不然层栈在冻结基础层上"演给瞎子看"（画面不刷新）。
    #[test]
    fn driver_layer_stack_advances_under_frozen_base_and_moves_signature() {
        let mut d = AnimationDriver::new();
        let mut e = entity_with_machine();
        e.animator
            .as_mut()
            .unwrap()
            .machine
            .as_mut()
            .unwrap()
            .states[0]
            .speed = 0.0;
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
        assert!((d.layers(e.id)[0].t - 0.4).abs() < 1e-6, "层时钟要继续走");
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

    // -- 形变目标权重轨道 ---------------------------------------------------

    /// 两个目标、两个关键帧：0 秒时 (0, 1)，1 秒时 (1, 0)。
    fn lip_sync_clip() -> AnimClip {
        AnimClip {
            name: "Talk".into(),
            duration: 1.0,
            tracks: vec![Track {
                node: 0,
                path: TrackPath::MorphWeights,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![0.0, 1.0, 1.0, 0.0],
            }],
        }
    }

    #[test]
    fn morph_weight_track_samples_every_target() {
        let clip = lip_sync_clip();
        let mut out = Vec::new();
        assert!(sample_morph_weights(&clip, 0.0, &mut out));
        assert_eq!(out, [0.0, 1.0]);

        assert!(sample_morph_weights(&clip, 0.5, &mut out));
        assert_eq!(out, [0.5, 0.5], "两个目标各自线性插值");

        assert!(sample_morph_weights(&clip, 1.0, &mut out));
        assert_eq!(out, [1.0, 0.0]);
    }

    /// 轨道 stride 必须由"值总数 ÷ 关键帧数"现算 —— 写死 3/4 的变换通道
    /// 规则套到权重轨道上会读串行。
    #[test]
    fn morph_weight_stride_follows_target_count() {
        let clip = lip_sync_clip();
        assert_eq!(clip.tracks[0].stride(), 2);
        // 关键帧值数对不上目标数时（坏资产）不应 panic，只按向下取整读。
        let ragged = Track {
            node: 0,
            path: TrackPath::MorphWeights,
            interpolation: Interpolation::Linear,
            times: vec![0.0, 1.0],
            values: vec![0.0, 1.0, 1.0],
        };
        assert_eq!(ragged.stride(), 1);
    }

    /// 剪辑没有权重轨道时返回 `false`，且**不动**输出缓冲 ——
    /// 调用方据此退回实体权重 / 资产默认，而不是把脸清零。
    #[test]
    fn clip_without_morph_track_reports_false() {
        let clip = move_x_clip();
        let mut out = vec![0.7, 0.3];
        assert!(!sample_morph_weights(&clip, 0.5, &mut out));
        assert_eq!(out, [0.7, 0.3], "没有轨道时不许清空调用方的表");
    }

    /// 权重轨道**不参与**姿态采样：它挂在节点上，但改的是顶点。
    #[test]
    fn morph_track_does_not_touch_the_pose() {
        let clip = lip_sync_clip();
        let mut pose = vec![LocalPose {
            translation: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }];
        sample_clip_into(&clip, 0.5, &mut pose);
        assert_eq!(pose[0].translation, Vec3::new(1.0, 2.0, 3.0));
    }
}
