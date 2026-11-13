use std::collections::BTreeMap;
use xgame_core::scene_v2::{ComponentData, PrefabV2, SceneV2};
use xgame_core::{
    Animator, AudioSource, Camera, CharacterController, Entity, MaterialOverride, ParticleEmitter,
    PhysicsBody, Prefab, RuntimeComponentKind, Scene, UiElement,
};

fn structural_apply_fixture(
    tag: &str,
) -> (
    std::path::PathBuf,
    xgame_core::ProjectHandle,
    String,
    PrefabV2,
    Vec<String>,
) {
    let base = std::env::temp_dir().join(format!("xgame-v2-apply-{tag}-{}", std::process::id()));
    assert!(!base.exists());
    let project = xgame_core::ProjectHandle::create(&base, "Project").unwrap();
    let source = "Prefabs/Actor.prefab.ron".to_owned();
    let mut legacy = Prefab::new("Actor");
    let mut root = Entity::new("Actor");
    root.camera = Some(Camera::default());
    root.audio = Some(AudioSource::default());
    legacy.entities.push(root);
    let asset = PrefabV2::from_v1(&legacy).unwrap();
    project.write_prefab_v2(&source, &asset).unwrap();

    let scenes = vec![
        project.manifest().entry_scene.clone(),
        "Scenes/Other.ron".to_owned(),
    ];
    for (index, relative) in scenes.iter().enumerate() {
        let mut scene = Scene::new(format!("Scene{index}"));
        scene.entities = legacy.instantiate(&source);
        let v2 = SceneV2::from_v1_with_prefabs(
            &scene,
            &BTreeMap::from([(source.clone(), asset.clone())]),
        )
        .unwrap();
        if project.path(relative).is_file() {
            std::fs::remove_file(project.path(relative)).unwrap();
        }
        project.write_scene_v2(relative, &v2).unwrap();
    }
    (base, project, source, asset, scenes)
}

#[test]
fn v2_reference_walk_uses_the_existing_asset_database_rules() {
    let base = std::env::temp_dir().join(format!("xgame-v2-reference-{}", std::process::id()));
    assert!(!base.exists(), "测试目录不能覆盖其他进程的文件");
    let project = xgame_core::ProjectHandle::create(&base, "Project").unwrap();
    for path in [
        "Resources/Models/Box.glb",
        "Resources/Models/BoxLow.glb",
        "Resources/Materials/Box.mat",
        "Resources/Audio/Sound.wav",
        "Resources/Textures/Button.png",
        "Scripts/actor.lua",
    ] {
        let full = project.path(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, b"asset").unwrap();
    }
    let mut assets = project.assets();
    assets.refresh();
    assets.ensure_metas().unwrap();

    let mut legacy = Scene::new("References");
    let mut actor = Entity::new("Actor");
    actor.model = "Resources/Models/Box.glb".into();
    actor.lod.levels.push(xgame_core::LodLevel::new(
        "Resources/Models/BoxLow.glb",
        50.0,
    ));
    actor.material_asset = Some("Resources/Materials/Box.mat".into());
    actor.audio = Some(AudioSource {
        clip: "Resources/Audio/Sound.wav".into(),
        ..Default::default()
    });
    let mut ui = UiElement::default();
    ui.image = Some(xgame_core::ui::UiImageStyle {
        path: "Resources/Textures/Button.png".into(),
        ..Default::default()
    });
    actor.ui = Some(ui);
    actor.scripts.push("Scripts/actor.lua".into());
    legacy.add_entity(actor);
    let mut v2 = SceneV2::from_v1_plain(&legacy).unwrap();

    let mut expected = legacy.clone();
    xgame_core::encode_scene_references(&mut expected, &assets);
    xgame_core::encode_scene_v2_references(&mut v2, &assets);
    assert_eq!(v2.clone().to_v1().unwrap(), expected);
    assert!(v2.to_ron().unwrap().contains("guid://"));

    let mut old_prefab = Prefab::new("References");
    old_prefab.entities = legacy.entities.clone();
    let mut v2_prefab = PrefabV2::from_v1(&old_prefab).unwrap();
    let mut expected_prefab = old_prefab.clone();
    xgame_core::encode_prefab_references(&mut expected_prefab, &assets);
    xgame_core::encode_prefab_v2_references(&mut v2_prefab, &assets);
    assert_eq!(
        v2_prefab.clone().to_v1().unwrap().entities,
        expected_prefab.entities
    );
    xgame_core::decode_prefab_v2_references(&mut v2_prefab, &assets);
    assert_eq!(v2_prefab.to_v1().unwrap().entities, old_prefab.entities);

    xgame_core::decode_scene_v2_references(&mut v2, &assets);
    assert_eq!(v2.to_v1().unwrap(), legacy);
    drop(assets);
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn scene_v2_keeps_typed_values_and_identity_across_ron_roundtrip() {
    let mut legacy = Scene::new("Codec");
    let mut actor = Entity::new("Actor");
    actor.model = "Resources/Box.glb".into();
    actor.material_asset = Some("Resources/Materials/Box.mat".into());
    actor.morph_weights = vec![0.25, 0.75];
    actor.camera = Some(Camera::default());
    actor.audio = Some(AudioSource::default());
    actor.physics = Some(PhysicsBody::default());
    actor.controller = Some(CharacterController::default());
    actor.animator = Some(Animator::default());
    actor.particle = Some(ParticleEmitter::default());
    actor.material = Some(MaterialOverride::default());
    actor.ui = Some(UiElement::default());
    actor.behaviors.push("Health".into());
    actor
        .behavior_params
        .insert("Health".into(), Default::default());
    actor.scripts.push("Scripts/actor.lua".into());
    legacy.add_entity(actor);

    let disk = SceneV2::from_v1_plain(&legacy).unwrap();
    let first = disk.to_ron().unwrap();
    assert!(first.contains("format_version: 2"), "{first}");
    assert!(
        !first.contains("behavior_params:"),
        "v2 不写旧字段: {first}"
    );
    let extra_camera_field = first.replacen("fov:", "unrecognized_camera_field: 7, fov:", 1);
    assert_ne!(
        extra_camera_field, first,
        "用真实 Camera 字段构造未知字段回归"
    );
    let err = SceneV2::from_ron(&extra_camera_field).unwrap_err();
    assert!(err.contains("unrecognized_camera_field"), "{err}");
    assert!(ron::from_str::<Camera>("(unrecognized_camera_field: 7, fov: 60.0)").is_ok());
    let loaded = SceneV2::from_ron(&first).unwrap();
    assert_eq!(loaded.to_ron().unwrap(), first);

    let entity = &loaded.entities[0];
    assert_eq!(entity.transform_identity.id.raw(), 1);
    assert_eq!(entity.components.next_component_id.next_id(), 13);
    assert_eq!(entity.components.entries.len(), 11);
    assert_eq!(loaded.to_v1().unwrap(), legacy);

    let mut duplicate = SceneV2::from_v1_plain(&legacy).unwrap();
    duplicate.entities.push(duplicate.entities[0].clone());
    assert!(duplicate.to_ron().unwrap_err().contains("重复"));

    let mut missing_render = SceneV2::from_v1_plain(&legacy).unwrap();
    if let xgame_core::scene_v2::ComponentData::Render(render) =
        &mut missing_render.entities[0].components.entries[0].data
    {
        render.model.clear();
        render.material_asset = None;
        render.morph_weights.clear();
    } else {
        panic!("首个组件应是 Render");
    }
    assert!(missing_render.to_v1().unwrap_err().contains("空的 Render"));
}

#[test]
fn prefab_v2_keeps_asset_entity_ids_and_refuses_bad_versions() {
    let mut prefab = Prefab::new("Actor");
    let mut entity = Entity::new("Actor");
    entity.behaviors.push("Health".into());
    prefab.entities.push(entity);

    let v2 = PrefabV2::from_v1(&prefab).unwrap();
    let text = v2.to_ron().unwrap();
    assert_eq!(
        PrefabV2::from_ron(&text).unwrap().to_v1().unwrap().entities[0].id,
        prefab.entities[0].id
    );
    assert!(PrefabV2::from_ron(&text.replace("format_version: 2", "format_version: 3")).is_err());
    assert!(
        SceneV2::from_ron("(format_version: 2, name: \"Bad\", entities: [], unknown: 1)").is_err()
    );
}

#[test]
fn v2_rejects_duplicate_rust_field_keys() {
    let mut scene = Scene::new("DuplicateFields");
    let mut actor = Entity::new("Actor");
    actor.behaviors.push("Health".into());
    actor.behavior_params.insert(
        "Health".into(),
        [("value".into(), xgame_core::FieldValue::F32(75.0))].into(),
    );
    scene.add_entity(actor);
    let text = SceneV2::from_v1_plain(&scene).unwrap().to_ron().unwrap();
    let duplicate = text.replacen("\"value\":", "\"value\": F32(5.0), \"value\":", 1);
    assert_ne!(text, duplicate, "必须真的复制一个字段名: {text}");
    assert!(
        SceneV2::from_ron(&duplicate).is_err(),
        "v2 不能覆盖前一个字段值"
    );
}

#[test]
fn v2_rejects_duplicate_animation_parameter_keys() {
    let mut scene = Scene::new("AnimatorMap");
    let mut actor = Entity::new("Actor");
    let animator = Animator {
        machine: Some(xgame_core::AnimMachine {
            parameters: [("speed".into(), 1.0)].into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    actor.animator = Some(animator);
    scene.add_entity(actor);
    let text = SceneV2::from_v1_plain(&scene).unwrap().to_ron().unwrap();
    let duplicate = text.replacen("\"speed\":", "\"speed\": 0.0, \"speed\":", 1);
    assert_ne!(text, duplicate, "必须真的复制一个动画参数键: {text}");
    assert!(
        SceneV2::from_ron(&duplicate).is_err(),
        "不能覆盖第一个参数值"
    );
}

#[test]
fn v2_rejects_duplicate_layer_animation_parameters_and_keeps_defaults() {
    let mut scene = Scene::new("LayerMap");
    let mut actor = Entity::new("Actor");
    let animator = Animator {
        machine: Some(xgame_core::AnimMachine {
            parameters: [("base".into(), 1.0)].into(),
            ..Default::default()
        }),
        layers: vec![xgame_core::AnimLayer {
            machine: Some(xgame_core::AnimMachine {
                parameters: [("aim".into(), 0.5)].into(),
                ..Default::default()
            }),
            ..Default::default()
        }],
        ..Default::default()
    };
    actor.animator = Some(animator);
    scene.add_entity(actor);
    let text = SceneV2::from_v1_plain(&scene).unwrap().to_ron().unwrap();
    let duplicate = text.replacen("\"aim\":", "\"aim\": 0.0, \"aim\":", 1);
    assert_ne!(duplicate, text, "必须真的复制分层参数键: {text}");
    assert!(SceneV2::from_ron(&duplicate)
        .unwrap_err()
        .contains("重复键"));
    assert_eq!(SceneV2::from_ron(&text).unwrap().to_v1().unwrap(), scene);

    // 层与播放器的缺省值由 v1 类型定义；v2 的严格读法应与它保持一致。
    let partial = text.replacen("fade_time: 0.25,", "", 1);
    assert_ne!(partial, text, "动画默认淡入时长必须出现在样本中");
    let decoded = SceneV2::from_ron(&partial).unwrap().to_v1().unwrap();
    assert_eq!(
        decoded.entities[0].animator.as_ref().unwrap().fade_time,
        0.25
    );
}

#[test]
fn v2_prefab_instance_uses_asset_component_ids_after_local_reordering() {
    let source_path = "Prefabs/Actor.prefab.ron";
    let mut prefab = Prefab::new("Actor");
    let mut asset = Entity::new("Actor");
    asset.behaviors = vec!["Health".into(), "Motion".into()];
    asset.scripts = vec!["Scripts/actor.lua".into()];
    prefab.entities.push(asset);
    let source_id = prefab.entities[0].id;
    let mut upgraded = PrefabV2::from_v1(&prefab).unwrap();
    // 来源资产可能早已删除过组件；实例必须使用资产实际存盘的 ID。
    upgraded.entities[0].components.entries[1].identity.id =
        xgame_core::component_id::ComponentId::from_raw(10).unwrap();
    upgraded.entities[0].components.next_component_id =
        xgame_core::component_id::ComponentIdSequence::from_next(11).unwrap();
    let mut instance = prefab.instantiate(source_path).remove(0);
    instance.behaviors.swap(0, 1);
    instance.scripts.push("Scripts/instance.lua".into());
    let mut scene = Scene::new("Instances");
    scene.add_entity(instance);
    let sources = BTreeMap::from([(source_path.to_owned(), upgraded)]);

    let v2 = SceneV2::from_v1_with_prefabs(&scene, &sources).unwrap();
    let entity = &v2.entities[0];
    assert_eq!(entity.prefab.as_ref().unwrap().source_id, source_id);
    assert_eq!(entity.transform_identity.id.raw(), 1);
    assert_eq!(
        entity.transform_identity.source_component_id.unwrap().raw(),
        1
    );
    let identity = |index: usize| entity.components.entries[index].identity;
    assert_eq!(identity(0).id.raw(), 2); // Motion 本地先执行，资产里排第二
    assert_eq!(identity(0).source_component_id.unwrap().raw(), 10);
    assert_eq!(identity(1).source_component_id.unwrap().raw(), 2);
    assert_eq!(identity(2).source_component_id.unwrap().raw(), 4);
    assert_eq!(identity(3).source_component_id, None); // 实例自行追加
    assert_eq!(entity.components.next_component_id.next_id(), 6);
    assert_eq!(
        SceneV2::from_ron(&v2.to_ron().unwrap())
            .unwrap()
            .to_v1()
            .unwrap(),
        scene
    );

    let mut tampered = v2.clone();
    tampered.entities[0].components.entries[1]
        .identity
        .source_component_id = Some(xgame_core::component_id::ComponentId::from_raw(9).unwrap());
    assert!(tampered.validate_prefab_sources(&sources).is_err());
    let mut duplicated = v2.clone();
    duplicated.entities[0].components.entries[1]
        .identity
        .source_component_id = duplicated.entities[0].components.entries[0]
        .identity
        .source_component_id;
    assert!(duplicated.to_ron().unwrap_err().contains("重复引用"));

    assert!(SceneV2::from_v1_plain(&scene).is_err());
    assert!(SceneV2::from_v1_with_prefabs(&scene, &BTreeMap::new()).is_err());
    let mut broken = sources.clone();
    broken.get_mut(source_path).unwrap().entities[0].id = xgame_core::EntityId::new();
    assert!(SceneV2::from_v1_with_prefabs(&scene, &broken).is_err());
}

#[test]
fn project_reads_v2_with_prefab_source_checks_without_writing() {
    let base = std::env::temp_dir().join(format!("xgame-v2-read-{}", std::process::id()));
    assert!(!base.exists());
    let project = xgame_core::ProjectHandle::create(&base, "Project").unwrap();
    let prefab_path = project.prefab_path_for("Actor");
    let mut prefab = Prefab::new("Actor");
    let mut asset = Entity::new("Actor");
    asset.camera = Some(Camera::default());
    prefab.entities.push(asset);
    let mut scene = Scene::new("Instance");
    scene.entities.extend(prefab.instantiate(&prefab_path));
    let v2_prefab = PrefabV2::from_v1(&prefab).unwrap();
    let v2_scene = SceneV2::from_v1_with_prefabs(
        &scene,
        &BTreeMap::from([(prefab_path.clone(), v2_prefab.clone())]),
    )
    .unwrap();
    let scene_path = project.manifest().entry_scene.clone();
    std::fs::create_dir_all(project.path(&prefab_path).parent().unwrap()).unwrap();
    std::fs::write(project.path(&prefab_path), v2_prefab.to_ron().unwrap()).unwrap();
    std::fs::write(project.path(&scene_path), v2_scene.to_ron().unwrap()).unwrap();
    let before = std::fs::read(project.path(&scene_path)).unwrap();
    assert!(
        project.load_scene(&scene_path).is_err(),
        "v1 加载器必须拒绝 v2"
    );
    assert!(project.load_prefab(&prefab_path).is_err());
    assert_eq!(
        project
            .load_scene_v2(&scene_path)
            .unwrap()
            .to_ron()
            .unwrap(),
        v2_scene.to_ron().unwrap()
    );
    assert_eq!(std::fs::read(project.path(&scene_path)).unwrap(), before);

    let mut encoded = v2_scene.clone();
    {
        let mut assets = project.assets();
        assets.refresh();
        assets.ensure_metas().unwrap();
        xgame_core::encode_scene_v2_references(&mut encoded, &assets);
    }
    assert!(encoded.to_ron().unwrap().contains("guid://"));
    std::fs::write(project.path(&scene_path), encoded.to_ron().unwrap()).unwrap();
    assert_eq!(
        project.load_scene_v2(&scene_path).unwrap().entities[0]
            .prefab
            .as_ref()
            .unwrap()
            .source,
        prefab_path
    );

    let mut broken = v2_scene.clone();
    broken.entities[0].components.entries[0]
        .identity
        .source_component_id = Some(xgame_core::component_id::ComponentId::from_raw(99).unwrap());
    let text = broken.to_ron().unwrap();
    std::fs::write(project.path(&scene_path), &text).unwrap();
    assert!(project
        .load_scene_v2(&scene_path)
        .unwrap_err()
        .contains("来源 ID"));
    assert_eq!(
        std::fs::read_to_string(project.path(&scene_path)).unwrap(),
        text
    );
    assert!(project.load_scene_v2("../outside.ron").is_err());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn project_previews_v1_scene_and_prefab_upgrade_without_touching_files() {
    let base =
        std::env::temp_dir().join(format!("xgame-v2-upgrade-preview-{}", std::process::id()));
    assert!(!base.exists());
    let project = xgame_core::ProjectHandle::create(&base, "Project").unwrap();
    let source_path = project.prefab_path_for("Actor");
    let mut prefab = Prefab::new("Actor");
    let mut asset = Entity::new("Actor");
    asset.behaviors = vec!["Health".into(), "Motion".into()];
    prefab.entities.push(asset);
    project.write_prefab(&source_path, &prefab).unwrap();
    let mut instance = prefab.instantiate(&source_path).remove(0);
    instance.behaviors.swap(0, 1);
    let mut scene = Scene::new("Upgrade");
    scene.add_entity(instance);
    let path = &project.manifest().entry_scene;
    project.write_scene(path, &scene).unwrap();
    let scene_bytes = std::fs::read(project.path(path)).unwrap();
    let prefab_bytes = std::fs::read(project.path(&source_path)).unwrap();
    let preview = project.preview_scene_v2(path).unwrap();
    assert_eq!(preview.clone().to_v1().unwrap(), scene);
    assert_eq!(
        preview.entities[0].components.entries[0]
            .identity
            .source_component_id
            .unwrap()
            .raw(),
        3
    );
    assert_eq!(std::fs::read(project.path(path)).unwrap(), scene_bytes);
    assert_eq!(
        std::fs::read(project.path(&source_path)).unwrap(),
        prefab_bytes
    );

    let mut upgraded = PrefabV2::from_v1(&prefab).unwrap();
    upgraded.entities[0].components.entries[1].identity.id =
        xgame_core::component_id::ComponentId::from_raw(18).unwrap();
    upgraded.entities[0].components.next_component_id =
        xgame_core::component_id::ComponentIdSequence::from_next(19).unwrap();
    std::fs::write(project.path(&source_path), upgraded.to_ron().unwrap()).unwrap();
    let mixed = project.preview_scene_v2(path).unwrap();
    assert_eq!(
        mixed.entities[0].components.entries[0]
            .identity
            .source_component_id
            .unwrap()
            .raw(),
        18
    );
    assert_eq!(std::fs::read(project.path(path)).unwrap(), scene_bytes);

    // 未知嵌套字段在 v1 的常规加载中被忽略；预览迁移必须看见并拒绝。
    let mut bad = String::from_utf8(scene_bytes.clone()).unwrap();
    let marker = "behaviors: [";
    assert!(bad.contains(marker));
    bad = bad.replacen(marker, "unexpected_data: 42, behaviors: [", 1);
    std::fs::write(project.path(path), &bad).unwrap();
    assert!(project
        .preview_scene_v2(path)
        .unwrap_err()
        .contains("unexpected_data"));
    assert_eq!(std::fs::read_to_string(project.path(path)).unwrap(), bad);
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn project_upgrade_preview_rejects_repeated_v1_map_keys() {
    let base = std::env::temp_dir().join(format!("xgame-v1-duplicate-map-{}", std::process::id()));
    assert!(!base.exists());
    let project = xgame_core::ProjectHandle::create(&base, "Project").unwrap();
    let mut scene = Scene::new("DuplicateMap");
    let mut actor = Entity::new("Actor");
    actor.behaviors.push("Health".into());
    actor.behavior_params.insert(
        "Health".into(),
        [("hp".into(), xgame_core::FieldValue::F32(5.0))].into(),
    );
    scene.add_entity(actor);
    let path = &project.manifest().entry_scene;
    project.write_scene(path, &scene).unwrap();
    let original = std::fs::read_to_string(project.path(path)).unwrap();
    let duplicated = original.replacen("\"hp\":", "\"hp\": F32(2.0), \"hp\":", 1);
    assert_ne!(duplicated, original);
    assert!(
        ron::from_str::<Scene>(&duplicated).is_ok(),
        "v1 常规解码确实会吞掉重复键"
    );
    std::fs::write(project.path(path), &duplicated).unwrap();
    assert!(project.preview_scene_v2(path).unwrap_err().contains("重复"));
    assert_eq!(
        std::fs::read_to_string(project.path(path)).unwrap(),
        duplicated
    );
    scene.entities[0].animator = Some(Animator {
        machine: Some(xgame_core::AnimMachine {
            parameters: [("speed".into(), 1.0)].into(),
            ..Default::default()
        }),
        ..Default::default()
    });
    project.write_scene(path, &scene).unwrap();
    let text = std::fs::read_to_string(project.path(path)).unwrap();
    let repeated = text.replacen("\"speed\":", "\"speed\": 0.0, \"speed\":", 1);
    assert_ne!(repeated, text);
    std::fs::write(project.path(path), &repeated).unwrap();
    assert!(project.preview_scene_v2(path).unwrap_err().contains("重复"));
    std::fs::write(project.path(path), &text).unwrap();
    assert!(project.preview_scene_v2(path).is_ok());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn explicit_project_upgrade_backs_up_originals_and_can_restore_them() {
    let base = std::env::temp_dir().join(format!("xgame-v2-migration-{}", std::process::id()));
    assert!(!base.exists());
    let project = xgame_core::ProjectHandle::create(&base, "Project").unwrap();
    let prefab_path = project.prefab_path_for("Actor");
    let mut prefab = Prefab::new("Actor");
    prefab
        .entities
        .push(Entity::new("Actor").with_model("builtin://cube"));
    project.write_prefab(&prefab_path, &prefab).unwrap();
    let mut scene = project.load_entry_scene().unwrap();
    scene.add_entity(prefab.instantiate(&prefab_path).remove(0));
    let scene_path = project.manifest().entry_scene.clone();
    project.write_scene(&scene_path, &scene).unwrap();
    let old_scene = std::fs::read(project.path(&scene_path)).unwrap();
    let old_prefab = std::fs::read(project.path(&prefab_path)).unwrap();

    let result = project.migrate_components_v2().unwrap();
    assert!(result.files.contains(&scene_path));
    assert!(result.files.contains(&prefab_path));
    assert_eq!(
        std::fs::read(project.path(&scene_path)).unwrap(),
        std::fs::read(result.backup.join("upgraded").join(&scene_path)).unwrap()
    );
    assert_eq!(
        std::fs::read(result.backup.join("original").join(&scene_path)).unwrap(),
        old_scene
    );
    assert_eq!(
        std::fs::read(result.backup.join("original").join(&prefab_path)).unwrap(),
        old_prefab
    );
    let upgraded = project.load_scene_v2(&scene_path).unwrap();
    assert_eq!(upgraded.entities.len(), scene.entities.len());
    assert!(upgraded.entities.last().unwrap().components.entries[0]
        .identity
        .source_component_id
        .is_some());
    assert!(project.load_prefab_v2(&prefab_path).is_ok());

    project.restore_components_v2(&result.backup).unwrap();
    assert_eq!(std::fs::read(project.path(&scene_path)).unwrap(), old_scene);
    assert_eq!(
        std::fs::read(project.path(&prefab_path)).unwrap(),
        old_prefab
    );
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn v2_upgrade_preflight_leaves_all_files_alone_if_one_cannot_convert() {
    let base =
        std::env::temp_dir().join(format!("xgame-v2-migration-reject-{}", std::process::id()));
    assert!(!base.exists());
    let project = xgame_core::ProjectHandle::create(&base, "Project").unwrap();
    let scene_path = project.manifest().entry_scene.clone();
    let original = std::fs::read(project.path(&scene_path)).unwrap();
    let broken_path = project.path("Scenes/Broken.ron");
    std::fs::write(&broken_path, "(format_version: 3)").unwrap();
    assert!(project.migrate_components_v2().is_err());
    assert_eq!(std::fs::read(project.path(&scene_path)).unwrap(), original);
    assert_eq!(std::fs::read(broken_path).unwrap(), b"(format_version: 3)");
    assert_eq!(project.list_scenes().len(), 2);
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn migration_recovery_checks_every_file_before_restoring_any() {
    let base = std::env::temp_dir().join(format!(
        "xgame-v2-migration-recovery-{}",
        std::process::id()
    ));
    assert!(!base.exists());
    let project = xgame_core::ProjectHandle::create(&base, "Project").unwrap();
    let other = "Scenes/Extra.ron";
    project.write_scene(other, &Scene::new("Extra")).unwrap();
    let result = project.migrate_components_v2().unwrap();
    let entry = project.manifest().entry_scene.clone();
    let upgraded_entry = std::fs::read(project.path(&entry)).unwrap();
    let upgraded_other = std::fs::read(project.path(other)).unwrap();
    std::fs::write(project.path(other), b"external edit").unwrap();
    assert!(project
        .restore_components_v2(&result.backup)
        .unwrap_err()
        .contains("已被修改"));
    assert_eq!(
        std::fs::read(project.path(&entry)).unwrap(),
        upgraded_entry,
        "拒绝恢复须发生在第一份文件被写之前"
    );
    std::fs::write(project.path(other), upgraded_other).unwrap();
    project.restore_components_v2(&result.backup).unwrap();
    assert!(project.load_entry_scene().is_ok());
    assert!(project.load_scene(other).is_ok());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn interrupted_component_upgrade_blocks_new_upgrades_until_recovered() {
    let base =
        std::env::temp_dir().join(format!("xgame-v2-migration-pending-{}", std::process::id()));
    assert!(!base.exists());
    let project = xgame_core::ProjectHandle::create(&base, "Project").unwrap();
    let other = "Scenes/Extra.ron";
    project.write_scene(other, &Scene::new("Extra")).unwrap();
    let result = project.migrate_components_v2().unwrap();
    let journal = result.backup.join("migration.ron");
    let completed = std::fs::read_to_string(&journal).unwrap();
    assert!(completed.contains("Complete"));
    std::fs::write(&journal, completed.replace("Complete", "Pending")).unwrap();
    let original = std::fs::read(result.backup.join("original").join(other)).unwrap();
    std::fs::write(project.path(other), original).unwrap();
    assert!(project
        .migrate_components_v2()
        .unwrap_err()
        .contains("未恢复"));
    project.restore_components_v2(&result.backup).unwrap();
    assert!(project.load_entry_scene().is_ok());
    assert!(project.load_scene(other).is_ok());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn runtime_loader_reads_v2_values_while_legacy_edit_loader_stays_guarded() {
    let base = std::env::temp_dir().join(format!("xgame-v2-runtime-load-{}", std::process::id()));
    assert!(!base.exists());
    let project = xgame_core::ProjectHandle::create(&base, "Project").unwrap();
    let path = project.manifest().entry_scene.clone();
    let mut scene = project.load_entry_scene().unwrap();
    scene.add_entity(Entity::new("Runner").with_script("Scripts/runner.lua"));
    let upgraded = SceneV2::from_v1_plain(&scene).unwrap();
    let runner = upgraded.entities.last().unwrap();
    let runner_id = runner.id;
    let transform_id = runner.transform_identity.id;
    let script_id = runner.components.entries[0].identity.id;
    project
        .write_scene_v2("Scenes/Extra.ron", &upgraded)
        .unwrap();
    std::fs::write(project.path(&path), upgraded.to_ron().unwrap()).unwrap();
    assert!(project.load_entry_scene().is_err());
    let runtime = project.load_entry_scene_runtime().unwrap();
    assert_eq!(
        runtime.entities.last().unwrap().scripts,
        ["Scripts/runner.lua"]
    );
    let loaded = project.load_entry_runtime_scene().unwrap();
    let identities = loaded.identities().expect("v2 运行载荷必须保留组件身份");
    let runner = identities.entity(runner_id).expect("Runner 身份存在");
    assert_eq!(runner.transform().identity.id, transform_id);
    assert_eq!(runner.next_component_id().next_id(), script_id.raw() + 1);
    let script = runner
        .component(script_id)
        .expect("Lua 组件可按稳定 ID 查询");
    assert_eq!(
        script.kind,
        RuntimeComponentKind::Lua("Scripts/runner.lua".into())
    );
    let from_switch = xgame_core::SceneLoader::load_scene(&project, "Scenes/Extra.ron").unwrap();
    assert_eq!(
        from_switch.entities.last().unwrap().scripts,
        ["Scripts/runner.lua"]
    );
    let prefab_path = "Prefabs/Runner.prefab.ron";
    let mut prefab = Prefab::new("Runner");
    prefab
        .entities
        .push(Entity::new("Runner").with_model("builtin://cube"));
    let prefab_v2 = PrefabV2::from_v1(&prefab).unwrap();
    let source_transform_id = prefab_v2.entities[0].transform_identity.id;
    let source_render_id = prefab_v2.entities[0].components.entries[0].identity.id;
    project.write_prefab_v2(prefab_path, &prefab_v2).unwrap();
    let from_spawn = xgame_core::SceneLoader::load_prefab(&project, prefab_path).unwrap();
    assert_eq!(from_spawn.entities[0].model, "builtin://cube");
    let loaded_prefab =
        xgame_core::SceneLoader::load_runtime_prefab(&project, prefab_path).unwrap();
    let instance = loaded_prefab.instantiate(prefab_path).unwrap();
    let identity = instance.identities()[0]
        .as_ref()
        .expect("v2 预制体实例必须携带组件身份");
    assert_eq!(
        identity.transform().identity.source_component_id,
        Some(source_transform_id)
    );
    assert_eq!(
        identity
            .component(source_render_id)
            .unwrap()
            .identity
            .source_component_id,
        Some(source_render_id)
    );
    assert!(project.load_prefab(prefab_path).is_err());
    assert!(project.load_scene("Scenes/Extra.ron").is_err());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn v2_component_edit_refuses_stale_scene_revision() {
    let base = std::env::temp_dir().join(format!("xgame-v2-stale-edit-{}", std::process::id()));
    assert!(!base.exists());
    let project = xgame_core::ProjectHandle::create(&base, "Project").unwrap();
    let path = "Scenes/Edit.ron";
    let mut scene = Scene::new("Edit");
    scene.add_entity(Entity::new("Actor"));
    let initial = SceneV2::from_v1_plain(&scene).unwrap();
    project.write_scene_v2(path, &initial).unwrap();
    let original = std::fs::read(project.path(path)).unwrap();
    let mut external = initial.clone();
    external.name = "Externally edited".into();
    project.write_scene_v2(path, &external).unwrap();
    let changed = std::fs::read(project.path(path)).unwrap();
    let mut requested = initial;
    requested.entities[0].name = "Agent edited".into();
    assert!(project
        .write_scene_v2_if_unchanged(path, &requested, &original)
        .is_err());
    assert_eq!(std::fs::read(project.path(path)).unwrap(), changed);
    project
        .write_scene_v2_if_unchanged(path, &requested, &changed)
        .unwrap();
    assert_eq!(
        project.load_scene_v2(path).unwrap().entities[0].name,
        "Agent edited"
    );
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn v2_editor_projection_keeps_ids_and_never_recycles_removed_components() {
    let mut scene = Scene::new("Editor");
    let mut actor = Entity::new("Actor").with_model("builtin://cube");
    actor.camera = Some(xgame_core::Camera::default());
    let id = actor.id;
    scene.entities.push(actor);
    let mut v2 = SceneV2::from_v1_plain(&scene).unwrap();
    let transform_id = v2.entities[0].transform_identity.id;
    let render_id = v2.entities[0].components.entries[0].identity.id;
    let camera_id = v2.entities[0].components.entries[1].identity.id;
    scene.entities[0].model = "builtin://sphere".into();
    scene.entities[0].camera.as_mut().unwrap().fov = 55.0;
    v2.reconcile_from_v1(&scene, &Default::default()).unwrap();
    assert_eq!(v2.entities[0].transform_identity.id, transform_id);
    assert_eq!(v2.entities[0].components.entries[0].identity.id, render_id);
    assert_eq!(v2.entities[0].components.entries[1].identity.id, camera_id);
    scene.entities[0].camera = None;
    v2.reconcile_from_v1(&scene, &Default::default()).unwrap();
    scene.entities[0].camera = Some(xgame_core::Camera::default());
    v2.reconcile_from_v1(&scene, &Default::default()).unwrap();
    let replaced = &v2
        .entities
        .iter()
        .find(|e| e.id == id)
        .unwrap()
        .components
        .entries[1];
    assert_ne!(replaced.identity.id, camera_id);
    assert!(replaced.identity.id.raw() > camera_id.raw());
    assert_eq!(
        v2.clone().to_v1().unwrap().entities[0].model,
        "builtin://sphere"
    );
}

#[test]
fn v2_editor_replaces_a_single_unlinked_lua_path_without_changing_component_id() {
    let mut scene = Scene::new("Lua edit");
    let mut actor = Entity::new("Actor");
    actor.scripts.push("Scripts/old.lua".into());
    scene.entities.push(actor);
    let mut v2 = SceneV2::from_v1_plain(&scene).unwrap();
    let id = v2.entities[0].components.entries[0].identity.id;
    scene.entities[0].scripts[0] = "Scripts/new.lua".into();
    v2.reconcile_from_v1(&scene, &Default::default()).unwrap();
    assert_eq!(v2.entities[0].components.entries[0].identity.id, id);
    assert_eq!(v2.to_v1().unwrap().entities[0].scripts, ["Scripts/new.lua"]);
}

#[test]
fn v2_structural_apply_preserves_survivors_and_links_new_structure() {
    let source = "Prefabs/Actor.prefab.ron";
    let mut legacy = Prefab::new("Actor");
    let mut root = Entity::new("Root");
    root.camera = Some(Camera::default());
    root.audio = Some(AudioSource::default());
    root.scripts.push("Scripts/old.lua".into());
    let mut child = Entity::new("OldChild");
    child.parent = Some(root.id);
    legacy.entities = vec![root, child];
    let old_asset = PrefabV2::from_v1(&legacy).unwrap();
    let old_root = &old_asset.entities[0];
    let old_transform = old_root.transform_identity.id;
    let old_next = old_root.components.next_component_id.next_id();
    let old_camera = old_root
        .components
        .entries
        .iter()
        .find(|entry| matches!(entry.data, ComponentData::Camera(_)))
        .unwrap()
        .identity
        .id;
    let old_lua = old_root
        .components
        .entries
        .iter()
        .find(|entry| matches!(entry.data, ComponentData::Lua { .. }))
        .unwrap()
        .identity
        .id;
    let removed_child_id = old_asset.entities[1].id;

    let mut scene = Scene::new("Instances");
    scene.entities = legacy.instantiate(source);
    let local_root_id = scene.entities[0].id;
    let mut identity = SceneV2::from_v1_with_prefabs(
        &scene,
        &BTreeMap::from([(source.to_owned(), old_asset.clone())]),
    )
    .unwrap();
    scene.entities[0].audio = None;
    scene.entities[0].particle = Some(ParticleEmitter::default());
    scene.entities[0].scripts[0] = "Scripts/new.lua".into();
    scene.entities.remove(1);
    let mut added_child = Entity::new("AddedChild");
    let local_added_id = added_child.id;
    added_child.parent = Some(local_root_id);
    scene.entities.push(added_child);
    identity
        .reconcile_from_v1_for_history(
            &scene,
            &BTreeMap::from([(source.to_owned(), old_asset.clone())]),
        )
        .unwrap();

    let next_asset = old_asset
        .apply_structure_from_instance(&mut scene, &mut identity, 0, source)
        .unwrap();
    let next_root = &next_asset.entities[0];
    assert_eq!(next_root.id, old_asset.entities[0].id);
    assert_eq!(next_root.transform_identity.id, old_transform);
    assert!(next_asset
        .entities
        .iter()
        .all(|entity| entity.id != removed_child_id));
    assert_eq!(
        next_root
            .components
            .entries
            .iter()
            .find(|entry| matches!(entry.data, ComponentData::Camera(_)))
            .unwrap()
            .identity
            .id,
        old_camera
    );
    assert_eq!(
        next_root
            .components
            .entries
            .iter()
            .find(|entry| matches!(entry.data, ComponentData::Lua { .. }))
            .unwrap()
            .identity
            .id,
        old_lua,
        "脚本路径改名仍是同一个来源组件"
    );
    assert!(next_root
        .components
        .entries
        .iter()
        .all(|entry| !matches!(entry.data, ComponentData::Audio(_))));
    let asset_particle = next_root
        .components
        .entries
        .iter()
        .find(|entry| matches!(entry.data, ComponentData::Particle(_)))
        .unwrap()
        .identity
        .id;
    assert!(asset_particle.raw() >= old_next);
    assert!(next_root.components.next_component_id.next_id() > asset_particle.raw());
    let added_source_id = scene
        .entities
        .iter()
        .find(|entity| entity.id == local_added_id)
        .unwrap()
        .prefab
        .as_ref()
        .unwrap()
        .source_id;
    assert!(next_asset
        .entities
        .iter()
        .any(|entity| entity.id == added_source_id));
    let local_root = identity
        .entities
        .iter()
        .find(|entity| entity.id == local_root_id)
        .unwrap();
    assert_eq!(
        local_root
            .components
            .entries
            .iter()
            .find(|entry| matches!(entry.data, ComponentData::Particle(_)))
            .unwrap()
            .identity
            .source_component_id,
        Some(asset_particle)
    );
}

#[test]
fn v2_prefab_metadata_sync_keeps_local_values_when_sources_disappear() {
    let source = "Prefabs/Actor.prefab.ron";
    let mut legacy = Prefab::new("Actor");
    let mut root = Entity::new("Root");
    root.camera = Some(Camera::default());
    root.audio = Some(AudioSource::default());
    root.scripts.push("Scripts/old.lua".into());
    let mut child = Entity::new("Child");
    child.parent = Some(root.id);
    legacy.entities = vec![root, child];
    let old_asset = PrefabV2::from_v1(&legacy).unwrap();
    let mut scene = Scene::new("Other");
    scene.entities = legacy.instantiate(source);
    let mut identity = SceneV2::from_v1_with_prefabs(
        &scene,
        &BTreeMap::from([(source.to_owned(), old_asset.clone())]),
    )
    .unwrap();

    let mut next_asset = old_asset.clone();
    let root = &mut next_asset.entities[0];
    root.components
        .entries
        .retain(|entry| !matches!(entry.data, ComponentData::Audio(_)));
    let lua = root
        .components
        .entries
        .iter_mut()
        .find(|entry| matches!(entry.data, ComponentData::Lua { .. }))
        .unwrap();
    if let ComponentData::Lua { path } = &mut lua.data {
        *path = "Scripts/new.lua".into();
    }
    next_asset.entities.remove(1);
    identity
        .synchronize_prefab_source_metadata(source, &next_asset)
        .unwrap();

    let root = &identity.entities[0];
    assert!(root
        .components
        .entries
        .iter()
        .find(|entry| matches!(entry.data, ComponentData::Camera(_)))
        .unwrap()
        .identity
        .source_component_id
        .is_some());
    for entry in &root.components.entries {
        if matches!(
            entry.data,
            ComponentData::Audio(_) | ComponentData::Lua { .. }
        ) {
            assert_eq!(entry.identity.source_component_id, None);
        }
    }
    assert!(matches!(
        root.components
            .entries
            .iter()
            .find(|entry| matches!(entry.data, ComponentData::Audio(_)))
            .unwrap()
            .data,
        ComponentData::Audio(_)
    ));
    assert!(identity.entities[1].prefab.is_none());
    assert!(identity.entities[1]
        .components
        .entries
        .iter()
        .all(|entry| entry.identity.source_component_id.is_none()));
}

#[test]
fn v2_structural_apply_transaction_updates_all_scenes_and_can_restore_exact_bytes() {
    let (base, project, source, asset, scenes) = structural_apply_fixture("transaction");
    let asset_before = std::fs::read(project.path(&source)).unwrap();
    let scene_before: Vec<_> = scenes
        .iter()
        .map(|relative| std::fs::read(project.path(relative)).unwrap())
        .collect();
    let mut next = asset.clone();
    let audio_id = next.entities[0]
        .components
        .entries
        .iter()
        .find(|entry| matches!(entry.data, ComponentData::Audio(_)))
        .unwrap()
        .identity
        .id;
    next.entities[0].remove_component(audio_id).unwrap();

    let report = project
        .apply_prefab_v2_transaction(
            &source,
            &next,
            &asset_before,
            Some((&scenes[0], &scene_before[0])),
        )
        .unwrap();
    assert_eq!(report.files.len(), 3);
    assert_eq!(
        report.guarded_scene_bytes.as_deref(),
        Some(std::fs::read(project.path(&scenes[0])).unwrap().as_slice())
    );
    for relative in &scenes {
        let scene = project.load_scene_v2(relative).unwrap();
        let audio = scene.entities[0]
            .components
            .entries
            .iter()
            .find(|entry| matches!(entry.data, ComponentData::Audio(_)))
            .expect("其他实例的本地值不能被结构性 Apply 删除");
        assert_eq!(audio.identity.source_component_id, None);
    }

    project.restore_prefab_apply_v2(&report.backup).unwrap();
    assert_eq!(std::fs::read(project.path(&source)).unwrap(), asset_before);
    for (relative, before) in scenes.iter().zip(scene_before) {
        assert_eq!(std::fs::read(project.path(relative)).unwrap(), before);
    }
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn v2_structural_apply_rejects_a_stale_guarded_scene_before_writing_asset() {
    let (base, project, source, mut asset, scenes) = structural_apply_fixture("stale_scene");
    let asset_before = std::fs::read(project.path(&source)).unwrap();
    let expected_scene = std::fs::read(project.path(&scenes[0])).unwrap();
    let mut external = project.load_scene_v2(&scenes[0]).unwrap();
    external.name = "External".into();
    project.write_scene_v2(&scenes[0], &external).unwrap();
    let external_bytes = std::fs::read(project.path(&scenes[0])).unwrap();
    let audio_id = asset.entities[0]
        .components
        .entries
        .iter()
        .find(|entry| matches!(entry.data, ComponentData::Audio(_)))
        .unwrap()
        .identity
        .id;
    asset.entities[0].remove_component(audio_id).unwrap();

    let error = project
        .apply_prefab_v2_transaction(
            &source,
            &asset,
            &asset_before,
            Some((&scenes[0], &expected_scene)),
        )
        .unwrap_err();
    assert!(error.contains("外部修改"), "{error}");
    assert_eq!(std::fs::read(project.path(&source)).unwrap(), asset_before);
    assert_eq!(
        std::fs::read(project.path(&scenes[0])).unwrap(),
        external_bytes
    );
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn v2_structural_apply_recovers_a_pending_journal_before_the_next_attempt() {
    let (base, project, source, mut asset, scenes) = structural_apply_fixture("pending_recovery");
    let abandoned_prepare = project.root().join(".xgame-prefab-v2-prepare-abandoned");
    std::fs::create_dir(&abandoned_prepare).unwrap();
    assert_eq!(
        project.recover_pending_prefab_applies_v2().unwrap(),
        0,
        "尚未公布的准备目录没有改过目标文件，不属于待恢复事务"
    );
    let asset_before = std::fs::read(project.path(&source)).unwrap();
    let scene_before: Vec<_> = scenes
        .iter()
        .map(|relative| std::fs::read(project.path(relative)).unwrap())
        .collect();
    let audio_id = asset.entities[0]
        .components
        .entries
        .iter()
        .find(|entry| matches!(entry.data, ComponentData::Audio(_)))
        .unwrap()
        .identity
        .id;
    asset.entities[0].remove_component(audio_id).unwrap();
    let report = project
        .apply_prefab_v2_transaction(
            &source,
            &asset,
            &asset_before,
            Some((&scenes[0], &scene_before[0])),
        )
        .unwrap();
    let applied_asset = std::fs::read(project.path(&source)).unwrap();
    let applied_scene = std::fs::read(project.path(&scenes[0])).unwrap();
    let journal_path = report.backup.join("apply.ron");
    let journal = std::fs::read_to_string(&journal_path).unwrap();
    assert!(journal.contains("Complete"));
    std::fs::write(&journal_path, journal.replace("Complete", "Pending")).unwrap();

    assert_eq!(project.recover_pending_prefab_applies_v2().unwrap(), 1);
    let error = project
        .apply_prefab_v2_transaction(
            &source,
            &asset,
            &applied_asset,
            Some((&scenes[0], &applied_scene)),
        )
        .unwrap_err();
    assert!(error.contains("外部修改"), "{error}");
    assert_eq!(std::fs::read(project.path(&source)).unwrap(), asset_before);
    for (relative, before) in scenes.iter().zip(scene_before) {
        assert_eq!(std::fs::read(project.path(relative)).unwrap(), before);
    }
    assert!(std::fs::read_to_string(journal_path)
        .unwrap()
        .contains("Restored"));
    std::fs::remove_dir_all(base).unwrap();
}

#[cfg(windows)]
#[test]
fn v2_structural_apply_write_failure_restores_every_file() {
    use std::os::windows::fs::OpenOptionsExt;

    let (base, project, source, mut asset, scenes) = structural_apply_fixture("rollback");
    let asset_before = std::fs::read(project.path(&source)).unwrap();
    let scene_before: Vec<_> = scenes
        .iter()
        .map(|relative| std::fs::read(project.path(relative)).unwrap())
        .collect();
    let audio_id = asset.entities[0]
        .components
        .entries
        .iter()
        .find(|entry| matches!(entry.data, ComponentData::Audio(_)))
        .unwrap()
        .identity
        .id;
    asset.entities[0].remove_component(audio_id).unwrap();
    // 允许读写但拒绝删除/替换；Prefabs 排在 Scenes 前，能证明前面已写的资产会回滚。
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(project.path(&scenes[1]))
        .unwrap();
    let error = project
        .apply_prefab_v2_transaction(
            &source,
            &asset,
            &asset_before,
            Some((&scenes[0], &scene_before[0])),
        )
        .unwrap_err();
    assert!(error.contains("自动恢复"), "{error}");
    assert_eq!(std::fs::read(project.path(&source)).unwrap(), asset_before);
    for (relative, before) in scenes.iter().zip(&scene_before) {
        assert_eq!(std::fs::read(project.path(relative)).unwrap(), *before);
    }
    let transaction = std::fs::read_dir(project.root())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with(".xgame-prefab-v2-apply-")
            })
        })
        .unwrap();
    let journal = std::fs::read_to_string(transaction.join("apply.ron")).unwrap();
    assert!(journal.contains("Restored"), "{journal}");
    drop(lock);
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn v2_prefab_apply_values_preserves_asset_ids_and_rejects_new_components() {
    let mut old = xgame_core::Prefab::new("Source");
    let mut entity = Entity::new("Camera");
    entity.camera = Some(xgame_core::Camera::default());
    entity.audio = Some(Default::default());
    old.entities.push(entity);
    let mut asset = xgame_core::scene_v2::PrefabV2::from_v1(&old).unwrap();
    let transform_id = asset.entities[0].transform_identity.id;
    let camera_id = asset.entities[0]
        .components
        .entries
        .iter()
        .find(|entry| matches!(entry.data, ComponentData::Camera(_)))
        .unwrap()
        .identity
        .id;
    asset.entities[0].components.entries.reverse();
    let order: Vec<_> = asset.entities[0]
        .components
        .entries
        .iter()
        .map(|entry| entry.identity.id)
        .collect();
    old.entities[0].camera.as_mut().unwrap().fov = 42.0;
    asset.apply_values_from_v1(&old).unwrap();
    assert_eq!(asset.entities[0].transform_identity.id, transform_id);
    assert_eq!(
        asset.entities[0]
            .components
            .entries
            .iter()
            .find(|entry| matches!(entry.data, ComponentData::Camera(_)))
            .unwrap()
            .identity
            .id,
        camera_id
    );
    assert_eq!(
        asset.entities[0]
            .components
            .entries
            .iter()
            .map(|entry| entry.identity.id)
            .collect::<Vec<_>>(),
        order
    );
    assert_eq!(
        asset.clone().to_v1().unwrap().entities[0]
            .camera
            .as_ref()
            .unwrap()
            .fov,
        42.0
    );
    old.entities[0].particle = Some(Default::default());
    let bytes = asset.to_ron().unwrap();
    assert!(asset.apply_values_from_v1(&old).is_err());
    assert_eq!(asset.to_ron().unwrap(), bytes);
}

#[test]
fn v2_prefab_conditional_write_rejects_stale_asset() {
    let base = std::env::temp_dir().join(format!("xgame-v2-stale-prefab-{}", std::process::id()));
    assert!(!base.exists());
    let project = xgame_core::ProjectHandle::create(&base, "Project").unwrap();
    let path = "Prefabs/Actor.prefab.ron";
    let mut prefab = Prefab::new("Actor");
    prefab.entities.push(Entity::new("Actor"));
    let initial = PrefabV2::from_v1(&prefab).unwrap();
    project.write_prefab_v2(path, &initial).unwrap();
    let original = std::fs::read(project.path(path)).unwrap();
    let mut external = initial.clone();
    external.name = "External".into();
    project.write_prefab_v2(path, &external).unwrap();
    let changed = std::fs::read(project.path(path)).unwrap();
    let mut requested = initial;
    requested.name = "Requested".into();
    assert!(project
        .write_prefab_v2_if_unchanged(path, &requested, &original)
        .is_err());
    assert_eq!(std::fs::read(project.path(path)).unwrap(), changed);
    project
        .write_prefab_v2_if_unchanged(path, &requested, &changed)
        .unwrap();
    assert_eq!(project.load_prefab_v2(path).unwrap().name, "Requested");
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn project_v2_write_preserves_ids_and_refuses_v1_overwrite() {
    let base = std::env::temp_dir().join(format!("xgame-v2-write-{}", std::process::id()));
    assert!(!base.exists());
    let project = xgame_core::ProjectHandle::create(&base, "Project").unwrap();
    let existing = project.manifest().entry_scene.clone();
    let before = std::fs::read(project.path(&existing)).unwrap();
    let source = project.prefab_path_for("Actor");
    let mut prefab = Prefab::new("Actor");
    let mut actor = Entity::new("Actor");
    actor.camera = Some(Camera::default());
    prefab.entities.push(actor);
    let v2_prefab = PrefabV2::from_v1(&prefab).unwrap();
    let mut scene = Scene::new("V2");
    scene.entities.extend(prefab.instantiate(&source));
    let v2_scene = SceneV2::from_v1_with_prefabs(
        &scene,
        &BTreeMap::from([(source.clone(), v2_prefab.clone())]),
    )
    .unwrap();
    let v2_path = "Scenes/Upgraded.ron";

    assert!(project.write_scene_v2(&existing, &v2_scene).is_err());
    assert_eq!(std::fs::read(project.path(&existing)).unwrap(), before);
    assert!(
        project.write_scene_v2(v2_path, &v2_scene).is_err(),
        "缺少来源资产不能写场景"
    );
    project.write_prefab_v2(&source, &v2_prefab).unwrap();
    project.write_scene_v2(v2_path, &v2_scene).unwrap();
    let saved = std::fs::read(project.path(v2_path)).unwrap();
    let mtime = std::fs::metadata(project.path(v2_path))
        .unwrap()
        .modified()
        .unwrap();
    assert!(String::from_utf8_lossy(&saved).contains("guid://"));
    assert_eq!(
        project.load_scene_v2(v2_path).unwrap().to_ron().unwrap(),
        v2_scene.to_ron().unwrap()
    );
    project.write_scene_v2(v2_path, &v2_scene).unwrap();
    assert_eq!(std::fs::read(project.path(v2_path)).unwrap(), saved);
    assert_eq!(
        std::fs::metadata(project.path(v2_path))
            .unwrap()
            .modified()
            .unwrap(),
        mtime
    );
    let corrupted = "(format_version: 2, name: \"Corrupt\", entities: [], unknown_field: 5)";
    std::fs::write(project.path(v2_path), corrupted).unwrap();
    assert!(project.write_scene_v2(v2_path, &v2_scene).is_err());
    assert_eq!(
        std::fs::read_to_string(project.path(v2_path)).unwrap(),
        corrupted
    );
    assert!(project.write_prefab_v2(&existing, &v2_prefab).is_err());
    assert!(project.write_scene_v2("../outside.ron", &v2_scene).is_err());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn v2_component_edits_keep_ids_and_never_recycle_deleted_ids() {
    use xgame_core::scene_v2::ComponentData;
    let mut scene = Scene::new("Components");
    let mut actor = Entity::new("Actor");
    actor.camera = Some(Camera::default());
    actor.scripts.push("Scripts/player.lua".into());
    scene.add_entity(actor);
    let mut upgraded = SceneV2::from_v1_plain(&scene).unwrap();
    let entity = &mut upgraded.entities[0];
    let camera_id = entity.components.entries[0].identity.id;
    let lua_id = entity.components.entries[1].identity.id;
    assert_eq!(entity.transform_identity.id.raw(), 1);

    let added = entity
        .add_component(ComponentData::Audio(AudioSource::default()))
        .unwrap();
    assert_eq!(added.raw(), 4);
    assert!(entity
        .add_component(ComponentData::Camera(Camera::default()))
        .is_err());
    assert_eq!(entity.components.next_component_id.next_id(), 5);
    assert!(entity
        .remove_component(entity.transform_identity.id)
        .is_err());
    assert!(entity.remove_component(camera_id).is_ok());
    let replacement = entity
        .add_component(ComponentData::Camera(Camera::default()))
        .unwrap();
    assert_eq!(replacement.raw(), 5);
    assert_eq!(entity.components.next_component_id.next_id(), 6);
    assert!(entity
        .replace_component(lua_id, ComponentData::Camera(Camera::default()))
        .is_err());
    assert!(entity
        .replace_component(
            lua_id,
            ComponentData::Lua {
                path: "Scripts/new.lua".into()
            }
        )
        .is_ok());
    entity.move_component(added, 0).unwrap();
    assert_eq!(entity.components.entries[0].identity.id, added);
    entity.move_component(lua_id, 0).unwrap();
    assert_eq!(entity.components.entries[0].identity.id, lua_id);
    assert_eq!(entity.components.entries[1].identity.id, added);
    assert_eq!(entity.components.entries[2].identity.id, replacement);
    assert_eq!(
        SceneV2::from_ron(&upgraded.to_ron().unwrap())
            .unwrap()
            .to_v1()
            .unwrap()
            .entities[0]
            .scripts,
        vec!["Scripts/new.lua"]
    );
}

#[test]
fn v2_rejects_unrepresentable_duplicate_script_or_rust_instances() {
    use xgame_core::scene_v2::ComponentData;
    let mut scene = Scene::new("Duplicates");
    scene.add_entity(Entity::new("Actor"));
    let mut v2 = SceneV2::from_v1_plain(&scene).unwrap();
    let entity = &mut v2.entities[0];
    entity
        .add_component(ComponentData::Lua {
            path: "Scripts/a.lua".into(),
        })
        .unwrap();
    assert!(entity
        .add_component(ComponentData::Lua {
            path: "Scripts/a.lua".into()
        })
        .is_err());
    entity
        .add_component(ComponentData::Rust {
            registered_name: "Health".into(),
            fields: None,
        })
        .unwrap();
    assert!(entity
        .add_component(ComponentData::Rust {
            registered_name: "Health".into(),
            fields: None
        })
        .is_err());
    let mut invalid = v2.clone();
    let repeated = invalid.entities[0].components.entries[0].clone();
    invalid.entities[0].components.entries.push(repeated);
    invalid.entities[0]
        .components
        .entries
        .last_mut()
        .unwrap()
        .identity
        .id = xgame_core::component_id::ComponentId::from_raw(99).unwrap();
    invalid.entities[0].components.next_component_id =
        xgame_core::component_id::ComponentIdSequence::from_next(100).unwrap();
    assert!(invalid.to_ron().unwrap_err().contains("重复"));
}
