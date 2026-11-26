//! 标准库的动态加载仍使用同一 VM 的内存和指令上限。

use mlua::{Lua, Value};

pub(super) fn configure(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    // Lua 默认装载的 base 库也有文件入口，与 io / os 的开关无关。
    globals.raw_set("dofile", Value::Nil)?;
    globals.raw_set("loadfile", Value::Nil)?;
    // 保留字符串、读取器和可选环境。只允许文本，避免绕过宿主的文本编译边界。
    lua.load(
        r#"
        local native_load, select_arg = load, select
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
    .exec()
}
