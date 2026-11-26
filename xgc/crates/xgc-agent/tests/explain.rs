//! 实体渲染诊断从真实的抽取条件得到结论，调用后世界版本与状态不变。

use serde_json::{json, Value as Json};
use xgc_agent::Agent;
use xgc_world::{Camera, GlobalTransform, LodGroup, LodLevel, MeshRenderer, World};

fn call(agent: &mut Agent, method: &str, params: Json) -> Json {
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    serde_json::from_str(&agent.handle_line(&request.to_string()).unwrap()).unwrap()
}

fn result(agent: &mut Agent, method: &str, params: Json) -> Json {
    let response = call(agent, method, params);
    assert!(response.get("error").is_none(), "{response}");
    response["result"].clone()
}

fn agent(world: &World) -> Agent {
    let mut agent = Agent::new();
    result(
        &mut agent,
        "world.restore",
        json!({"data": world.world_data()}),
    );
    agent
}

fn at(x: f32) -> GlobalTransform {
    GlobalTransform {
        position: [x, 0.0, 0.0],
        ..GlobalTransform::default()
    }
}

fn mesh(model: &str) -> MeshRenderer {
    MeshRenderer {
        model: model.into(),
    }
}

#[test]
fn reports_draw_extraction_reasons_and_the_selected_lod() {
    let mut world = World::new();
    let no_global = world.spawn((mesh("builtin://cube"),));
    let empty = world.spawn((at(0.0), MeshRenderer::default()));
    let empty_lod = world.spawn((
        at(0.0),
        LodGroup {
            levels: vec![LodLevel::default()],
        },
    ));
    let no_model_component = world.spawn((at(0.0),));
    let lod = world.spawn((
        at(5.0),
        mesh("builtin://plane"),
        LodGroup {
            levels: [("builtin://cube", 2.0), ("builtin://sphere", 4.0)]
                .into_iter()
                .map(|(model, max_distance)| LodLevel {
                    model: model.into(),
                    max_distance,
                })
                .collect(),
        },
    ));
    let mut agent = agent(&world);
    let before = result(&mut agent, "world.snapshot", Json::Null);
    for (key, reason) in [
        (no_global, "missing_global_transform"),
        (empty, "empty_model"),
        (empty_lod, "empty_model"),
        (no_model_component, "missing_model_component"),
        (lod, "outside_lod_distance"),
    ] {
        let id = world.entity(key).unwrap();
        let explanation = result(&mut agent, "world.explain", json!({"id": id}));
        assert_eq!(explanation["entity"], json!(id));
        let render = &explanation["render"];
        assert_eq!(render["stage"], "model_extraction");
        assert_eq!(render["included"], false, "{render}");
        assert_eq!(render["reason"], reason);
        assert!(render["model"].is_null());
        assert!(render.get("visible").is_none());
    }
    let id = world.entity(lod).unwrap();
    for (eye, bias, model) in [
        ([1.0, 0.0, 0.0], 1.0, "builtin://sphere"),
        ([3.0, 0.0, 0.0], 1.0, "builtin://cube"),
        ([1.0, 0.0, 0.0], 2.0, "builtin://cube"),
        ([50.0, 0.0, 0.0], 0.0, "builtin://cube"),
    ] {
        let explanation = result(
            &mut agent,
            "world.explain",
            json!({"id": id, "eye": eye, "lod_bias": bias}),
        );
        let render = &explanation["render"];
        assert_eq!(render["included"], true, "{render}");
        assert!(render["reason"].is_null());
        assert_eq!(render["model"], model);
        assert_eq!(render["eye"], json!(eye));
        assert_eq!(render["lod_bias"], bias);
    }
    assert_eq!(result(&mut agent, "world.snapshot", Json::Null), before);
}

#[test]
fn defaults_to_the_render_camera_and_checks_explicit_inputs() {
    let mut world = World::new();
    let inactive = world.spawn((
        at(20.0),
        Camera {
            active: false,
            ..Camera::default()
        },
    ));
    let missing_pose = world.spawn((Camera::default(),));
    world.spawn((at(2.0), Camera::default()));
    let second_active = world.spawn((at(3.0), Camera::default()));
    let target = world.spawn((at(0.0), mesh("guid://unresolved-model")));
    let id = world.entity(target).unwrap();
    let mut agent = agent(&world);
    let automatic = result(&mut agent, "world.explain", json!({"id": id.raw()}));
    assert_eq!(automatic["render"]["eye"], json!([2.0, 0.0, 0.0]));
    assert_eq!(automatic["render"]["model"], "guid://unresolved-model");
    for (camera, eye) in [
        (inactive, [20.0, 0.0, 0.0]),
        (second_active, [3.0, 0.0, 0.0]),
    ] {
        let response = result(
            &mut agent,
            "world.explain",
            json!({"id": id, "camera": world.entity(camera).unwrap()}),
        );
        assert_eq!(response["render"]["eye"], json!(eye));
        assert_eq!(response["render"]["included"], true);
        assert!(response["render"].get("visible").is_none());
    }

    for (params, field) in [
        (json!({"id": "999"}), "id"),
        (json!({"id": id, "camera": "999"}), "camera"),
        (json!({"id": id, "camera": id}), "camera"),
        (
            json!({"id": id, "camera": world.entity(missing_pose)}),
            "camera",
        ),
        (json!({"id": id, "camera": 1, "eye": [0, 0, 0]}), "camera"),
        (json!({"id": id, "eye": [0, 0]}), "eye"),
        (json!({"id": id, "eye": [1e39, 0, 0]}), "eye"),
        (json!({"id": id, "lod_bias": "0"}), "lod_bias"),
    ] {
        let response = call(&mut agent, "world.explain", params);
        assert_eq!(response["error"]["code"], -32000);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains(field));
    }
}
