//! 音频空间化回归。

use std::sync::Arc;

use xgc_sim::{AudioClip, AudioMixerSystem, Schedule, SoftwareMixer};
use xgc_world::{
    AudioBus, AudioGain, AudioListener, AudioMixer, AudioSource, PlaySound, Transform, World,
};

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

#[test]
fn wav_decoder_reads_pcm16_and_library() {
    let path = std::env::temp_dir().join(format!(
        "xgc_audio_{}_{}.wav",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let samples: [i16; 8] = [0, 16384, -16384, 32767, -32768, 0, 8192, -8192];
    let data_len = (samples.len() * 2) as u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&8000u32.to_le_bytes());
    bytes.extend_from_slice(&16000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(&path, &bytes).unwrap();
    let clip = xgc_sim::load_wav(&path).unwrap();
    assert_eq!(clip.channels, 1);
    assert_eq!(clip.sample_rate, 8000);
    assert_eq!(clip.frames(), 8);
    assert!((clip.duration() - 0.001).abs() < 1e-6);
    assert!((clip.peak() - 1.0).abs() < 1e-5);
    let dir = path.parent().unwrap();
    let (library, issues) = xgc_sim::AudioLibrary::load_dir(dir).unwrap();
    assert!(!library.is_empty());
    assert!(issues.is_empty());
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn software_mixer_applies_bus_and_master() {
    let clip = AudioClip {
        name: "tone".to_string(),
        sample_rate: 8000,
        channels: 1,
        samples: vec![0.5; 4],
    };
    let mut mixer = SoftwareMixer::new(8000);
    mixer.set_mix(AudioMixer {
        master: 0.5,
        sfx: 0.5,
        ..AudioMixer::default()
    });
    mixer.play(Arc::new(clip), PlaySound::new("tone"));
    let samples = mixer.mix(4);
    assert_eq!(samples.len(), 4);
    for sample in &samples {
        assert!((sample - 0.125).abs() < 1e-5);
    }
}

#[test]
fn software_mixer_uses_source_sample_rate() {
    let clip = AudioClip {
        name: "ramp".into(),
        sample_rate: 4000,
        channels: 1,
        samples: vec![0.25, 0.75],
    };
    let mut mixer = SoftwareMixer::new(8000);
    mixer.play(Arc::new(clip), PlaySound::new("ramp"));

    assert_eq!(mixer.mix(5), [0.25, 0.5, 0.75, 0.75, 0.0]);
    assert_eq!(mixer.voices(), 0);
}

#[test]
fn looping_stereo_resamples_across_buffer_boundaries() {
    let clip = Arc::new(AudioClip {
        name: "ramp".into(),
        sample_rate: 4000,
        channels: 2,
        samples: vec![0.25, 0.75, 0.0, -0.5],
    });
    let mut mixer = SoftwareMixer::new(8000);
    mixer.play(
        Arc::clone(&clip),
        PlaySound {
            pitch: 0.5,
            looping: true,
            ..PlaySound::new("ramp")
        },
    );
    let expected = mixer.clone().mix(10);
    let mut block = [f32::NAN; 10];
    let (first, rest) = block.split_at_mut(3);
    mixer.mix_into(first);
    mixer.mix_into(rest);

    assert_eq!(block, expected.as_slice());
    assert_eq!(
        block,
        [0.5, 0.3125, 0.125, -0.0625, -0.25, -0.0625, 0.125, 0.3125, 0.5, 0.3125]
    );
    assert_eq!(mixer.voices(), 1);
    assert_eq!(Arc::strong_count(&clip), 2);
}

#[test]
fn mix_and_listener_changes_affect_playing_voices() {
    let clip = Arc::new(AudioClip {
        name: "tone".into(),
        sample_rate: 8000,
        channels: 1,
        samples: vec![0.5; 2],
    });
    let mut mixer = SoftwareMixer::new(8000);
    mixer.set_listener_position([3.0, 0.0, 0.0]);
    let sfx = PlaySound {
        volume: 0.5,
        looping: true,
        ..PlaySound::new("a/tone.wav")
    };
    mixer.play(Arc::clone(&clip), sfx.clone());
    mixer.play(
        Arc::clone(&clip),
        PlaySound {
            bus: AudioBus::Bgm,
            position: Some([0.0; 3]),
            looping: true,
            ..PlaySound::new("music/tone.wav")
        },
    );
    mixer.play(
        Arc::clone(&clip),
        PlaySound {
            volume: 0.25,
            bus: AudioBus::Voice,
            ..sfx
        },
    );
    assert_eq!(mixer.mix(1), [0.5]);

    mixer.set_mix(AudioMixer {
        master: 0.5,
        sfx: 0.25,
        bgm: 0.5,
        voice: 0.75,
    });
    mixer.set_listener_position([1.0, 0.0, 0.0]);
    assert_eq!(mixer.mix(1), [0.140625]);
    mixer.stop_clip("tone");
    assert_eq!(mixer.voices(), 3);
    mixer.stop_clip("a/tone.wav");
    assert_eq!(mixer.voices(), 1);
    assert_eq!(mixer.mix(1), [0.0625]);
    mixer.stop_all();
    assert_eq!(mixer.mix(1), [0.0]);
    assert_eq!(mixer.voices(), 0);
    assert_eq!(Arc::strong_count(&clip), 1);
}

#[test]
fn malformed_audio_does_not_poison_or_leak_voices() {
    let clip = Arc::new(AudioClip {
        name: "tone".into(),
        sample_rate: 8000,
        channels: 1,
        samples: vec![0.5, 0.25, f32::NAN],
    });
    let mut mixer = SoftwareMixer::new(8000);
    mixer.set_mix(AudioMixer {
        master: 2.0,
        bgm: f32::INFINITY,
        ..AudioMixer::default()
    });
    for sound in [
        PlaySound {
            volume: f32::NAN,
            ..PlaySound::new("tone")
        },
        PlaySound {
            bus: AudioBus::Bgm,
            ..PlaySound::new("tone")
        },
        PlaySound {
            position: Some([f32::NAN, 0.0, 0.0]),
            ..PlaySound::new("tone")
        },
        PlaySound {
            pitch: f32::NAN,
            volume: 2.0,
            ..PlaySound::new("tone")
        },
    ] {
        mixer.play(Arc::clone(&clip), sound);
    }
    mixer.play(
        Arc::new(AudioClip {
            channels: 2,
            samples: vec![0.5],
            ..(*clip).clone()
        }),
        PlaySound {
            looping: true,
            ..PlaySound::new("partial")
        },
    );
    assert_eq!(mixer.voices(), 4);
    assert_eq!(mixer.mix(4), [0.5, 0.25, 0.0, 0.0]);
    assert_eq!(mixer.voices(), 0);
}

#[test]
fn mixer_output_writes_readable_wav() {
    let path = std::env::temp_dir().join(format!(
        "xgc_mix_{}_{}.wav",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    xgc_sim::write_wav_mono(&path, 8000, &[0.5, -0.5, 0.0, 0.25]).unwrap();
    let decoded = xgc_sim::load_wav(&path).unwrap();
    assert_eq!(decoded.frames(), 4);
    assert!((decoded.peak() - 0.5).abs() < 0.01);
    std::fs::remove_file(&path).unwrap();
}
