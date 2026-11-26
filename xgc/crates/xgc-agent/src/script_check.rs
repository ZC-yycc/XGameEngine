//! 将 Lua 编译诊断投影成 Agent 响应；内联源不需要项目会话。

use serde_json::{json, Value as Json};

use crate::required_str;

pub(super) const INLINE_NAME: &str = "inline.lua";

pub(super) fn check(params: &Json) -> Result<Json, String> {
    let source = required_str(params, "source")?;
    let name = match params.get("name") {
        None | Some(Json::Null) => INLINE_NAME,
        Some(Json::String(name)) => name,
        Some(_) => return Err("name 必须是字符串".to_string()),
    };
    let diagnostics = xgc_script::check_source(name, source)
        .err()
        .map(|diagnostic| {
            json!({
                "code": diagnostic.code,
                "severity": diagnostic.severity.to_string(),
                "script": name,
                "line": diagnostic.span.map(|span| span.line),
                "message": diagnostic.message,
            })
        });
    Ok(json!({
        "valid": diagnostics.is_none(),
        "diagnostics": diagnostics.into_iter().collect::<Vec<_>>(),
    }))
}
