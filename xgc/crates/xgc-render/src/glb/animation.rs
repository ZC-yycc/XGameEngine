//! glTF 动画剪辑：节点层级 + TRS 关键帧采样。
//!
//! 为什么在**模型**上而不是场景里：旧引擎把 glTF 动画放在渲染内部的节点层级上
//! （`gltf_loader.rs` 解析 animations，`animation_runtime.rs` 采样后算关节矩阵），
//! 场景里没有骨骼实体；XGC 的迁移世界同样只有 `animation.animator`，没有
//! `animation.skeleton` / `bone`。所以 glTF 剪辑挂在 `Mesh` 上，由渲染侧按
//! `Animator` 的剪辑名与时间采样，算出的关节全局矩阵正好是现有蒙皮 palette
//! 路径要的 `pose`（`palette_bytes` 再乘逆绑定矩阵）。
//!
//! 采样口径与旧运行时一致：LINEAR 对平移 / 缩放做线性插值、对旋转做 slerp；
//! STEP 取前一关键帧；时间按 looping 取模。CUBICSPLINE 暂不支持——遇到时明确
//! 报错而不是悄悄降级（当前割接资产 Fox / Box 全是 LINEAR）。
//!
//! 只处理节点 TRS 通道；morph target 的 `weights` 通道直接跳过（XGC 的 morph 走
//! 组件覆盖，不从这里驱动），未知 path 也跳过而不是让整个模型加载失败。

use serde::Deserialize;
use xgc_world::{RootMotionClip, RootMotionNodeTrack, RootMotionSet, RootMotionTrack};

use crate::camera::{mul, Mat4};

use super::{accessor_layout, read_f32, MeshError, Root};

const COMPONENT_F32: u32 = 5126;

#[derive(Debug, Deserialize)]
pub(super) struct AnimationDef {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    channels: Vec<ChannelDef>,
    #[serde(default)]
    samplers: Vec<SamplerDef>,
}

#[derive(Debug, Deserialize)]
struct ChannelDef {
    sampler: usize,
    target: TargetDef,
}

#[derive(Debug, Deserialize)]
struct TargetDef {
    #[serde(default)]
    node: Option<usize>,
    #[serde(default)]
    path: String,
}

#[derive(Debug, Deserialize)]
struct SamplerDef {
    input: usize,
    output: usize,
    #[serde(default = "default_interpolation")]
    interpolation: String,
}

fn default_interpolation() -> String {
    "LINEAR".to_string()
}

/// 关键帧作用在哪条 TRS 通道上。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationPath {
    Translation,
    Rotation,
    Scale,
}

impl AnimationPath {
    pub fn name(self) -> &'static str {
        match self {
            Self::Translation => "translation",
            Self::Rotation => "rotation",
            Self::Scale => "scale",
        }
    }

    fn components(self) -> usize {
        match self {
            Self::Translation | Self::Scale => 3,
            Self::Rotation => 4,
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "translation" => Some(Self::Translation),
            "rotation" => Some(Self::Rotation),
            "scale" => Some(Self::Scale),
            // `weights`（morph target）等通道不在这里驱动。
            _ => None,
        }
    }
}

/// 关键帧插值方式。旧运行时支持三种，XGC 目前只接入 LINEAR / STEP。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    Linear,
    Step,
}

impl Interpolation {
    pub fn name(self) -> &'static str {
        match self {
            Self::Linear => "LINEAR",
            Self::Step => "STEP",
        }
    }

    fn parse(text: &str) -> Result<Self, MeshError> {
        match text.to_ascii_uppercase().as_str() {
            "LINEAR" => Ok(Self::Linear),
            "STEP" => Ok(Self::Step),
            other => Err(MeshError::Unsupported(format!(
                "动画插值 {other}（只支持 LINEAR / STEP；CUBICSPLINE 尚未接入）"
            ))),
        }
    }
}

/// 一个节点的静态 TRS（动画没写到的通道用它）。
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationNode {
    /// glTF 节点名（匿名节点为空串）；根运动与 layers 的关节名匹配都靠它。
    pub name: String,
    pub parent: Option<usize>,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

/// 一条 TRS 通道的关键帧。`values` 平铺：每个关键帧占 `path.components()` 个 f32。
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationChannel {
    pub node: usize,
    pub path: AnimationPath,
    pub interpolation: Interpolation,
    pub times: Vec<f32>,
    pub values: Vec<f32>,
}

impl AnimationChannel {
    fn stride(&self) -> usize {
        self.path.components()
    }

    fn key_value(&self, key: usize) -> [f32; 4] {
        let stride = self.stride();
        let start = key * stride;
        let mut out = [0.0; 4];
        out[..stride].copy_from_slice(&self.values[start..start + stride]);
        out
    }

    /// 在 `time` 处采样；调用方保证 `times` 非空且递增。
    fn sample(&self, time: f32) -> [f32; 4] {
        if self.times.is_empty() {
            return [0.0; 4];
        }
        let last = self.times.len() - 1;
        if time <= self.times[0] {
            return self.key_value(0);
        }
        if time >= self.times[last] {
            return self.key_value(last);
        }
        let mut index = 0;
        while index + 1 < self.times.len() && self.times[index + 1] <= time {
            index += 1;
        }
        let (start, end) = (self.times[index], self.times[index + 1]);
        let u = if end > start {
            (time - start) / (end - start)
        } else {
            0.0
        };
        let a = self.key_value(index);
        let b = self.key_value(index + 1);
        match self.interpolation {
            Interpolation::Step => a,
            Interpolation::Linear => match self.path {
                AnimationPath::Rotation => slerp(a, b, u),
                _ => [
                    lerp(a[0], b[0], u),
                    lerp(a[1], b[1], u),
                    lerp(a[2], b[2], u),
                    0.0,
                ],
            },
        }
    }
}

/// 一个 glTF 动画剪辑。
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationClip {
    pub name: String,
    /// 最大关键帧时间（秒）；0 表示没有可用通道。
    pub duration: f32,
    pub channels: Vec<AnimationChannel>,
}

/// 一个节点的局部 TRS。叠加层在**局部空间**混合后再统一算全局矩阵。
#[derive(Clone, Debug, PartialEq)]
pub struct LocalPose {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

/// 模型自带的全部 glTF 动画：节点层级 + 剪辑。`None` = 没有 animations。
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationSet {
    pub nodes: Vec<AnimationNode>,
    pub clips: Vec<AnimationClip>,
}

impl AnimationSet {
    pub fn clip(&self, name: &str) -> Option<&AnimationClip> {
        self.clips.iter().find(|clip| clip.name == name)
    }

    /// 抽出模型的根运动轨道（按节点名索引，给 `xgc_sim::apply_root_motion` 用）。
    ///
    /// 只取平移通道；匿名节点进不了表（旧端同样按名字找节点）。CUBICSPLINE 在
    /// 解析阶段就被拒，所以这里只剩 LINEAR / STEP 两种采样方式。
    pub fn root_motion_set(&self) -> RootMotionSet {
        let mut clips = Vec::new();
        for clip in &self.clips {
            let mut tracks = Vec::new();
            for channel in &clip.channels {
                if !matches!(channel.path, AnimationPath::Translation) {
                    continue;
                }
                let Some(node) = self.nodes.get(channel.node) else {
                    continue;
                };
                if node.name.is_empty() {
                    continue;
                }
                let values: Vec<[f32; 3]> = channel
                    .values
                    .chunks_exact(3)
                    .map(|value| [value[0], value[1], value[2]])
                    .collect();
                let step = matches!(channel.interpolation, Interpolation::Step);
                let Some(track) = RootMotionTrack::new(channel.times.clone(), values, step) else {
                    continue;
                };
                tracks.push(RootMotionNodeTrack {
                    node: node.name.clone(),
                    track,
                });
            }
            if !tracks.is_empty() {
                clips.push(RootMotionClip {
                    name: clip.name.clone(),
                    duration: clip.duration,
                    tracks,
                });
            }
        }
        RootMotionSet { clips }
    }

    /// 采样剪辑得到**节点局部** TRS（未做层级合成）。
    ///
    /// 叠加层要在局部空间做混合（旋转增量必须乘在父空间里），所以这里把采样
    /// 与「算全局矩阵」拆开；`joint_pose` 只是两步的薄封装。
    pub fn local_pose(&self, clip_name: &str, time: f32, looping: bool) -> Option<Vec<LocalPose>> {
        if !time.is_finite() {
            return None;
        }
        let clip = self.clip(clip_name)?;
        self.sample_locals(clip, time, looping)
    }

    /// 采样一条剪辑到节点局部 TRS；时间按 `looping` 取模 / 钳制。
    fn sample_locals(
        &self,
        clip: &AnimationClip,
        time: f32,
        looping: bool,
    ) -> Option<Vec<LocalPose>> {
        if clip.duration <= 0.0 {
            return None;
        }
        let time = if looping {
            time.rem_euclid(clip.duration)
        } else {
            time.clamp(0.0, clip.duration)
        };
        let mut locals: Vec<LocalPose> = self
            .nodes
            .iter()
            .map(|node| LocalPose {
                translation: node.translation,
                rotation: node.rotation,
                scale: node.scale,
            })
            .collect();
        for channel in &clip.channels {
            let Some(node) = locals.get_mut(channel.node) else {
                continue;
            };
            let value = channel.sample(time);
            match channel.path {
                AnimationPath::Translation => node.translation = [value[0], value[1], value[2]],
                AnimationPath::Rotation => node.rotation = [value[0], value[1], value[2], value[3]],
                AnimationPath::Scale => node.scale = [value[0], value[1], value[2]],
            }
        }
        Some(locals)
    }

    /// 采样剪辑得到关节**全局**矩阵，顺序与 `joint_nodes` 一致。
    ///
    /// 返回 `None`：剪辑不存在 / 没有通道 / 时间非法 / 节点层级成环。
    /// 渲染侧拿到 `None` 就退回绑定姿态，不让一次坏数据把实体画没。
    pub fn joint_pose(
        &self,
        clip_name: &str,
        time: f32,
        looping: bool,
        joint_nodes: &[usize],
    ) -> Option<Vec<Mat4>> {
        if joint_nodes.is_empty() {
            return None;
        }
        let locals = self.local_pose(clip_name, time, looping)?;
        self.joint_matrices(&locals, joint_nodes)
    }

    pub(crate) fn joint_matrices(
        &self,
        locals: &[LocalPose],
        joint_nodes: &[usize],
    ) -> Option<Vec<Mat4>> {
        let globals = self.global_transforms(locals)?;
        let mut pose = Vec::with_capacity(joint_nodes.len());
        for node in joint_nodes {
            pose.push(*globals.get(*node)?);
        }
        Some(pose)
    }

    fn global_transforms(&self, locals: &[LocalPose]) -> Option<Vec<Mat4>> {
        let count = self.nodes.len();
        let mut globals: Vec<Option<Mat4>> = vec![None; count];
        for start in 0..count {
            if globals[start].is_some() {
                continue;
            }
            let mut chain = Vec::new();
            let mut cursor = Some(start);
            while let Some(index) = cursor {
                if globals.get(index).is_some_and(Option::is_some) {
                    break;
                }
                chain.push(index);
                if chain.len() > count {
                    return None;
                }
                cursor = self.nodes.get(index).and_then(|node| node.parent);
            }
            while let Some(index) = chain.pop() {
                let pose = locals.get(index)?;
                let local = compose(pose.translation, pose.rotation, pose.scale);
                let global = match self.nodes.get(index).and_then(|node| node.parent) {
                    Some(parent) => mul(globals.get(parent).copied().flatten()?, local),
                    None => local,
                };
                *globals.get_mut(index)? = Some(global);
            }
        }
        globals.into_iter().collect()
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn parse(root: &Root, bin: &[u8]) -> Result<Option<AnimationSet>, MeshError> {
    if root.animations.is_empty() {
        return Ok(None);
    }
    let nodes = node_hierarchy(root);
    let mut clips = Vec::with_capacity(root.animations.len());
    for (index, definition) in root.animations.iter().enumerate() {
        let name = definition
            .name
            .clone()
            .unwrap_or_else(|| format!("clip_{index}"));
        let mut channels = Vec::new();
        let mut duration = 0.0f32;
        for channel in &definition.channels {
            let Some(node) = channel.target.node else {
                continue;
            };
            if node >= nodes.len() {
                return Err(MeshError::Glb(format!("动画通道节点 {node} 越界")));
            }
            let Some(path) = AnimationPath::parse(&channel.target.path) else {
                continue;
            };
            let sampler = definition
                .samplers
                .get(channel.sampler)
                .ok_or_else(|| MeshError::Missing(format!("动画 sampler {}", channel.sampler)))?;
            let interpolation = Interpolation::parse(&sampler.interpolation)?;
            let times = read_times(root, sampler.input, bin)?;
            if times.is_empty() {
                continue;
            }
            let stride = path.components();
            let values = read_values(root, sampler.output, bin, stride)?;
            if values.len() != times.len() * stride {
                return Err(MeshError::Glb(format!(
                    "动画关键帧数量不一致：{} 帧 × {stride} != {} 个值",
                    times.len(),
                    values.len()
                )));
            }
            if times.windows(2).any(|pair| pair[1] < pair[0]) {
                return Err(MeshError::Glb("动画时间轴不是递增".into()));
            }
            duration = duration.max(*times.last().unwrap_or(&0.0));
            channels.push(AnimationChannel {
                node,
                path,
                interpolation,
                times,
                values,
            });
        }
        clips.push(AnimationClip {
            name,
            duration,
            channels,
        });
    }
    Ok(Some(AnimationSet { nodes, clips }))
}

fn node_hierarchy(root: &Root) -> Vec<AnimationNode> {
    let mut parents = vec![None; root.nodes.len()];
    for (index, node) in root.nodes.iter().enumerate() {
        for child in &node.children {
            if let Some(slot) = parents.get_mut(*child) {
                *slot = Some(index);
            }
        }
    }
    root.nodes
        .iter()
        .enumerate()
        .map(|(index, node)| AnimationNode {
            name: node.name.clone().unwrap_or_default(),
            parent: parents[index],
            translation: node.translation.unwrap_or([0.0; 3]),
            rotation: normalize_quat(node.rotation.unwrap_or([0.0, 0.0, 0.0, 1.0])),
            scale: node.scale.unwrap_or([1.0; 3]),
        })
        .collect()
}

fn read_times(root: &Root, accessor: usize, bin: &[u8]) -> Result<Vec<f32>, MeshError> {
    let definition = root
        .accessors
        .get(accessor)
        .ok_or_else(|| MeshError::Missing("动画 input accessor".into()))?;
    if definition.component_type != COMPONENT_F32 || definition.ty != "SCALAR" {
        return Err(MeshError::Unsupported(format!(
            "动画时间轴需要 float SCALAR，实际 type {} component {}",
            definition.ty, definition.component_type
        )));
    }
    let (base, stride) = accessor_layout(root, definition, 4)?;
    let mut out = Vec::with_capacity(definition.count);
    for index in 0..definition.count {
        out.push(read_f32(bin, base + index * stride)?);
    }
    Ok(out)
}

fn read_values(
    root: &Root,
    accessor: usize,
    bin: &[u8],
    components: usize,
) -> Result<Vec<f32>, MeshError> {
    let definition = root
        .accessors
        .get(accessor)
        .ok_or_else(|| MeshError::Missing("动画 output accessor".into()))?;
    let expected = match components {
        3 => "VEC3",
        4 => "VEC4",
        _ => return Err(MeshError::Glb("动画通道分量数只能是 3 / 4".into())),
    };
    if definition.component_type != COMPONENT_F32 || definition.ty != expected {
        return Err(MeshError::Unsupported(format!(
            "动画关键帧需要 float {expected}，实际 type {} component {}",
            definition.ty, definition.component_type
        )));
    }
    let element_size = components * 4;
    let (base, stride) = accessor_layout(root, definition, element_size)?;
    let mut out = Vec::with_capacity(definition.count * components);
    for index in 0..definition.count {
        let start = base + index * stride;
        for component in 0..components {
            out.push(read_f32(bin, start + component * 4)?);
        }
    }
    Ok(out)
}

/// 列主序 `T * R * S`（平移不缩放，缩放逐列乘）。
fn compose(translation: [f32; 3], rotation: [f32; 4], scale: [f32; 3]) -> Mat4 {
    let [x, y, z, w] = rotation;
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (xw, yw, zw) = (x * w, y * w, z * w);
    let [sx, sy, sz] = scale;
    [
        (1.0 - 2.0 * (yy + zz)) * sx,
        (2.0 * (xy + zw)) * sx,
        (2.0 * (xz - yw)) * sx,
        0.0,
        (2.0 * (xy - zw)) * sy,
        (1.0 - 2.0 * (xx + zz)) * sy,
        (2.0 * (yz + xw)) * sy,
        0.0,
        (2.0 * (xz + yw)) * sz,
        (2.0 * (yz - xw)) * sz,
        (1.0 - 2.0 * (xx + yy)) * sz,
        0.0,
        translation[0],
        translation[1],
        translation[2],
        1.0,
    ]
}

fn lerp(a: f32, b: f32, u: f32) -> f32 {
    a + (b - a) * u
}

fn normalize_quat(value: [f32; 4]) -> [f32; 4] {
    let length =
        (value[0] * value[0] + value[1] * value[1] + value[2] * value[2] + value[3] * value[3])
            .sqrt();
    if length > 0.0 && length.is_finite() {
        [
            value[0] / length,
            value[1] / length,
            value[2] / length,
            value[3] / length,
        ]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// 四元数球面插值；`dot` 为负时先翻号走短弧（与旧端 `q0.slerp(q1, u)` 同语义）。
pub(crate) fn slerp(a: [f32; 4], b: [f32; 4], u: f32) -> [f32; 4] {
    let mut b = b;
    let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    if dot < 0.0 {
        b = [-b[0], -b[1], -b[2], -b[3]];
        dot = -dot;
    }
    if dot > 0.9995 {
        let out = [
            lerp(a[0], b[0], u),
            lerp(a[1], b[1], u),
            lerp(a[2], b[2], u),
            lerp(a[3], b[3], u),
        ];
        return normalize_quat(out);
    }
    let theta = dot.clamp(-1.0, 1.0).acos();
    let sin_theta = theta.sin();
    if sin_theta.abs() < f32::EPSILON {
        return a;
    }
    let weight_a = ((1.0 - u) * theta).sin() / sin_theta;
    let weight_b = (u * theta).sin() / sin_theta;
    normalize_quat([
        a[0] * weight_a + b[0] * weight_b,
        a[1] * weight_a + b[1] * weight_b,
        a[2] * weight_a + b[2] * weight_b,
        a[3] * weight_a + b[3] * weight_b,
    ])
}
