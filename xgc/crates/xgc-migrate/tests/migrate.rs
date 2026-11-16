//! 迁移回归：旧 SceneV2 RON 的实体 / 父子 / 变换映射。

use xgc_migrate::{migrate_str, MigrationError};
use xgc_world::{
    Animator as XgcAnimator, AudioSource, Camera, CharacterController as XgcCharacter, Component,
    ComponentId, GlobalTransform, Material, MeshRenderer, Name, Parent, ParticleEmitter,
    PhysicsBody, Script, Transform, UiElement, Value,
};

const SCENE: &str = r#"(
    format_version: 2,
    name: "Migrated",
    lights: [],
    ambient: (0.0, 0.0, 0.0),
    entities: [
        (
            id: (1),
            name: "Root",
            parent: None,
            layer: 0,
            transform: (
                position: (10.0, 0.0, 0.0),
                rotation: (0.0, 0.0, 0.0, 1.0),
                scale: (1.0, 1.0, 1.0),
            ),
            transform_identity: (id: 1),
            components: (
                next_component_id: (next_id: 2),
                entries: [],
            ),
        ),
        (
            id: (2),
            name: "Child",
            parent: Some((1)),
            layer: 0,
            transform: (
                position: (1.0, 2.0, 3.0),
                rotation: (0.0, 0.0, 0.0, 1.0),
                scale: (2.0, 2.0, 2.0),
            ),
            transform_identity: (id: 2),
            components: (
                next_component_id: (next_id: 1),
                entries: [],
            ),
        ),
    ],
)"#;

#[test]
fn migrates_identity_transform_and_parent() {
    let (world, report) = migrate_str(SCENE).unwrap();
    assert_eq!(report.entities, 2);
    assert_eq!(report.with_old_components, 0);
    assert_eq!(report.mapped_components, 0);
    assert_eq!(report.missing_parents, 0);
    assert_eq!(world.next_id, 3);
    assert_eq!(world.entities.len(), 2);

    let root = &world.entities[0];
    assert_eq!(root.id.raw(), 1);
    assert!(root
        .components
        .iter()
        .any(|component| component.component == Transform::ID));
    assert!(root
        .components
        .iter()
        .any(|component| component.component == Name::ID));
    assert!(root
        .components
        .iter()
        .any(|component| component.component == GlobalTransform::ID));

    let child = &world.entities[1];
    let parent = child
        .components
        .iter()
        .find(|component| component.component == Parent::ID)
        .unwrap();
    assert_eq!(parent.fields[0].1, Value::Entity(1));
    let transform = child
        .components
        .iter()
        .find(|component| component.component == Transform::ID)
        .unwrap();
    assert_eq!(transform.fields[0].1, Value::Vec3([1.0, 2.0, 3.0]));
    assert_eq!(transform.fields[2].1, Value::Vec3([2.0, 2.0, 2.0]));

    assert_eq!(
        xgc_migrate::component_id_name(ComponentId(0x0101)),
        "core.transform"
    );
}

#[test]
fn maps_mesh_material_and_camera() {
    let entries = r#"entries: [
                    (
                        identity: (id: 2),
                        data: Mesh((model: "builtin://cube", morph_weights: [])),
                    ),
                    (
                        identity: (id: 3),
                        data: Material((asset: None, overrides: Some((base_color: Some((0.5, 0.6, 0.7, 1.0)), roughness: Some(0.9))))),
                    ),
                    (
                        identity: (id: 4),
                        data: Camera((fov: 75.0, near: 0.05, far: 500.0, active: true)),
                    ),
                ],"#;
    let scene = SCENE.replacen("entries: [],", entries, 1);
    let (world, report) = migrate_str(&scene).unwrap();
    assert_eq!(report.mapped_components, 3);
    let root = &world.entities[0];
    let mesh = root
        .components
        .iter()
        .find(|component| component.component == MeshRenderer::ID)
        .unwrap();
    assert_eq!(
        mesh.fields[0].1,
        Value::String("builtin://cube".to_string())
    );
    let material = root
        .components
        .iter()
        .find(|component| component.component == Material::ID)
        .unwrap();
    let Value::Vec4(color) = material.fields[0].1 else {
        panic!("material base_color 不是 vec4");
    };
    assert!((color[0] - 0.5).abs() < 1e-6);
    assert!((color[1] - 0.6).abs() < 1e-6);
    assert!((color[2] - 0.7).abs() < 1e-6);
    let Value::F64(roughness) = material.fields[1].1 else {
        panic!("material roughness 不是 f64");
    };
    assert!((roughness - 0.9).abs() < 1e-6);
    let camera = root
        .components
        .iter()
        .find(|component| component.component == Camera::ID)
        .unwrap();
    assert_eq!(camera.fields[0].1, Value::F64(75.0));
    assert_eq!(camera.fields[3].1, Value::Bool(true));
}

#[test]
fn maps_script_and_audio() {
    let entries = r#"entries: [
                    (
                        identity: (id: 2),
                        data: Lua(path: "Scripts/player.lua"),
                    ),
                    (
                        identity: (id: 3),
                        data: Audio((
                            clip: "Resources/Audio/Beep.wav",
                            volume: 0.5,
                            bus: Sfx,
                            pitch: 1.25,
                            looping: true,
                            autoplay: true,
                            spatial: false,
                        )),
                    ),
                ],"#;
    let scene = SCENE.replacen("entries: [],", entries, 1);
    let (world, report) = migrate_str(&scene).unwrap();
    assert_eq!(report.mapped_components, 2);
    let root = &world.entities[0];
    let script = root
        .components
        .iter()
        .find(|component| component.component == Script::ID)
        .unwrap();
    assert_eq!(
        script.fields[0].1,
        Value::String("Scripts/player.lua".to_string())
    );
    let audio = root
        .components
        .iter()
        .find(|component| component.component == AudioSource::ID)
        .unwrap();
    assert_eq!(
        audio.fields[0].1,
        Value::String("Resources/Audio/Beep.wav".to_string())
    );
    assert_eq!(audio.fields[1].1, Value::F64(0.5));
    assert_eq!(audio.fields[2].1, Value::F64(1.25));
    assert_eq!(audio.fields[3].1, Value::Bool(true));
    assert_eq!(audio.fields[4].1, Value::Bool(true));
    assert_eq!(audio.fields[5].1, Value::Bool(false));
}

#[test]
fn maps_physics_shape_and_body() {
    let entries = r#"entries: [
                    (
                        identity: (id: 2),
                        data: Physics((
                            kind: Static,
                            shape: Cuboid(
                                half_extents: (12.0, 0.5, 12.0),
                            ),
                            mass: 0.0,
                            friction: 0.8,
                            restitution: 0.1,
                            gravity_scale: 1.0,
                            is_sensor: false,
                        )),
                    ),
                ],"#;
    let scene = SCENE.replacen("entries: [],", entries, 1);
    let (world, report) = migrate_str(&scene).unwrap();
    assert_eq!(report.mapped_components, 1);
    let physics = world.entities[0]
        .components
        .iter()
        .find(|component| component.component == PhysicsBody::ID)
        .unwrap();
    assert_eq!(physics.fields[0].1, Value::String("static".to_string()));
    assert_eq!(physics.fields[1].1, Value::String("cuboid".to_string()));
    assert_eq!(physics.fields[2].1, Value::Vec3([12.0, 0.5, 12.0]));
    assert_eq!(physics.fields[5].1, Value::F64(f64::from(0.8f32)));
}

#[test]
fn unsupported_version_and_duplicate_ids_are_rejected() {
    let wrong = SCENE.replace("format_version: 2", "format_version: 1");
    assert!(matches!(
        migrate_str(&wrong),
        Err(MigrationError::UnsupportedVersion(1))
    ));

    let duplicate = SCENE.replace("id: (2)", "id: (1)");
    assert!(matches!(
        migrate_str(&duplicate),
        Err(MigrationError::DuplicateId(1))
    ));
}

#[test]
fn missing_parent_is_counted_not_fatal() {
    let scene = SCENE.replace("Some((1))", "Some((99))");
    let (_, report) = migrate_str(&scene).unwrap();
    assert_eq!(report.missing_parents, 1);
}

#[test]
fn migrate_dir_writes_each_scene_and_reports_failures() {
    let dir = std::env::temp_dir().join(format!(
        "xgc_migrate_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let input = dir.join("Scenes");
    let output = dir.join("out");
    std::fs::create_dir_all(&input).unwrap();
    std::fs::write(input.join("good.ron"), SCENE).unwrap();
    std::fs::write(
        input.join("bad.ron"),
        SCENE.replace("format_version: 2", "format_version: 1"),
    )
    .unwrap();

    let batch = xgc_migrate::migrate_dir(&input, &output).unwrap();
    assert_eq!(batch.scenes, 1);
    assert_eq!(batch.entities, 2);
    assert_eq!(batch.failures.len(), 1);
    let migrated = xgc_store::load_world(&output.join("good.json")).unwrap();
    assert_eq!(migrated.entities.len(), 2);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn maps_animator() {
    let scene = r#"(
        format_version: 2,
        name: "Anim",
        lights: [],
        ambient: (0.0, 0.0, 0.0),
        entities: [
            (
                id: (1),
                name: "Animated",
                parent: None,
                layer: 0,
                transform: (
                    position: (0.0, 0.0, 0.0),
                    rotation: (0.0, 0.0, 0.0, 1.0),
                    scale: (1.0, 1.0, 1.0),
                ),
                transform_identity: (id: 1),
                components: (
                    next_component_id: (next_id: 2),
                    entries: [
                        (
                            identity: (id: 2),
                            data: Animator((
                                clip: "Walk",
                                speed: 2.0,
                                looping: false,
                                autoplay: false,
                                fade_time: 0.25,
                            )),
                        ),
                    ],
                ),
            ),
        ],
    )"#;
    let (world, report) = migrate_str(scene).unwrap();
    assert_eq!(report.mapped_components, 1);
    let animator = world.entities[0]
        .components
        .iter()
        .find(|component| component.component == XgcAnimator::ID)
        .unwrap();
    let field = |name: &str| {
        animator
            .fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.clone())
            .unwrap()
    };
    assert_eq!(field("clip"), Value::String("Walk".to_string()));
    assert_eq!(field("speed"), Value::F64(2.0));
    assert_eq!(field("looping"), Value::Bool(false));
    assert_eq!(field("autoplay"), Value::Bool(false));
    assert_eq!(field("fade_time"), Value::F64(0.25));
}

#[test]
fn maps_character() {
    let scene = r#"(
        format_version: 2,
        name: "Char",
        lights: [],
        ambient: (0.0, 0.0, 0.0),
        entities: [
            (
                id: (1),
                name: "Player",
                parent: None,
                layer: 0,
                transform: (
                    position: (0.0, 0.0, 0.0),
                    rotation: (0.0, 0.0, 0.0, 1.0),
                    scale: (1.0, 1.0, 1.0),
                ),
                transform_identity: (id: 1),
                components: (
                    next_component_id: (next_id: 2),
                    entries: [
                        (
                            identity: (id: 2),
                            data: Character((
                                radius: 0.3,
                                height: 1.8,
                                step_offset: 0.35,
                                slope_limit: 45.0,
                                skin_width: 0.02,
                                snap_to_ground: 0.3,
                            )),
                        ),
                    ],
                ),
            ),
        ],
    )"#;
    let (world, report) = migrate_str(scene).unwrap();
    assert_eq!(report.mapped_components, 1);
    let character = world.entities[0]
        .components
        .iter()
        .find(|component| component.component == XgcCharacter::ID)
        .unwrap();
    let field = |name: &str| {
        character
            .fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.clone())
            .unwrap()
    };
    assert_eq!(field("radius").as_f32(), Some(0.3));
    assert_eq!(field("height").as_f32(), Some(1.8));
    assert_eq!(field("step_offset").as_f32(), Some(0.35));
    assert_eq!(field("snap_to_ground").as_f32(), Some(0.3));
}

#[test]
fn maps_ui_element() {
    let scene = r#"(
        format_version: 2,
        name: "Ui",
        lights: [],
        ambient: (0.0, 0.0, 0.0),
        entities: [
            (
                id: (1),
                name: "HUD",
                parent: None,
                layer: 0,
                transform: (
                    position: (0.0, 0.0, 0.0),
                    rotation: (0.0, 0.0, 0.0, 1.0),
                    scale: (1.0, 1.0, 1.0),
                ),
                transform_identity: (id: 1),
                components: (
                    next_component_id: (next_id: 2),
                    entries: [
                        (
                            identity: (id: 2),
                            data: Ui((
                                kind: Panel,
                                anchor_min: (0.0, 0.0),
                                anchor_max: (0.0, 0.0),
                                offset_min: (24.0, 24.0),
                                offset_max: (364.0, 200.0),
                                style: (
                                    background: (0.1, 0.2, 0.3, 0.8),
                                    border_color: (0.2, 0.2, 0.2, 1.0),
                                    border_width: 1.0,
                                    corner_radius: 10.0,
                                    padding: 6.0,
                                ),
                                text: (
                                    content: "",
                                    size: 20.0,
                                    color: (1.0, 1.0, 1.0, 1.0),
                                ),
                                visible: true,
                                interactable: true,
                            )),
                        ),
                    ],
                ),
            ),
        ],
    )"#;
    let (world, report) = migrate_str(scene).unwrap();
    assert_eq!(report.mapped_components, 1);
    let ui = world.entities[0]
        .components
        .iter()
        .find(|component| component.component == UiElement::ID)
        .unwrap();
    let field = |name: &str| {
        ui.fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.clone())
            .unwrap()
    };
    assert_eq!(field("kind"), Value::String("panel".to_string()));
    assert_eq!(field("offset_min").as_vec2(), Some([24.0, 24.0]));
    assert_eq!(field("corner_radius").as_f32(), Some(10.0));
    assert_eq!(field("visible"), Value::Bool(true));
}

#[test]
fn maps_particle() {
    let scene = r#"(
        format_version: 2,
        name: "Fx",
        lights: [],
        ambient: (0.0, 0.0, 0.0),
        entities: [
            (
                id: (1),
                name: "Fire",
                parent: None,
                layer: 0,
                transform: (
                    position: (0.0, 0.0, 0.0),
                    rotation: (0.0, 0.0, 0.0, 1.0),
                    scale: (1.0, 1.0, 1.0),
                ),
                transform_identity: (id: 1),
                components: (
                    next_component_id: (next_id: 2),
                    entries: [
                        (
                            identity: (id: 2),
                            data: Particle((
                                emitting: true,
                                rate: 30.0,
                                max_particles: 100,
                                lifetime: (0.5, 1.0),
                                speed: (1.0, 2.0),
                                gravity: (0.0, -1.0, 0.0),
                                start_color: (1.0, 0.5, 0.1, 0.9),
                                seed: 7,
                            )),
                        ),
                    ],
                ),
            ),
        ],
    )"#;
    let (world, report) = migrate_str(scene).unwrap();
    assert_eq!(report.mapped_components, 1);
    let particle = world.entities[0]
        .components
        .iter()
        .find(|component| component.component == ParticleEmitter::ID)
        .unwrap();
    let field = |name: &str| {
        particle
            .fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.clone())
            .unwrap()
    };
    assert_eq!(field("rate").as_f32(), Some(30.0));
    assert_eq!(field("max_particles"), Value::I64(100));
    assert_eq!(field("lifetime").as_vec2(), Some([0.5, 1.0]));
    assert_eq!(field("seed"), Value::I64(7));
}
