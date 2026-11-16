//! 动画叠加层：按权重把多个 clip 的偏移 / 旋转 / 缩放混到当前 Transform。

use serde::Deserialize;
use xgc_world::{AnimationLayers, Animator, Bone, Component, Name, Transform, World};

use super::animation::{slerp, AnimationLibrary};
use super::{System, SystemAccess};

#[derive(Clone, Debug, Deserialize)]
pub struct LayerDef {
    pub clip: String,
    #[serde(default = "default_weight")]
    pub weight: f32,
    /// 实体名白名单；空 = 全部实体。
    #[serde(default)]
    pub mask: Vec<String>,
    /// 关节白名单；非空时这一层只驱动这些关节，不动根变换。
    #[serde(default)]
    pub joints: Vec<u32>,
}

fn default_weight() -> f32 {
    1.0
}

pub fn parse_layers(text: &str) -> Result<Vec<LayerDef>, String> {
    ron::from_str(text).map_err(|err| format!("动画层解析失败：{err}"))
}

pub struct AnimationLayerSystem {
    pub library: AnimationLibrary,
}

impl Default for AnimationLayerSystem {
    fn default() -> Self {
        Self {
            library: AnimationLibrary::new(),
        }
    }
}

impl AnimationLayerSystem {
    pub fn new(library: AnimationLibrary) -> Self {
        Self { library }
    }
}

impl System for AnimationLayerSystem {
    fn name(&self) -> &'static str {
        "animation_layers"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![AnimationLayers::ID, Animator::ID],
            writes: vec![Transform::ID],
        }
    }

    fn run(&mut self, world: &mut World, _dt: f32) {
        let mut updates = Vec::new();
        let mut joint_requests = Vec::new();
        world.for_each2::<AnimationLayers, Animator>(|key, layers, animator| {
            let Ok(parsed) = parse_layers(&layers.layers) else {
                return;
            };
            let Some(mut transform) = world.get::<Transform>(key).copied() else {
                return;
            };
            let name = world.get::<Name>(key).map(|name| name.value.as_str());
            let mut changed = false;
            for layer in parsed {
                let weight = layer.weight.clamp(0.0, 1.0);
                if weight <= 0.0 {
                    continue;
                }
                if !layer.mask.is_empty()
                    && !name.is_some_and(|name| layer.mask.iter().any(|mask| mask == name))
                {
                    continue;
                }
                let Some(clip) = self.library.get(&layer.clip) else {
                    continue;
                };
                if !layer.joints.is_empty() {
                    if let Some(root) = world.entity(key) {
                        for joint in &layer.joints {
                            joint_requests.push((
                                root,
                                *joint,
                                layer.clip.clone(),
                                animator.time,
                                weight,
                            ));
                        }
                    }
                    continue;
                }
                let offset = clip.sample_looped(animator.time);
                let rotation = clip.sample_looped_rotation(animator.time);
                let scale = clip.sample_looped_scale(animator.time);
                for axis in 0..3 {
                    transform.position[axis] += offset[axis] * weight;
                    transform.scale[axis] *= 1.0 - weight + scale[axis] * weight;
                }
                transform.rotation = slerp(transform.rotation, rotation, weight);
                changed = true;
            }
            if changed {
                updates.push((key, transform));
            }
        });
        for (key, transform) in updates {
            if let Some(target) = world.get_mut::<Transform>(key) {
                *target = transform;
            }
        }
        let mut joint_updates = Vec::new();
        for (root, joint, clip_name, time, weight) in joint_requests {
            let Some(clip) = self.library.get(&clip_name) else {
                continue;
            };
            world.for_each::<Bone>(|key, bone| {
                if bone.index != joint || !super::animation::joints::descendant_of(world, key, root)
                {
                    return;
                }
                if let Some((position, rotation, scale)) = clip.sample_joint_looped(joint, time) {
                    joint_updates.push((key, position, rotation, scale, weight));
                }
            });
        }
        for (key, position, rotation, scale, weight) in joint_updates {
            if let Some(target) = world.get_mut::<Transform>(key) {
                for axis in 0..3 {
                    target.position[axis] += position[axis] * weight;
                    target.scale[axis] *= 1.0 - weight + scale[axis] * weight;
                }
                target.rotation = slerp(target.rotation, rotation, weight);
            }
        }
    }
}
