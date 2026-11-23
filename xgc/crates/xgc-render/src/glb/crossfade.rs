//! 交叉淡入：基础层用「旧剪辑 → 新剪辑」两份局部姿势按 alpha 混合。
//!
//! 与旧端 `compute_bones_composed` 的 fade 分支同一拍
//! （`crates/xgame-core/src/animation_runtime.rs`）：先采旧剪辑到姿势，再采新剪辑，
//! `blend_into(pose, new_pose, alpha)`（平移 / 缩放 lerp、旋转 slerp）；alpha =
//! `min(fade_t / duration, 1)`。旧剪辑按**它自己**的时钟与循环方式采样 ——
//! Walk→Run 淡出的那半边仍是 Walk 的步态。混完再交给叠加层，最后统一算关节矩阵。

use super::additive::AdditiveLayer;
use super::animation::{slerp, AnimationSet, LocalPose};
use super::layers::LayerSample;
use crate::camera::Mat4;

/// 基础层一次采样的全部参数（剪辑 + 时钟 + 循环 + 可选根运动剥离配置）。
///
/// 根运动：实体已经在世界空间前进，蒙皮里的该节点平移要回到绑定姿态；`None` = 不剥离。
pub struct PlayedClip<'a> {
    pub clip: &'a str,
    pub time: f32,
    pub looping: bool,
    pub root_motion: Option<(&'a str, [bool; 3])>,
}

/// 一次进行中的交叉淡入：旧剪辑 + 它的时钟 / 循环方式 + 当前混合权重。
pub struct CrossfadeLayer<'a> {
    pub clip: &'a str,
    pub time: f32,
    pub looping: bool,
    /// 0 = 纯旧剪辑，1 = 淡入完成。
    pub alpha: f32,
}

impl AnimationSet {
    /// 采样「基础剪辑（可带交叉淡入）+ 可选叠加层」得到关节全局矩阵。
    ///
    /// 旧剪辑缺失 / 采样失败时静默退回新剪辑（旧端缺剪辑同样是跳过旧侧）。
    pub fn joint_pose_played(
        &self,
        played: PlayedClip<'_>,
        crossfade: Option<CrossfadeLayer<'_>>,
        additive: Option<AdditiveLayer<'_>>,
        layers: &[LayerSample],
        joint_nodes: &[usize],
    ) -> Option<Vec<Mat4>> {
        if joint_nodes.is_empty() {
            return None;
        }
        // 层栈可以单独生效（基础剪辑为空时从绑定姿态起算），旧的「层改根
        // Transform」语义已删除：层只作用于模型内部节点姿势。
        let mut locals = if played.clip.is_empty() {
            self.bind_locals()
        } else {
            self.local_pose(played.clip, played.time, played.looping)?
        };
        if let Some(fade) = crossfade {
            let alpha = fade.alpha.clamp(0.0, 1.0);
            if alpha < 1.0 {
                if let Some(mut old) = self.local_pose(fade.clip, fade.time, fade.looping) {
                    blend_into(&mut old, &locals, alpha);
                    locals = old;
                }
            }
        }
        // 根运动：实体已经在世界空间前进，蒙皮里的根节点平移要回到绑定姿态，
        // 顺序与旧端一致（基础 + 淡入之后、叠加层之前）。
        if let Some((node, axes)) = played.root_motion {
            super::root_motion::strip(&mut locals, &self.nodes, node, axes);
        }
        self.apply_additive_into(&mut locals, additive);
        // 层栈在 additive 之后按顺序叠加（后层盖前层，旧端同序）。
        self.apply_layers_into(&mut locals, layers);
        self.joint_matrices(&locals, joint_nodes)
    }
}

/// 把 `new_pose` 按 `alpha` 混进 `pose`（就地）—— 与旧端 `blend_into` 逐行同式。
pub(crate) fn blend_into(pose: &mut [LocalPose], new_pose: &[LocalPose], alpha: f32) {
    for (base, target) in pose.iter_mut().zip(new_pose) {
        for axis in 0..3 {
            base.translation[axis] += (target.translation[axis] - base.translation[axis]) * alpha;
            base.scale[axis] += (target.scale[axis] - base.scale[axis]) * alpha;
        }
        base.rotation = slerp(base.rotation, target.rotation, alpha);
    }
}
