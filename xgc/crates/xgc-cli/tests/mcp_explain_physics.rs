//! 通过 MCP stdio 对照建体时的重叠诊断与 Rapier 实际步进。

use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;

use serde_json::{json, Value as Json};
use xgc_base::EntityId;
use xgc_world::{GlobalTransform, MeshRenderer, Parent, PhysicsBody, Transform, World};

#[test]
fn sensor_and_solid_overlap_explanations_match_preview_trajectories() {
    let mut world = World::new();
    let parent = world.spawn((Transform {
        position: [4.0, 2.0, 0.0],
        scale: [2.0, 1.0, 2.0],
        ..Transform::default()
    },));
    let platform = world.spawn((
        Parent {
            entity: world.entity(parent).unwrap(),
        },
        // 只有沿父链取世界位置与缩放，Auto 盒才会在球的下方。故意省去派生变换。
        Transform {
            position: [0.5, 0.0, 0.0],
            scale: [4.0, 1.0, 4.0],
            ..Transform::default()
        },
        PhysicsBody {
            kind: "static".into(),
            is_sensor: true,
            ..PhysicsBody::default()
        },
    ));
    let probe = world.spawn((
        Transform {
            position: [8.0, 2.875, 0.0],
            ..Transform::default()
        },
        GlobalTransform::default(),
        PhysicsBody {
            shape: "ball".into(),
            ..PhysicsBody::default()
        },
        MeshRenderer {
            model: "builtin://sphere".into(),
        },
    ));
    let id = world.entity(probe).unwrap();
    let other = world.entity(platform).unwrap();
    let (sensor, sensor_y) = explain_and_preview(&world, id, other);
    world.get_mut::<PhysicsBody>(platform).unwrap().is_sensor = false;
    let (solid, solid_y) = explain_and_preview(&world, id, other);

    // 平台范围 y=1.5..2.5，球半径 0.5。实体场景只切换了 sensor 标志。
    println!("第 60 帧球心 y，sensor {sensor_y:.6}，solid {solid_y:.6}");
    println!("sensor 解释 {sensor}\nsolid 解释 {solid}");
    assert!(sensor_y + 0.5 < 1.5, "球没有穿过传感器：{sensor_y}");
    assert!(
        (solid_y - 3.0).abs() < 0.02,
        "球没有停在实体平台上：{solid_y}"
    );
    for (physics, reason, is_sensor) in [
        (sensor, "sensor_overlap", true),
        (solid, "solid_overlap", false),
    ] {
        assert_eq!(physics["reason"], reason, "{physics}");
        assert_eq!(physics["stage"], "collider_overlap");
        assert_eq!(physics["source"], "scene_snapshot");
        assert_eq!(physics["other"], json!(other));
        assert_eq!(physics["intersects"], true);
        assert_eq!(physics["body"]["kind"], "dynamic");
        assert_eq!(physics["body"]["sensor"], false);
        assert_eq!(physics["other_body"]["kind"], "static");
        assert_eq!(physics["other_body"]["sensor"], is_sensor);
    }
}

fn explain_and_preview(world: &World, id: EntityId, other: EntityId) -> (Json, f64) {
    let (_, replies) = run_mcp(&[
        ("world_restore", json!({"data": world.world_data()})),
        ("world_snapshot", json!({})),
        (
            "world_explain",
            json!({"id": id, "eye": [0, 0, 4], "lod_bias": 2}),
        ),
        (
            "world_explain",
            json!({"id": id, "eye": [0, 0, 4], "lod_bias": 2, "physics_with": other}),
        ),
        (
            "sim_run",
            json!({"frames": 60, "watch": [{"id": id, "path": "core.transform:position"}]}),
        ),
        ("world_snapshot", json!({})),
    ]);
    tool_result(&replies[0]);
    let before = tool_result(&replies[1]);
    let render = tool_result(&replies[2]);
    assert_eq!(
        render,
        json!({"entity": id, "render": {
            "stage": "model_extraction", "included": true, "model": "builtin://sphere",
            "reason": null, "eye": [0.0, 0.0, 4.0], "lod_bias": 2.0,
        }})
    );
    let mut pair = tool_result(&replies[3]);
    let physics = pair
        .as_object_mut()
        .unwrap()
        .remove("physics")
        .unwrap_or(Json::Null);
    assert_eq!(pair, render);
    let trace = tool_result(&replies[4]);
    assert_eq!(trace["frames_run"], 60);
    let positions = trace["watch"][0]["values"].as_array().unwrap();
    assert_eq!(positions.len(), 61);
    assert_eq!(positions[0]["value"], json!([8.0, 2.875, 0.0]));
    assert_eq!(tool_result(&replies[5]), before);
    if !physics.is_null() {
        assert_eq!(physics["revision"], before["revision"]);
    }
    (physics, positions[60]["value"][1].as_f64().unwrap())
}

#[test]
fn pair_query_reports_disjoint_shapes_and_unbuilt_bodies() {
    let mut world = World::new();
    let body = PhysicsBody {
        kind: "static".into(),
        shape: "ball".into(),
        ..PhysicsBody::default()
    };
    let a = world.spawn((Transform::default(), body.clone()));
    // 两球的 AABB 三轴均重叠，中心距大于直径；包围盒不能代替形状求交。
    let b = world.spawn((
        Transform {
            position: [0.8, 0.8, 0.0],
            ..Transform::default()
        },
        body.clone(),
    ));
    let no_pose = world.spawn((body,));
    let no_body = world.spawn((Transform::default(),));
    let a = world.entity(a).unwrap();
    let b = world.entity(b).unwrap();
    let no_pose = world.entity(no_pose).unwrap();
    let no_body = world.entity(no_body).unwrap();
    let (tools, replies) = run_mcp(&[
        ("world_restore", json!({"data": world.world_data()})),
        ("world_explain", json!({"id": a, "physics_with": b.raw()})),
        ("world_explain", json!({"id": a, "physics_with": no_pose})),
        ("world_explain", json!({"id": no_body, "physics_with": a})),
        ("world_explain", json!({"id": a, "physics_with": null})),
        ("world_explain", json!({"id": a})),
        ("world_explain", json!({"id": a, "physics_with": a})),
        ("world_explain", json!({"id": a, "physics_with": "999"})),
        ("world_explain", json!({"id": a, "physics_with": true})),
    ]);
    tool_result(&replies[0]);
    let disjoint = &tool_result(&replies[1])["physics"];
    assert_eq!(disjoint["reason"], "shapes_disjoint", "{disjoint}");
    assert_eq!(disjoint["intersects"], false);
    for (reply, missing, present) in [
        (&replies[2], "other_body", "body"),
        (&replies[3], "body", "other_body"),
    ] {
        let parsed = tool_result(reply);
        let physics = &parsed["physics"];
        assert_eq!(physics["reason"], "missing_body", "{physics}");
        assert!(physics["intersects"].is_null());
        assert!(physics[missing].is_null());
        assert_eq!(physics[present]["sensor"], false);
    }
    assert_eq!(tool_result(&replies[4]), tool_result(&replies[5]));
    for reply in &replies[6..] {
        assert_eq!(reply["result"]["isError"], true, "{reply}");
        assert!(reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("physics_with"));
    }
    let explain = tools
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "world_explain")
        .unwrap();
    let schema = &explain["inputSchema"];
    assert_eq!(schema["required"], json!(["id"]));
    let variants = &schema["properties"]["physics_with"]["oneOf"];
    assert_eq!(variants[0]["type"], "integer");
    assert_eq!(variants[1]["type"], "string");
    assert_eq!(variants[2]["type"], "null");
}

fn run_mcp(calls: &[(&str, Json)]) -> (Json, Vec<Json>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_xgc"))
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    // 同时读写管道，避免一端缓冲区被快照或轨迹填满时双方互等。
    let output = thread::scope(|scope| {
        let writer = scope.spawn(move || {
            let mut send = |id, method, params| {
                let request =
                    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
                writeln!(stdin, "{request}").unwrap();
            };
            send(1, "initialize", json!({"protocolVersion": "2025-06-18"}));
            send(2, "tools/list", json!({}));
            for (index, (name, arguments)) in calls.iter().enumerate() {
                send(
                    index + 3,
                    "tools/call",
                    json!({"name": name, "arguments": arguments}),
                );
            }
        });
        let output = child.wait_with_output().unwrap();
        writer.join().unwrap();
        output
    });
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut responses: Vec<Json> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), calls.len() + 2, "{stdout}");
    for (index, reply) in responses.iter().enumerate() {
        assert_eq!(reply["id"], index + 1, "{reply}");
    }
    assert_eq!(responses[0]["result"]["protocolVersion"], "2025-06-18");
    let replies = responses.split_off(2);
    let tools = std::mem::take(&mut responses[1]["result"]["tools"]);
    (tools, replies)
}

fn tool_result(response: &Json) -> Json {
    assert_eq!(response["result"]["isError"], false, "{response}");
    serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}
