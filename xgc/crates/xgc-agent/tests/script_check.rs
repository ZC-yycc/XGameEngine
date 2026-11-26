//! 静态检查走实际 Agent 协议，脚本诊断与请求参数错误分别返回。

use serde_json::{json, Value as Json};
use xgc_agent::Agent;

fn call(agent: &mut Agent, method: &str, params: Json) -> Json {
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let response = agent.handle_line(&request.to_string()).unwrap();
    serde_json::from_str(&response).unwrap()
}

fn result(agent: &mut Agent, method: &str, params: Json) -> Json {
    let response = call(agent, method, params);
    assert!(response.get("error").is_none(), "{response}");
    response["result"].clone()
}

#[test]
fn compiles_lua_text_without_running_it_or_changing_world() {
    let mut agent = Agent::new();
    result(
        &mut agent,
        "world.restore",
        json!({"data": {"revision": 7, "next_id": 2, "entities": [
            {"id": "1", "revision": 7, "components": []}
        ]}}),
    );
    let before = result(&mut agent, "world.snapshot", Json::Null);
    let checked = result(
        &mut agent,
        "script.check",
        json!({"source": "local value <const> = 1\nerror('静态检查不得执行顶层')"}),
    );
    assert_eq!(checked, json!({"valid": true, "diagnostics": []}));
    assert_eq!(result(&mut agent, "world.snapshot", Json::Null), before);
}

#[test]
fn syntax_error_reports_original_name_and_one_based_line() {
    let mut agent = Agent::new();
    // 显示名称可以很长并带冒号，解析 Lua 错误时不能从中误读行号。
    let name = format!("D:/{}:73:角色.lua", "角色".repeat(40));
    let params = json!({"name": name, "source": "-- line 1\nlocal ok = true\nlocal = 4\n"});
    let checked = result(&mut agent, "script.check", params.clone());
    assert_eq!(checked["valid"], false);
    let diagnostics = checked["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic["code"], "SCRIPT.COMPILE");
    assert_eq!(diagnostic["severity"], "error");
    assert_eq!(diagnostic["script"], name);
    assert_eq!(diagnostic["line"], 3);
    assert!(diagnostic["message"].as_str().unwrap().contains("near '='"));
    assert_eq!(result(&mut agent, "script.check", params), checked);
}

#[test]
fn malformed_requests_return_rpc_errors() {
    let mut agent = Agent::new();
    for (params, field) in [
        (json!({"path": "scripts/behavior.lua"}), "source"),
        (json!({"source": 17}), "source"),
        (json!({"source": "return true", "name": 3}), "name"),
    ] {
        let response = call(&mut agent, "script.check", params);
        assert_eq!(response["error"]["code"], -32000);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains(field));
    }
}
