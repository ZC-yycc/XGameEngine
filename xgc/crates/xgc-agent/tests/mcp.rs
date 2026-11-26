//! MCP 适配器：initialize / tools/list / tools/call 的线协议。

use serde_json::{json, Value as Json};
use xgc_agent::Mcp;

fn call(mcp: &mut Mcp, method: &str, params: Json) -> Json {
    let request = json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params});
    let response = mcp
        .handle_line(&request.to_string())
        .expect("请求应当有响应");
    serde_json::from_str(&response).unwrap()
}

fn invoke(mcp: &mut Mcp, name: &str, arguments: Json) -> Json {
    let response = call(
        mcp,
        "tools/call",
        json!({"name": name, "arguments": arguments}),
    );
    let result = &response["result"];
    assert_eq!(result["isError"], false, "{response}");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn initialize_negotiates_a_protocol_and_declares_tools() {
    let mut mcp = Mcp::new();
    let response = call(
        &mut mcp,
        "initialize",
        json!({"protocolVersion": "2025-06-18"}),
    );
    let result = &response["result"];
    assert_eq!(result["protocolVersion"], "2025-06-18");
    assert_eq!(result["serverInfo"]["name"], "xgc");
    assert!(result["capabilities"]["tools"].is_object());
}

#[test]
fn tools_list_exposes_every_agent_method() {
    let mut mcp = Mcp::new();
    let response = call(&mut mcp, "tools/list", Json::Null);
    let tools = response["result"]["tools"].as_array().unwrap();
    let description = invoke(&mut mcp, "engine_describe", json!({}));
    let methods = [
        "engine.describe",
        "engine.diagnostics",
        "engine.history",
        "engine.replay",
        "world.query",
        "world.get",
        "world.digest",
        "world.stats",
        "world.validate",
        "world.load",
        "world.save",
        "world.snapshot",
        "world.restore",
        "edit.plan",
        "edit.apply",
        "edit.revert",
        "edit.undo",
        "script.check",
        "world.explain",
    ];
    assert_eq!(description["methods"], json!(methods));
    let details = description["method_details"].as_array().unwrap();
    assert_eq!(details.len(), methods.len());
    assert_eq!(tools.len(), methods.len());
    for ((tool, detail), method) in tools.iter().zip(details).zip(methods) {
        assert_eq!(detail["name"], method);
        assert_eq!(tool["name"], method.replace('.', "_"));
        assert_eq!(tool["description"], detail["description"]);
        assert_eq!(tool["inputSchema"], detail["input_schema"]);
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert!(!tool["description"].as_str().unwrap().is_empty());
        let response = call(&mut mcp, "tools/call", json!({"name": tool["name"]}));
        assert!(
            response["result"]["isError"].is_boolean(),
            "{method}: {response}"
        );
    }
}

#[test]
fn checks_inline_lua_through_the_same_tool() {
    let mut mcp = Mcp::new();
    let checked = invoke(
        &mut mcp,
        "script_check",
        json!({"source": "\nlocal = 5", "name": null}),
    );
    assert_eq!(checked["valid"], false);
    assert_eq!(checked["diagnostics"][0]["script"], "inline.lua");
    assert_eq!(checked["diagnostics"][0]["line"], 2);
    assert_eq!(
        checked["diagnostics"][0]["code"],
        xgc_base::diagnostics::SCRIPT_COMPILE
    );
}

#[test]
fn tools_advertise_their_actual_inputs() {
    let mut mcp = Mcp::new();
    let response = call(&mut mcp, "tools/list", Json::Null);
    let tools = response["result"]["tools"].as_array().unwrap();
    let schema =
        |name: &str| &tools.iter().find(|tool| tool["name"] == name).unwrap()["inputSchema"];

    let query = &schema("world_query")["properties"];
    assert_eq!(query["component"]["type"], json!(["string", "null"]));
    assert_eq!(query["offset"]["default"], 0);
    assert_eq!(query["cursor"]["type"], json!(["string", "null"]));
    assert_eq!(query["limit"]["default"], 100);
    assert_eq!(query["select"]["items"]["type"], "string");
    let filter = &query["filter"];
    assert_eq!(filter["required"], json!(["path", "op", "value"]));
    assert_eq!(
        filter["properties"]["op"]["enum"],
        json!(["eq", "ne", "lt", "le", "gt", "ge"])
    );
    assert_eq!(
        filter["properties"]["axis"]["enum"],
        json!(["x", "y", "z", "w", null])
    );

    for (name, field) in [
        ("world_get", "id"),
        ("world_load", "path"),
        ("world_save", "path"),
        ("world_restore", "data"),
        ("edit_plan", "commands"),
        ("edit_apply", "plan_id"),
        ("edit_revert", "tx_id"),
        ("script_check", "source"),
        ("world_explain", "id"),
    ] {
        assert_eq!(schema(name)["required"], json!([field]), "{name}");
        assert!(schema(name)["properties"][field].is_object(), "{name}");
    }
    for (name, field) in [
        ("world_get", "id"),
        ("edit_apply", "plan_id"),
        ("edit_revert", "tx_id"),
    ] {
        let types = &schema(name)["properties"][field]["oneOf"];
        assert_eq!(types[0]["type"], "integer");
        assert_eq!(types[1]["type"], "string");
    }

    let script = &schema("script_check")["properties"];
    assert_eq!(script["source"]["type"], "string");
    assert_eq!(script["name"]["type"], json!(["string", "null"]));
    assert_eq!(script["name"]["default"], "inline.lua");

    let explain = schema("world_explain");
    assert_eq!(
        explain["properties"]["camera"]["oneOf"][1]["type"],
        "string"
    );
    assert_eq!(explain["properties"]["eye"]["minItems"], 3);
    assert_eq!(explain["properties"]["eye"]["maxItems"], 3);
    assert_eq!(explain["properties"]["lod_bias"]["default"], 1.0);
    assert_eq!(explain["not"]["required"], json!(["camera", "eye"]));

    let variants = schema("edit_plan")["properties"]["commands"]["items"]["oneOf"]
        .as_array()
        .unwrap();
    let commands: Vec<_> = variants
        .iter()
        .map(|item| item["required"][0].as_str().unwrap())
        .collect();
    assert_eq!(
        commands,
        [
            "SetField",
            "AddComponent",
            "RemoveComponent",
            "Spawn",
            "Despawn",
            "Replace"
        ]
    );
    assert_eq!(
        variants[0]["properties"]["SetField"]["properties"]["entity"]["type"],
        "string"
    );
    assert_eq!(
        schema("world_restore")["properties"]["data"]["required"],
        json!(["revision", "next_id", "entities"])
    );
    assert_eq!(
        schema("world_restore")["$defs"]["EntityData"]["properties"]["id"]["type"],
        "string"
    );
    assert_eq!(
        schema("world_restore")["$defs"],
        schema("edit_plan")["$defs"]
    );
}

#[test]
fn tools_call_returns_text_content_from_the_agent() {
    let mut mcp = Mcp::new();
    let response = call(
        &mut mcp,
        "tools/call",
        json!({"name": "world_stats", "arguments": {}}),
    );
    let result = &response["result"];
    assert_eq!(result["isError"], false);
    let text = result["content"][0]["text"].as_str().unwrap();
    let value: Json = serde_json::from_str(text).unwrap();
    assert_eq!(value["entities"], 0);
}

#[test]
fn tools_forward_commands_snapshots_and_string_ids() {
    let mut mcp = Mcp::new();
    // 高于 JSON 客户端的安全整型范围，从编辑命令到世界快照都使用十进制字符串。
    let id = "9007199254740993";
    let plan = invoke(
        &mut mcp,
        "edit_plan",
        json!({
            "commands": [{"Spawn": {"data": {"id": id, "revision": 0, "components": []}}}]
        }),
    );
    assert_eq!(plan["valid"], true);
    let applied = invoke(
        &mut mcp,
        "edit_apply",
        json!({"plan_id": format!("+{}", plan["plan_id"])}),
    );
    let entity = invoke(&mut mcp, "world_get", json!({"id": id}));
    assert_eq!(entity["entity"]["id"], id);
    let explanation = invoke(&mut mcp, "world_explain", json!({"id": id, "eye": null}));
    assert_eq!(explanation["entity"], id);
    assert_eq!(explanation["render"]["reason"], "missing_model_component");
    let snapshot = invoke(&mut mcp, "world_snapshot", json!({}));
    let reverted = invoke(
        &mut mcp,
        "edit_revert",
        json!({"tx_id": applied["tx_id"].to_string()}),
    );
    assert_eq!(reverted["tx_id"], applied["tx_id"]);
    assert_eq!(
        invoke(&mut mcp, "world_get", json!({"id": id}))["entity"],
        Json::Null
    );
    let restored = invoke(&mut mcp, "world_restore", json!({"data": snapshot["data"]}));
    assert_eq!(restored["entities"], 1);
    assert_eq!(invoke(&mut mcp, "world_get", json!({"id": id})), entity);
}

#[test]
fn unknown_tool_returns_invalid_params() {
    let mut mcp = Mcp::new();
    let response = call(
        &mut mcp,
        "tools/call",
        json!({"name": "nope", "arguments": {}}),
    );
    assert!(response.get("error").is_some());
    assert_eq!(response["error"]["code"], -32602);
}

#[test]
fn agent_failures_are_reported_as_is_error() {
    let mut mcp = Mcp::new();
    let response = call(
        &mut mcp,
        "tools/call",
        json!({"name": "edit_plan", "arguments": {}}),
    );
    let result = &response["result"];
    assert_eq!(result["isError"], true);
    assert!(result["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("缺少 commands"));
}

#[test]
fn notifications_and_bad_json() {
    let mut mcp = Mcp::new();
    let notification = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    assert!(mcp.handle_line(&notification.to_string()).is_none());
    assert!(mcp.handle_line("   ").is_none());
    let response: Json = serde_json::from_str(&mcp.handle_line("{").unwrap()).unwrap();
    assert_eq!(response["error"]["code"], -32700);
}
