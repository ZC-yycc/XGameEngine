//! Lua 5.4 把原生指令钩子继承到协程。一个宿主调用内的各线程共用同一计数。

use std::cell::Cell;
use std::ffi::c_int;

use mlua::{ffi, Lua};

pub(super) const DEFAULT_INSTRUCTION_LIMIT: usize = 200_000;
const HOOK_INTERVAL: c_int = 1024;
const HOOK_EVENTS: c_int = ffi::LUA_MASKCOUNT | ffi::LUA_MASKRET;

#[derive(Clone, Copy)]
struct Usage {
    limit: usize,
    used: usize,
}

thread_local! {
    static USAGE: Cell<Option<Usage>> = const { Cell::new(None) };
}

pub(super) struct BudgetScope(Option<Usage>);

impl BudgetScope {
    pub(super) fn enter(limit: usize) -> Self {
        Self(USAGE.replace(Some(Usage { limit, used: 0 })))
    }

    pub(super) fn exhausted(&self) -> bool {
        USAGE.get().is_some_and(|usage| usage.used > usage.limit)
    }
}

impl Drop for BudgetScope {
    fn drop(&mut self) {
        USAGE.set(self.0);
    }
}

unsafe extern "C-unwind" {
    fn xgc_lua_instruction_hook(state: *mut ffi::lua_State, event: *mut ffi::lua_Debug);
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // SAFETY: exec_raw 持有 Lua 状态，lua_sethook 只设置钩子，不执行代码或修改栈。
    unsafe {
        lua.exec_raw((), |state| {
            ffi::lua_sethook(
                state,
                Some(xgc_lua_instruction_hook),
                HOOK_EVENTS,
                HOOK_INTERVAL,
            );
        })
    }
}

#[no_mangle]
unsafe extern "C" fn xgc_lua_budget_tick(
    state: *mut ffi::lua_State,
    event: *mut ffi::lua_Debug,
) -> c_int {
    let Some(mut usage) = USAGE.get() else {
        return 0;
    };
    // SAFETY: 原生钩子传入存活的 Lua 线程及这次钩子的事件。只读事件类型。
    let is_count = unsafe { (*event).event == ffi::LUA_HOOKCOUNT };
    if !is_count && usage.used <= usage.limit {
        return 0;
    }
    // SAFETY: Lua 线程仍在钩子调用中，读取或修改钩子不会调用 Lua 代码。
    let interval = unsafe { ffi::lua_gethookcount(state) };
    if is_count {
        usage.used = usage.used.saturating_add(interval as usize);
        USAGE.set(Some(usage));
    }
    let exhausted = usage.used > usage.limit;
    // 超限后下一条指令也检查，避免受保护调用捕获错误后继续执行。
    // 下一次宿主调用恢复正常间隔，协程保留原有的 yield / resume 语义。
    let next_interval = if exhausted { 1 } else { HOOK_INTERVAL };
    if interval != next_interval {
        unsafe {
            ffi::lua_sethook(
                state,
                Some(xgc_lua_instruction_hook),
                HOOK_EVENTS,
                next_interval,
            );
        }
    }
    c_int::from(exhausted)
}
