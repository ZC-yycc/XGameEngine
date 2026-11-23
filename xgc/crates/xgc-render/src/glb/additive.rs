//! additive 叠加层：在基础姿势的**局部空间**加一层剪辑增量。
//!
//! 与旧端 `blend_additive` 逐行同式（`crates/xgame-core/src/animation_runtime.rs`）：
//! 平移 / 缩放做线性差，旋转算 `reference⁻¹ · current` 的局部增量再向单位四元数
//! slerp；权重 0 时逐位等于基础姿势。参考姿势是叠加剪辑**自己的第 0 帧**。
//!
//! 与场景骨骼的 `animation.layers`（`AnimationLayerSystem`，组件里的 RON 层）不是
//! 同一回事：那条路改实体 `Transform` / `Bone`，这里只服务模型内部节点层级。

use super::animation::{slerp, AnimationSet, LocalPose};
use crate::camera::Mat4;

/// 叠加层采样参数：`weight` 钳在 [0,1]，增量为「剪辑当前帧 − 该剪辑第 0 帧」。
///
/// 与旧端 `AnimAdditive` / `blend_additive` 同口径：平移 / 缩放做线性差，旋转在
/// 局部空间算 `ref⁻¹·current` 再按权重向单位四元数 slerp 收；权重 0 逐位等于
/// 基础姿势。
pub struct AdditiveLayer<'a> {
    pub clip: &'a str,
    pub time: f32,
    pub looping: bool,
    pub weight: f32,
}

impl AnimationSet {
    /// 采样「基础剪辑 + 可选叠加层」得到关节全局矩阵。
    ///
    /// 叠加剪辑不存在 / 时间非有限时静默退回基础姿势——旧端「空剪辑 = 不生效」
    /// 也是这个口径。
    pub fn joint_pose_with_additive(
        &self,
        clip_name: &str,
        time: f32,
        looping: bool,
        additive: Option<AdditiveLayer<'_>>,
        joint_nodes: &[usize],
    ) -> Option<Vec<Mat4>> {
        if joint_nodes.is_empty() {
            return None;
        }
        let mut locals = self.local_pose(clip_name, time, looping)?;
        self.apply_additive_into(&mut locals, additive);
        self.joint_matrices(&locals, joint_nodes)
    }

    /// 把叠加层增量加进**已采样**的局部姿态。
    ///
    /// 单独拆出来是给交叉淡入用：基础层先把旧 / 新剪辑混成一份局部姿态，再加
    /// 叠加层。叠加层数学只此一份，别在别处再抄一遍。
    pub(crate) fn apply_additive_into(
        &self,
        locals: &mut [LocalPose],
        additive: Option<AdditiveLayer<'_>>,
    ) {
        let Some(layer) = additive else {
            return;
        };
        let weight = layer.weight.clamp(0.0, 1.0);
        if weight <= 0.0 || !layer.time.is_finite() {
            return;
        }
        let (Some(current), Some(reference)) = (
            self.local_pose(layer.clip, layer.time, layer.looping),
            self.local_pose(layer.clip, 0.0, false),
        ) else {
            return;
        };
        for (base, (current, reference)) in locals.iter_mut().zip(current.iter().zip(&reference)) {
            blend_additive(base, current, reference, weight);
        }
    }
}

/// 四元数乘法 `a * b`（先 b 后 a 的旋转合成，列主序矩阵同序）。
fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

fn quat_inverse(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// 单节点 additive：`b += w × (a − r)`；旋转在**局部空间**算增量 `r⁻¹·a`，
/// 再按权重向单位四元数 slerp —— 与旧端 `blend_additive` 逐行同式。
///
/// w=0 时平移 / 缩放逐位不变、旋转 slerp 回原值；中间权重走最短弧，
/// 直接对四元数分量 lerp 会走反向长弧。
fn blend_additive(base: &mut LocalPose, additive: &LocalPose, reference: &LocalPose, weight: f32) {
    for axis in 0..3 {
        base.translation[axis] +=
            (additive.translation[axis] - reference.translation[axis]) * weight;
        base.scale[axis] += (additive.scale[axis] - reference.scale[axis]) * weight;
    }
    let delta = quat_mul(quat_inverse(reference.rotation), additive.rotation);
    base.rotation = slerp(base.rotation, quat_mul(base.rotation, delta), weight);
}
