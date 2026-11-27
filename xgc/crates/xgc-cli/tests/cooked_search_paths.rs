use std::fs;
use std::path::PathBuf;
use std::process::Command;

use xgc_store::{
    load_world, save_project, save_world, ProjectContent, ProjectManifest, PROJECT_FORMAT_VERSION,
};
use xgc_world::{MeshRenderer, Name, Script, Transform, World};

#[test]
fn cooked_run_uses_the_same_ordered_search_roots_as_the_project() {
    let fixture =
        Temp(std::env::temp_dir().join(format!("xgc-cooked-search-{}", std::process::id())));
    let source = fixture.0.join("source");
    let cooked = fixture.0.join("cooked");
    for dir in ["scenes", "Zulu", "Alpha", "Config"] {
        fs::create_dir_all(source.join(dir)).unwrap();
    }
    let project = ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        name: "Search fixture".into(),
        entry_scene: "scenes/entry.json".into(),
        scenes: vec!["scenes/entry.json".into()],
        content: ProjectContent {
            models: "Resources".into(),
            textures: "Resources".into(),
            audio: "Resources".into(),
            shaders: None,
            prefabs: "Prefabs".into(),
            scripts: "Scripts".into(),
        },
        search_paths: vec!["Zulu".into(), "Alpha".into()],
        input: Some("Config/actions.ron".into()),
        render: xgc_render::RenderSettings {
            exposure: 2.25,
            tone_map: xgc_render::ToneMapMode::Off,
            ..Default::default()
        },
        quality: xgc_render::RenderQuality {
            msaa: Some(1),
            lod_scale: Some(0.75),
            ..Default::default()
        },
    };
    save_project(&source.join("project.xgc"), &project).unwrap();
    fs::write(
        source.join("Zulu/move.lua"),
        "function update(ctx)\n  xgc.set(self_id(ctx), 'core.name', 'value', 'moved')\n  set_pos(ctx, 3, 0, 0)\nend",
    )
    .unwrap();
    fs::write(source.join("Alpha/move.lua"), "local =").unwrap();
    fs::write(
        source.join("Alpha/finish.lua"),
        "function update(ctx) set_pos(ctx, 7, 0, 0) end",
    )
    .unwrap();
    fs::write(
        source.join("Alpha/finish.lua.meta"),
        r#"(guid: "aabb1234")"#,
    )
    .unwrap();
    fs::write(source.join("Alpha/model.glb"), "model fixture").unwrap();
    fs::write(source.join("Config/actions.ron"), "(actions: {}, axes: {})").unwrap();
    fs::write(source.join("input.ron"), "unused").unwrap();
    let mut world = World::new();
    let key = world.spawn((
        Transform::default(),
        Name::default(),
        Script {
            paths: vec!["move.lua".into(), "guid://aabb1234".into()],
        },
        MeshRenderer {
            model: "model.glb".into(),
        },
    ));
    let entity = world.entity(key).unwrap();
    let scene = source.join(&project.entry_scene);
    save_world(&scene, &world.world_data()).unwrap();

    success(xgc().arg("pack").args([&scene, &source, &cooked]));
    assert!(cooked.join("Zulu/move.lua").is_file());
    assert!(cooked.join("Alpha/finish.lua").is_file());
    assert!(cooked.join("Alpha/finish.lua.meta").is_file());
    assert!(cooked.join("Alpha/model.glb").is_file());
    assert!(cooked.join("Config/actions.ron").is_file());
    assert!(!cooked.join("Alpha/move.lua").exists());
    assert!(!cooked.join("input.ron").exists());
    let manifest = cooked.join("cook.json");
    let manifest_bytes = fs::read(&manifest).unwrap();
    let runtime = xgc_asset::verify_manifest(&cooked)
        .unwrap()
        .unwrap()
        .runtime;
    assert_eq!(
        (runtime.render, runtime.quality),
        (project.render, project.quality)
    );
    success(xgc().arg("pack").args([&scene, &source, &cooked]));
    assert_eq!(fs::read(&manifest).unwrap(), manifest_bytes);

    let snapshot = fixture.0.join("snapshot.json");
    let expected = success(
        xgc()
            .arg("run-scripts")
            .args([&scene, &source])
            .arg("1")
            .arg(&snapshot),
    );
    world
        .restore_world_data(&load_world(&snapshot).unwrap())
        .unwrap();
    let key = world.key(entity).unwrap();
    assert_eq!(world.get::<Name>(key).unwrap().value, "moved");
    assert_eq!(
        world.get::<Transform>(key).unwrap().position,
        [7.0, 0.0, 0.0]
    );
    fs::rename(&source, fixture.0.join("relocated")).unwrap();
    let actual = success(xgc().arg("run-cooked").arg(&cooked).arg("1"));
    assert!(actual.contains("scripts=2"), "{actual}");
    assert_eq!(hash(&actual), hash(&expected));

    let mut changed: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
    changed["runtime"]["search_paths"]
        .as_array_mut()
        .unwrap()
        .reverse();
    fs::write(manifest, serde_json::to_vec(&changed).unwrap()).unwrap();
    let rejected = xgc()
        .arg("run-cooked")
        .arg(&cooked)
        .arg("1")
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("manifest 签名不匹配"));
}

fn xgc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_xgc"))
}

fn hash(output: &str) -> &str {
    output
        .split_whitespace()
        .find_map(|word| word.strip_prefix("hash="))
        .unwrap()
}

fn success(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
