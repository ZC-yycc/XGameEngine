//! 旧引擎 UI 焦点脚本 API：`ui_focused` / `ui_set_focus` / `ui_clear_focus`。
//!
//! 语义照旧端 `xgame-script/src/ui_ctx.rs`：
//!
//! - `ui_focused(id)` 读焦点标志；同帧刚 `ui_set_focus` 过的元素必须立刻读到 `true`
//!   （旧端直接改运行时状态，这里读命令队列里未结算的写入）；
//! - `ui_set_focus(id)` 只认 `kind == "button"` 且当前 `visible && interactable` 的元素，
//!   否则彻底 no-op（不写半截命令）；成功时先清其它元素、再置目标，
//!   与旧端「一帧只有最后一次请求生效」一致；
//! - `ui_clear_focus()` 清掉所有元素的焦点标志。
//!
//! 焦点标志落在 `ui.interaction.focused`（comp `0x0603`）。这里不新增命令类型：
//! 每个 UI 实体一条 `SetField`，随脚本命令在帧末统一 `plan` / `apply`，与其它
//! 脚本写入同一套结算路径。方向键导航的出发点在下一帧（`UiFocusSystem` 读结算后的状态）。

use mlua::Lua;
use std::rc::Rc;

use xgc_base::EntityId;
use xgc_world::{Command, Component, UiInteraction, Value as FieldValue};

use crate::legacy::{Commands, View};
use crate::ui_api::{current_bool, element};

/// 命令队列里同实体 / 同字段最后一条 `ui.interaction` 写入。
fn queued_focused(commands: &Commands, id: u64) -> Option<bool> {
    let entity = EntityId::from_raw(id);
    commands
        .borrow()
        .iter()
        .rev()
        .find_map(|command| match command {
            Command::SetField {
                entity: target,
                component,
                field,
                value: FieldValue::Bool(value),
            } if *target == entity && *component == UiInteraction::ID && field == "focused" => {
                Some(*value)
            }
            _ => None,
        })
}

/// 队列里已有同字段写入就替换（一实体一字段只留一条），否则追加。
fn push_focused(commands: &Commands, id: u64, focused: bool) {
    let entity = EntityId::from_raw(id);
    let mut queue = commands.borrow_mut();
    for command in queue.iter_mut().rev() {
        if let Command::SetField {
            entity: target,
            component,
            field,
            value,
        } = command
        {
            if *target == entity && *component == UiInteraction::ID && field == "focused" {
                *value = FieldValue::Bool(focused);
                return;
            }
        }
    }
    queue.push(Command::SetField {
        entity,
        component: UiInteraction::ID,
        field: "focused".to_string(),
        value: FieldValue::Bool(focused),
    });
}

/// 当前焦点：先看本帧尚未结算的写入，再退回帧首快照。
fn current_focused(commands: &Commands, view: &crate::view::ScriptView, id: u64) -> bool {
    if let Some(focused) = queued_focused(commands, id) {
        return focused;
    }
    view.entity(id).is_some_and(|entity| entity.focused)
}

/// 目标能不能拿焦点：有 `ui.element`、`kind == "button"`、当前可见且可交互。
fn focusable(commands: &Commands, view: &crate::view::ScriptView, id: u64) -> bool {
    let Some(row) = view.entity(id) else {
        return false;
    };
    if !row.has_interaction {
        return false;
    }
    let Some(ui) = element(view, id) else {
        return false;
    };
    if !ui.kind.eq_ignore_ascii_case("button") {
        return false;
    }
    current_bool(commands, view, id, "visible").unwrap_or(true)
        && current_bool(commands, view, id, "interactable").unwrap_or(true)
}

/// 快照里所有 UI 实体的 id（焦点是排他的：置新焦点要清掉其它元素）。
fn ui_entities(view: &crate::view::ScriptView) -> Vec<u64> {
    view.entities
        .iter()
        .filter(|entity| entity.ui.is_some() && entity.has_interaction)
        .map(|entity| entity.id)
        .collect()
}

/// 注册焦点三件套。`ui_api::register` 末尾一行调用。
pub fn register(lua: &Lua, commands: &Commands, view: &View) -> Result<(), String> {
    let globals = lua.globals();

    let focused_commands = Rc::clone(commands);
    let focused_view = Rc::clone(view);
    globals
        .set(
            "ui_focused",
            lua.create_function(move |_, (_ctx, id): (mlua::Value, u64)| {
                Ok(current_focused(
                    &focused_commands,
                    &focused_view.borrow(),
                    id,
                ))
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let set_commands = Rc::clone(commands);
    let set_view = Rc::clone(view);
    globals
        .set(
            "ui_set_focus",
            lua.create_function(move |_, (_ctx, id): (mlua::Value, u64)| {
                let view = set_view.borrow();
                if !focusable(&set_commands, &view, id) {
                    return Ok(());
                }
                for other in ui_entities(&view) {
                    push_focused(&set_commands, other, other == id);
                }
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let clear_commands = Rc::clone(commands);
    let clear_view = Rc::clone(view);
    globals
        .set(
            "ui_clear_focus",
            lua.create_function(move |_, _ctx: mlua::Value| {
                let view = clear_view.borrow();
                for entity in ui_entities(&view) {
                    push_focused(&clear_commands, entity, false);
                }
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    Ok(())
}
