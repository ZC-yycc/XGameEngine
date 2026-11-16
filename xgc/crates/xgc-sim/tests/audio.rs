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
    let clip = xgc_sim::AudioClip {
        name: "tone".to_string(),
        sample_rate: 8000,
        channels: 1,
        samples: vec![0.5; 4],
    };
    let mut mixer = xgc_sim::SoftwareMixer::new(8000);
    mixer.set_bus("sfx", 0.5);
    mixer.master = 0.5;
    mixer.play(&clip, 1.0, "sfx", false);
    let samples = mixer.mix(4);
    assert_eq!(samples.len(), 4);
    for sample in &samples {
        assert!((sample - 0.125).abs() < 1e-5);
    }
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
