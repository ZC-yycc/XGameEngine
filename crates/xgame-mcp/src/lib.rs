//! # xgame-mcp —— 让 AI 智能体直接操纵引擎的 MCP server
//!
//! # 为什么（Agent-Native 的判据）
//!
//! 判据是：**智能体没有鼠标时，这条路走得通吗？** 编辑器的 `--check` 只解决
//! "改完能验证"；要"能改"，得把项目的读写暴露成标准工具。MCP（Model Context
//! Protocol）是当前智能体接入工具的事实标准 —— 接上它，Cursor / Claude 等
//! 客户端就能直接调本引擎的工具，不需要任何定制胶水。
//!
//! # 协议面（刻意小）
//!
//! stdio 传输，**按行分隔的 JSON-RPC 2.0**（不是 LSP 的 Content-Length 帧）：
//! 一行一个消息，响应也是一行。只实现四个方法：
//!
//! - `initialize` — 握手，回显双方都支持的协议版本 + `capabilities.tools`；
//! - `ping` — 保活；
//! - `tools/list` — 返回 8 个工具的 JSON Schema（见 [`tools`]）；
//! - `tools/call` — 分发调用。
//!
//! 其余方法回 `-32601`；通知（无 `id`）不回包；坏 JSON 回 `-32700`。
//!
//! # 错误分两层（MCP 规定，别混）
//!
//! - **协议错误**（JSON-RPC `error` 对象）：消息本身不合法 —— 坏 JSON、未知方法、
//!   未知工具名、缺必填参数。
//! - **工具结果错误**（`result.isError: true` + 文本）：工具跑了，但没干成 ——
//!   项目打不开、RON 解析失败、实体不存在。智能体读文本就知道发生了什么。
//!
//! # 状态
//!
//! **无会话状态**：每个工具都要求 `project` 参数（项目根路径）。这样 server
//! 不维护"当前项目"，可被并发调用、可开多个实例、测试也只需喂行进喂行出。

pub mod tools;

use serde_json::{json, Value};

/// 服务器标识（握手回给客户端）。
pub const SERVER_NAME: &str = "xgame-mcp";

/// 我们实现的协议版本。tools API 在这些版本间形状一致，客户端请求其中任一版
/// 本都回显它，请求了没见过的版本则回这个兜底值（让客户端自己决定降级/断开）。
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// 认识的协议版本（握手时可回显）。
const SUPPORTED_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

/// 处理 stdin 上的一行，返回该写到 stdout 的一行；返回 `None` = 无需响应
/// （通知、空行、杂音）。
///
/// 这是整个 server 的可测入口：协议测试直接喂行进、断言行出，不碰真实 stdin。
pub fn handle_line(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(error_line(&Value::Null, -32700, &format!("parse error: {e}"))),
    };
    if !msg.is_object() {
        return Some(error_line(&Value::Null, -32600, "invalid request: not an object"));
    }

    let id = msg.get("id").cloned();
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        // 没有 method：只有带 id 的才值得报错（否则是无归属的杂音，丢弃）。
        return id.map(|id| error_line(&id, -32600, "invalid request: missing method"));
    };
    // 没有 id = 通知（notifications/initialized 等）：规范要求不回包。
    let id = id?;
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    Some(match method {
        "initialize" => result_line(&id, initialize_result(&params)),
        "ping" => result_line(&id, json!({})),
        "tools/list" => result_line(&id, json!({ "tools": tools::schemas() })),
        "tools/call" => match call_tool(&params) {
            Ok(result) => result_line(&id, result),
            Err((code, message)) => error_line(&id, code, &message),
        },
        other => error_line(&id, -32601, &format!("method not found: {other}")),
    })
}

/// `initialize` 的 result：版本协商 + 能力声明 + 服务器信息。
fn initialize_result(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = match requested {
        Some(v) if SUPPORTED_VERSIONS.contains(&v) => v,
        _ => PROTOCOL_VERSION,
    };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": env!("CARGO_PKG_VERSION"),
            "description": "XGameEngine 项目/场景工具集（读写场景、改实体、检查与无头运行）",
        },
    })
}

/// `tools/call` 分发。`Err((code, msg))` 是协议错误；工具自身的成败在
/// `result.isError` 里表达（见模块头"错误分两层"）。
fn call_tool(params: &Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "tools/call requires params.name".to_string()))?;
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    if !args.is_object() {
        return Err((-32602, "params.arguments must be an object".to_string()));
    }
    match tools::call(name, &args) {
        Ok(output) => Ok(tool_result_content(output)),
        Err(tools::CallError::UnknownTool(name)) => {
            Err((-32602, format!("unknown tool: {name}")))
        }
        Err(tools::CallError::Failed(message)) => {
            Ok(json!({ "content": [{ "type": "text", "text": message }], "isError": true }))
        }
    }
}

/// 工具输出 → `tools/call` 的 result：文本块打头，之后每个随附图片一个 MCP
/// `image` 块（视觉模型直接看画面；纯文本客户端至少能读到信封里的元数据）。
///
/// 单独抽出来是为了能不加 GPU 直接测协议形状：`mimeType` 是协议拼写，
/// 写错客户端会把图片当未知块静默丢掉。
fn tool_result_content(output: tools::ToolOutput) -> Value {
    let mut content = vec![json!({ "type": "text", "text": output.text })];
    for image in &output.images {
        content.push(json!({
            "type": "image",
            "data": image.base64,
            "mimeType": image.mime,
        }));
    }
    json!({ "content": content, "isError": false })
}

fn result_line(id: &Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error_line(id: &Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(input: &str) -> Value {
        let line = handle_line(input).expect("该请求必须有响应");
        serde_json::from_str(&line).expect("响应必须是合法 JSON")
    }

    /// 握手：回显我们支持的协议版本、声明 tools 能力、自报家门。
    #[test]
    fn initialize_handshake() {
        let resp = send(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
        );
        assert_eq!(resp["id"], 1);
        let result = &resp["result"];
        assert_eq!(result["protocolVersion"], "2025-06-18", "认识的版本必须回显");
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], SERVER_NAME);
    }

    /// 客户端请求了没见过的版本 → 回兜底版本，交给客户端决策。
    #[test]
    fn initialize_unknown_version_falls_back() {
        let resp = send(
            r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        );
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    /// 通知（无 id）必须不回包 —— 回了会把客户端的解析器搞乱。
    #[test]
    fn notification_gets_no_response() {
        assert!(handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        assert!(handle_line("").is_none());
        assert!(handle_line("   \n").is_none());
    }

    /// 未知方法 → -32601；坏 JSON → -32700 且 id 为 null。
    #[test]
    fn protocol_errors() {
        let resp = send(r#"{"jsonrpc":"2.0","id":9,"method":"resources/list"}"#);
        assert_eq!(resp["error"]["code"], -32601);
        assert_eq!(resp["id"], 9);

        let resp = send("{not json");
        assert_eq!(resp["error"]["code"], -32700);
        assert!(resp["id"].is_null());
    }

    /// `tools/list` 给出 8 个工具，每个都带描述和 `project` 必填的 JSON Schema。
    #[test]
    fn tools_list_shape() {
        let resp = send(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#);
        let list = resp["result"]["tools"].as_array().expect("tools 数组");
        assert_eq!(list.len(), 8, "最小工具集 = 8 个");
        for tool in list {
            assert!(tool["name"].as_str().is_some());
            assert!(
                tool["description"].as_str().unwrap_or_default().len() > 10,
                "{} 缺描述",
                tool["name"]
            );
            let schema = &tool["inputSchema"];
            assert_eq!(schema["type"], "object");
            let required: Vec<&str> = schema["required"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            assert!(
                required.contains(&"project"),
                "{} 必填参数缺 project",
                tool["name"]
            );
            assert!(schema["properties"]["project"]["type"] == "string");
        }
    }

    /// 未知工具名是**协议错误**（-32602，参数不合法），不是工具执行失败。
    #[test]
    fn unknown_tool_is_protocol_error() {
        let resp = send(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"delete_universe","arguments":{}}}"#,
        );
        assert_eq!(resp["error"]["code"], -32602);
        assert!(resp["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("delete_universe"));
    }

    /// 工具执行失败（如项目路径不存在）走 `result.isError: true`，不是协议错误。
    #[test]
    fn tool_failure_is_iserror_not_protocol_error() {
        let resp = send(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"project_info","arguments":{"project":"Z:/definitely/not/a/project"}}}"#,
        );
        assert_eq!(resp["error"], serde_json::Value::Null, "不该是协议错误");
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("打开项目失败"), "{text}");
    }

    /// 缺 `name` 的 tools/call → -32602；arguments 不是对象 → -32602。
    #[test]
    fn bad_call_params_rejected() {
        let resp = send(r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{}}"#);
        assert_eq!(resp["error"]["code"], -32602);

        let resp = send(
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"project_info","arguments":"nope"}}"#,
        );
        assert_eq!(resp["error"]["code"], -32602);
    }

    /// 带图的工具输出 → 文本块 + `image` 块（字段拼写是协议契约：`mimeType`、
    /// `data` 里是裸 base64，不带 data URL 前缀）。
    #[test]
    fn tool_output_with_images_maps_to_content_blocks() {
        let result = tool_result_content(tools::ToolOutput {
            text: "{\"ok\":true}".into(),
            images: vec![tools::ToolImage {
                mime: "image/png",
                base64: "iVBORw0KGgo=".into(),
            }],
        });
        assert_eq!(result["isError"], false);
        let content = result["content"].as_array().expect("content 数组");
        assert_eq!(content.len(), 2, "一个文本块 + 一个图片块");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "{\"ok\":true}");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["mimeType"], "image/png");
        assert_eq!(content[1]["data"], "iVBORw0KGgo=");
    }

    /// ping 是保活，回空 result。
    #[test]
    fn ping_pong() {
        let resp = send(r#"{"jsonrpc":"2.0","id":8,"method":"ping"}"#);
        assert_eq!(resp["result"], serde_json::json!({}));
    }
}
