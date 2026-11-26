//! 查询分页使用持久 ID，时间对照经过与客户端相同的 JSON-RPC 边界。

use std::hint::black_box;
use std::time::Instant;

use serde_json::{json, Value as Json};
use xgc_agent::Agent;
use xgc_base::EntityId;
use xgc_reflect::Component;
use xgc_world::{
    ComponentData, ComponentId, EntityData, Material, MeshRenderer, Name, Transform, Value,
    Velocity, WorldData,
};

fn request(method: &str, params: Json) -> String {
    json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string()
}

fn call(agent: &mut Agent, method: &str, params: Json) -> Json {
    serde_json::from_str(&agent.handle_line(&request(method, params)).unwrap()).unwrap()
}

fn result(agent: &mut Agent, method: &str, params: Json) -> Json {
    let response = call(agent, method, params);
    assert!(response.get("error").is_none(), "{response}");
    response["result"].clone()
}

fn component(component: ComponentId, fields: &[(&str, Value)]) -> ComponentData {
    ComponentData {
        component,
        fields: fields
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect(),
    }
}

fn entity(id: u64, x: f64) -> EntityData {
    EntityData {
        id: EntityId::from_raw(id),
        revision: 5,
        components: vec![
            component(Transform::ID, &[("position", Value::Vec3([x, 0.0, 0.0]))]),
            component(
                Name::ID,
                &[("value", Value::String(format!("Entity {id}")))],
            ),
            component(Velocity::ID, &[]),
        ],
    }
}

fn restore(agent: &mut Agent, entities: Vec<EntityData>) -> Json {
    let data = WorldData {
        revision: 7,
        next_id: entities.iter().map(|entity| entity.id.raw()).max().unwrap() + 1,
        entities,
        ..Default::default()
    };
    result(agent, "world.restore", json!({"data": data}));
    serde_json::to_value(data).unwrap()
}

fn first_cursor(agent: &mut Agent) -> Json {
    let first = result(agent, "world.query", json!({"limit": 1}));
    assert!(first["next_cursor"].is_string(), "{first}");
    first["next_cursor"].clone()
}

fn assert_stale(agent: &mut Agent, cursor: Json) {
    let response = call(agent, "world.query", json!({"cursor": cursor}));
    assert_eq!(response["error"]["code"], -32000);
    let message = response["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("cursor") && message.contains("失效"),
        "{response}"
    );
}

#[test]
fn cursor_keeps_filtered_offsets_and_exact_id_order() {
    let mut agent = Agent::new();
    let mut unnamed = entity(11, 2.0);
    unnamed
        .components
        .retain(|component| component.component != Name::ID);
    restore(
        &mut agent,
        vec![
            entity(9007199254740994, 3.0),
            unnamed,
            entity(9007199254740993, 2.0),
            entity(10, -1.0),
            entity(2, 1.0),
        ],
    );
    let mut params = json!({
        "component": "core.name",
        "select": ["core.name:value", "core.parent:entity"],
        "filter": {"path": "core.transform:position", "axis": "x", "op": "gt", "value": 0},
        "limit": 2,
    });
    let first = result(&mut agent, "world.query", params.clone());
    assert_eq!(first["revision"], 7);
    assert_eq!(first["total"], 3);
    assert_eq!(first["offset"], 0);
    assert_eq!(first["next_offset"], 2);
    assert_eq!(first["rows"][0]["id"], "2");
    assert_eq!(first["rows"][1]["id"], "9007199254740993");
    assert_eq!(
        first["rows"][1]["values"],
        json!({
            "core.name:value": {"kind": "string", "value": "Entity 9007199254740993"},
            "core.parent:entity": null,
        })
    );
    assert!(first["next_cursor"].is_string(), "{first}");
    assert_eq!(result(&mut agent, "world.query", params.clone()), first);

    params["offset"] = json!(2);
    let by_offset = result(&mut agent, "world.query", params.clone());
    assert_eq!(by_offset["rows"][0]["id"], "9007199254740994");
    assert_eq!(by_offset["next_offset"], Json::Null);
    assert_eq!(by_offset.get("next_cursor"), Some(&Json::Null));
    params["offset"] = json!(0);
    params["cursor"] = first["next_cursor"].clone();
    assert_eq!(result(&mut agent, "world.query", params), by_offset);
}

#[test]
fn cursor_rejects_mutations_replacements_and_foreign_sessions() {
    let mut agent = Agent::new();
    let data = restore(&mut agent, vec![entity(2, 1.0), entity(4, 2.0)]);
    let before_edit = first_cursor(&mut agent);
    let plan = result(
        &mut agent,
        "edit.plan",
        json!({"commands": [{"Spawn": {"data": entity(6, 3.0)}}]}),
    );
    assert_eq!(first_cursor(&mut agent), before_edit);
    result(
        &mut agent,
        "edit.apply",
        json!({"plan_id": plan["plan_id"]}),
    );
    assert_stale(&mut agent, before_edit);

    let before_replay = first_cursor(&mut agent);
    let revision = agent.world().revision();
    assert_eq!(
        result(&mut agent, "engine.replay", Json::Null)["revision"],
        revision
    );
    assert_stale(&mut agent, before_replay);

    result(&mut agent, "world.restore", json!({"data": data}));
    let before_restore = first_cursor(&mut agent);
    let mut replacement = data.clone();
    replacement["entities"][1] = serde_json::to_value(entity(5, 2.0)).unwrap();
    replacement["next_id"] = json!(6);
    result(&mut agent, "world.restore", json!({"data": replacement}));
    assert_eq!(agent.world().revision(), 7);
    assert_stale(&mut agent, before_restore);

    let path = std::env::temp_dir().join(format!("xgc-query-cursor-{}.json", std::process::id()));
    xgc_store::save_world(&path, &serde_json::from_value(data.clone()).unwrap()).unwrap();
    let before_load = first_cursor(&mut agent);
    let loaded = call(&mut agent, "world.load", json!({"path": path}));
    std::fs::remove_file(&path).unwrap();
    assert_eq!(loaded["result"]["revision"], 7, "{loaded}");
    assert_stale(&mut agent, before_load);

    let mut other = Agent::new();
    result(&mut other, "world.restore", json!({"data": data}));
    assert_stale(&mut other, first_cursor(&mut agent));
    for invalid in [json!(100), json!("garbled")] {
        let response = call(&mut agent, "world.query", json!({"cursor": invalid}));
        assert_eq!(response["error"]["code"], -32000);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cursor"));
    }
}

#[test]
fn zero_limit_retains_offset_without_advancing_cursor() {
    let mut agent = Agent::new();
    restore(&mut agent, vec![entity(10, 0.0), entity(2, 0.0)]);
    let first = result(
        &mut agent,
        "world.query",
        json!({"cursor": null, "limit": 0, "offset": 1}),
    );
    assert_eq!(first["entities"], json!([]));
    assert_eq!(first["offset"], 1);
    assert_eq!(first["next_offset"], 1);
    assert_eq!(first.get("next_cursor"), Some(&Json::Null));

    let next = first_cursor(&mut agent);
    let page = result(&mut agent, "world.query", json!({"cursor": next}));
    assert_eq!(page["entities"][0]["id"], "10");
    assert_eq!(
        page["entities"][0],
        result(&mut agent, "world.get", json!({"id": "10"}))["entity"]
    );
    assert_eq!(page["offset"], 1);
    assert_eq!(page["next_offset"], Json::Null);
}

#[test]
#[ignore = "用 release 同机对照查询延迟和响应大小"]
fn projected_query_budget() {
    let mut agent = Agent::new();
    let request = request(
        "world.query",
        json!({
            "component": "core.transform",
            "filter": {"path": "core.transform:position", "axis": "x", "op": "gt", "value": 0},
            "select": ["core.transform:position", "core.transform:scale", "core.name:value", "core.velocity:linear"],
            "limit": 200,
        }),
    );
    for count in [1_000_u64, 12_000] {
        let entities = (1..=count)
            .rev()
            .map(|id| {
                let mut entity = entity(id, if id % 2 == 0 { 1.0 } else { -1.0 });
                entity.components.extend([
                    component(
                        MeshRenderer::ID,
                        &[("model", Value::String("builtin://cube".into()))],
                    ),
                    component(Material::ID, &[]),
                ]);
                entity
            })
            .collect();
        restore(&mut agent, entities);
        let warmup = agent.handle_line(&request).unwrap();
        let decoded: Json = serde_json::from_str(&warmup).unwrap();
        assert_eq!(decoded["result"]["total"], count / 2);
        assert_eq!(decoded["result"]["rows"].as_array().unwrap().len(), 200);
        for _ in 0..4 {
            black_box(agent.handle_line(&request).unwrap());
        }
        let mut times = [0; 40];
        for time in &mut times {
            let start = Instant::now();
            let response = black_box(agent.handle_line(&request).unwrap());
            *time = start.elapsed().as_micros();
            assert_eq!(response.len(), warmup.len());
        }
        times.sort_unstable();
        println!(
            "entities={count} rows=200 bytes={} median_us={} p95_us={}",
            warmup.len(),
            times[20],
            times[37]
        );
    }
}
