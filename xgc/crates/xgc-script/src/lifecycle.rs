//! 旧生命周期：`start(ctx)` / `update(ctx)`。

use mlua::{IntoLuaMulti, Table, Value as LuaValue};

use super::LuaHost;

impl LuaHost {
    fn call_legacy<A: IntoLuaMulti>(
        &self,
        name: &str,
        args: impl FnOnce(Table) -> A,
    ) -> Result<bool, String> {
        self.with_budget(
            || format!("Lua 钩子 {name} 失败"),
            || {
                let LuaValue::Function(function) = self.environment().get::<LuaValue>(name)? else {
                    return Ok(false);
                };
                let ctx = self.lua.create_table()?;
                ctx.set("entity", self.self_id)?;
                function.call::<()>(args(ctx))?;
                Ok(true)
            },
        )
    }

    /// 旧生命周期：优先 `start(ctx)`，否则 `on_start()`。
    pub fn start_legacy(&self) -> Result<bool, String> {
        if self.started.replace(true) {
            return Ok(false);
        }
        if self.call_legacy("start", |ctx| ctx)? {
            return Ok(true);
        }
        self.call_hook("on_start", 0.0)
    }

    /// 旧生命周期：有 `update(ctx)` 就调用它，否则回退 `on_update(dt)`。
    pub fn update_legacy(&self, dt: f32) -> Result<bool, String> {
        if self.call_legacy("update", |ctx| ctx)? {
            return Ok(true);
        }
        self.call_hook("on_update", dt)
    }

    /// `fixed_update(ctx)` 在 `update` 阶段之后派发。XGC 宿主在每个固定步依次运行两者。
    pub fn fixed_legacy(&self) -> Result<bool, String> {
        self.call_legacy("fixed_update", |ctx| ctx)
    }

    /// 销毁时世界里可能已经没有对应实体，钩子仍可读取原 VM 的自身 ID 和状态。
    pub fn destroy(&self) -> Result<bool, String> {
        self.call_legacy("on_destroy", |ctx| ctx)
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
        self.call_legacy(hook, |ctx| (ctx, other.raw() as i64))
    }

    /// 旧生命周期：`on_anim_event(ctx, name)`。
    ///
    /// 事件由 `AnimatorSystem` 在动画时钟推进后写入 `World` 的瞬态队列；宿主
    /// 阶段在动画之前，所以脚本在**下一个固定步**的 update/fixed_update 之后
    /// 收到它（一帧延迟，见 `AnimatorSystem` 文档）。
    pub fn anim_event_legacy(&self, name: &str) -> Result<bool, String> {
        self.call_legacy("on_anim_event", |ctx| (ctx, name))
    }
}

impl std::fmt::Debug for LuaHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LuaHost").finish_non_exhaustive()
    }
}
