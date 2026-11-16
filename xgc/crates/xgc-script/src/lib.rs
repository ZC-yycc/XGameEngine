//! Lua 脚本宿主：只开放安全标准库，脚本按名字加载、执行与调用。

use mlua::{FromLua, FromLuaMulti, IntoLua, IntoLuaMulti, Lua, LuaOptions, StdLib, Value};

pub struct LuaHost {
    lua: Lua,
}

impl LuaHost {
    /// 安全沙箱：table / string / math / utf8 / coroutine；不开 io / os / package / debug。
    pub fn new() -> Result<Self, String> {
        let libs = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE;
        let lua = Lua::new_with(libs, LuaOptions::default())
            .map_err(|err| format!("创建 Lua 运行时失败：{err}"))?;
        Ok(Self { lua })
    }

    pub fn run(&self, name: &str, source: &str) -> Result<(), String> {
        self.lua
            .load(source)
            .set_name(name)
            .exec()
            .map_err(|err| format!("脚本 {name} 执行失败：{err}"))
    }

    pub fn eval<T: FromLua>(&self, source: &str) -> Result<T, String> {
        self.lua
            .load(source)
            .eval()
            .map_err(|err| format!("Lua 求值失败：{err}"))
    }

    pub fn call<T: FromLua>(&self, name: &str, args: impl IntoLuaMulti) -> Result<T, String> {
        let function: mlua::Function = self
            .lua
            .globals()
            .get(name)
            .map_err(|err| format!("读取 Lua 函数 {name} 失败：{err}"))?;
        function
            .call(args)
            .map_err(|err| format!("Lua 函数 {name} 调用失败：{err}"))
    }

    pub fn call_multi<T: FromLuaMulti>(
        &self,
        name: &str,
        args: impl IntoLuaMulti,
    ) -> Result<T, String> {
        let function: mlua::Function = self
            .lua
            .globals()
            .get(name)
            .map_err(|err| format!("读取 Lua 函数 {name} 失败：{err}"))?;
        function
            .call(args)
            .map_err(|err| format!("Lua 函数 {name} 调用失败：{err}"))
    }

    pub fn set_global(&self, name: &str, value: impl IntoLua) -> Result<(), String> {
        self.lua
            .globals()
            .set(name, value)
            .map_err(|err| format!("写入 Lua 全局 {name} 失败：{err}"))
    }

    pub fn global_is_nil(&self, name: &str) -> Result<bool, String> {
        let value: Value = self
            .lua
            .globals()
            .get(name)
            .map_err(|err| format!("读取 Lua 全局 {name} 失败：{err}"))?;
        Ok(matches!(value, Value::Nil))
    }

    /// 只读脚本名列表：`package.loaded` 不可用，这里用宿主登记表代替。
    pub fn lua(&self) -> &Lua {
        &self.lua
    }
}

/// `LuaHost` 不是 `Send`，显式声明以便调用方知道不能跨线程共享。
impl std::fmt::Debug for LuaHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LuaHost").finish_non_exhaustive()
    }
}
