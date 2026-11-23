//! 旧引擎 UI 脚本 API：布局 / 文本 / 样式的 setter 与 getter。
//!
//! 语义与旧端 `xgame-script/src/ui_ctx.rs` 逐条对齐：
//!
//! - `ui_set_size` 保持 `offset_min`（左上角）不动，只改 `offset_max`；
//! - `ui_set_pos` 保持尺寸，平移 `offset_min` / `offset_max`；
//! - 颜色参数是 **sRGB 0..1**，组件里存**线性**（`srgb_to_linear` 是唯一入口）；
//! - 目标实体没有 `ui.element` 时所有调用都是彻底 no-op（不写半截命令）；
//! - 未知的对齐字符串忽略，不报错。
//!
//! # 同帧写入必须互相看见
//!
//! 脚本一帧内可能先 `ui_set_size` 再 `ui_set_pos`（或反过来）。旧端 setter 直接
//! 改运行时 `UiElement`，第二次调用看到的是第一次改完的值；这里写操作走命令队列，
//! 所以读「当前值」先看队列里同实体 / 同字段尚未结算的 `SetField`，没有才退回帧首
//! 快照。少了这一步，后一次调用会拿帧首的旧矩形覆盖前一次的结果。
//!
//! 焦点三件套（`ui_focused` / `ui_set_focus` / `ui_clear_focus`）在
//! `ui_focus_api`：写 `ui.interaction.focused`，不需要新组件字段。
//! 图片 `ui_set_image` / `ui_set_image_tint` / `ui_set_fill`、`ui_spawn_*` 仍需要
//! 新的组件字段或命令面，未注册。

use mlua::Lua;
use std::rc::Rc;

use xgc_base::EntityId;
use xgc_world::{Command, Component, UiElement, Value as FieldValue};

use crate::legacy::{srgb_to_linear, Commands, View};
use crate::view::{ScriptView, UiElementView};

pub(crate) fn has_element(view: &ScriptView, id: u64) -> bool {
    view.entity(id).is_some_and(|entity| entity.ui.is_some())
}

/// 命令队列里同实体 / 同组件 / 同字段的最后一条写入。
fn queued(commands: &Commands, id: u64, field: &str) -> Option<FieldValue> {
    let entity = EntityId::from_raw(id);
    commands
        .borrow()
        .iter()
        .rev()
        .find_map(|command| match command {
            Command::SetField {
                entity: target,
                component,
                field: name,
                value,
            } if *target == entity && *component == UiElement::ID && name == field => {
                Some(value.clone())
            }
            _ => None,
        })
}

/// 写一个 UI 字段：队列里已有同字段写入就替换（保持一条命令），否则追加。
fn push(commands: &Commands, id: u64, field: &str, value: FieldValue) {
    let entity = EntityId::from_raw(id);
    let mut queue = commands.borrow_mut();
    for command in queue.iter_mut().rev() {
        if let Command::SetField {
            entity: target,
            component,
            field: name,
            value: slot,
        } = command
        {
            if *target == entity && *component == UiElement::ID && name == field {
                *slot = value;
                return;
            }
        }
    }
    queue.push(Command::SetField {
        entity,
        component: UiElement::ID,
        field: field.to_string(),
        value,
    });
}

fn current_vec2(commands: &Commands, view: &ScriptView, id: u64, field: &str) -> Option<[f32; 2]> {
    if let Some(FieldValue::Vec2(value)) = queued(commands, id, field) {
        return Some([value[0] as f32, value[1] as f32]);
    }
    let ui = element(view, id)?;
    Some(match field {
        "anchor_min" => ui.anchor_min,
        "anchor_max" => ui.anchor_max,
        "offset_min" => ui.offset_min,
        "offset_max" => ui.offset_max,
        _ => return None,
    })
}

pub(crate) fn current_bool(
    commands: &Commands,
    view: &ScriptView,
    id: u64,
    field: &str,
) -> Option<bool> {
    if let Some(FieldValue::Bool(value)) = queued(commands, id, field) {
        return Some(value);
    }
    let ui = element(view, id)?;
    match field {
        "visible" => Some(ui.visible),
        "interactable" => Some(ui.interactable),
        _ => None,
    }
}

fn current_text(commands: &Commands, view: &ScriptView, id: u64) -> Option<String> {
    if let Some(FieldValue::String(value)) = queued(commands, id, "text") {
        return Some(value);
    }
    Some(element(view, id)?.text.clone())
}

pub(crate) fn element(view: &ScriptView, id: u64) -> Option<&UiElementView> {
    view.entity(id)?.ui.as_ref()
}

fn vec2(x: f32, y: f32) -> FieldValue {
    FieldValue::Vec2([f64::from(x), f64::from(y)])
}

fn color(r: f32, g: f32, b: f32, a: f32) -> FieldValue {
    let linear = |channel: f32| f64::from(srgb_to_linear(channel));
    FieldValue::Vec4([linear(r), linear(g), linear(b), f64::from(a)])
}

/// 注册全部 UI 脚本函数。`legacy::register` 里一行调用。
pub fn register(lua: &Lua, commands: &Commands, view: &View) -> Result<(), String> {
    let globals = lua.globals();

    let has_view = Rc::clone(view);
    globals
        .set(
            "ui_has",
            lua.create_function(move |_, (_ctx, id): (mlua::Value, u64)| {
                Ok(has_element(&has_view.borrow(), id))
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let text_commands = Rc::clone(commands);
    let text_view = Rc::clone(view);
    globals
        .set(
            "ui_text",
            lua.create_function(move |_, (_ctx, id): (mlua::Value, u64)| {
                Ok(current_text(&text_commands, &text_view.borrow(), id).unwrap_or_default())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let set_text_commands = Rc::clone(commands);
    let set_text_view = Rc::clone(view);
    globals
        .set(
            "ui_set_text",
            lua.create_function(move |_, (_ctx, id, text): (mlua::Value, u64, String)| {
                if !has_element(&set_text_view.borrow(), id) {
                    return Ok(());
                }
                push(&set_text_commands, id, "text", FieldValue::String(text));
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let size_commands = Rc::clone(commands);
    let size_view = Rc::clone(view);
    globals
        .set(
            "ui_set_size",
            lua.create_function(
                move |_, (_ctx, id, width, height): (mlua::Value, u64, f32, f32)| {
                    let view = size_view.borrow();
                    if !has_element(&view, id) {
                        return Ok(());
                    }
                    let min =
                        current_vec2(&size_commands, &view, id, "offset_min").unwrap_or([0.0; 2]);
                    push(
                        &size_commands,
                        id,
                        "offset_max",
                        vec2(min[0] + width, min[1] + height),
                    );
                    Ok(())
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let pos_commands = Rc::clone(commands);
    let pos_view = Rc::clone(view);
    globals
        .set(
            "ui_set_pos",
            lua.create_function(move |_, (_ctx, id, x, y): (mlua::Value, u64, f32, f32)| {
                let view = pos_view.borrow();
                if !has_element(&view, id) {
                    return Ok(());
                }
                let min = current_vec2(&pos_commands, &view, id, "offset_min").unwrap_or([0.0; 2]);
                let max = current_vec2(&pos_commands, &view, id, "offset_max").unwrap_or([0.0; 2]);
                let size = [max[0] - min[0], max[1] - min[1]];
                push(&pos_commands, id, "offset_min", vec2(x, y));
                push(
                    &pos_commands,
                    id,
                    "offset_max",
                    vec2(x + size[0], y + size[1]),
                );
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let offset_commands = Rc::clone(commands);
    let offset_view = Rc::clone(view);
    globals
        .set(
            "ui_set_offset",
            lua.create_function(
                move |_, (_ctx, id, ox0, oy0, ox1, oy1): (mlua::Value, u64, f32, f32, f32, f32)| {
                    if !has_element(&offset_view.borrow(), id) {
                        return Ok(());
                    }
                    push(&offset_commands, id, "offset_min", vec2(ox0, oy0));
                    push(&offset_commands, id, "offset_max", vec2(ox1, oy1));
                    Ok(())
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let anchor_commands = Rc::clone(commands);
    let anchor_view = Rc::clone(view);
    globals
        .set(
            "ui_set_anchor",
            lua.create_function(
                move |_, (_ctx, id, ax0, ay0, ax1, ay1): (mlua::Value, u64, f32, f32, f32, f32)| {
                    if !has_element(&anchor_view.borrow(), id) {
                        return Ok(());
                    }
                    push(&anchor_commands, id, "anchor_min", vec2(ax0, ay0));
                    push(&anchor_commands, id, "anchor_max", vec2(ax1, ay1));
                    Ok(())
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let rect_commands = Rc::clone(commands);
    let rect_view = Rc::clone(view);
    globals
        .set(
            "ui_set_rect",
            lua.create_function(
                move |_,
                      (_ctx, id, ax0, ay0, ax1, ay1, ox0, oy0, ox1, oy1): (
                    mlua::Value,
                    u64,
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                    f32,
                )| {
                    if !has_element(&rect_view.borrow(), id) {
                        return Ok(());
                    }
                    push(&rect_commands, id, "anchor_min", vec2(ax0, ay0));
                    push(&rect_commands, id, "anchor_max", vec2(ax1, ay1));
                    push(&rect_commands, id, "offset_min", vec2(ox0, oy0));
                    push(&rect_commands, id, "offset_max", vec2(ox1, oy1));
                    Ok(())
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let bg_commands = Rc::clone(commands);
    let bg_view = Rc::clone(view);
    globals
        .set(
            "ui_set_bg_color",
            lua.create_function(
                move |_, (_ctx, id, r, g, b, a): (mlua::Value, u64, f32, f32, f32, f32)| {
                    if !has_element(&bg_view.borrow(), id) {
                        return Ok(());
                    }
                    push(&bg_commands, id, "background", color(r, g, b, a));
                    Ok(())
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let text_color_commands = Rc::clone(commands);
    let text_color_view = Rc::clone(view);
    globals
        .set(
            "ui_set_text_color",
            lua.create_function(
                move |_, (_ctx, id, r, g, b, a): (mlua::Value, u64, f32, f32, f32, f32)| {
                    if !has_element(&text_color_view.borrow(), id) {
                        return Ok(());
                    }
                    push(&text_color_commands, id, "text_color", color(r, g, b, a));
                    Ok(())
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let text_size_commands = Rc::clone(commands);
    let text_size_view = Rc::clone(view);
    globals
        .set(
            "ui_set_text_size",
            lua.create_function(move |_, (_ctx, id, size): (mlua::Value, u64, f32)| {
                if !has_element(&text_size_view.borrow(), id) {
                    return Ok(());
                }
                push(
                    &text_size_commands,
                    id,
                    "text_size",
                    FieldValue::F64(f64::from(size.clamp(1.0, 512.0))),
                );
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let align_commands = Rc::clone(commands);
    let align_view = Rc::clone(view);
    globals
        .set(
            "ui_set_text_align",
            lua.create_function(move |_, (_ctx, id, align): (mlua::Value, u64, String)| {
                if !has_element(&align_view.borrow(), id) {
                    return Ok(());
                }
                let align = match align.to_ascii_lowercase().as_str() {
                    "left" => "left",
                    "center" | "centre" | "middle" => "center",
                    "right" => "right",
                    _ => return Ok(()),
                };
                push(
                    &align_commands,
                    id,
                    "text_align",
                    FieldValue::String(align.to_string()),
                );
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let radius_commands = Rc::clone(commands);
    let radius_view = Rc::clone(view);
    globals
        .set(
            "ui_set_radius",
            lua.create_function(move |_, (_ctx, id, radius): (mlua::Value, u64, f32)| {
                if !has_element(&radius_view.borrow(), id) {
                    return Ok(());
                }
                push(
                    &radius_commands,
                    id,
                    "corner_radius",
                    FieldValue::F64(f64::from(radius.max(0.0))),
                );
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let visible_commands = Rc::clone(commands);
    let visible_view = Rc::clone(view);
    globals
        .set(
            "ui_set_visible",
            lua.create_function(move |_, (_ctx, id, visible): (mlua::Value, u64, bool)| {
                if !has_element(&visible_view.borrow(), id) {
                    return Ok(());
                }
                push(&visible_commands, id, "visible", FieldValue::Bool(visible));
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let is_visible_commands = Rc::clone(commands);
    let is_visible_view = Rc::clone(view);
    globals
        .set(
            "ui_visible",
            lua.create_function(move |_, (_ctx, id): (mlua::Value, u64)| {
                Ok(current_bool(
                    &is_visible_commands,
                    &is_visible_view.borrow(),
                    id,
                    "visible",
                )
                .unwrap_or(false))
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let enabled_commands = Rc::clone(commands);
    let enabled_view = Rc::clone(view);
    globals
        .set(
            "ui_set_enabled",
            lua.create_function(move |_, (_ctx, id, enabled): (mlua::Value, u64, bool)| {
                if !has_element(&enabled_view.borrow(), id) {
                    return Ok(());
                }
                push(
                    &enabled_commands,
                    id,
                    "interactable",
                    FieldValue::Bool(enabled),
                );
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let is_enabled_commands = Rc::clone(commands);
    let is_enabled_view = Rc::clone(view);
    globals
        .set(
            "ui_enabled",
            lua.create_function(move |_, (_ctx, id): (mlua::Value, u64)| {
                Ok(current_bool(
                    &is_enabled_commands,
                    &is_enabled_view.borrow(),
                    id,
                    "interactable",
                )
                .unwrap_or(false))
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let click_view = Rc::clone(view);
    globals
        .set(
            "ui_clicked",
            lua.create_function(move |_, (_ctx, id): (mlua::Value, u64)| {
                Ok(click_view
                    .borrow()
                    .entity(id)
                    .is_some_and(|entity| entity.clicked))
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let hover_view = Rc::clone(view);
    globals
        .set(
            "ui_hovered",
            lua.create_function(move |_, (_ctx, id): (mlua::Value, u64)| {
                Ok(hover_view
                    .borrow()
                    .entity(id)
                    .is_some_and(|entity| entity.hovered))
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    crate::ui_focus_api::register(lua, commands, view)?;

    Ok(())
}
