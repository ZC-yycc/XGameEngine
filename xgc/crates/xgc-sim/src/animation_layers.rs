//! 动画叠加层：按权重把多个 clip 的偏移 / 旋转 / 缩放混到当前 Transform。

use serde::Deserialize;
use xgc_world::{AnimationLayers, Animator, Component, Transform, World};

use super::animation::{slerp, AnimationLibrary};
use super::{System, SystemAccess};

#[derive(Clone, Debug, Deserialize)]
pub struct LayerDef {
    pub clip: String,
    #[serde(default = "default_weight")]
    pub weight: f32,
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
        world.for_each2::<AnimationLayers, Animator>(|key, layers, animator| {
            let Ok(parsed) = parse_layers(&layers.layers) else {
                return;
            };
            let Some(mut transform) = world.get::<Transform>(key).copied() else {
                return;
            };
            let mut changed = false;
            for layer in parsed {
                let weight = layer.weight.clamp(0.0, 1.0);
                if weight <= 0.0 {
                    continue;
                }
                let Some(clip) = self.library.get(&layer.clip) else {
                    continue;
                };
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
    }
}
