use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use xgc_host::{asset_paths, ScriptedRunner, AUDIO_SAMPLE_RATE};
use xgc_sim::{load_wav, write_wav_mono};
use xgc_world::{
    AudioMixer, AudioSource, Command, Component, ComponentData, GlobalTransform, Name, Parent,
    Script, Transform, Value, World,
};

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

#[test]
fn autoplay_follows_the_entity_and_stops_when_the_source_is_removed() {
    let root = std::env::temp_dir().join(format!(
        "xgc_autoplay_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let clip = root.join("tone.wav");
    let mut pcm = vec![0.5; 800];
    pcm.extend([0.25; 800]);
    pcm.extend([0.75; 800]);
    write_wav_mono(&clip, AUDIO_SAMPLE_RATE, &pcm).unwrap();
    let pcm = load_wav(&clip).unwrap().samples;
    fs::write(
        root.join("source.lua"),
        r#"
        local frame = 0
        function update(ctx)
            frame = frame + 1
            if frame == 2 then
                xgc.set(entity_id(ctx, 'parent'), 'core.transform', 'position', {x=2, y=0, z=0})
                xgc.add(ctx.entity, 'core.velocity')
            elseif frame == 3 then
                xgc.remove(ctx.entity, 'audio.source')
                play_sound(ctx, 'tone.wav', 0.5, 1)
            elseif frame == 4 then
                stop_all_sounds(ctx)
                xgc.add(ctx.entity, 'audio.source')
                xgc.set(ctx.entity, 'audio.source', 'clip', 'tone.wav')
                xgc.set(ctx.entity, 'audio.source', 'autoplay', true)
                xgc.set(ctx.entity, 'audio.source', 'spatial', false)
                xgc.set(ctx.entity, 'audio.source', 'looping', true)
            elseif frame == 5 then
                stop_sound(ctx, 'tone.wav')
            end
        end
        "#,
    )
    .unwrap();
    let mut world = World::new();
    let parent = world.spawn((
        Name {
            value: "parent".into(),
        },
        Transform {
            position: [1.0, 0.0, 0.0],
            ..Transform::default()
        },
        GlobalTransform::default(),
    ));
    let source = world.spawn((
        Transform {
            position: [1.0, 0.0, 0.0],
            ..Transform::default()
        },
        GlobalTransform::default(),
        Parent {
            entity: world.entity(parent).unwrap(),
        },
        AudioSource {
            clip: "tone.wav".into(),
            autoplay: true,
            looping: true,
            ..AudioSource::default()
        },
    ));
    let plan = world.plan(vec![Command::AddComponent {
        entity: world.entity(source).unwrap(),
        component: ComponentData {
            component: Script::ID,
            fields: vec![("path".into(), Value::String("source.lua".into()))],
        },
    }]);
    world.apply(&plan).unwrap();
    let (mut runner, _) = ScriptedRunner::with_assets(world, asset_paths(Some(&root), &[]));
    let expected = [pcm[0] / 3.0, pcm[800] / 4.0, pcm[0] / 2.0, pcm[0], 0.0, 0.0];
    for (index, expected) in expected.into_iter().enumerate() {
        runner.step().unwrap();
        let actual = runner.audio_samples();
        assert!(
            actual
                .iter()
                .all(|sample| (*sample - expected).abs() < 1e-7),
            "第 {} 帧首个采样为 {}，预期 {expected}",
            index + 1,
            actual[0]
        );
    }
    fs::remove_dir_all(root).unwrap();
}
