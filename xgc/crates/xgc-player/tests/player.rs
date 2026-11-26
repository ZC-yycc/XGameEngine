//! 播放器：cooked 世界 + 相对路径脚本可跑帧。

use xgc_world::{Script, Transform, World};

#[test]
fn runs_cooked_world_and_scripts() {
    let dir = std::env::temp_dir().join(format!(
        "xgc_player_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("move.lua"),
        "function update(ctx)\n  local p = entity_pos(ctx, self_id(ctx))\n  set_pos(ctx, p.x + 1.0, p.y, p.z)\nend\n",
    )
    .unwrap();
    let mut world = World::new();
    let key = world.spawn((
        Transform::default(),
        Script {
            path: "move.lua".to_string(),
        },
    ));
    let id = world.entity(key).unwrap();
    xgc_store::save_world(&dir.join("world.json"), &world.world_data()).unwrap();
    let stats = xgc_player::run_cooked(&dir, 3).unwrap();
    assert_eq!(stats.frames, 3);
    assert_eq!(stats.entities, 1);
    assert_eq!(stats.scripts, 1);
    let reloaded = xgc_store::load_world(&dir.join("world.json")).unwrap();
    let mut check = World::new();
    check.restore_world_data(&reloaded).unwrap();
    assert!(check.entity_data(id).is_some());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn headless_player_reports_the_frame_and_script_error() {
    let dir = std::env::temp_dir().join(format!(
        "xgc_player_script_error_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("broken.lua"),
        "function update(ctx) error('fixture failure') end",
    )
    .unwrap();
    let mut world = World::new();
    world.spawn((Script {
        path: "broken.lua".into(),
    },));
    xgc_store::save_world(&dir.join("world.json"), &world.world_data()).unwrap();
    let result = xgc_player::run_cooked(&dir, 3);
    std::fs::remove_dir_all(&dir).unwrap();
    let error = result.expect_err("脚本帧失败时应拒绝报告成功");
    for text in ["第 1 帧", "broken.lua", "fixture failure"] {
        assert!(error.contains(text), "缺 {text}：{error}");
    }
}

#[test]
fn player_applies_root_motion_in_the_same_order_as_the_cli() {
    use xgc_render::MeshCache;
    use xgc_sim::{apply_root_motion, Simulation};
    use xgc_world::{Animator, GlobalTransform, MeshRenderer};

    let resources = std::path::Path::new("../../../projects/XGame/Resources");
    if !resources.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let dir = std::env::temp_dir().join(format!(
        "xgc_player_root_motion_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["Fox.gltf", "Fox.gltf.meta", "Fox.bin", "Texture.png"] {
        std::fs::copy(resources.join(name), dir.join(name)).unwrap();
    }
    let mut world = World::new();
    world.spawn((
        Transform::default(),
        GlobalTransform::default(),
        MeshRenderer {
            model: "guid://247e6655b676415d6c77cb91ceb3281b".to_string(),
        },
        Animator {
            clip: "Walk".to_string(),
            autoplay: true,
            root_motion: "(node:\"b_Hip_01\",axes:(true,false,true))".to_string(),
            ..Animator::default()
        },
    ));
    xgc_store::save_world(&dir.join("world.json"), &world.world_data()).unwrap();

    // 宿主（`run_cooked`）跑 10 帧。
    let stats = xgc_player::run_cooked(&dir, 10).unwrap();

    // 同一份世界用「显式建表 + 每帧 apply_root_motion」跑同样帧数，应当逐位同哈希。
    let data = xgc_store::load_world(&dir.join("world.json")).unwrap();
    let mut expected = World::new();
    expected.restore_world_data(&data).unwrap();
    let root_motion = {
        let catalog = xgc_asset::scan(&dir).unwrap();
        let mut meshes = MeshCache::with_catalog(&dir, catalog);
        meshes.root_motion_library(&expected)
    };
    assert_eq!(root_motion.len(), 1);
    let mut simulation = Simulation::new(expected);
    for _ in 0..10 {
        simulation.step(Vec::new()).unwrap();
        apply_root_motion(simulation.world_mut(), 1.0 / 60.0, &root_motion);
    }
    assert_eq!(
        stats.state_hash,
        simulation.world().state_hash(),
        "player 宿主必须和显式流程逐位一致（漏接根运动时这里会分叉）"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn selects_a_named_scene_from_the_project_manifest() {
    use xgc_store::{save_project, ProjectContent, ProjectManifest};

    let dir = std::env::temp_dir().join(format!(
        "xgc_player_project_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("Shared/Scripts")).unwrap();
    std::fs::write(
        dir.join("Shared/Scripts/move.lua"),
        "function update(ctx)\n  local p = entity_pos(ctx, self_id(ctx))\n  set_pos(ctx, p.x + 1.0, p.y, p.z)\nend\n",
    )
    .unwrap();
    let mut world = World::new();
    world.spawn((
        Transform::default(),
        Script {
            path: "Scripts/move.lua".to_string(),
        },
    ));
    xgc_store::save_world(&dir.join("scenes/main.json"), &world.world_data()).unwrap();
    let mut extra = World::new();
    extra.spawn((Transform::default(),));
    extra.spawn((Transform::default(),));
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
            search_paths: vec!["Shared".into()],
            input: None,
        },
    )
    .unwrap();

    let stats = xgc_player::run_cooked(&dir, 3).unwrap();
    assert_eq!(stats.frames, 3);
    assert_eq!(stats.entities, 1);
    assert_eq!(stats.scripts, 1);

    let selected =
        xgc_player::run_cooked_scene(&dir, 3, Some("extra")).expect("按文件名主干选场景");
    assert_eq!(selected.entities, 2);
    assert_eq!(selected.scripts, 0);

    let exact = xgc_player::run_cooked_scene(&dir, 3, Some("scenes/extra.json")).unwrap();
    assert_eq!(exact.entities, 2);
    let from_manifest = xgc_player::run_cooked(&dir.join("project.xgc"), 3).unwrap();
    assert_eq!(from_manifest, stats, "脚本和资源应从项目根加载");

    let missing = xgc_player::run_cooked_scene(&dir, 3, Some("nope")).unwrap_err();
    assert!(missing.contains("没有场景"), "{missing}");

    let cooked = std::env::temp_dir().join(format!(
        "xgc_player_cooked_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&cooked).unwrap();
    xgc_store::save_world(&cooked.join("world.json"), &world.world_data()).unwrap();
    let cooked_scene = xgc_player::run_cooked_scene(&cooked, 1, Some("extra")).unwrap_err();
    assert!(cooked_scene.contains("没有场景清单"), "{cooked_scene}");
    std::fs::remove_dir_all(&cooked).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}
