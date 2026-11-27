//! 通过 JSON-RPC 验证世界恢复的提交边界。

use serde_json::{json, Value as Json};
use xgc_agent::Agent;
use xgc_base::EntityId;
use xgc_reflect::Component;
use xgc_world::{Command, ComponentData, EntityData, Name, SceneEnvironment, Value, WorldData};

const LARGE_ID: u64 = 9_007_199_254_740_993;
const EDITED_ID: u64 = 7;

fn call(agent: &mut Agent, method: &str, params: Json) -> Json {
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    serde_json::from_str(&agent.handle_line(&request.to_string()).unwrap()).unwrap()
}

fn result(agent: &mut Agent, method: &str, params: Json) -> Json {
    let response = call(agent, method, params);
    assert!(response.get("error").is_none(), "{method}: {response}");
    response["result"].clone()
}

fn error_message(response: &Json) -> &str {
    assert_eq!(response["error"]["code"], -32000, "{response}");
    response["error"]["message"].as_str().unwrap()
}

fn entity(id: u64, name: &str) -> EntityData {
    EntityData {
        id: EntityId::from_raw(id),
        revision: 3,
        components: vec![ComponentData::from_component(&Name { value: name.into() })],
    }
}

fn rename_plan(agent: &mut Agent, name: &str) -> Json {
    let plan = result(
        agent,
        "edit.plan",
        json!({"commands": [Command::SetField {
            entity: EntityId::from_raw(EDITED_ID),
            component: Name::ID,
            field: "value".to_owned(),
            value: Value::String(name.to_owned()),
        }]}),
    );
    assert_eq!(plan["valid"], true, "{plan}");
    plan["plan_id"].clone()
}

fn edited_session() -> (Agent, Json) {
    let mut agent = Agent::new();
    let data = WorldData {
        revision: 4,
        next_id: LARGE_ID + 100,
        entities: vec![entity(EDITED_ID, "原名"), entity(LARGE_ID, "后页")],
        environment: SceneEnvironment {
            lights: Vec::new(),
            ambient: [0.125, 0.25, 0.5],
        },
    };
    result(&mut agent, "world.restore", json!({"data": data}));
    let plan = rename_plan(&mut agent, "已修改");
    let edit = result(&mut agent, "edit.apply", json!({"plan_id": plan}));
    assert_eq!(edit["revision"], 5);
    (agent, edit["tx_id"].clone())
}

fn first_cursor(agent: &mut Agent) -> Json {
    let page = result(agent, "world.query", json!({"limit": 1}));
    assert_eq!(page["entities"][0]["id"], EDITED_ID.to_string());
    assert!(page["next_cursor"].is_string(), "{page}");
    page["next_cursor"].clone()
}

#[test]
fn invalid_restore_preserves_the_edit_session() {
    for (next_edit, replacement_revision) in [("edit.undo", 5), ("edit.apply", 99)] {
        let (mut agent, tx_id) = edited_session();
        let plan_id = rename_plan(&mut agent, "待结算");
        let before = result(&mut agent, "world.snapshot", Json::Null);
        let history = result(&mut agent, "engine.history", Json::Null);
        let cursor = first_cursor(&mut agent);

        // 第一行可入库，第二行可反序列化但违背组件字段类型。
        let mut invalid = entity(2, "无效字段");
        invalid.components[0].fields[0].1 = Value::Bool(false);
        let data = WorldData {
            revision: replacement_revision,
            next_id: 3,
            entities: vec![entity(1, "不应提交"), invalid],
            ..Default::default()
        };
        let rejected = call(&mut agent, "world.restore", json!({"data": data}));
        assert!(error_message(&rejected).starts_with("恢复快照失败：core.name"));
        assert_eq!(result(&mut agent, "world.snapshot", Json::Null), before);
        assert_eq!(result(&mut agent, "engine.history", Json::Null), history);
        let page = result(&mut agent, "world.query", json!({"cursor": cursor}));
        assert_eq!(page["entities"][0]["id"], LARGE_ID.to_string());
        assert_eq!(page["revision"], 5);

        let edited = result(&mut agent, next_edit, json!({"plan_id": plan_id}));
        let expected_name = if next_edit == "edit.undo" {
            assert_eq!(edited["tx_id"], tx_id);
            "原名"
        } else {
            "待结算"
        };
        assert_eq!(edited["revision"], 6);
        let row = result(
            &mut agent,
            "world.get",
            json!({"id": EDITED_ID.to_string()}),
        );
        assert_eq!(
            row["entity"]["components"][0]["fields"][0][1]["value"],
            expected_name
        );
    }
}

#[test]
fn valid_restore_replaces_the_world_and_rebases_the_edit_session() {
    let (mut agent, tx_id) = edited_session();
    let plan_id = rename_plan(&mut agent, "待结算");
    let cursor = first_cursor(&mut agent);

    // revision 相同且旧计划的目标仍在，只有提交时换代和清理才能拒绝旧操作。
    let data = WorldData {
        revision: 5,
        next_id: LARGE_ID + 200,
        entities: vec![entity(EDITED_ID, "新世界"), entity(LARGE_ID + 1, "新后页")],
        environment: SceneEnvironment {
            lights: Vec::new(),
            ambient: [0.5, 0.25, 0.125],
        },
    };
    let restored = result(&mut agent, "world.restore", json!({"data": data}));
    assert_eq!(restored, json!({"revision": 5, "entities": 2}));
    let previous_page = call(&mut agent, "world.query", json!({"cursor": cursor}));
    assert!(error_message(&previous_page).contains("cursor 已失效"));
    let previous_plan = call(&mut agent, "edit.apply", json!({"plan_id": plan_id}));
    assert!(error_message(&previous_plan).contains("未知 plan_id"));
    let previous_tx = call(&mut agent, "edit.revert", json!({"tx_id": tx_id}));
    assert!(error_message(&previous_tx).contains("未知 tx_id"));
    assert_eq!(
        result(&mut agent, "engine.history", Json::Null),
        json!({"records": [], "count": 0, "next_tx": 0})
    );
    let fresh_cursor = first_cursor(&mut agent);
    let page = result(&mut agent, "world.query", json!({"cursor": fresh_cursor}));
    assert_eq!(page["entities"][0]["id"], (LARGE_ID + 1).to_string());
    let replay = result(&mut agent, "engine.replay", Json::Null);
    assert_eq!(replay["records"], 0);
    assert_eq!(
        result(&mut agent, "world.snapshot", Json::Null),
        json!({"revision": 5, "data": data})
    );
}
