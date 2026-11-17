//! 场景环境：世界 / 世界数据的往返与落盘形态。
//!
//! 环境不是实体，因此必须单独验证：默认环境不落盘（旧世界文件字节不变），
//! 非默认环境随 `WorldData` 存取，且 `sun()` 只认可用的方向光。

use xgc_world::{LightKind, SceneEnvironment, SceneLight, World, WorldData};

#[test]
fn default_environment_is_not_serialized() {
    let data = World::new().world_data();
    let json = serde_json::to_string(&data).unwrap();
    assert!(
        !json.contains("environment"),
        "缺省环境不应落盘，实际 {json}"
    );

    // 读到没有 environment 字段的旧世界：拿到与旧引擎相同的缺省光照。
    let legacy: WorldData = serde_json::from_str(r#"{"revision":0,"next_id":0,"entities":[]}"#)
        .expect("旧世界 JSON 必须仍然可读");
    assert_eq!(legacy.environment, SceneEnvironment::default());
    assert_eq!(legacy.environment.ambient, [0.06, 0.06, 0.08]);
    assert_eq!(legacy.environment.lights.len(), 1);
}

#[test]
fn custom_environment_survives_world_data_roundtrip() {
    let environment = SceneEnvironment {
        lights: vec![SceneLight::directional(
            "Sun",
            [0.45, -1.0, 0.35],
            [1.0, 0.97, 0.92],
            1.4,
        )],
        ambient: [0.09, 0.1, 0.13],
    };
    let mut world = World::new();
    world.set_environment(environment.clone());
    let data = world.world_data();
    let json = serde_json::to_string(&data).unwrap();
    assert!(json.contains("environment"), "非缺省环境必须落盘：{json}");

    let decoded: WorldData = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.environment, environment);
    let mut restored = World::new();
    restored.restore_world_data(&decoded).unwrap();
    assert_eq!(restored.environment(), &environment);
}

#[test]
fn sun_skips_inactive_and_point_lights() {
    let environment = SceneEnvironment {
        lights: vec![
            SceneLight {
                name: "dead".to_string(),
                intensity: 0.0,
                ..SceneLight::default()
            },
            SceneLight {
                name: "lamp".to_string(),
                kind: LightKind::Point,
                intensity: 5.0,
                ..SceneLight::default()
            },
            SceneLight::directional("sun", [0.0, -1.0, 0.0], [1.0; 3], 2.0),
        ],
        ambient: [0.0; 3],
    };
    assert_eq!(
        environment.sun().map(|light| light.name.as_str()),
        Some("sun")
    );
    assert_eq!(
        environment.sun().unwrap().normalized_direction(),
        [0.0, -1.0, 0.0]
    );

    let dark = SceneEnvironment {
        lights: vec![SceneLight {
            intensity: 0.0,
            ..SceneLight::default()
        }],
        ambient: [0.0; 3],
    };
    assert!(dark.sun().is_none());
}

#[test]
fn zero_direction_degrades_to_straight_down() {
    let light = SceneLight {
        direction: [0.0, 0.0, 0.0],
        ..SceneLight::default()
    };
    assert_eq!(light.normalized_direction(), [0.0, -1.0, 0.0]);
}
