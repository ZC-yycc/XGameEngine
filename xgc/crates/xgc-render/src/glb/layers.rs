//! 动画混合层：在基础姿势 + additive 之后按遮罩叠加 RON 层栈。
//!
//! 语义与旧端 `compute_bones_composed_with_root_motion` 的 layers 段逐条对齐
//! （`crates/xgame-core/src/animation_runtime.rs:576-660`）：
//! - 每层先按自己的 `weight` 钳到 [0,1]；0 或剪辑为空直接跳过；
//! - Additive：在**局部空间**把「层剪辑当前帧 − 该剪辑第 0 帧」的增量加上去
//!   （复用 `additive::blend_additive`，与快捷 additive 层同一份数学）；
//! - Override：对遮罩命中节点做 `blend_override`（平移 / 缩放 lerp、旋转 slerp）；
//! - 遮罩是**子树根名单**：空名单 = 整副骨架；节点名命中或沿 parent 链上某个
//!   祖先命中即作用（与旧端 `mask_covers` 同口径；匿名节点永远匹配不到）；
//! - 每层可带自己的淡入（旧剪辑 + 旧时钟 + alpha）：先按 alpha 混出层姿势再用，
//!   Additive 的参考姿势（剪辑第 0 帧）也按同一 alpha 从旧剪辑第 0 帧混过来。
//!
//! 只有 model 内部节点姿势；不再改实体根 `Transform`（那是已删除的半成品语义）。

use xgc_world::LayerMode;

use super::additive::blend_additive;
use super::animation::{slerp, AnimationSet, LocalPose};

/// 一层在渲染侧需要知道的全部信息（由 `draw.rs` 从 `animation.layers` 组件投影）。
#[derive(Clone, Debug, PartialEq)]
pub struct LayerSample {
    pub mode: LayerMode,
    pub clip: String,
    pub time: f32,
    pub looping: bool,
    /// 已钳到 [0,1]。
    pub weight: f32,
    /// 本层淡入：旧剪辑 / 旧时间 / 旧循环方式 / alpha。
    pub fade: Option<(String, f32, bool, f32)>,
    pub mask: Vec<String>,
}

impl AnimationSet {
    /// 把整条层栈叠进**已采样**的局部姿态（顺序：基础 + fade → 根运动 → additive → 本函数）。
    pub fn apply_layers_into(&self, locals: &mut [LocalPose], layers: &[LayerSample]) {
        for layer in layers {
            let weight = layer.weight.clamp(0.0, 1.0);
            if weight <= 0.0 || layer.clip.is_empty() {
                continue;
            }
            let fade = layer
                .fade
                .as_ref()
                .map(|(clip, time, looping, alpha)| (clip.as_str(), *time, *looping, *alpha));
            let Some(layer_pose) =
                self.sample_with_fade(&layer.clip, layer.time, layer.looping, fade)
            else {
                continue;
            };
            match layer.mode {
                LayerMode::Additive => {
                    // 参考 = 同一剪辑第 0 帧；淡入时参考同样从旧剪辑第 0 帧混过来
                    // （旧端 `ref_fade` 语义），否则权重中途会跳一下。
                    let ref_fade = fade.map(|(clip, _, _, alpha)| (clip, 0.0, false, alpha));
                    let Some(reference) = self.sample_with_fade(&layer.clip, 0.0, false, ref_fade)
                    else {
                        continue;
                    };
                    for (index, (base, (current, reference))) in locals
                        .iter_mut()
                        .zip(layer_pose.iter().zip(&reference))
                        .enumerate()
                    {
                        if self.covered(index, &layer.mask) {
                            blend_additive(base, current, reference, weight);
                        }
                    }
                }
                LayerMode::Override => {
                    for (index, (base, target)) in
                        locals.iter_mut().zip(layer_pose.iter()).enumerate()
                    {
                        if self.covered(index, &layer.mask) {
                            blend_override(base, target, weight);
                        }
                    }
                }
            }
        }
    }

    /// 采样一条剪辑并可选地按 alpha 混入它的淡出那半边（层自己的 fade）。
    fn sample_with_fade(
        &self,
        clip: &str,
        time: f32,
        looping: bool,
        fade: Option<(&str, f32, bool, f32)>,
    ) -> Option<Vec<LocalPose>> {
        let mut locals = self.local_pose(clip, time, looping)?;
        if let Some((old_clip, old_time, old_looping, alpha)) = fade {
            let alpha = alpha.clamp(0.0, 1.0);
            if alpha < 1.0 {
                if let Some(mut old) = self.local_pose(old_clip, old_time, old_looping) {
                    super::crossfade::blend_into(&mut old, &locals, alpha);
                    locals = old;
                }
            }
        }
        Some(locals)
    }

    /// 该节点是否被遮罩覆盖：空名单 = 全部；节点名或任一祖先名命中即覆盖。
    fn covered(&self, node: usize, mask: &[String]) -> bool {
        if mask.is_empty() {
            return true;
        }
        let mut current = Some(node);
        for _ in 0..=self.nodes.len() {
            let Some(index) = current else {
                return false;
            };
            let Some(definition) = self.nodes.get(index) else {
                return false;
            };
            if !definition.name.is_empty() && mask.iter().any(|name| name == &definition.name) {
                return true;
            }
            current = definition.parent;
        }
        false
    }
}

/// 单节点 Override：把基础姿势按权重 lerp / slerp 向层姿势 —— 与旧端
/// `blend_override` 逐行同式。
fn blend_override(base: &mut LocalPose, target: &LocalPose, weight: f32) {
    for axis in 0..3 {
        base.translation[axis] += (target.translation[axis] - base.translation[axis]) * weight;
        base.scale[axis] += (target.scale[axis] - base.scale[axis]) * weight;
    }
    base.rotation = slerp(base.rotation, target.rotation, weight);
}
