use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use xgc_host::{asset_paths, ScriptedRunner, AUDIO_SAMPLE_RATE};
use xgc_sim::{load_wav, write_wav_mono};
use xgc_world::{AudioMixer, Script, World};

#[test]
fn lua_sound_commands_use_search_paths_and_mix_in_frame_order() {
    let root = std::env::temp_dir().join(format!(
        "xgc_host_audio_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let shared = root.join("Shared");
    fs::create_dir_all(&shared).unwrap();
    let clip = shared.join("tone.wav");
    write_wav_mono(&clip, AUDIO_SAMPLE_RATE, &[0.5; 2400]).unwrap();
    let sample = load_wav(&clip).unwrap().samples[0];
    fs::write(shared.join("tone.wav.meta"), r#"(guid: "aabb1234")"#).unwrap();
    fs::write(
        root.join("audio.lua"),
        r#"
        local frame = 0
        function update(ctx)
            frame = frame + 1
            if frame == 1 then
                assert(bus_volume(ctx, ' Music ') == 0.75)
                set_master_volume(ctx, 0.5)
                set_bus_volume(ctx, 'BGM', 0.5)
                assert(bus_volume(ctx, 'm') == 0.5)
                assert(bus_volume(ctx, 'unknown') == nil)
                play_sound(ctx, 'tone.wav', 1.0, 1.0)
                stop_sound(ctx, 'tone.wav')
                play_sound_bus(ctx, 'tone.wav', 'bgm', 0.5, 1.0)
                play_sound_bus(ctx, 'tone.wav', 'unknown', 1.0, 1.0)
            elseif frame == 2 then
                set_bus_volume(ctx, 'bgm', 0.25)
            elseif frame == 3 then
                stop_all_sounds(ctx)
                set_listener(ctx, 2, 0, 0, 0, 0, -1)
                play_sound_at_bus(ctx, 'guid://aabb1234', 2, 0, 0, 'voice', 1.0)
            else
                stop_all_sounds(ctx)
            end
        end
        "#,
    )
    .unwrap();
    let mut world = World::new();
    world.spawn((
        Script {
            path: "audio.lua".into(),
        },
        AudioMixer {
            bgm: 0.75,
            ..AudioMixer::default()
        },
    ));
    let (mut runner, _) =
        ScriptedRunner::with_assets(world, asset_paths(Some(&root), &["Shared".into()]));
    let author_data = runner.world().world_data();
    let initial_hash = runner.world().state_hash();
    for gain in [0.125, 0.0625, 0.5, 0.0] {
        runner.step().unwrap();
        let block = runner.audio_samples();
        assert_eq!(block.len(), 800);
        assert!(block.iter().all(|value| *value == sample * gain));
        assert_eq!(runner.world().world_data(), author_data);
        assert_eq!(runner.world().state_hash(), initial_hash);
    }
    fs::remove_dir_all(root).unwrap();
}
