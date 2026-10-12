//! `xgame-mcp` 入口：stdin 的每一行交给 [`xgame_mcp::handle_line`]，有响应
//! 写到 stdout（一行一条）。
//!
//! 协议逻辑全在库里（喂行进、断言行出，不碰真实 I/O），这里只做管道 ——
//! 保持到最小，让"用脚本驱动 MCP server"成为可能：`echo <json> | xgame-mcp`。

use std::io::{BufRead, Write};

fn main() {
    // stderr 允许当日志用（MCP stdio 规范），这行是排查"客户端连上了没"的线索。
    eprintln!(
        "{} {} (MCP stdio, newline-delimited JSON-RPC)",
        xgame_mcp::SERVER_NAME,
        env!("CARGO_PKG_VERSION")
    );

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if let Some(response) = xgame_mcp::handle_line(&line) {
            let mut out = stdout.lock();
            if writeln!(out, "{response}").is_err() || out.flush().is_err() {
                break; // 对端关了
            }
        }
    }
}
