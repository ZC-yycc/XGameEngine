//! MCP stdio 适配器：把 `xgc.agent/1` 的方法原样暴露成 MCP 工具。
//!
//! 从 Agent 方法目录生成工具列表，参数和返回值直接交给该目录对应的方法。
//! 帧格式与 MCP 的 stdio 传输一致：一行一条 JSON-RPC 消息，空行忽略。

use serde_json::{json, Value as Json};

use crate::{methods, Agent, PROTOCOL};

fn tools() -> Json {
    let tools: Vec<_> = methods::METHODS
        .iter()
        .map(|method| {
            json!({
                "name": method.tool_name(),
                "description": method.description,
                "inputSchema": (method.input_schema)(),
            })
        })
        .collect();
    json!({ "tools": tools })
}

/// 一行一条消息的 MCP 服务端状态。
pub struct Mcp {
    agent: Agent,
}

impl Default for Mcp {
    fn default() -> Self {
        Self::new()
    }
}

impl Mcp {
    pub fn new() -> Self {
        Self {
            agent: Agent::new(),
        }
    }

    /// 处理一行；通知（无 id）返回 None。
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let request: Json = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(err) => {
                return Some(error_response(
                    Json::Null,
                    -32700,
                    format!("JSON 解析失败：{err}"),
                ));
            }
        };
        let id = request.get("id").cloned();
        let method = request
            .get("method")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let params = request.get("params").cloned().unwrap_or(Json::Null);

        // 通知没有 id，按 JSON-RPC 不回应；MCP 的 initialized 就是通知。
        let id = id?;
        let response = match method.as_str() {
            "initialize" => Ok(initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(tools()),
            "tools/call" => self.call_tool(&params),
            other => Err((-32601, format!("未知方法：{other}"))),
        };
        Some(match response {
            Ok(value) => json!({"jsonrpc": "2.0", "id": id, "result": value}).to_string(),
            Err((code, message)) => error_response(id, code, message),
        })
    }

    fn call_tool(&mut self, params: &Json) -> Result<Json, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Json::as_str)
            .ok_or((-32602, "缺少工具名".to_string()))?;
        let method = methods::by_tool_name(name).ok_or((-32602, format!("未知工具：{name}")))?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let text = match method.call(&mut self.agent, &arguments) {
            // 工具执行失败按 MCP 约定放进 isError，而不是 JSON-RPC 错误。
            Ok(value) => {
                let text = serde_json::to_string_pretty(&value)
                    .unwrap_or_else(|err| format!("结果序列化失败：{err}"));
                json!({
                    "content": [{"type": "text", "text": text}],
                    "isError": false
                })
            }
            Err(message) => json!({
                "content": [{"type": "text", "text": message}],
                "isError": true
            }),
        };
        Ok(text)
    }
}

fn initialize(params: &Json) -> Json {
    // 客户端给已知版本就回同一个；否则回一个广受支持的版本，让客户端决定是否继续。
    let requested = params.get("protocolVersion").and_then(Json::as_str);
    let version = match requested {
        Some("2024-11-05" | "2025-03-26" | "2025-06-18") => requested.unwrap_or("2024-11-05"),
        _ => "2024-11-05",
    };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "xgc", "version": env!("CARGO_PKG_VERSION") },
        "instructions": format!("xgc MCP 适配器；底层协议 {PROTOCOL}，工具与 agent 方法一一对应。")
    })
}

fn error_response(id: Json, code: i64, message: String) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
    .to_string()
}
