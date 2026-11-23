//! 动画状态机：按数值参数选择转换，把状态对应的 clip 写回 Animator。

use xgc_world::{parse_machine, parse_parameters, AnimationState, Animator, Component, World};

use super::{System, SystemAccess};

#[derive(Clone, Copy, Debug, Default)]
pub struct AnimationStateSystem;

impl System for AnimationStateSystem {
    fn runs_after_host(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "animation_state"
    }

    fn access(&self) -> SystemAccess {
        SystemAccess {
            reads: Vec::new(),
            writes: vec![AnimationState::ID, Animator::ID],
        }
    }

    fn run(&mut self, world: &mut World, dt: f32) {
        world.for_each2_mut::<AnimationState, Animator>(|_, state, animator| {
            if state.machine.is_empty() {
                return;
            }
            let Ok(machine) = parse_machine(&state.machine) else {
                return;
            };
            if state.state.is_empty() {
                state.state = machine.initial.clone();
            }
            let parameters = parse_parameters(&state.parameters);
            let current = state.state.clone();
            let transition = machine.transitions.iter().find(|transition| {
                transition
                    .from
                    .as_deref()
                    .is_none_or(|from| from == current)
                    && transition
                        .when
                        .as_ref()
                        .is_none_or(|when| when.matches(&parameters))
            });
            if let Some(transition) = transition {
                let next = transition.to.clone();
                if next != current {
                    // 旧端：转移没写 duration 时用 Animator.fade_time 兜底——两者都是
                    // "这一刀换剪辑的淡入时长"，只是作用域不同。
                    let duration = if transition.duration > 0.0 {
                        transition.duration
                    } else {
                        animator.fade_time
                    };
                    if duration > 0.0 {
                        animator.blend_from = machine
                            .states
                            .iter()
                            .find(|definition| definition.name == current)
                            .map(|definition| definition.clip.clone())
                            .unwrap_or_else(|| animator.clip.clone());
                        animator.blend_elapsed = 0.0;
                        animator.blend_duration = duration;
                    }
                    state.state = next;
                    state.time = 0.0;
                    // 新剪辑从第 0 帧起播：旧端换状态后播放相位重置，否则同一个
                    // Walk 循环会从旧剪辑的相位中途接上。
                    animator.time = 0.0;
                }
            }
            state.time += dt;
            if let Some(definition) = machine
                .states
                .iter()
                .find(|definition| definition.name == state.state)
            {
                animator.clip = definition.clip.clone();
                animator.looping = definition.looping;
                animator.speed = definition.speed;
            }
        });
    }
}
