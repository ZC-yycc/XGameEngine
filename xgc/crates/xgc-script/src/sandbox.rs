//! 标准库的动态加载仍使用同一 VM 的内存和指令上限。

use std::{cell::RefCell, fmt};

use mlua::{Lua, String as LuaString, Value};

fn emit(message: fmt::Arguments<'_>) {
    eprintln!("[lua] {message}");
}

pub(super) fn log(_: &Lua, message: LuaString) -> mlua::Result<()> {
    emit(format_args!("{}", message.to_string_lossy()));
    Ok(())
}

pub(super) fn configure(lua: &Lua) -> mlua::Result<()> {
    let warning = RefCell::new((false, false, String::new()));
    lua.set_warning_function(move |_, text, continues| {
        let (enabled, continuing, message) = &mut *warning.borrow_mut();
        if !*continuing && !continues && text.starts_with('@') {
            match text {
                "@on" => *enabled = true,
                "@off" => *enabled = false,
                _ => {}
            }
        } else if *enabled {
            message.push_str(text);
            if !continues {
                emit(format_args!("warning: {message}"));
                message.clear();
            }
        }
        *continuing = continues;
        Ok(())
    });
    let globals = lua.globals();
    // Lua 默认装载的 base 库也有文件入口，与 io / os 的开关无关。
    globals.raw_set("dofile", Value::Nil)?;
    globals.raw_set("loadfile", Value::Nil)?;
    // stdout 由帧轨迹和 stdio 协议使用；脚本诊断统一写到 stderr。
    let emit = lua.create_function(log)?;
    // 保留字符串、读取器和可选环境。只允许文本，避免绕过宿主的文本编译边界。
    lua.load(
        r#"
        local emit, to_text, join = ..., tostring, table.concat
        local native_load, select_arg = load, select
        function print(...)
            local fields = {}
            for i = 1, select_arg('#', ...) do
                fields[i] = to_text((select_arg(i, ...)))
            end
            emit(join(fields, '\t'))
        end
        function load(chunk, name, mode, ...)
            if mode == 'b' then return nil, 'Lua only accepts text chunks' end
            if select_arg('#', ...) == 0 then
                return native_load(chunk, name, 't')
            end
            return native_load(chunk, name, 't', ...)
        end
        "#,
    )
    .set_name("=xgc-sandbox")
    .call(emit)
}
