//! 旧生命周期：`start(ctx)` / `update(ctx)`。

use mlua::Value as LuaValue;

use super::LuaHost;

impl LuaHost {
    fn legacy_context(&self) -> Result<mlua::Table, String> {
        let table = self
            .lua
            .create_table()
            .map_err(|err| format!("创建脚本上下文失败：{err}"))?;
        table
            .set("entity", self.self_id)
            .map_err(|err| format!("写入脚本上下文失败：{err}"))?;
        Ok(table)
    }

    /// 旧生命周期：优先 `start(ctx)`，否则 `on_start()`。
    pub fn start_legacy(&self) -> Result<bool, String> {
        if self.started.replace(true) {
            return Ok(false);
        }
        let value: LuaValue = self
            .lua
            .globals()
            .get("start")
            .map_err(|err| format!("读取 start 失败：{err}"))?;
        if let LuaValue::Function(function) = value {
            self.reset_budget();
            let ctx = self.legacy_context()?;
            function
                .call::<()>(ctx)
                .map_err(|err| format!("Lua 钩子 start 失败：{err}"))?;
            return Ok(true);
        }
        self.call_hook("on_start", 0.0)
    }

    /// 旧生命周期：有 `update(ctx)` 就调用它，否则回退 `on_update(dt)`。
    pub fn update_legacy(&self, dt: f32) -> Result<bool, String> {
        let value: LuaValue = self
            .lua
            .globals()
            .get("update")
            .map_err(|err| format!("读取 update 失败：{err}"))?;
        if let LuaValue::Function(function) = value {
            self.reset_budget();
            let ctx = self.legacy_context()?;
            function
                .call::<()>(ctx)
                .map_err(|err| format!("Lua 钩子 update 失败：{err}"))?;
            return Ok(true);
        }
        self.call_hook("on_update", dt)
    }
}

impl std::fmt::Debug for LuaHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LuaHost").finish_non_exhaustive()
    }
}
