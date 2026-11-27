//! 脚本运行和静态检查使用同一 Lua 文本编译入口。

use mlua::{ChunkMode, Error, Function, Lua, Table};
use xgc_base::{diagnostics::SCRIPT_COMPILE, Diagnostic, Severity, Span, Subject};

use crate::LuaHost;

pub(super) fn compile_text(
    lua: &Lua,
    name: &str,
    source: &str,
    environment: Option<Table>,
) -> mlua::Result<Function> {
    let chunk = lua.load(source).set_name(name).set_mode(ChunkMode::Text);
    match environment {
        Some(environment) => chunk.set_environment(environment).into_function(),
        None => chunk.into_function(),
    }
}

/// 在独立 Lua VM 中编译文本，不调用生成的函数。
/// `name` 用来标注诊断，可以是调用方的脚本名或路径。
pub fn check_source(name: &str, source: &str) -> Result<(), Diagnostic> {
    let diagnostic = |message| {
        Diagnostic::new(
            SCRIPT_COMPILE,
            Severity::Error,
            Subject::Asset(name.to_string()),
            message,
        )
    };
    let host = LuaHost::new().map_err(diagnostic)?;
    // Lua 会截断长 chunk 名。固定短名能可靠读取行号，展示时仍使用原名。
    if let Err(err) = compile_text(&host.lua, "=xgc-script-check", source, None) {
        if let Error::SyntaxError { message, .. } = &err {
            if let Some((line, message)) = syntax_location(message) {
                return Err(diagnostic(message.to_string()).with_span(Span::new(name, line, 0)));
            }
        }
        return Err(diagnostic(host.describe_error(&err)));
    }
    Ok(())
}

fn syntax_location(message: &str) -> Option<(u32, &str)> {
    let (line, message) = message.strip_prefix("xgc-script-check:")?.split_once(':')?;
    let line = line.parse().ok()?;
    Some((line, message.trim_start()))
}
