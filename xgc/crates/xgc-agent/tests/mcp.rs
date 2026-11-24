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
    assert_eq!(tools.len(), 17);
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    for expected in ["engine_describe", "world_stats", "edit_plan", "edit_undo"] {
        assert!(names.contains(&expected), "缺少工具 {expected}");
    }
    assert!(tools[0]["inputSchema"].is_object());
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
fn unknown_tool_is_a_tool_error_not_a_protocol_error() {
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
