use std::collections::BTreeMap;
use xgame_core::scene_v2::{PrefabV2, SceneV2};
use xgame_core::{
    Animator, AudioSource, Camera, CharacterController, Entity, MaterialOverride, ParticleEmitter,
    PhysicsBody, Prefab, Scene, UiElement,
};

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
