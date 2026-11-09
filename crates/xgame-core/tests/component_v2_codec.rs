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
