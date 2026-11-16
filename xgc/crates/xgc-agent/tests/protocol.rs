//! Agent 协议回归：describe / plan / apply / revert / query / save / load。

use serde_json::{json, Value as Json};
use xgc_agent::Agent;
use xgc_base::EntityId;
use xgc_world::{Command, ComponentData, ComponentId, EntityData, Value};

fn call(agent: &mut Agent, method: &str, params: Json) -> Json {
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let response = agent
        .handle_line(&request.to_string())
        .expect("请求应当有响应");
    serde_json::from_str(&response).unwrap()
}

fn result(response: Json) -> Json {
    assert!(response.get("error").is_none(), "响应错误：{response}");
    response["result"].clone()
}

fn spawn_command() -> Command {
    let data = EntityData {
        id: EntityId::from_raw(1),
        revision: 0,
        components: vec![
            ComponentData {
                component: ComponentId(0x0101),
                fields: vec![("position".to_string(), Value::Vec3([1.0, 2.0, 3.0]))],
            },
            ComponentData {
                component: ComponentId(0x0102),
                fields: vec![("linear".to_string(), Value::Vec3([0.0, 0.0, -1.0]))],
            },
        ],
    };
    Command::Spawn { data }
}

fn spawn_at(id: u64, x: f32, name: &str) -> Command {
    let data = EntityData {
        id: EntityId::from_raw(id),
        revision: 0,
        components: vec![
            ComponentData {
                component: ComponentId(0x0101),
                fields: vec![(
                    "position".to_string(),
                    Value::Vec3([f64::from(x), 0.0, 0.0]),
                )],
            },
            ComponentData {
                component: ComponentId(0x0103),
                fields: vec![("value".to_string(), Value::String(name.to_string()))],
            },
        ],
    };
    Command::Spawn { data }
}

#[test]
fn describe_lists_protocol_components_and_world_state() {
    let mut agent = Agent::new();
    let response = result(call(&mut agent, "engine.describe", Json::Null));
    assert_eq!(response["protocol"], "xgc.agent/1");
    assert_eq!(response["engine"], "xgc");
    assert_eq!(
        response["components"]["types"].as_array().unwrap().len(),
        30
    );
    assert_eq!(response["world"]["entities"], 0);
    assert_eq!(
        response["capabilities"]["sim"],
        Json::from(vec![
            "physics",
            "character_jump",
            "velocity",
            "ground",
            "character",
            "character_step",
            "character_block",
            "aabb",
            "animation_state",
            "animation",
            "animation_apply",
            "animation_layers",
            "skeleton_pose",
            "ui_layout",
            "ui_interaction",
            "ui_focus",
            "audio_mixer",
            "hierarchy"
        ])
    );
    assert!(response["capabilities"]["render"]
        .as_array()
        .unwrap()
        .contains(&Json::from("instancing")));
    let diagnostics = result(call(&mut agent, "engine.diagnostics", Json::Null));
    assert!(diagnostics["codes"].as_array().unwrap().len() >= 20);
    assert!(diagnostics["codes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|code| code["code"] == "EDIT.UNKNOWN_ENTITY"));
    assert!(response["methods"]
        .as_array()
        .unwrap()
        .contains(&Json::from("world.get")));
    assert!(response["methods"]
        .as_array()
        .unwrap()
        .contains(&Json::from("world.validate")));
}

#[test]
fn plan_apply_query_and_revert_roundtrip() {
    let mut agent = Agent::new();
    let commands = serde_json::to_value(vec![spawn_command()]).unwrap();
    let plan = result(call(&mut agent, "edit.plan", json!({"commands": commands})));
    assert_eq!(plan["valid"], true);
    let plan_id = plan["plan_id"].as_u64().unwrap();

    let applied = result(call(&mut agent, "edit.apply", json!({"plan_id": plan_id})));
    assert_eq!(applied["revision"], 1);
    let history = result(call(&mut agent, "engine.history", Json::Null));
    assert_eq!(history["count"], 1);
    assert_eq!(history["records"][0]["tx_id"], 0);
    assert_eq!(history["records"][0]["revision"], 1);

    let queried = result(call(
        &mut agent,
        "world.query",
        json!({"component": "core.transform"}),
    ));
    assert_eq!(queried["total"], 1);
    assert_eq!(queried["entities"][0]["id"], "1");
    assert_eq!(queried["entities"][0]["components"][0]["component"], 257);

    let got = result(call(&mut agent, "world.get", json!({"id": 1})));
    assert_eq!(got["entity"]["id"], "1");
    let missing = result(call(&mut agent, "world.get", json!({"id": 999})));
    assert!(missing["entity"].is_null());
    let digest = result(call(&mut agent, "world.digest", Json::Null));
    assert_eq!(digest["entities"], 1);
    assert_eq!(digest["revision"], 1);
    assert_eq!(digest["state_hash"].as_str().unwrap().len(), 16);
    let stats = result(call(&mut agent, "world.stats", Json::Null));
    assert_eq!(stats["components"].as_array().unwrap().len(), 2);
    let validated = result(call(&mut agent, "world.validate", Json::Null));
    assert_eq!(validated["valid"], true);
    assert!(validated["diagnostics"].as_array().unwrap().is_empty());

    let set = Command::SetField {
        entity: EntityId::from_raw(1),
        component: ComponentId(0x0101),
        field: "position".to_string(),
        value: Value::Vec3([9.0, 9.0, 9.0]),
    };
    let commands = serde_json::to_value(vec![set]).unwrap();
    let plan = result(call(&mut agent, "edit.plan", json!({"commands": commands})));
    assert_eq!(plan["preview"][0]["kind"], "set_field");
    assert_eq!(
        plan["preview"][0]["before"],
        json!({"kind": "vec3", "value": [1.0, 2.0, 3.0]})
    );
    let plan_id = plan["plan_id"].as_u64().unwrap();
    let applied = result(call(&mut agent, "edit.apply", json!({"plan_id": plan_id})));
    let tx_id1 = applied["tx_id"].as_u64().unwrap();

    let queried = result(call(
        &mut agent,
        "world.query",
        json!({"component": "core.transform"}),
    ));
    assert_eq!(
        queried["entities"][0]["components"][0]["fields"][0][1]["value"][0],
        Json::from(9.0)
    );

    let reverted = result(call(&mut agent, "edit.revert", json!({"tx_id": tx_id1})));
    assert_eq!(reverted["revision"], 3);
    let queried = result(call(
        &mut agent,
        "world.query",
        json!({"component": "core.transform"}),
    ));
    assert_eq!(
        queried["entities"][0]["components"][0]["fields"][0][1]["value"][0],
        Json::from(1.0)
    );
}

#[test]
fn query_projection_filter_and_stable_pagination() {
    let mut agent = Agent::new();
    let commands = serde_json::to_value(vec![
        spawn_at(1, 1.0, "one"),
        spawn_at(2, 10.0, "two"),
        spawn_at(3, 4.0, "three"),
    ])
    .unwrap();
    let plan = result(call(&mut agent, "edit.plan", json!({"commands": commands})));
    result(call(
        &mut agent,
        "edit.apply",
        json!({"plan_id": plan["plan_id"]}),
    ));

    let filtered = result(call(
        &mut agent,
        "world.query",
        json!({
            "select": ["core.transform:position", "core.name:value"],
            "filter": {
                "path": "core.transform:position",
                "axis": "x",
                "op": "gt",
                "value": 5.0
            }
        }),
    ));
    assert_eq!(filtered["total"], 1);
    assert_eq!(filtered["rows"][0]["id"], "2");
    assert_eq!(
        filtered["rows"][0]["values"]["core.transform:position"],
        json!({"kind": "vec3", "value": [10.0, 0.0, 0.0]})
    );
    assert_eq!(
        filtered["rows"][0]["values"]["core.name:value"],
        json!({"kind": "string", "value": "two"})
    );

    let by_name = result(call(
        &mut agent,
        "world.query",
        json!({
            "select": ["core.name:value"],
            "filter": {"path": "core.name:value", "op": "eq", "value": "three"}
        }),
    ));
    assert_eq!(by_name["rows"][0]["id"], "3");

    let page = result(call(
        &mut agent,
        "world.query",
        json!({"select": ["core.name:value"], "limit": 1, "offset": 1}),
    ));
    assert_eq!(page["total"], 3);
    assert_eq!(page["rows"].as_array().unwrap().len(), 1);
    assert_eq!(page["rows"][0]["id"], "2");
    assert_eq!(page["next_offset"], 2);

    let missing = result(call(&mut agent, "world.get", json!({"id": "2"})));
    assert_eq!(missing["entity"]["id"], "2");

    let error = call(
        &mut agent,
        "world.query",
        json!({"select": ["core.transform:nope"]}),
    );
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("没有字段"));
}

#[test]
fn save_load_and_unknown_method() {
    let mut agent = Agent::new();
    let commands = serde_json::to_value(vec![spawn_command()]).unwrap();
    let plan = result(call(&mut agent, "edit.plan", json!({"commands": commands})));
    result(call(
        &mut agent,
        "edit.apply",
        json!({"plan_id": plan["plan_id"]}),
    ));

    let path = std::env::temp_dir().join(format!(
        "xgc_agent_{}_{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let saved = result(call(
        &mut agent,
        "world.save",
        json!({"path": path.to_string_lossy()}),
    ));
    assert_eq!(saved["entities"], 1);

    let mut other = Agent::new();
    let loaded = result(call(
        &mut other,
        "world.load",
        json!({"path": path.to_string_lossy()}),
    ));
    assert_eq!(loaded["entities"], 1);
    let queried = result(call(
        &mut other,
        "world.query",
        json!({"component": "core.transform"}),
    ));
    assert_eq!(queried["total"], 1);
    std::fs::remove_file(path).ok();

    let error = call(&mut agent, "nope", Json::Null);
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("未知方法"));
}

#[test]
fn invalid_plan_and_missing_plan_are_reported() {
    let mut agent = Agent::new();
    let set = Command::SetField {
        entity: EntityId::from_raw(42),
        component: ComponentId(0x0101),
        field: "position".to_string(),
        value: Value::Vec3([0.0; 3]),
    };
    let commands = serde_json::to_value(vec![set]).unwrap();
    let plan = result(call(&mut agent, "edit.plan", json!({"commands": commands})));
    assert_eq!(plan["valid"], false);
    assert_eq!(plan["diagnostics"][0]["code"], "EDIT.UNKNOWN_ENTITY");

    let error = call(&mut agent, "edit.apply", json!({"plan_id": 999}));
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("未知 plan_id"));
}

#[test]
fn query_unknown_component_is_rejected() {
    let mut agent = Agent::new();
    let response = call(&mut agent, "world.query", json!({"component": "nope"}));
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("未知组件"));
}

#[test]
fn undo_reverts_last_applied_plan() {
    let mut agent = Agent::new();
    let commands = serde_json::to_value(vec![spawn_command()]).unwrap();
    let plan = result(call(&mut agent, "edit.plan", json!({"commands": commands})));
    let plan_id = plan["plan_id"].as_u64().unwrap();
    let applied = result(call(&mut agent, "edit.apply", json!({"plan_id": plan_id})));
    assert_eq!(applied["revision"], 1);
    let undone = result(call(&mut agent, "edit.undo", Json::Null));
    assert_eq!(undone["tx_id"], 0);
    assert_eq!(undone["revision"], 2);
    let queried = result(call(&mut agent, "world.query", Json::Null));
    assert_eq!(queried["total"], 0);
    let empty = call(&mut agent, "edit.undo", Json::Null);
    assert!(empty["error"]["message"]
        .as_str()
        .unwrap()
        .contains("没有可撤销"));
}

#[test]
fn replay_rebuilds_world_from_journal() {
    let mut agent = Agent::new();
    let commands = serde_json::to_value(vec![spawn_command()]).unwrap();
    let plan = result(call(&mut agent, "edit.plan", json!({"commands": commands})));
    let plan_id = plan["plan_id"].as_u64().unwrap();
    result(call(&mut agent, "edit.apply", json!({"plan_id": plan_id})));
    let replay = result(call(&mut agent, "engine.replay", Json::Null));
    assert_eq!(replay["revision"], 1);
    assert_eq!(replay["records"], 1);
    assert!(replay["hash"].as_u64().is_some());
    assert_eq!(replay["entities"], 1);
    let queried = result(call(&mut agent, "world.query", Json::Null));
    assert_eq!(queried["total"], 1);
}

#[test]
fn snapshot_and_restore_roundtrip() {
    let mut agent = Agent::new();
    let commands = serde_json::to_value(vec![spawn_command()]).unwrap();
    let plan = result(call(&mut agent, "edit.plan", json!({"commands": commands})));
    let plan_id = plan["plan_id"].as_u64().unwrap();
    result(call(&mut agent, "edit.apply", json!({"plan_id": plan_id})));
    let snapshot = result(call(&mut agent, "world.snapshot", Json::Null));
    assert_eq!(snapshot["revision"], 1);
    result(call(&mut agent, "edit.undo", Json::Null));
    assert_eq!(
        result(call(&mut agent, "world.query", Json::Null))["total"],
        0
    );
    let restored = result(call(
        &mut agent,
        "world.restore",
        json!({"data": snapshot["data"]}),
    ));
    assert_eq!(restored["entities"], 1);
    assert_eq!(
        result(call(&mut agent, "world.query", Json::Null))["total"],
        1
    );
}
