//! Lua 音频参数在入口转成请求，帧末由宿主按调用顺序结算。

use std::rc::Rc;

use mlua::{Lua, Value};
use xgc_world::{audio_volume, AudioBus, AudioCommand, AudioMixer, ListenerPose, PlaySound};

use crate::legacy::View;

#[derive(Clone, Debug, Default)]
pub struct ScriptAudio {
    pub(crate) commands: Vec<AudioCommand>,
    pub(crate) listener: Option<ListenerPose>,
    pub(crate) authored_mix: AudioMixer,
    master: Option<f32>,
    buses: [Option<f32>; AudioBus::ALL.len()],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct AudioCheckpoint {
    commands: usize,
    listener: Option<ListenerPose>,
    master: Option<f32>,
    buses: [Option<f32>; AudioBus::ALL.len()],
}

impl ScriptAudio {
    pub(crate) fn checkpoint(&self) -> AudioCheckpoint {
        AudioCheckpoint {
            commands: self.commands.len(),
            listener: self.listener,
            master: self.master,
            buses: self.buses,
        }
    }

    pub(crate) fn restore(&mut self, saved: AudioCheckpoint) {
        self.commands.truncate(saved.commands);
        self.listener = saved.listener;
        self.master = saved.master;
        self.buses = saved.buses;
    }

    fn bus_volume(&self, bus: AudioBus) -> f32 {
        self.buses[bus.index()].unwrap_or_else(|| self.authored_mix.bus_volume(bus))
    }

    pub(crate) fn mix(&self, mut authored: AudioMixer) -> AudioMixer {
        if let Some(master) = self.master {
            authored.master = master;
        }
        for bus in AudioBus::ALL {
            if let Some(volume) = self.buses[bus.index()] {
                authored.set_bus_volume(bus, volume);
            }
        }
        authored
    }
}

fn resolve_bus(name: &str) -> Option<AudioBus> {
    let bus = AudioBus::from_name(name);
    if bus.is_none() {
        eprintln!("script: 未知音量总线 '{name}'（可选：sfx / bgm / voice）");
    }
    bus
}

fn queue(view: &View, command: AudioCommand) {
    view.borrow_mut().audio.commands.push(command);
}

pub(crate) fn register(lua: &Lua, view: &View) -> mlua::Result<()> {
    let globals = lua.globals();
    let state = Rc::clone(view);
    globals.set(
        "play_sound",
        lua.create_function(
            move |_, (_ctx, clip, volume, pitch): (Value, String, f32, f32)| {
                queue(
                    &state,
                    AudioCommand::Play(PlaySound {
                        volume,
                        pitch,
                        ..PlaySound::new(clip)
                    }),
                );
                Ok(())
            },
        )?,
    )?;
    let state = Rc::clone(view);
    globals.set(
        "play_sound_at",
        lua.create_function(
            move |_, (_ctx, clip, x, y, z, volume): (Value, String, f32, f32, f32, f32)| {
                queue(
                    &state,
                    AudioCommand::Play(PlaySound {
                        volume,
                        position: Some([x, y, z]),
                        ..PlaySound::new(clip)
                    }),
                );
                Ok(())
            },
        )?,
    )?;
    let state = Rc::clone(view);
    globals.set(
        "play_sound_bus",
        lua.create_function(
            move |_, (_ctx, clip, name, volume, pitch): (Value, String, String, f32, f32)| {
                if let Some(bus) = resolve_bus(&name) {
                    queue(
                        &state,
                        AudioCommand::Play(PlaySound {
                            bus,
                            volume,
                            pitch,
                            ..PlaySound::new(clip)
                        }),
                    );
                }
                Ok(())
            },
        )?,
    )?;
    let state = Rc::clone(view);
    globals.set(
        "play_sound_at_bus",
        lua.create_function(
            move |_,
                  (_ctx, clip, x, y, z, name, volume): (
                Value,
                String,
                f32,
                f32,
                f32,
                String,
                f32,
            )| {
                if let Some(bus) = resolve_bus(&name) {
                    queue(
                        &state,
                        AudioCommand::Play(PlaySound {
                            bus,
                            volume,
                            position: Some([x, y, z]),
                            ..PlaySound::new(clip)
                        }),
                    );
                }
                Ok(())
            },
        )?,
    )?;
    let state = Rc::clone(view);
    globals.set(
        "stop_sound",
        lua.create_function(move |_, (_ctx, clip): (Value, String)| {
            queue(&state, AudioCommand::StopClip(clip));
            Ok(())
        })?,
    )?;
    let state = Rc::clone(view);
    globals.set(
        "stop_all_sounds",
        lua.create_function(move |_, _ctx: Value| {
            queue(&state, AudioCommand::StopAll);
            Ok(())
        })?,
    )?;
    let state = Rc::clone(view);
    globals.set(
        "set_bus_volume",
        lua.create_function(move |_, (_ctx, name, gain): (Value, String, f32)| {
            if let Some(bus) = resolve_bus(&name) {
                state.borrow_mut().audio.buses[bus.index()] = Some(audio_volume(gain));
            }
            Ok(())
        })?,
    )?;
    let state = Rc::clone(view);
    globals.set(
        "bus_volume",
        lua.create_function(move |_, (_ctx, name): (Value, String)| {
            Ok(AudioBus::from_name(&name).map(|bus| state.borrow().audio.bus_volume(bus)))
        })?,
    )?;
    let state = Rc::clone(view);
    globals.set(
        "set_master_volume",
        lua.create_function(move |_, (_ctx, gain): (Value, f32)| {
            state.borrow_mut().audio.master = Some(audio_volume(gain));
            Ok(())
        })?,
    )?;
    let state = Rc::clone(view);
    globals.set(
        "set_listener",
        lua.create_function(
            move |_, (_ctx, x, y, z, fx, fy, fz): (Value, f32, f32, f32, f32, f32, f32)| {
                let position = [x, y, z];
                let forward = [fx, fy, fz];
                if position.into_iter().chain(forward).all(f32::is_finite) {
                    state.borrow_mut().audio.listener = Some(ListenerPose { position, forward });
                }
                Ok(())
            },
        )?,
    )
}
