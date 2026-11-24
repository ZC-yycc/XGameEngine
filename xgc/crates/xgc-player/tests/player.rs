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
    for name in ["Fox.gltf", "Fox.bin", "Texture.png"] {
        std::fs::copy(resources.join(name), dir.join(name)).unwrap();
    }
    let mut world = World::new();
    world.spawn((
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
    xgc_store::save_world(&dir.join("world.json"), &world.world_data()).unwrap();

    // 宿主（`run_cooked`）跑 10 帧。
    let stats = xgc_player::run_cooked(&dir, 10).unwrap();

    // 同一份世界用「显式建表 + 每帧 apply_root_motion」跑同样帧数，应当逐位同哈希。
    let data = xgc_store::load_world(&dir.join("world.json")).unwrap();
    let mut expected = World::new();
    expected.restore_world_data(&data).unwrap();
    let root_motion = {
        let mut meshes = MeshCache::new(&dir);
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
fn runs_project_entry_scene_from_manifest() {
    use xgc_store::{save_project, ProjectContent, ProjectManifest};

    let dir = std::env::temp_dir().join(format!(
        "xgc_player_project_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("Scripts")).unwrap();
    std::fs::write(
        dir.join("Scripts/move.lua"),
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
    save_project(
        &dir.join("project.xgc"),
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
            input: None,
        },
    )
    .unwrap();

    let stats = xgc_player::run_cooked(&dir, 3).unwrap();
    assert_eq!(stats.frames, 3);
    assert_eq!(stats.entities, 1);
    assert_eq!(stats.scripts, 1);
    std::fs::remove_dir_all(&dir).unwrap();
}
