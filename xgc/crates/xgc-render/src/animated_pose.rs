//! 把 `animation.animator` 采样成蒙皮 palette 输入（`DrawItem.pose`）。
//!
//! 旧引擎的 glTF 动画在渲染内部的节点层级上；XGC 的迁移世界只有
//! `animation.animator`，没有骨骼实体。这里按 `Animator.clip` / `Animator.time`
//! 在模型自带的剪辑里采样，关节全局矩阵直接写回 `pose`——`palette_bytes` 会再乘
//! 逆绑定矩阵，与场景骨骼路径共用同一套 GPU 蒙皮。
//!
//! 场景骨骼姿态（`SkeletonPose`）优先：有它时 `build_draw_list` 不会填
//! `animation`，这里也不覆盖已有 pose。

use super::assets::MeshCache;
use super::draw::DrawList;
use super::glb::{AdditiveLayer, CrossfadeLayer, PlayedClip};
use super::gpu::GpuError;

pub(crate) fn resolve(meshes: &mut MeshCache, list: &mut DrawList) -> Result<(), GpuError> {
    for item in &mut list.items {
        if item.pose.is_some() {
            continue;
        }
        // 混合层可以单独生效（基础剪辑为空时从绑定姿态起算）。
        if item.animation.is_none() && item.layers.is_empty() {
            continue;
        }
        // 一次性拷出采样参数与层栈：下面要拿 `item.pose` 的可变引用，先把只读
        // 输入解耦（每帧只对确实带动画的实体付这一份 clone；数值世界不进这里）。
        let sample = item.animation.clone();
        let layers = item.layers.clone();
        let Some(mesh) = meshes
            .get(&item.model)
            .map_err(|err| GpuError::Mesh(err.to_string()))?
        else {
            continue;
        };
        let (Some(skin), Some(animations)) = (&mesh.skin, &mesh.animations) else {
            continue;
        };
        // 叠加层：权重 0 / 空剪辑时不付采样代价（与旧端零权重逐位一致）。
        let additive = sample
            .as_ref()
            .filter(|sample| !sample.additive_clip.is_empty() && sample.additive_weight > 0.0)
            .map(|sample| AdditiveLayer {
                clip: sample.additive_clip.as_str(),
                time: sample.additive_time,
                looping: sample.additive_looping,
                weight: sample.additive_weight,
            });
        // 交叉淡入：状态机切换后 `alpha = min(已淡秒数 / 总时长, 1)`；alpha=0 时
        // 完全等于旧剪辑（旧端「切换当帧 = 纯旧剪辑」）。
        let crossfade = sample
            .as_ref()
            .filter(|sample| !sample.blend_from.is_empty() && sample.blend_duration > 0.0)
            .map(|sample| CrossfadeLayer {
                clip: sample.blend_from.as_str(),
                time: sample.blend_from_time,
                looping: sample.blend_from_looping,
                alpha: (sample.blend_elapsed / sample.blend_duration).clamp(0.0, 1.0),
            });
        let root_motion = sample
            .as_ref()
            .filter(|sample| !sample.root_motion_node.is_empty())
            .map(|sample| (sample.root_motion_node.as_str(), sample.root_motion_axes));
        let played = PlayedClip {
            clip: sample
                .as_ref()
                .map(|sample| sample.clip.as_str())
                .unwrap_or(""),
            time: sample.as_ref().map(|sample| sample.time).unwrap_or(0.0),
            looping: sample.as_ref().is_none_or(|sample| sample.looping),
            root_motion,
        };
        let Some(pose) =
            animations.joint_pose_played(played, crossfade, additive, &layers, &skin.joint_nodes)
        else {
            continue;
        };
        // 坏数据（NaN/Inf）不进 palette：一次数值事故不该把整个角色画成无面片。
        if pose
            .iter()
            .all(|matrix| matrix.iter().all(|value| value.is_finite()))
        {
            item.pose = Some(pose);
        }
    }
    Ok(())
}
