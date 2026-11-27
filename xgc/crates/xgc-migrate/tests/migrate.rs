//! 迁移回归：旧 SceneV2 RON 的实体 / 父子 / 变换映射。

use xgc_migrate::{migrate_str, MigrationError};
use xgc_world::{
    AudioSource, Camera, Component, GlobalTransform, Material, MeshRenderer, Name, Parent,
    PhysicsBody, Script, Transform, Value, World,
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
fn maps_all_lua_attachments_in_order() {
    let entries = r#"entries: [
        (identity: (id: 8), data: Lua(path: "Scripts/first.lua")),
        (identity: (id: 2), data: Mesh((model: "builtin://cube"))),
        (identity: (id: 6), data: Lua(path: "guid://0123456789abcdef")),
        (identity: (id: 4), data: Lua(path: "Scripts/last.lua")),
    ],"#;
    let (world, report) = migrate_str(&SCENE.replacen("entries: [],", entries, 1)).unwrap();
    let scripts: Vec<_> = world.entities[0]
        .components
        .iter()
        .filter(|component| component.component == Script::ID)
        .collect();
    assert_eq!(scripts.len(), 1);
    assert_eq!(
        scripts[0].fields,
        [(
            "paths".to_owned(),
            Value::List(
                [
                    "Scripts/first.lua",
                    "guid://0123456789abcdef",
                    "Scripts/last.lua"
                ]
                .map(|path| Value::String(path.to_owned()))
                .into(),
            ),
        )],
    );
    assert_eq!(report.mapped_components, 2);
    assert!(!world.entities[1]
        .components
        .iter()
        .any(|component| component.component == Script::ID));
}

#[test]
fn rejects_the_same_lua_path_twice_on_one_entity() {
    let entries = r#"entries: [
        (identity: (id: 2), data: Lua(path: "Scripts/player.lua")),
        (identity: (id: 3), data: Lua(path: "Scripts/player.lua")),
    ],"#;
    let error = migrate_str(&SCENE.replacen("entries: [],", entries, 1)).unwrap_err();
    assert!(
        matches!(&error, MigrationError::Parse(message) if message.contains("Scripts/player.lua") && message.contains("重复挂载")),
        "{error}"
    );
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
        script.fields,
        [(
            "paths".into(),
            Value::List(vec![Value::String("Scripts/player.lua".into())]),
        )],
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
    let scripted = SCENE.replacen(
        "entries: [],",
        r#"entries: [(data: Lua(path: "first.lua")), (data: Lua(path: "last.lua"))],"#,
        1,
    );
    std::fs::write(input.join("good.ron"), scripted).unwrap();
    std::fs::write(
        input.join("bad.ron"),
        SCENE.replace("format_version: 2", "format_version: 1"),
    )
    .unwrap();

    let batch = xgc_migrate::migrate_dir(&input, &output).unwrap();
    assert_eq!(batch.scenes, 1);
    assert_eq!(batch.entities, 2);
    assert_eq!(batch.mapped_components, 1);
    assert_eq!(batch.failures.len(), 1);
    let migrated = xgc_store::load_world(&output.join("good.json")).unwrap();
    assert_eq!(migrated.entities.len(), 2);
    let mut world = World::new();
    world.restore_world_data(&migrated).unwrap();
    let root = world.key(migrated.entities[0].id).unwrap();
    assert_eq!(
        world.get::<Script>(root).unwrap().paths,
        ["first.lua", "last.lua"]
    );

    std::fs::remove_dir_all(dir).ok();
}

/// 旧场景把光源与环境光放在场景顶层（不是实体）；迁移必须原样搬到
/// `WorldData.environment`，缺省字段回落到旧 `Light` 的缺省值。
const LIT_SCENE: &str = r#"(
    format_version: 2,
    name: "Lit",
    lights: [
        (
            name: "Sun",
            kind: Directional,
            direction: (0.45, -1.0, 0.35),
            position: (0.0, 0.0, 0.0),
            color: (1.0, 0.97, 0.92),
            intensity: 1.4,
            range: 10.0,
            casts_shadow: true,
            shadow_resolution: 0,
        ),
        (
            name: "Lamp",
            kind: Point,
            position: (1.0, 2.0, 3.0),
            color: (1.0, 0.5, 0.2),
            intensity: 2.5,
            range: 6.0,
        ),
    ],
    ambient: (0.09, 0.1, 0.13),
    entities: [],
)"#;

/// 旧 `Material.asset` 必须进 `render.material.asset`：材质资产是共享乘子，
/// 迁移丢掉它等于把资产基色从画面里抹掉。
#[test]
fn migrates_material_asset_reference() {
    use xgc_world::Material;

    let path = std::path::Path::new("../../../projects/XGame/Scenes/showcase.ron");
    if !path.exists() {
        eprintln!("skip: 没有旧项目 showcase.ron");
        return;
    }
    let text = std::fs::read_to_string(path).unwrap();
    let (world, _) = migrate_str(&text).unwrap();
    let fields = world
        .entities
        .iter()
        .flat_map(|entity| &entity.components)
        .filter(|component| component.component == Material::ID)
        .map(|component| {
            component
                .fields
                .iter()
                .find(|(name, _)| name == "asset")
                .and_then(|(_, value)| value.clone().as_string().map(str::to_string))
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    assert!(
        fields
            .iter()
            .any(|asset| asset == "guid://235b50780baf2866df65bddf31c31470"),
        "Concrete 材质引用必须保留：{fields:?}"
    );
    assert!(
        fields.iter().any(|asset| asset.is_empty()),
        "没有材质资产的实体不应凭空得到引用：{fields:?}"
    );
}

#[test]
fn migrates_scene_lights_and_ambient() {
    use xgc_world::LightKind;

    let (world, _) = migrate_str(LIT_SCENE).unwrap();
    let environment = &world.environment;
    assert_eq!(environment.ambient, [0.09, 0.1, 0.13]);
    assert_eq!(environment.lights.len(), 2);

    let sun = &environment.lights[0];
    assert_eq!(sun.name, "Sun");
    assert_eq!(sun.kind, LightKind::Directional);
    assert_eq!(sun.direction, [0.45, -1.0, 0.35]);
    assert_eq!(sun.color, [1.0, 0.97, 0.92]);
    assert_eq!(sun.intensity, 1.4);
    assert!(sun.casts_shadow);
    assert_eq!(
        environment.sun().map(|light| light.name.as_str()),
        Some("Sun")
    );

    let lamp = &environment.lights[1];
    assert_eq!(lamp.kind, LightKind::Point);
    assert_eq!(lamp.position, [1.0, 2.0, 3.0]);
    assert_eq!(lamp.range, 6.0);
    // 省略的字段按旧 `Light` 的 serde 缺省补齐，不留在零值上。
    assert_eq!(lamp.direction, [0.5, -1.0, 0.5]);
    assert_eq!(lamp.shadow_resolution, 0);
}

/// 旧场景不存刚体速度（由物理后端持有）。动态刚体迁移时必须补零速度 ——
/// XGC 的物理系统按 `(Transform, Velocity, PhysicsBody)` 查询，缺列就永远不动。
#[test]
fn dynamic_bodies_get_zero_velocity_but_static_ones_do_not() {
    use xgc_world::Velocity;

    let scene = r#"(
        format_version: 2,
        name: "Bodies",
        lights: [],
        ambient: (0.0, 0.0, 0.0),
        entities: [
            (id: (1), parent: None, layer: 0,
             transform: (position: (0.0, 5.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0), scale: (1.0, 1.0, 1.0)),
             transform_identity: (id: 1),
             components: (next_component_id: (next_id: 2), entries: [
                (identity: (id: 1), data: Physics((
                    kind: Dynamic,
                    shape: Auto,
                    mass: 1.0, friction: 0.5, restitution: 0.0, gravity_scale: 1.0, is_sensor: false,
                ))),
             ])),
            (id: (2), parent: None, layer: 0,
             transform: (position: (0.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0), scale: (1.0, 1.0, 1.0)),
             transform_identity: (id: 2),
             components: (next_component_id: (next_id: 2), entries: [
                (identity: (id: 1), data: Physics((
                    kind: Static,
                    shape: Auto,
                    mass: 0.0, friction: 0.5, restitution: 0.0, gravity_scale: 1.0, is_sensor: false,
                ))),
             ])),
        ],
    )"#;
    let (world, _) = migrate_str(scene).unwrap();
    let velocity_of = |id: u64| {
        world
            .entities
            .iter()
            .find(|entity| entity.id.raw() == id)
            .unwrap()
            .components
            .iter()
            .find(|component| component.component == Velocity::ID)
            .cloned()
    };
    let dynamic = velocity_of(1).expect("动态刚体必须带零速度");
    assert!(dynamic
        .fields
        .iter()
        .any(|(name, value)| name == "linear" && *value == Value::Vec3([0.0; 3])));
    assert!(velocity_of(2).is_none(), "静态刚体不应被塞速度列");
}
