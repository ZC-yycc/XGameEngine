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
use super::glb::AdditiveLayer;
use super::gpu::GpuError;

pub(crate) fn resolve(meshes: &mut MeshCache, list: &mut DrawList) -> Result<(), GpuError> {
    for item in &mut list.items {
        if item.pose.is_some() {
            continue;
        }
        let Some(sample) = item.animation.as_ref() else {
            continue;
        };
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
        let additive = (!sample.additive_clip.is_empty() && sample.additive_weight > 0.0)
            .then_some(AdditiveLayer {
                clip: sample.additive_clip.as_str(),
                time: sample.additive_time,
                looping: sample.additive_looping,
                weight: sample.additive_weight,
            });
        let Some(pose) = animations.joint_pose_with_additive(
            &sample.clip,
            sample.time,
            sample.looping,
            additive,
            &skin.joint_nodes,
        ) else {
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
