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
            let transition = machine.transition_for(&current, &parameters);
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
                    // 旧端 `Prev` 把旧剪辑的时钟 / 速度 / 循环方式一起定格：淡出的
                    // 那半边按旧状态自己的节奏走，不跟新状态事后调速。
                    let from = machine
                        .state_def(&current)
                        .map(|definition| definition.clip.clone())
                        .unwrap_or_else(|| animator.clip.clone());
                    if duration > 0.0 {
                        animator.blend_from = from;
                        animator.blend_from_time = animator.time;
                        animator.blend_from_speed = animator.speed;
                        animator.blend_from_looping = animator.looping;
                        animator.blend_elapsed = 0.0;
                        animator.blend_duration = duration;
                    } else {
                        // 硬切：旧侧直接消失，别留着上一条过渡的残影。
                        animator.blend_from.clear();
                        animator.blend_from_time = 0.0;
                        animator.blend_from_speed = 1.0;
                        animator.blend_from_looping = true;
                        animator.blend_elapsed = 0.0;
                        animator.blend_duration = 0.0;
                    }
                    state.state = next;
                    state.time = 0.0;
                    // 新剪辑从第 0 帧起播：旧端换状态后播放相位重置，否则同一个
                    // Walk 循环会从旧剪辑的相位中途接上。
                    animator.time = 0.0;
                }
            }
            state.time += dt;
            if let Some(definition) = machine.state_def(&state.state) {
                animator.clip = definition.clip.clone();
                animator.looping = definition.looping;
                animator.speed = definition.speed;
            }
        });
    }
}
