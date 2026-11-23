//! 动画混合层：推进每层自己的状态机 / 时钟 / 淡入，把结果写回组件。
//!
//! 旧端层栈在 `Animator.layers` 里由 `AnimationDriver` 逐层 tick：独立状态机、
//! 独立时钟、转移淡入按本层 `fade_time` 兜底；脚本按层名写权重与参数。XGC 的
//! 组件（`animation.layers` 的 RON 快照）同时存配置与运行时状态，本系统只负责
//! **推进**（状态转移、时钟、淡入），姿势合成在渲染侧按模型关节遮罩完成——
//! 这里绝不改实体 `Transform`（旧端层栈也只作用于骨架姿势）。

use xgc_world::{
    find_layer, layer_parameters, layers_to_ron, parse_layers, AnimLayer, AnimationLayers,
    Animator, Component, World, WriteSet,
};

use super::{System, SystemAccess};

#[derive(Clone, Copy, Debug, Default)]
pub struct AnimationLayerSystem;

impl System for AnimationLayerSystem {
    fn runs_after_host(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "animation_layers"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: vec![Animator::ID],
            writes: vec![AnimationLayers::ID],
        }
    }

    fn supports_parted(&self) -> bool {
        true
    }

    fn run_parted(&mut self, world: &World, writes: &mut WriteSet, dt: f32) {
        let mut autoplay = std::collections::BTreeMap::new();
        world.for_each::<Animator>(|key, animator| {
            autoplay.insert(key, animator.autoplay);
        });
        writes.for_each_mut::<AnimationLayers>(|key, layers| {
            if let Some(&autoplay) = autoplay.get(&key) {
                advance_layers(layers, autoplay, dt);
            }
        });
    }

    fn run(&mut self, world: &mut World, dt: f32) {
        world.for_each2_mut::<AnimationLayers, Animator>(|_, layers, animator| {
            advance_layers(layers, animator.autoplay, dt);
        });
    }
}

/// 推进一个实体的层栈：状态转移 → 时钟 → 淡入，写回 RON 快照。
fn advance_layers(layers: &mut AnimationLayers, autoplay: bool, dt: f32) {
    if layers.layers.trim().is_empty() {
        return;
    }
    let Ok(mut parsed) = parse_layers(&layers.layers) else {
        return;
    };
    for layer in &mut parsed {
        advance_layer(layer, autoplay, dt);
    }
    let updated = layers_to_ron(&parsed);
    if updated != layers.layers {
        layers.layers = updated;
    }
}

/// 推进一层：状态转移（旧端命中即切、当帧就开始走新状态时钟）→ 时钟 → 淡入。
///
/// 顺序与旧端 `AnimationDriver::tick` 一致：先 sync 状态机 / 建 `Prev`，再推进
/// 时钟与淡入；转移当帧 `alpha = dt/duration`（旧端同款，切换帧已淡出一小步）。
fn advance_layer(layer: &mut AnimLayer, autoplay: bool, dt: f32) {
    if layer.name.is_empty() && layer.clip.is_empty() && layer.machine.is_none() {
        return;
    }
    let machine = layer.usable_machine().cloned();
    let Some(machine) = machine else {
        if autoplay {
            layer.time += dt * layer.speed.max(0.0);
        }
        return;
    };
    if layer.state.is_empty() {
        layer.state = machine.initial.clone();
    }
    if !autoplay {
        return;
    }
    let current = layer.state.clone();
    let parameters = layer_parameters(layer);
    if let Some(transition) = machine.transition_for(&current, &parameters) {
        let next = transition.to.clone();
        if next != current {
            // 旧端：转移没写 duration 时用本层 `fade_time` 兜底；旧侧时钟 / 速度 /
            // 循环方式在切换那一刻定格（淡出的那半边按旧状态节奏走）。
            let duration = if transition.duration > 0.0 {
                transition.duration
            } else {
                layer.fade_time
            };
            let old = machine.state_def(&current);
            if duration > 0.0 {
                layer.fade_from = old
                    .map(|state| state.clip.clone())
                    .unwrap_or_else(|| layer.clip.clone());
                layer.fade_from_time = layer.time;
                layer.fade_from_speed = old.map(|state| state.speed).unwrap_or(layer.speed);
                layer.fade_from_looping = old.map(|state| state.looping).unwrap_or(layer.looping);
                layer.fade_elapsed = 0.0;
                layer.fade_duration = duration;
            } else {
                layer.clear_fade();
            }
            layer.state = next;
            layer.time = 0.0;
        }
    }
    layer.time += dt * layer.effective_speed();
    if layer.fade_duration > 0.0 {
        layer.fade_from_time += dt * layer.fade_from_speed;
        layer.fade_elapsed += dt;
        if layer.fade_elapsed >= layer.fade_duration {
            layer.clear_fade();
        }
    }
}

/// 按层名改权重（脚本 `set_anim_layer_weight` 的唯一实现处）。
///
/// 返回是否真的改到：层名不存在 / 非有限值 → false（脚本侧静默 no-op）。
pub fn set_layer_weight(layers: &mut [AnimLayer], name: &str, weight: f32) -> bool {
    if !weight.is_finite() {
        return false;
    }
    let Some(layer) = find_layer(layers, name) else {
        return false;
    };
    layer.weight = weight.clamp(0.0, 1.0);
    true
}
