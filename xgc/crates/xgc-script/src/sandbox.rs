//! 标准库的动态加载仍使用同一 VM 的内存和指令上限。

use std::{cell::RefCell, fmt};

use mlua::{Function, Lua, String as LuaString, Table, Value};

fn emit(message: fmt::Arguments<'_>) {
    eprintln!("[lua] {message}");
}

pub(super) fn log(_: &Lua, message: LuaString) -> mlua::Result<()> {
    emit(format_args!("{}", message.to_string_lossy()));
    Ok(())
}

pub(super) fn configure(lua: &Lua) -> mlua::Result<Function> {
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
    // 垃圾回收由宿主管理，模块初始化不能在活动 VM 中主动触发终结器。
    globals.raw_set("collectgarbage", Value::Nil)?;
    // stdout 由帧轨迹和 stdio 协议使用；脚本诊断统一写到 stderr。
    let emit = lua.create_function(log)?;
    // 保留字符串、读取器和可选环境。只允许文本，避免绕过宿主的文本编译边界。
    lua.load(
        r#"
        local emit, to_text, join = ..., tostring, table.concat
        local native_load, select_arg = load, select
        -- 字符串的方法表属于整个 VM，候选模块不能借它改写活动模块的标准库。
        getmetatable('').__metatable = false
        function print(...)
            local fields = {}
            for i = 1, select_arg('#', ...) do
                fields[i] = to_text((select_arg(i, ...)))
            end
            emit(join(fields, '\t'))
        end
        local function make_load(environment)
            return function(chunk, name, mode, ...)
                if mode == 'b' then return nil, 'Lua only accepts text chunks' end
                if select_arg('#', ...) == 0 then
                    return native_load(chunk, name, 't', environment)
                end
                return native_load(chunk, name, 't', ...)
            end
        end
        load = make_load(_G)
        return make_load
        "#,
    )
    .set_name("=xgc-sandbox")
    .call(emit)
}

pub(super) fn module_environment(lua: &Lua, load_factory: &Function) -> mlua::Result<Table> {
    let globals = lua.globals();
    let fallback = lua.create_table()?;
    for pair in globals.pairs::<mlua::String, Value>() {
        let (key, value) = pair?;
        if key == "_G" || key == "load" {
            continue;
        }
        // 标准库和绑定表内只有常量与函数。候选脚本改表时只会改到自己的副本。
        let value = if let Value::Table(source) = value {
            let table = lua.create_table()?;
            for entry in source.pairs::<Value, Value>() {
                let (key, value) = entry?;
                table.raw_set(key, value)?;
            }
            Value::Table(table)
        } else {
            value
        };
        fallback.raw_set(key, value)?;
    }
    let environment = lua.create_table()?;
    let metatable = lua.create_table()?;
    metatable.raw_set("__index", fallback)?;
    metatable.raw_set("__metatable", false)?;
    environment.set_metatable(Some(metatable));
    environment.raw_set("_G", environment.clone())?;
    environment.raw_set("load", load_factory.call::<Function>(environment.clone())?)?;
    Ok(environment)
}
