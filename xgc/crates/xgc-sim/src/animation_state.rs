//! 动画状态机：按数值参数选择转换，把状态对应的 clip 写回 Animator。

use std::collections::BTreeMap;

use serde::Deserialize;
use xgc_world::{AnimationState, Animator, Component, World};

use super::{System, SystemAccess};

#[derive(Clone, Debug, Deserialize)]
pub struct StateMachine {
    pub initial: String,
    #[serde(default)]
    pub states: Vec<StateDef>,
    #[serde(default)]
    pub transitions: Vec<TransitionDef>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StateDef {
    pub name: String,
    pub clip: String,
    #[serde(default = "default_looping")]
    pub looping: bool,
    #[serde(default = "default_speed")]
    pub speed: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TransitionDef {
    #[serde(default)]
    pub from: Option<String>,
    pub to: String,
    pub parameter: String,
    #[serde(default)]
    pub at_least: f32,
}

fn default_looping() -> bool {
    true
}

fn default_speed() -> f32 {
    1.0
}

pub fn parse_machine(text: &str) -> Result<StateMachine, String> {
    ron::from_str(text).map_err(|err| format!("状态机解析失败：{err}"))
}

pub fn parse_parameters(text: &str) -> BTreeMap<String, f32> {
    ron::from_str(text).unwrap_or_default()
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AnimationStateSystem;

impl System for AnimationStateSystem {
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
            let next = machine
                .transitions
                .iter()
                .find(|transition| {
                    transition
                        .from
                        .as_deref()
                        .is_none_or(|from| from == current)
                        && parameters
                            .get(&transition.parameter)
                            .copied()
                            .unwrap_or(0.0)
                            >= transition.at_least
                })
                .map(|transition| transition.to.clone());
            if let Some(next) = next {
                if next != current {
                    state.state = next;
                    state.time = 0.0;
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
