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

#[test]
fn describe_lists_protocol_components_and_world_state() {
    let mut agent = Agent::new();
    let response = result(call(&mut agent, "engine.describe", Json::Null));
    assert_eq!(response["protocol"], "xgc.agent/1");
    assert_eq!(response["engine"], "xgc");
    assert_eq!(response["components"]["types"].as_array().unwrap().len(), 3);
    assert_eq!(response["world"]["entities"], 0);
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

    let queried = result(call(
        &mut agent,
        "world.query",
        json!({"component": "core.transform"}),
    ));
    assert_eq!(queried["total"], 1);
    assert_eq!(queried["entities"][0]["id"], 1);
    assert_eq!(queried["entities"][0]["components"][0]["component"], 257);

    let set = Command::SetField {
        entity: EntityId::from_raw(1),
        component: ComponentId(0x0101),
        field: "position".to_string(),
        value: Value::Vec3([9.0, 9.0, 9.0]),
    };
    let commands = serde_json::to_value(vec![set]).unwrap();
    let plan = result(call(&mut agent, "edit.plan", json!({"commands": commands})));
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
