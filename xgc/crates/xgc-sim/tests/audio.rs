//! 音频空间化回归。

use xgc_sim::{AudioMixerSystem, Schedule};
use xgc_world::{AudioGain, AudioListener, AudioMixer, AudioSource, Transform, World};

#[test]
fn spatial_source_attenuates_with_listener_distance() {
    let mut world = World::new();
    let source = world.spawn((
        Transform::default(),
        AudioSource::default(),
        AudioGain::default(),
    ));
    world.spawn((
        Transform {
            position: [3.0, 0.0, 0.0],
            ..Transform::default()
        },
        AudioListener::default(),
    ));
    let mut schedule = Schedule::new();
    schedule.add(AudioMixerSystem);
    schedule.run(&mut world, 0.016);
    let gain = world.get::<AudioGain>(source).unwrap().gain;
    assert!((gain - 0.25).abs() < 1e-5);
}

#[test]
fn non_spatial_source_keeps_volume() {
    let mut world = World::new();
    let source = world.spawn((
        Transform::default(),
        AudioSource {
            spatial: false,
            ..AudioSource::default()
        },
        AudioGain::default(),
    ));
    let mut schedule = Schedule::new();
    schedule.add(AudioMixerSystem);
    schedule.run(&mut world, 0.016);
    assert_eq!(world.get::<AudioGain>(source).unwrap().gain, 1.0);
}

#[test]
fn mixer_bus_scales_spatial_gain() {
    let mut world = World::new();
    let source = world.spawn((
        Transform::default(),
        AudioSource::default(),
        AudioGain::default(),
    ));
    world.spawn((
        Transform {
            position: [3.0, 0.0, 0.0],
            ..Transform::default()
        },
        AudioListener::default(),
    ));
    world.spawn((AudioMixer {
        sfx: 0.5,
        ..AudioMixer::default()
    },));
    let mut schedule = Schedule::new();
    schedule.add(AudioMixerSystem);
    schedule.run(&mut world, 0.016);
    let gain = world.get::<AudioGain>(source).unwrap().gain;
    assert!((gain - 0.125).abs() < 1e-5);
}
