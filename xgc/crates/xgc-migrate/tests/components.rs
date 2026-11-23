//! 迁移回归：各旧组件 → XGC 组件映射。

use xgc_migrate::migrate_str;
use xgc_world::{
    Animator as XgcAnimator, CharacterController as XgcCharacter, Component, EmissionShape,
    LodGroup, ParticleBlend, ParticleConfig, ParticleEmitter, PluginComponent, UiElement, Value,
};

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

/// 旧端 `Character` 的每个字段都有 serde 缺省，作者手写的场景可以只写关心的
/// 字段；迁移必须按同一批缺省（1.8 / 0.3 / 0.35 / 45 / 0.02 / 0.3）把没写的补出来，
/// 而不是拿 "missing field" 把这类场景挡在迁移入口。（RON 里整份省略只能写成
/// `Character()`，那是非法语法；现实场景是省略其中几个字段。）
#[test]
fn maps_character_with_old_defaults_when_fields_are_omitted() {
    let scene = r#"(
        format_version: 2,
        name: "CharDefaults",
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
                            data: Character((height: 2.0, radius: 0.4)),
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
        .find(|component| component.component == xgc_world::CharacterController::ID)
        .expect("角色组件必须迁出");
    let field = |name: &str| {
        character
            .fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.clone())
            .unwrap()
    };
    // 写了的字段按原值迁出，没写的按旧端缺省补齐。
    assert_eq!(field("height").as_f32(), Some(2.0));
    assert_eq!(field("radius").as_f32(), Some(0.4));
    assert_eq!(field("step_offset").as_f32(), Some(0.35));
    assert_eq!(field("slope_limit").as_f32(), Some(45.0));
    assert_eq!(field("skin_width").as_f32(), Some(0.02));
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
    assert!(world.entities[0]
        .components
        .iter()
        .any(|component| component.component == xgc_world::UiLayout::ID));
}

#[test]
fn maps_ui_image_tint_fill_and_keep_aspect() {
    let scene = r#"(
        format_version: 2,
        name: "UiImage",
        lights: [],
        ambient: (0.0, 0.0, 0.0),
        entities: [
            (
                id: (1),
                name: "Bar",
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
                                offset_min: (0.0, 0.0),
                                offset_max: (200.0, 24.0),
                                image: Some((
                                    path: "Resources/Textures/bar.png",
                                    tint: (0.5, 0.25, 0.0, 1.0),
                                    fill: 0.5,
                                    keep_aspect: true,
                                )),
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
    // 旧端 tint 也是线性 RGBA ⇒ 原值搬运；fill 原样；keep_aspect 映射成 preserve。
    assert_eq!(
        field("image_path"),
        Value::String("Resources/Textures/bar.png".to_string())
    );
    assert_eq!(field("image_tint").as_vec4(), Some([0.5, 0.25, 0.0, 1.0]));
    assert_eq!(field("image_fill").as_f32(), Some(0.5));
    assert_eq!(field("image_mode"), Value::String("preserve".to_string()));
    assert_eq!(field("kind"), Value::String("image".to_string()));
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
                                duration: 2.5,
                                looping: false,
                                lifetime: (0.5, 1.0),
                                speed: (1.0, 2.0),
                                direction: (1.0, 0.0, 0.0),
                                spread: 12.0,
                                shape: Sphere(radius: 0.25),
                                gravity: (0.0, -1.0, 0.0),
                                drag: 0.75,
                                start_size: 0.4,
                                end_size: 0.9,
                                start_color: (1.0, 0.5, 0.1, 0.9),
                                end_color: (0.2, 0.3, 0.4, 0.0),
                                blend: Additive,
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
    let Value::String(config) = field("config") else {
        panic!("完整粒子配置必须写入 config");
    };
    let config: ParticleConfig = ron::from_str(&config).unwrap();
    assert_eq!(config.duration, 2.5);
    assert!(!config.looping);
    assert_eq!(config.direction, [1.0, 0.0, 0.0]);
    assert_eq!(config.shape, EmissionShape::Sphere { radius: 0.25 });
    assert_eq!(config.drag, 0.75);
    assert_eq!(config.start_size, 0.4);
    assert_eq!(config.end_size, 0.9);
    assert_eq!(config.end_color, [0.2, 0.3, 0.4, 0.0]);
    assert_eq!(config.blend, ParticleBlend::Additive);
}

#[test]
fn maps_rust_plugin_component() {
    let scene = r#"(
        format_version: 2,
        name: "Plugin",
        lights: [],
        ambient: (0.0, 0.0, 0.0),
        entities: [
            (
                id: (1),
                name: "Spinner",
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
                            data: Rust(
                                registered_name: "Spinner",
                                fields: None,
                            ),
                        ),
                    ],
                ),
            ),
        ],
    )"#;
    let (world, report) = migrate_str(scene).unwrap();
    assert_eq!(report.mapped_components, 1);
    let plugin = world.entities[0]
        .components
        .iter()
        .find(|component| component.component == PluginComponent::ID)
        .unwrap();
    let field = |name: &str| {
        plugin
            .fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.clone())
            .unwrap()
    };
    assert_eq!(field("registered_name"), Value::String("Spinner".into()));
    assert_eq!(field("fields"), Value::String(String::new()));
}

#[test]
fn maps_lod() {
    let scene = r#"(
        format_version: 2,
        name: "Lod",
        lights: [],
        ambient: (0.0, 0.0, 0.0),
        entities: [
            (
                id: (1),
                name: "Tree",
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
                            data: Lod((
                                levels: [
                                    (model: "builtin://cube", max_distance: 10.0),
                                    (model: "builtin://sphere", max_distance: 30.0),
                                ],
                            )),
                        ),
                    ],
                ),
            ),
        ],
    )"#;
    let (world, report) = migrate_str(scene).unwrap();
    assert_eq!(report.mapped_components, 1);
    let lod = world.entities[0]
        .components
        .iter()
        .find(|component| component.component == LodGroup::ID)
        .unwrap();
    let levels = lod
        .fields
        .iter()
        .find(|(field, _)| field == "levels")
        .map(|(_, value)| value.as_string().unwrap())
        .unwrap();
    assert!(levels.contains("builtin://cube"));
    assert!(levels.contains("30.0"));
}
