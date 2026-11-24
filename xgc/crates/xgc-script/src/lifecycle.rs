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
    /// 旧生命周期：`fixed_update(ctx)`，每个物理固定步调用一次。
    ///
    /// 与 `update` 分开是因为两个回调的频率语义不同：`update` 对应渲染帧，
    /// `fixed_update` 对应固定步长。脚本只定义其中一个时另一个保持 no-op。
    pub fn fixed_legacy(&self) -> Result<bool, String> {
        let value: LuaValue = self
            .lua
            .globals()
            .get("fixed_update")
            .map_err(|err| format!("读取 fixed_update 失败：{err}"))?;
        let LuaValue::Function(function) = value else {
            return Ok(false);
        };
        self.reset_budget();
        let ctx = self.legacy_context()?;
        function
            .call::<()>(ctx)
            .map_err(|err| format!("Lua 钩子 fixed_update 失败：{err}"))?;
        Ok(true)
    }

    /// 旧生命周期：`on_collision_enter(ctx, other)` / `on_collision_exit(ctx, other)`。
    ///
    /// `other` 是对面实体的数字 ID（旧端 `LuaCtx` 之后的第二个参数）。每个接触对
    /// 双方各收一次；持续接触不刷新，只在进入 / 离开那一步各调一次。
    pub fn collision_legacy(&self, enter: bool, other: xgc_base::EntityId) -> Result<bool, String> {
        let hook = if enter {
            "on_collision_enter"
        } else {
            "on_collision_exit"
        };
        let value: LuaValue = self
            .lua
            .globals()
            .get(hook)
            .map_err(|err| format!("读取 {hook} 失败：{err}"))?;
        let LuaValue::Function(function) = value else {
            return Ok(false);
        };
        self.reset_budget();
        let ctx = self.legacy_context()?;
        function
            .call::<()>((ctx, other.raw() as i64))
            .map_err(|err| format!("Lua 钩子 {hook} 失败：{err}"))?;
        Ok(true)
    }

    /// 旧生命周期：`on_anim_event(ctx, name)`。
    ///
    /// 事件由 `AnimatorSystem` 在动画时钟推进后写入 `World` 的瞬态队列；宿主
    /// 阶段在动画之前，所以脚本在**下一个固定步**的 update/fixed_update 之后
    /// 收到它（一帧延迟，见 `AnimatorSystem` 文档）。
    pub fn anim_event_legacy(&self, name: &str) -> Result<bool, String> {
        let value: LuaValue = self
            .lua
            .globals()
            .get("on_anim_event")
            .map_err(|err| format!("读取 on_anim_event 失败：{err}"))?;
        let LuaValue::Function(function) = value else {
            return Ok(false);
        };
        self.reset_budget();
        let ctx = self.legacy_context()?;
        function
            .call::<()>((ctx, name.to_string()))
            .map_err(|err| format!("Lua 钩子 on_anim_event 失败：{err}"))?;
        Ok(true)
    }
}

impl std::fmt::Debug for LuaHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LuaHost").finish_non_exhaustive()
    }
}
