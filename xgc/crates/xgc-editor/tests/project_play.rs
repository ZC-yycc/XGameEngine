//! 项目入口的 Play 回归：GUID、清单输入、碰撞矩阵与失败后的编辑态。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use xgc_editor::EditorState;
use xgc_reflect::{Component, Value};
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
            paths: vec!["guid://move-script".into()],
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
        next_entity_id: world.next_entity_id().raw(),
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
        render: Default::default(),
        quality: Default::default(),
        layers: Default::default(),
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

    world.get_mut::<Script>(key).unwrap().paths = vec!["move.lua".into()];
    save_world(&dir.join("scenes/main.json"), &world.world_data()).unwrap();
    std::fs::write(&script, "function update(ctx) set_pos(ctx, 4, 0, 0) end").unwrap();
    manifest.search_paths = vec!["Scripts".into()];
    save_project(&manifest_path, &manifest).unwrap();
    std::fs::write(
        dir.join("Scripts/finish.lua"),
        "function update(ctx) xgc.set(self_id(ctx), 'core.name', 'value', 'done') end",
    )
    .unwrap();
    let mut fallback = EditorState::open(&dir).unwrap();
    fallback
        .edit_field(
            entity,
            Script::ID.0,
            "paths",
            Value::List(vec![
                Value::String("move.lua".into()),
                Value::String("finish.lua".into()),
            ]),
        )
        .unwrap();
    fallback.save().unwrap();
    let mut fallback = EditorState::open(&dir).unwrap();
    fallback.play().unwrap();
    fallback.advance_play(fixed).unwrap();
    assert_eq!(fallback.entity_position(entity), Some([4.0, 0.0, 0.0]));
    let played = fallback.world();
    assert_eq!(
        played
            .get::<Name>(played.key(entity).unwrap())
            .unwrap()
            .value,
        "done"
    );
    fallback.stop();
    let edited = fallback.world();
    assert_eq!(
        edited
            .get::<Name>(edited.key(entity).unwrap())
            .unwrap()
            .value,
        "hero"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn project_play_respects_collision_masks_in_the_play_copy() {
    use xgc_base::layer::LayerIndex;
    use xgc_world::{CollisionLayer, PhysicsBody, Velocity};

    let dir = std::env::temp_dir().join(format!("xgc-editor-play-layers-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    std::fs::write(
        dir.join("project.xgc"),
        r#"(format_version: 2, next_entity_id: 3, name: "Groups", entry_scene: "scenes/main.json",
            scenes: ["scenes/main.json"], content: (models: "Resources", textures: "Resources",
                audio: "Resources", prefabs: "Prefabs", scripts: "Scripts"),
            layers: (masks: {31: 0}))"#,
    )
    .unwrap();
    let scene = dir.join("scenes/main.json");
    let mut world = World::new();
    world.spawn((
        Transform {
            position: [0.0, 2.0, 0.0],
            ..Transform::default()
        },
        PhysicsBody {
            kind: "static".into(),
            shape: "cuboid".into(),
            half_extents: [3.0, 0.5, 3.0],
            ..PhysicsBody::default()
        },
        CollisionLayer {
            index: LayerIndex::new(31).unwrap(),
        },
    ));
    let dropping = world.spawn((
        Transform {
            position: [0.0, 3.05, 0.0],
            ..Transform::default()
        },
        Velocity::default(),
        PhysicsBody {
            shape: "ball".into(),
            ..PhysicsBody::default()
        },
    ));
    let id = world.entity(dropping).unwrap();
    save_world(&scene, &world.world_data()).unwrap();
    let original = std::fs::read(&scene).unwrap();
    let mut editor = EditorState::open(&dir).unwrap();
    editor.play().unwrap();
    editor.pause();
    for _ in 0..60 {
        assert!(editor.step().unwrap().is_some());
    }
    assert!(editor.entity_position(id).unwrap()[1] < 0.0);
    editor.stop();
    assert_eq!(editor.entity_position(id), Some([0.0, 3.05, 0.0]));
    assert_eq!(std::fs::read(&scene).unwrap(), original);
    std::fs::remove_dir_all(&dir).unwrap();
}
