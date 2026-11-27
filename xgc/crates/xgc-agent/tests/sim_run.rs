//! 仿真预览通过真实协议运行，编辑会话仍可继续原来的分页与事务。

use serde_json::{json, Value as Json};
use xgc_agent::Agent;
use xgc_base::EntityId;
use xgc_world::{Command, Component, ComponentData, EntityData, Name, Transform, Value, Velocity};

const PROBED_ID: u64 = 9_007_199_254_740_993;

fn request(method: &str, params: Json) -> String {
    json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string()
}

fn call(agent: &mut Agent, method: &str, params: Json) -> Json {
    let response = agent.handle_line(&request(method, params)).unwrap();
    serde_json::from_str(&response).unwrap()
}

fn result(agent: &mut Agent, method: &str, params: Json) -> Json {
    let response = call(agent, method, params);
    assert!(response.get("error").is_none(), "{response}");
    response["result"].clone()
}

fn plan(agent: &mut Agent, commands: Vec<Command>) -> Json {
    let plan = result(agent, "edit.plan", json!({"commands": commands}));
    assert_eq!(plan["valid"], true, "{plan}");
    plan["plan_id"].clone()
}

fn spawn(id: u64, components: Vec<ComponentData>) -> Command {
    Command::Spawn {
        data: EntityData {
            id: EntityId::from_raw(id),
            revision: 0,
            components,
        },
    }
}

#[test]
fn previews_each_checkpoint_and_preserves_the_edit_session() {
    let mut agent = Agent::new();
    let initial_plan = plan(
        &mut agent,
        vec![
            spawn(1, vec![ComponentData::from_component(&Name::default())]),
            spawn(
                PROBED_ID,
                vec![
                    ComponentData::from_component(&Transform::default()),
                    ComponentData::from_component(&Velocity {
                        linear: [2.0, 0.0, 0.0],
                    }),
                ],
            ),
        ],
    );
    result(&mut agent, "edit.apply", json!({"plan_id": initial_plan}));
    let pending_plan = plan(
        &mut agent,
        vec![Command::SetField {
            entity: EntityId::from_raw(PROBED_ID),
            component: Transform::ID,
            field: "position".into(),
            value: Value::Vec3([9.0, 0.0, 0.0]),
        }],
    );
    let start = result(&mut agent, "world.digest", Json::Null);
    let snapshot = result(&mut agent, "world.snapshot", Json::Null);
    let history = result(&mut agent, "engine.history", Json::Null);
    let first_page = result(&mut agent, "world.query", json!({"limit": 1}));

    let params = json!({
        "frames": 2,
        "dt": 0.25,
        "watch": [
            {"id": PROBED_ID.to_string(), "path": "core.transform:position"},
            {"id": "1", "path": "core.transform:position"}
        ]
    });
    let request = request("sim.run", params);
    let first = agent.handle_line(&request).unwrap();
    let response: Json = serde_json::from_str(&first).unwrap();
    assert!(response.get("error").is_none(), "{response}");
    let preview = &response["result"];
    assert_eq!(preview["frames_run"], 2);
    assert!(preview.get("trace_id").is_none());
    assert_eq!(preview["checkpoints"].as_array().unwrap().len(), 3);
    assert_eq!(preview["checkpoints"][0]["frame"], 0);
    assert_eq!(preview["checkpoints"][0]["state_hash"], start["state_hash"]);
    assert_eq!(preview["checkpoints"][1]["frame"], 1);
    assert_eq!(preview["checkpoints"][2]["frame"], 2);
    assert_eq!(
        preview["checkpoints"][2]["state_hash"],
        preview["state_hash"]
    );
    assert_ne!(preview["state_hash"], start["state_hash"]);
    assert_eq!(preview["watch"][0]["id"], PROBED_ID.to_string());
    assert_eq!(preview["watch"][0]["path"], "core.transform:position");
    assert_eq!(
        preview["watch"][0]["values"],
        json!([
            {"kind": "vec3", "value": [0.0, 0.0, 0.0]},
            {"kind": "vec3", "value": [0.5, 0.0, 0.0]},
            {"kind": "vec3", "value": [1.0, 0.0, 0.0]}
        ])
    );
    assert_eq!(preview["watch"][1]["values"], json!([null, null, null]));
    assert_eq!(agent.handle_line(&request).unwrap(), first);

    assert_eq!(result(&mut agent, "world.digest", Json::Null), start);
    assert_eq!(result(&mut agent, "world.snapshot", Json::Null), snapshot);
    assert_eq!(result(&mut agent, "engine.history", Json::Null), history);
    let second_page = result(
        &mut agent,
        "world.query",
        json!({"cursor": first_page["next_cursor"], "limit": 1}),
    );
    assert_eq!(second_page["entities"][0]["id"], PROBED_ID.to_string());
    result(&mut agent, "edit.apply", json!({"plan_id": pending_plan}));
    assert_eq!(result(&mut agent, "engine.history", Json::Null)["count"], 2);
    result(&mut agent, "edit.undo", Json::Null);
    assert_eq!(
        result(&mut agent, "world.digest", Json::Null)["state_hash"],
        start["state_hash"]
    );
}

#[test]
fn rejects_invalid_parameters_before_stepping() {
    let mut agent = Agent::new();
    for (params, expected) in [
        (json!({}), "frames"),
        (json!({"frames": -1}), "frames"),
        (json!({"frames": 1.5}), "frames"),
        (json!({"frames": "2"}), "frames"),
        (json!({"frames": 301}), "frames"),
        (json!({"frames": 2, "inputs": []}), "inputs"),
        (json!({"frames": 2, "dt": 0}), "dt"),
        (json!({"frames": 2, "dt": 0.001}), "dt"),
        (json!({"frames": 2, "dt": 2}), "dt"),
        (json!({"frames": 2, "dt": "0.25"}), "dt"),
        (json!({"frames": 2, "project": ""}), "project"),
        (json!({"frames": 2, "project": true}), "project"),
        (
            json!({"frames": 2, "watch": "core.transform:position"}),
            "watch",
        ),
        (
            json!({"frames": 2, "watch": [{"id": "-1", "path": "core.transform:position"}]}),
            "id",
        ),
        (
            json!({"frames": 0, "watch": [{"id": u64::MAX.to_string(), "path": "core.transform:position"}]}),
            "超出持久 ID 上限",
        ),
        (
            json!({"frames": 0, "watch": [{"id": u64::MAX, "path": "core.transform:position"}]}),
            "超出持久 ID 上限",
        ),
        (
            json!({"frames": 2, "watch": [{"id": "1", "path": "core.transform:absent"}]}),
            "absent",
        ),
    ] {
        let response = call(&mut agent, "sim.run", params);
        assert_eq!(response["error"]["code"], -32000, "{response}");
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains(expected),
            "{response}"
        );
    }
    let watch = json!({"id": "1", "path": "core.transform:position"});
    let too_many = call(
        &mut agent,
        "sim.run",
        json!({"frames": 0, "watch": vec![watch; 17]}),
    );
    assert!(too_many["error"]["message"]
        .as_str()
        .unwrap()
        .contains("watch"));

    let start = result(&mut agent, "world.digest", Json::Null);
    let preview = result(&mut agent, "sim.run", json!({"frames": 0}));
    assert_eq!(preview["frames_run"], 0);
    assert_eq!(preview["state_hash"], start["state_hash"]);
    assert_eq!(
        preview["checkpoints"],
        json!([{"frame": 0, "state_hash": start["state_hash"]}])
    );
    assert_eq!(preview["watch"], json!([]));

    let last_id = EntityId::MAX_ASSIGNABLE_RAW.to_string();
    let path = "core.transform:position";
    let last_id_preview = result(
        &mut agent,
        "sim.run",
        json!({"frames": 0, "watch": [{"id": last_id, "path": path}]}),
    );
    assert_eq!(
        last_id_preview["watch"],
        json!([{"id": last_id, "path": path, "values": [null]}])
    );

    let bounded = result(
        &mut agent,
        "sim.run",
        json!({"frames": 300, "dt": 1.0 / 240.0}),
    );
    assert_eq!(bounded["frames_run"], 300);
    assert_eq!(bounded["checkpoints"].as_array().unwrap().len(), 301);
    assert_eq!(bounded["checkpoints"][300]["frame"], 300);
}
