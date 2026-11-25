//! 编辑器状态模型回归：选择 / Inspector / 编辑 / 保存 / Play 快照恢复。

use xgc_base::EntityId;
use xgc_editor::EditorState;
use xgc_reflect::Value;
use xgc_world::{GlobalTransform, Name, Transform, World};

fn demo() -> (EditorState, EntityId) {
    let mut world = World::new();
    let entity = world.spawn((
        Name {
            value: "hero".to_string(),
        },
        Transform::default(),
    ));
    let id = world.entity(entity).unwrap();
    (EditorState::new(world), id)
}

#[test]
fn lists_names_and_inspects_components() {
    let (mut editor, id) = demo();
    let rows = editor.entities();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "hero");
    editor.select(Some(id));
    let inspector = editor.inspector();
    assert!(inspector.iter().any(|row| row.name == "core.transform"));
    assert!(inspector.iter().any(|row| row.name == "core.name"));
}

#[test]
fn edits_fields_and_saves_roundtrip() {
    let (mut editor, id) = demo();
    editor
        .edit_field(id, 0x0101, "position", Value::Vec3([1.0, 2.0, 3.0]))
        .unwrap();
    assert!(editor.is_dirty());
    let dir = std::env::temp_dir().join(format!(
        "xgc_editor_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("world.json");
    editor.save_as(&path).unwrap();
    assert!(!editor.is_dirty());
    let loaded = EditorState::open(&path).unwrap();
    assert_eq!(loaded.entities()[0].name, "hero");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn play_runs_and_stop_restores_snapshot() {
    let (mut editor, id) = demo();
    editor.play().unwrap();
    assert!(editor.is_playing());
    assert_eq!(editor.step().unwrap().unwrap().frame, 0);
    assert!(editor
        .edit_field(id, 0x0101, "position", Value::Vec3([9.0, 0.0, 0.0]))
        .is_err());
    editor.stop().unwrap();
    assert!(!editor.is_playing());
    assert_eq!(editor.selected(), None);
}

#[test]
fn spawns_and_deletes_entities() {
    let mut editor = EditorState::default();
    let id = editor.spawn_entity("box").unwrap();
    assert_eq!(editor.entities().len(), 1);
    assert_eq!(editor.selected(), Some(id));
    editor.delete_selected().unwrap();
    assert_eq!(editor.entities().len(), 0);
    assert_eq!(editor.selected(), None);
}

#[test]
fn adds_and_removes_components() {
    let mut editor = EditorState::default();
    let id = editor.spawn_entity("box").unwrap();
    editor.add_component(id, "core.velocity").unwrap();
    let names: Vec<_> = editor
        .inspector()
        .into_iter()
        .map(|component| component.name)
        .collect();
    assert!(names.contains(&"core.velocity".to_string()));
    editor.remove_component(id, 0x0102).unwrap();
    let names: Vec<_> = editor
        .inspector()
        .into_iter()
        .map(|component| component.name)
        .collect();
    assert!(!names.contains(&"core.velocity".to_string()));
}

#[test]
fn undo_redo_restores_field_edits() {
    let (mut editor, id) = demo();
    editor
        .edit_field(id, 0x0101, "position", Value::Vec3([5.0, 6.0, 7.0]))
        .unwrap();
    assert!(editor.undo().unwrap());
    assert_eq!(
        editor
            .world()
            .get::<Transform>(editor.world().key(id).unwrap())
            .unwrap()
            .position,
        [0.0, 0.0, 0.0]
    );
    assert!(editor.redo().unwrap());
    assert_eq!(
        editor
            .world()
            .get::<Transform>(editor.world().key(id).unwrap())
            .unwrap()
            .position,
        [5.0, 6.0, 7.0]
    );
}

#[test]
fn undo_restores_spawned_entities() {
    let mut editor = EditorState::default();
    let id = editor.spawn_entity("box").unwrap();
    assert!(editor.undo().unwrap());
    assert!(editor.world().key(id).is_none());
    assert!(editor.redo().unwrap());
    assert!(editor.world().key(id).is_some());
}

#[test]
fn undo_is_rejected_during_play() {
    let (mut editor, _) = demo();
    editor.play().unwrap();
    assert!(editor.undo().is_err());
    assert!(editor.redo().is_err());
}

#[test]
fn demo_scene_has_camera_and_drawable() {
    let editor = EditorState::demo();
    assert_eq!(editor.entities().len(), 2);
    assert!(xgc_render::camera_view_projection(editor.world(), 800, 450).is_some());
    let list = xgc_render::build_draw_list(editor.world());
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.items[0].model, "builtin://quad");
}

#[test]
fn orbit_camera_places_and_faces_origin() {
    let mut editor = EditorState::demo();
    let camera = editor.camera_entity().expect("demo 应有相机");
    editor.set_camera_orbit(0.0, 0.0, 5.0).unwrap();
    let position = editor
        .world()
        .get::<GlobalTransform>(editor.world().key(camera).unwrap())
        .unwrap()
        .position;
    assert!((position[0]).abs() < 1e-5);
    assert!((position[2] - 5.0).abs() < 1e-5);

    editor
        .set_camera_orbit(std::f32::consts::FRAC_PI_2, 0.0, 5.0)
        .unwrap();
    let position = editor
        .world()
        .get::<GlobalTransform>(editor.world().key(camera).unwrap())
        .unwrap()
        .position;
    assert!((position[0] - 5.0).abs() < 1e-4);
    assert!((position[2]).abs() < 1e-4);
    assert!(xgc_render::camera_view_projection(editor.world(), 800, 450).is_some());
}

#[test]
fn gizmo_moves_transform_and_global() {
    let mut editor = EditorState::demo();
    let id = editor.entities()[1].id;
    assert_eq!(editor.entity_position(id), Some([0.0, 0.0, 0.0]));
    editor.set_entity_position(id, [1.0, 2.0, 3.0]).unwrap();
    let world = editor.world();
    let key = world.key(id).unwrap();
    assert_eq!(
        world.get::<Transform>(key).unwrap().position,
        [1.0, 2.0, 3.0]
    );
    assert_eq!(
        world.get::<GlobalTransform>(key).unwrap().position,
        [1.0, 2.0, 3.0]
    );
}

#[test]
fn scale_and_rotation_sync_global_transform() {
    let mut editor = EditorState::demo();
    let id = editor.entities()[1].id;
    editor.set_entity_scale(id, [2.0, 3.0, 4.0]).unwrap();
    editor.rotate_entity(id, [0.0, 1.0, 0.0], 90.0).unwrap();
    let world = editor.world();
    let key = world.key(id).unwrap();
    assert_eq!(world.get::<Transform>(key).unwrap().scale, [2.0, 3.0, 4.0]);
    assert_eq!(
        world.get::<GlobalTransform>(key).unwrap().scale,
        [2.0, 3.0, 4.0]
    );
    let rotation = world.get::<Transform>(key).unwrap().rotation;
    assert!((rotation[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
    assert_eq!(
        rotation,
        world.get::<GlobalTransform>(key).unwrap().rotation
    );
}

#[test]
fn snapping_quantizes_to_step() {
    assert_eq!(xgc_editor::snap(1.24, 0.5), 1.0);
    assert_eq!(xgc_editor::snap(1.26, 0.5), 1.5);
    assert_eq!(xgc_editor::snap(7.0, 15.0), 0.0);
    assert_eq!(xgc_editor::snap(1.234, 0.0), 1.234);
}

#[test]
fn editor_play_runs_scripts_and_reads_input() {
    use xgc_sim::{InputState, KeyCode};
    use xgc_world::{Script, Transform};

    let dir = std::env::temp_dir().join(format!(
        "xgc_editor_scripts_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("input.ron"),
        "InputMap(actions: { \"Jump\": [Key(Space)] }, axes: {})",
    )
    .unwrap();
    std::fs::write(
        dir.join("move.lua"),
        "function update(ctx)\n  if action_pressed(ctx, 'Jump') then\n    local p = entity_pos(ctx, self_id(ctx))\n    set_pos(ctx, p.x + 1.0, p.y, p.z)\n  end\nend\n",
    )
    .unwrap();
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        Script {
            path: "move.lua".to_string(),
        },
    ));
    let id = world.entity(entity).unwrap();
    let mut editor = EditorState::new(world);
    editor.play_with_scripts(&dir).unwrap();
    assert!(editor.input_map().is_some(), "input.ron 应被解析");
    let mut input = InputState::default();
    input.keys.insert(KeyCode::Space);
    editor.apply_input(&input);
    for _ in 0..3 {
        editor.step().unwrap();
    }
    assert_eq!(editor.script_entity_count(), 1, "脚本应已装载");
    assert_eq!(editor.entity_position(id), Some([3.0, 0.0, 0.0]));
    editor.stop().unwrap();
    assert_eq!(editor.entity_position(id), Some([0.0, 0.0, 0.0]));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn editor_play_applies_root_motion_like_the_cli() {
    use xgc_world::{Animator, MeshRenderer, Transform};

    let resources = std::path::Path::new("../../../projects/XGame/Resources");
    if !resources.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let dir = std::env::temp_dir().join(format!(
        "xgc_editor_root_motion_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    // 编辑器宿主只认项目根下的资产；开箱即用的 Fox 直接拷过来（约 180 KiB）。
    for name in ["Fox.gltf", "Fox.bin", "Texture.png"] {
        std::fs::copy(resources.join(name), dir.join(name)).unwrap();
    }
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        GlobalTransform::default(),
        MeshRenderer {
            model: "Fox.gltf".to_string(),
        },
        Animator {
            clip: "Walk".to_string(),
            autoplay: true,
            root_motion: "(node:\"b_Hip_01\",axes:(true,false,true))".to_string(),
            ..Animator::default()
        },
    ));
    let id = world.entity(entity).unwrap();
    let mut editor = EditorState::new(world);
    editor.play_with_scripts(&dir).unwrap();
    for _ in 0..60 {
        editor.step().unwrap();
    }
    let position = editor.entity_position(id).expect("Fox 应有 Transform");
    assert!(
        position[0].abs() + position[2].abs() > 1e-4,
        "编辑器宿主必须像 CLI 一样应用根运动（位置不该一直停在原点），实际 {position:?}"
    );
    // 横向位移必须是根运动给的，纵向（y）不该被根运动改写。
    assert!(
        position[1].abs() < 1e-6,
        "根运动不改 y 轴，实际 {position:?}"
    );
    editor.stop().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn opens_a_migrated_project_through_its_manifest() {
    use xgc_store::{save_project, ProjectContent, ProjectManifest};

    let dir = std::env::temp_dir().join(format!(
        "xgc_editor_project_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let mut world = World::new();
    world.spawn((
        Name {
            value: "hero".to_string(),
        },
        Transform::default(),
    ));
    xgc_store::save_world(&dir.join("scenes/main.json"), &world.world_data()).unwrap();
    std::fs::create_dir_all(dir.join("Config")).unwrap();
    std::fs::write(
        dir.join("Config/controls.ron"),
        r#"InputMap(actions: {"Jump": [Key(Space)]})"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("input.ron"),
        r#"InputMap(actions: {"WrongMap": [Key(Space)]})"#,
    )
    .unwrap();
    let manifest_path = dir.join("project.xgc");
    save_project(
        &manifest_path,
        &ProjectManifest {
            format_version: xgc_store::PROJECT_FORMAT_VERSION,
            name: "demo".to_string(),
            entry_scene: "scenes/main.json".to_string(),
            scenes: vec!["scenes/main.json".to_string()],
            content: ProjectContent {
                models: "Resources".to_string(),
                textures: "Resources".to_string(),
                audio: "Resources".to_string(),
                shaders: None,
                prefabs: "Prefabs".to_string(),
                scripts: "Scripts".to_string(),
            },
            search_paths: Vec::new(),
            input: Some("./Config\\controls.ron".to_string()),
        },
    )
    .unwrap();

    let mut editor = EditorState::open(&dir).unwrap();
    assert_eq!(editor.project_name(), Some("demo"));
    editor.play_with_scripts(&dir).unwrap();
    assert_eq!(editor.input_map().unwrap().action_names(), ["Jump"]);
    editor.stop().unwrap();
    assert_eq!(editor.project_root(), Some(dir.as_path()));
    assert_eq!(editor.entities().len(), 1);
    assert!(editor.path().unwrap().ends_with("scenes/main.json"));
    editor
        .edit_field(
            editor.entities()[0].id,
            0x0101,
            "position",
            Value::Vec3([1.0, 2.0, 3.0]),
        )
        .unwrap();
    editor.save().unwrap();
    let reloaded = EditorState::open(&dir).unwrap();
    assert_eq!(reloaded.project_name(), Some("demo"));
    assert_eq!(reloaded.entities().len(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn switches_scenes_through_the_project_manifest() {
    use xgc_store::{save_project, ProjectContent, ProjectManifest};

    let dir = std::env::temp_dir().join(format!(
        "xgc_editor_switch_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    let mut main = World::new();
    main.spawn((
        Name {
            value: "main".to_string(),
        },
        Transform::default(),
    ));
    xgc_store::save_world(&dir.join("scenes/main.json"), &main.world_data()).unwrap();
    let mut extra = World::new();
    for name in ["a", "b"] {
        extra.spawn((
            Name {
                value: name.to_string(),
            },
            Transform::default(),
        ));
    }
    xgc_store::save_world(&dir.join("scenes/extra.json"), &extra.world_data()).unwrap();
    save_project(
        &dir.join("project.xgc"),
        &ProjectManifest {
            format_version: xgc_store::PROJECT_FORMAT_VERSION,
            name: "demo".to_string(),
            entry_scene: "scenes/main.json".to_string(),
            scenes: vec![
                "scenes/main.json".to_string(),
                "scenes/extra.json".to_string(),
            ],
            content: ProjectContent {
                models: "Resources".to_string(),
                textures: "Resources".to_string(),
                audio: "Resources".to_string(),
                shaders: None,
                prefabs: "Prefabs".to_string(),
                scripts: "Scripts".to_string(),
            },
            search_paths: Vec::new(),
            input: None,
        },
    )
    .unwrap();

    let mut editor = EditorState::open(&dir).unwrap();
    assert_eq!(editor.current_scene(), Some("scenes/main.json"));
    assert_eq!(editor.scenes().len(), 2);
    editor.open_scene("scenes/extra.json").unwrap();
    assert_eq!(editor.entities().len(), 2);
    assert_eq!(editor.current_scene(), Some("scenes/extra.json"));
    assert!(editor.path().unwrap().ends_with("scenes/extra.json"));

    editor
        .edit_field(
            editor.entities()[0].id,
            0x0101,
            "position",
            Value::Vec3([1.0, 0.0, 0.0]),
        )
        .unwrap();
    assert!(
        editor.open_scene("scenes/main.json").is_err(),
        "脏场景拒绝切换"
    );
    editor.save().unwrap();
    editor.open_scene("scenes/main.json").unwrap();
    assert_eq!(editor.entities().len(), 1);
    assert!(editor.open_scene("scenes/missing.json").is_err());
    assert!(EditorState::demo().open_scene("scenes/main.json").is_err());

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn lists_project_assets_without_meta_files() {
    use xgc_editor::AssetKind;
    use xgc_store::{save_project, ProjectContent, ProjectManifest};

    let dir = std::env::temp_dir().join(format!(
        "xgc_editor_assets_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    std::fs::create_dir_all(dir.join("Resources/Characters")).unwrap();
    std::fs::create_dir_all(dir.join("Prefabs")).unwrap();
    std::fs::create_dir_all(dir.join("Scripts")).unwrap();
    std::fs::write(dir.join("Resources/Characters/hero.glb"), b"glb").unwrap();
    std::fs::write(dir.join("Resources/Characters/hero.glb.meta"), b"guid").unwrap();
    std::fs::write(dir.join("Resources/hero.png"), b"png").unwrap();
    std::fs::write(dir.join("Prefabs/Pillar.prefab.json"), b"prefab").unwrap();
    std::fs::write(dir.join("Scripts/move.lua"), b"function update() end").unwrap();
    let mut world = World::new();
    world.spawn((
        Name {
            value: "main".to_string(),
        },
        Transform::default(),
    ));
    xgc_store::save_world(&dir.join("scenes/main.json"), &world.world_data()).unwrap();
    save_project(
        &dir.join("project.xgc"),
        &ProjectManifest {
            format_version: xgc_store::PROJECT_FORMAT_VERSION,
            name: "assets".to_string(),
            entry_scene: "scenes/main.json".to_string(),
            scenes: vec!["scenes/main.json".to_string()],
            content: ProjectContent {
                models: "./Resources//".to_string(),
                textures: "Resources/Characters".to_string(),
                audio: "Resources".to_string(),
                shaders: None,
                prefabs: "Prefabs".to_string(),
                scripts: "Scripts".to_string(),
            },
            search_paths: Vec::new(),
            input: None,
        },
    )
    .unwrap();

    let editor = EditorState::open(&dir).unwrap();
    let assets = editor.assets();
    assert_eq!(assets.len(), 4, "同目录别名与嵌套内容根不能重复列出资产");
    assert!(assets.iter().any(|asset| {
        asset.path == "Resources/Characters/hero.glb" && asset.kind == AssetKind::Model
    }));
    assert!(assets
        .iter()
        .any(|asset| { asset.path == "Resources/hero.png" && asset.kind == AssetKind::Texture }));
    assert!(assets.iter().any(|asset| {
        asset.path == "Prefabs/Pillar.prefab.json" && asset.kind == AssetKind::Prefab
    }));
    assert!(assets
        .iter()
        .any(|asset| { asset.path == "Scripts/move.lua" && asset.kind == AssetKind::Script }));
    assert!(assets.iter().all(|asset| !asset.path.ends_with(".meta")));
    assert!(EditorState::demo().assets().is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}
