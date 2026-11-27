//! 查询分页使用持久 ID，时间对照经过与客户端相同的 JSON-RPC 边界。

use std::hint::black_box;
use std::time::Instant;

use serde_json::{json, Value as Json};
use xgc_agent::{Agent, Mcp};
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
fn columns_keep_projection_order_missing_values_and_exact_ids_over_mcp() {
    let mut agent = Agent::new();
    let parent = 9007199254740993;
    let mut child = entity(2, 1.0);
    child.components.push(component(
        xgc_world::Parent::ID,
        &[("entity", Value::Entity(parent))],
    ));
    let data = restore(&mut agent, vec![entity(parent, 2.0), child]);
    let mut mcp = Mcp::new();
    let reply = mcp
        .handle_line(&mcp_request("world_restore", json!({"data": data})))
        .unwrap();
    mcp_text(&serde_json::from_str(&reply).unwrap());

    let mut params = json!({
        "format": "columns",
        "select": ["core.parent:entity", "core.transform:position", "core.name:value"],
        "limit": 1,
    });
    let first = result(&mut agent, "world.query", params.clone());
    assert_eq!(
        first["columns"],
        json!([
            {"name": "id", "type": "entity"},
            {"name": "core.parent:entity", "type": "entity"},
            {"name": "core.transform:position", "type": "vec3"},
            {"name": "core.name:value", "type": "string"},
        ])
    );
    assert_eq!(
        first["rows"],
        json!([["2", parent.to_string(), [1.0, 0.0, 0.0], "Entity 2"]])
    );
    assert_eq!(first["next_offset"], 1);

    let reply = mcp
        .handle_line(&mcp_request("world_query", params.clone()))
        .unwrap();
    let response: Json = serde_json::from_str(&reply).unwrap();
    let text = mcp_text(&response);
    let decoded: Json = serde_json::from_str(text).unwrap();
    assert_eq!(text, decoded.to_string());
    assert_eq!(decoded["columns"], first["columns"]);
    assert_eq!(decoded["rows"], first["rows"]);

    params["cursor"] = first["next_cursor"].clone();
    let next = result(&mut agent, "world.query", params);
    assert_eq!(next["columns"], first["columns"]);
    assert_eq!(
        next["rows"],
        json!([[
            parent.to_string(),
            null,
            [2.0, 0.0, 0.0],
            format!("Entity {parent}")
        ]])
    );
    assert!(next["next_cursor"].is_null());
}

#[test]
fn query_omits_defaults_while_preserving_missing_and_signed_zero() {
    let mut agent = Agent::new();
    let mut missing = entity(3, 0.0);
    missing
        .components
        .retain(|component| component.component != Velocity::ID);
    let mut scaled = entity(4, 2.0);
    scaled.components[0]
        .fields
        .push(("scale".into(), Value::Vec3([2.0, 1.0, 1.0])));
    let data = restore(&mut agent, vec![scaled, missing, entity(2, -0.0)]);
    let initial = agent.world().state_hash();
    let mut params = json!({
        "format": "columns",
        "select": [
            "core.transform:position", "core.velocity:linear", "core.name:value", "core.transform:scale",
        ],
        "include_defaults": false,
        "limit": 2,
    });
    let first = result(&mut agent, "world.query", params.clone());
    assert_eq!(first["total"], 3);
    assert_eq!(first["columns"][1]["default_rows"], json!([[1, 2]]));
    assert_eq!(first["columns"][2]["default_rows"], json!([[0, 1]]));
    assert!(first["columns"][3].get("default_rows").is_none());
    assert_eq!(first["columns"][4]["default_rows"], json!([[0, 2]]));
    assert_eq!(
        first["rows"],
        json!([
            ["2", [-0.0, 0, 0], null, "Entity 2"],
            ["3", null, null, "Entity 3"]
        ])
    );
    assert!(first["rows"][0][1][0].as_f64().unwrap().is_sign_negative());
    assert!(first["next_cursor"].is_string());

    let mut mcp = Mcp::new();
    let restore = mcp
        .handle_line(&mcp_request("world_restore", json!({"data": data})))
        .unwrap();
    mcp_text(&serde_json::from_str(&restore).unwrap());
    let reply = mcp
        .handle_line(&mcp_request("world_query", params.clone()))
        .unwrap();
    let response: Json = serde_json::from_str(&reply).unwrap();
    let text = mcp_text(&response);
    let decoded: Json = serde_json::from_str(text).unwrap();
    assert_eq!(text, decoded.to_string());
    assert_eq!(decoded["columns"], first["columns"]);
    assert_eq!(decoded["rows"], first["rows"]);

    params["cursor"] = first["next_cursor"].clone();
    let next = result(&mut agent, "world.query", params.clone());
    assert_eq!(next["columns"][2]["default_rows"], json!([[0, 1]]));
    assert!(next["columns"][4].get("default_rows").is_none());
    assert_eq!(
        next["rows"],
        json!([["4", [2, 0, 0], null, "Entity 4", [2, 1, 1]]])
    );
    assert!(next["next_cursor"].is_null());

    params["format"] = json!("rows");
    params["cursor"] = Json::Null;
    let rows = result(&mut agent, "world.query", params);
    let present = &rows["rows"][0]["values"];
    let missing = &rows["rows"][1]["values"];
    assert!(present.get("core.velocity:linear").is_none());
    assert_eq!(missing.get("core.velocity:linear"), Some(&Json::Null));
    assert!(missing.get("core.transform:position").is_none());
    assert!(present["core.transform:position"]["value"][0]
        .as_f64()
        .unwrap()
        .is_sign_negative());

    let entities = result(
        &mut agent,
        "world.query",
        json!({"include_defaults": false}),
    );
    let entities: Vec<EntityData> = serde_json::from_value(entities["entities"].clone()).unwrap();
    assert_eq!(entities[0].components.len(), 3);
    let velocity = entities[0]
        .components
        .iter()
        .find(|row| row.component == Velocity::ID)
        .unwrap();
    assert!(velocity.fields.is_empty());
    assert!(!entities[1]
        .components
        .iter()
        .any(|row| row.component == Velocity::ID));
    assert_eq!(agent.world().state_hash(), initial);
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

const WARMUP: usize = 4;
const SAMPLES: usize = 40;

fn mcp_request(tool: &str, arguments: Json) -> String {
    request("tools/call", json!({"name": tool, "arguments": arguments}))
}

fn mcp_text(response: &Json) -> &str {
    assert_eq!(response["result"]["isError"], false, "{response}");
    response["result"]["content"][0]["text"].as_str().unwrap()
}

fn latencies(mut times: [u128; SAMPLES]) -> String {
    times.sort_unstable();
    format!(
        "p50_us={} p95_us={} p99_us={}",
        times[SAMPLES / 2],
        times[SAMPLES * 95 / 100 - 1],
        times[SAMPLES * 99 / 100]
    )
}

fn time_query(mut handle: impl FnMut() -> String) -> (String, [u128; SAMPLES]) {
    let first = handle();
    for _ in 0..WARMUP {
        black_box(handle());
    }
    let mut times = [0; SAMPLES];
    for time in &mut times {
        let start = Instant::now();
        let response = black_box(handle());
        *time = start.elapsed().as_micros();
        assert_eq!(response.len(), first.len());
    }
    (first, times)
}

#[test]
#[ignore = "用 release 同机对照查询延迟和响应大小"]
fn projected_query_budget() {
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
        let mut agent = Agent::new();
        let mut mcp = Mcp::new();
        let data = restore(&mut agent, entities);
        let restored = mcp
            .handle_line(&mcp_request("world_restore", json!({"data": data})))
            .unwrap();
        mcp_text(&serde_json::from_str(&restored).unwrap());
        for limit in [200, 1_000].into_iter().filter(|limit| *limit <= count / 2) {
            let params = json!({
                "component": "core.transform",
                "filter": {"path": "core.transform:position", "axis": "x", "op": "gt", "value": 0},
                "select": ["core.transform:position", "core.transform:scale", "core.name:value", "core.velocity:linear"],
                "limit": limit,
            });
            for (format, include_defaults) in
                [("rows", true), ("columns", true), ("columns", false)]
            {
                let mut params = params.clone();
                params["format"] = json!(format);
                if !include_defaults {
                    params["include_defaults"] = json!(false);
                }
                let rpc = request("world.query", params.clone());
                let mcp_call = mcp_request("world_query", params);
                let (rpc_response, rpc_times) = time_query(|| agent.handle_line(&rpc).unwrap());
                let (mcp_response, mcp_times) = time_query(|| mcp.handle_line(&mcp_call).unwrap());
                let rpc_json: Json = serde_json::from_str(&rpc_response).unwrap();
                let mcp_json: Json = serde_json::from_str(&mcp_response).unwrap();
                let text = mcp_text(&mcp_json);
                let decoded: Json = serde_json::from_str(text).unwrap();
                assert_eq!(decoded["total"], count / 2);
                assert_eq!(decoded["rows"].as_array().unwrap().len() as u64, limit);
                if format == "columns" {
                    assert_eq!(decoded["columns"].as_array().unwrap().len(), 5);
                    assert_eq!(decoded["rows"][0][0], "2");
                    assert_eq!(
                        decoded["rows"][(limit - 1) as usize][0],
                        (limit * 2).to_string()
                    );
                }
                if !include_defaults {
                    for column in [2, 4] {
                        assert_eq!(
                            decoded["columns"][column]["default_rows"],
                            json!([[0, limit]])
                        );
                    }
                    assert_eq!(
                        decoded["rows"][0],
                        json!(["2", [1, 0, 0], null, "Entity 2"])
                    );
                }
                // 游标绑定各自会话，业务荷载必须一致。
                assert_eq!(decoded["rows"], rpc_json["result"]["rows"]);
                println!(
                    "entities={count} rows={limit} format={format} include_defaults={include_defaults} rpc_bytes={} {} mcp_bytes={} mcp_text_bytes={} {}",
                    rpc_response.len(),
                    latencies(rpc_times),
                    mcp_response.len(),
                    text.len(),
                    latencies(mcp_times),
                );
            }
        }
    }
}

#[test]
#[ignore = "用 release 测量一百次字段编辑的计划和提交往返"]
fn edit_transaction_budget() {
    let mut agent = Agent::new();
    restore(&mut agent, (1..=12_000).map(|id| entity(id, 0.0)).collect());
    let plans = ["第一组", "第二组"].map(|name| {
        let commands: Vec<_> = (1..=100)
            .map(|id| xgc_world::Command::SetField {
                entity: EntityId::from_raw(id),
                component: Name::ID,
                field: "value".into(),
                value: Value::String(name.into()),
            })
            .collect();
        json!({"commands": commands})
    });
    let mut times = [0; SAMPLES];
    for trial in 0..WARMUP + SAMPLES {
        let revision = agent.world().revision();
        let params = plans[trial % 2].clone();
        let start = Instant::now();
        let plan = result(&mut agent, "edit.plan", params);
        let applied = result(
            &mut agent,
            "edit.apply",
            json!({"plan_id": plan["plan_id"], "base_revision": revision}),
        );
        let micros = start.elapsed().as_micros();
        assert_eq!(plan["valid"], true, "{plan}");
        assert_eq!(plan["commands"], 100);
        assert_eq!(applied["revision"], revision + 1);
        assert_eq!(applied["inverse"].as_array().unwrap().len(), 100);
        if trial >= WARMUP {
            times[trial - WARMUP] = micros;
        }
    }
    let query = result(
        &mut agent,
        "world.query",
        json!({"select": ["core.name:value"], "limit": 100}),
    );
    for (id, row) in (1..=100).zip(query["rows"].as_array().unwrap()) {
        assert_eq!(row["id"], id.to_string());
        assert_eq!(row["values"]["core.name:value"]["value"], "第二组");
    }
    let expected_hash = agent.world().state_hash();
    let replay = result(&mut agent, "engine.replay", Json::Null);
    assert_eq!(replay["records"], WARMUP + SAMPLES);
    assert_eq!(replay["hash"], expected_hash);
    println!("entities=12000 edit_ops=100 {}", latencies(times));
}
