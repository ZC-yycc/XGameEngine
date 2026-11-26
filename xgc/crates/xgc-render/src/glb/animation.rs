//! glTF 动画剪辑：节点层级 + TRS 关键帧采样。
//!
//! 为什么在**模型**上而不是场景里：旧引擎把 glTF 动画放在渲染内部的节点层级上
//! （`gltf_loader.rs` 解析 animations，`animation_runtime.rs` 采样后算关节矩阵），
//! 场景里没有骨骼实体；XGC 的迁移世界同样只有 `animation.animator`，没有
//! `animation.skeleton` / `bone`。所以 glTF 剪辑挂在 `Mesh` 上，由渲染侧按
//! `Animator` 的剪辑名与时间采样，算出的关节全局矩阵正好是现有蒙皮 palette
//! 路径要的 `pose`（`palette_bytes` 再乘逆绑定矩阵）。
//!
//! LINEAR 对平移 / 缩放做线性插值、对旋转做 slerp；STEP 取前一关键帧。
//! CUBICSPLINE 保留入切线、值、出切线，按 Hermite 基函数采样。时间按 looping
//! 取模或钳制。
//!
//! 只处理节点 TRS 通道；morph target 的 `weights` 通道直接跳过（XGC 的 morph 走
//! 组件覆盖，不从这里驱动），未知 path 也跳过而不是让整个模型加载失败。

use serde::Deserialize;

use crate::camera::{mul, Mat4};

use super::{accessor_layout, read_f32, Accessor, MeshError, Root};

mod sample;

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

/// glTF 关键帧插值方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    Linear,
    Step,
    CubicSpline,
}

impl Interpolation {
    pub fn name(self) -> &'static str {
        match self {
            Self::Linear => "LINEAR",
            Self::Step => "STEP",
            Self::CubicSpline => "CUBICSPLINE",
        }
    }

    fn parse(text: &str) -> Result<Self, MeshError> {
        match text.to_ascii_uppercase().as_str() {
            "LINEAR" => Ok(Self::Linear),
            "STEP" => Ok(Self::Step),
            "CUBICSPLINE" => Ok(Self::CubicSpline),
            other => Err(MeshError::Unsupported(format!("动画插值 {other}"))),
        }
    }

    fn values_per_key(self) -> usize {
        match self {
            Self::Linear | Self::Step => 1,
            Self::CubicSpline => 3,
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

/// 一条 TRS 通道的关键帧。`values` 按帧平铺，样条帧依次存入切线、值、出切线。
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationChannel {
    pub node: usize,
    pub path: AnimationPath,
    pub interpolation: Interpolation,
    pub times: Vec<f32>,
    pub values: Vec<f32>,
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

    /// 每个节点的绑定（默认）局部 TRS：不含任何剪辑。
    ///
    /// 只有混合层、没有基础剪辑的实体（层权重 > 0 但 Animator.clip 为空）用这份
    /// 姿态当基底；旧端同样从 `nodes` 的默认 TRS 起算。
    pub fn bind_locals(&self) -> Vec<LocalPose> {
        self.nodes
            .iter()
            .map(|node| LocalPose {
                translation: node.translation,
                rotation: node.rotation,
                scale: node.scale,
            })
            .collect()
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
            let Some(value) = channel.sample(time) else {
                continue;
            };
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
            if times.iter().any(|time| !time.is_finite()) {
                return Err(MeshError::Glb("动画时间轴有非有限值".into()));
            }
            if times.windows(2).any(|pair| {
                pair[1] < pair[0]
                    || (interpolation == Interpolation::CubicSpline && pair[1] == pair[0])
            }) {
                return Err(MeshError::Glb("动画时间轴不是递增".into()));
            }
            let values = read_values(
                root,
                sampler.output,
                bin,
                path.components(),
                times.len() * interpolation.values_per_key(),
            )?;
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
    let (base, stride) = checked_animation_layout(root, definition, 4, bin.len())?;
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
    expected_count: usize,
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
    if definition.count != expected_count {
        return Err(MeshError::Glb(format!(
            "动画关键帧数量不一致：需要 {expected_count} 个 {expected}，实际 {} 个",
            definition.count
        )));
    }
    let element_size = components * 4;
    let (base, stride) = checked_animation_layout(root, definition, element_size, bin.len())?;
    let mut out = Vec::with_capacity(definition.count * components);
    for index in 0..definition.count {
        let start = base + index * stride;
        for component in 0..components {
            out.push(read_f32(bin, start + component * 4)?);
        }
    }
    Ok(out)
}

fn checked_animation_layout(
    root: &Root,
    accessor: &Accessor,
    element_size: usize,
    bin_len: usize,
) -> Result<(usize, usize), MeshError> {
    let view = accessor
        .buffer_view
        .and_then(|index| root.buffer_views.get(index));
    if view.is_some_and(|view| view.byte_offset.checked_add(accessor.byte_offset).is_none()) {
        return Err(MeshError::Glb("动画访问器偏移溢出".into()));
    }
    let (base, stride) = accessor_layout(root, accessor, element_size)?;
    if accessor.count == 0 {
        return Ok((base, stride));
    }
    // 在按 count 分配前确认最后一个元素仍落在这个 bufferView 和 BIN 内。
    let end = (accessor.count - 1)
        .checked_mul(stride)
        .and_then(|last| base.checked_add(last))
        .and_then(|last| last.checked_add(element_size))
        .ok_or_else(|| MeshError::Glb("动画访问器长度溢出".into()))?;
    if end > bin_len || view.is_some_and(|view| end - view.byte_offset > view.byte_length) {
        return Err(MeshError::Glb("动画访问器超出 bufferView 或 BIN".into()));
    }
    Ok((base, stride))
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
