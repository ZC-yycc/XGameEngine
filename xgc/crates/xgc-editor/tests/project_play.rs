//! 项目入口的 Play 回归：GUID、清单输入与失败后的编辑态。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use xgc_editor::EditorState;
use xgc_reflect::Value;
use xgc_sim::{InputState, KeyCode};
use xgc_store::{save_project, save_world, ProjectContent, ProjectManifest};
use xgc_world::{Name, Script, Transform, World};

#[test]
fn project_play_uses_guid_script_and_manifest_input() {
    let dir = std::env::temp_dir().join(format!(
        "xgc_editor_project_play_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("Config")).unwrap();
    std::fs::create_dir_all(dir.join("Scripts")).unwrap();
    let script = dir.join("Scripts/move.lua");
    std::fs::write(
        &script,
        r#"
function update(ctx)
    if action_pressed(ctx, 'Jump') then
        local p = entity_pos(ctx, self_id(ctx))
        set_pos(ctx, p.x + 1.0, p.y, p.z)
    end
end
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Scripts/move.lua.meta"),
        r#"(guid: "move-script")"#,
    )
    .unwrap();
    let mut world = World::new();
    let key = world.spawn((
        Name {
            value: "hero".into(),
        },
        Transform::default(),
        Script {
            path: "guid://move-script".into(),
        },
    ));
    let entity = world.entity(key).unwrap();
    save_world(&dir.join("scenes/main.json"), &world.world_data()).unwrap();
    let controls = dir.join("Config/controls.ron");
    std::fs::write(&controls, r#"InputMap(actions: {"Jump": [Key(Space)]})"#).unwrap();
    std::fs::write(
        dir.join("input.ron"),
        r#"InputMap(actions: {"WrongMap": [Key(Space)]})"#,
    )
    .unwrap();
    let manifest_path = dir.join("project.xgc");
    let mut manifest = ProjectManifest {
        format_version: xgc_store::PROJECT_FORMAT_VERSION,
        name: "demo".into(),
        entry_scene: "scenes/main.json".into(),
        scenes: vec!["scenes/main.json".into()],
        content: ProjectContent {
            models: "Resources".into(),
            textures: "Resources".into(),
            audio: "Resources".into(),
            shaders: None,
            prefabs: "Prefabs".into(),
            scripts: "Scripts".into(),
        },
        search_paths: Vec::new(),
        input: Some("./Config\\controls.ron".into()),
    };
    save_project(&manifest_path, &manifest).unwrap();

    let mut editor = EditorState::open(&dir).unwrap();
    assert_eq!(editor.project_name(), Some("demo"));
    editor.play().unwrap();
    assert_eq!(editor.input_map().unwrap().action_names(), ["Jump"]);
    let mut input = InputState::default();
    input.keys.insert(KeyCode::Space);
    editor.apply_input(&input);
    let fixed = Duration::from_secs_f32(1.0 / 60.0);
    editor.advance_play(fixed).unwrap();
    assert_eq!(editor.entity_position(entity), Some([1.0, 0.0, 0.0]));
    editor.stop();
    assert_eq!(editor.entity_position(entity), Some([0.0, 0.0, 0.0]));
    assert_eq!(editor.project_root(), Some(dir.as_path()));
    assert!(editor.path().unwrap().ends_with("scenes/main.json"));

    std::fs::write(&script, "function update(ctx) error('play-regression') end").unwrap();
    editor.play().unwrap();
    let err = editor.advance_play(fixed * 2).unwrap_err();
    assert!(
        err.contains("第 1 帧") && err.contains("play-regression"),
        "{err}"
    );
    assert!(editor.is_paused());
    assert_eq!(editor.advance_play(fixed).unwrap(), None);
    editor.stop();

    std::fs::write(&controls, "invalid input map").unwrap();
    assert!(editor.play().unwrap_err().contains("controls.ron"));
    assert!(!editor.is_playing());
    std::fs::remove_file(&controls).unwrap();
    assert!(editor.play().unwrap_err().contains("读取项目输入映射失败"));
    assert!(!editor.is_playing());

    manifest.input = None;
    save_project(&manifest_path, &manifest).unwrap();
    let mut editor = EditorState::open(&manifest_path).unwrap();
    editor.play().unwrap();
    assert!(editor.input_map().is_none());
    editor.stop();
    editor
        .edit_field(entity, 0x0101, "position", Value::Vec3([1.0, 2.0, 3.0]))
        .unwrap();
    editor.save().unwrap();
    let reloaded = EditorState::open(&dir).unwrap();
    assert_eq!(reloaded.project_name(), Some("demo"));
    assert_eq!(reloaded.entity_position(entity), Some([1.0, 2.0, 3.0]));

    world.get_mut::<Script>(key).unwrap().path = "move.lua".into();
    save_world(&dir.join("scenes/main.json"), &world.world_data()).unwrap();
    std::fs::write(&script, "function update(ctx) set_pos(ctx, 4, 0, 0) end").unwrap();
    manifest.search_paths = vec!["Scripts".into()];
    save_project(&manifest_path, &manifest).unwrap();
    let mut fallback = EditorState::open(&dir).unwrap();
    fallback.play().unwrap();
    fallback.advance_play(fixed).unwrap();
    assert_eq!(fallback.entity_position(entity), Some([4.0, 0.0, 0.0]));
    std::fs::remove_dir_all(&dir).unwrap();
}
