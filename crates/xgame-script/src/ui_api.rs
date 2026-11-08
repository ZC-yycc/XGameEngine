//! UI 脚本 API 的 Lua 转发壳。
//!
//! 每个函数一行，真相在 [`crate::GameCtx`] 的 `ui_*` 方法（`ui_ctx.rs`）。
//! 参数一律固定个数：写错参数个数的脚本当场报错，而不是静默把 `nil`
//! 当成 0 传进引擎（那会得到"界面莫名其妙跑到左上角"这类难查的 bug）。
//!
//! # 命名与语义速查
//!
//! ```lua
//! local id = ui_spawn_button(ctx, "开始游戏", 40, 40, 200, 48)
//! ui_set_bg_color(ctx, id, 0.15, 0.5, 0.9, 1.0)   -- sRGB 0..1
//! if ui_clicked(ctx, id) then log(ctx, "clicked") end
//! ```
//!
//! 完整清单见 `Docs/脚本API.md` 的「UI」一节。

use mlua::{Lua, Table};

use crate::lua_api::{id_of, LuaCtx};

/// 注册全部 `ui_*` 全局函数。
pub(crate) fn register_ui(lua: &Lua, globals: &Table) -> mlua::Result<()> {
    // -- 内容 --

    globals.set(
        "ui_set_text",
        lua.create_function(|_, (ctx, id, text): (LuaCtx, i64, String)| {
            ctx.game().ui_set_text(id_of(id), &text);
            Ok(())
        })?,
    )?;
    globals.set(
        "ui_text",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| Ok(ctx.game().ui_text(id_of(id))))?,
    )?;
    globals.set(
        "ui_set_text_size",
        lua.create_function(|_, (ctx, id, size): (LuaCtx, i64, f32)| {
            ctx.game().ui_set_text_size(id_of(id), size);
            Ok(())
        })?,
    )?;
    globals.set(
        "ui_set_text_color",
        lua.create_function(
            |_, (ctx, id, r, g, b, a): (LuaCtx, i64, f32, f32, f32, f32)| {
                ctx.game().ui_set_text_color(id_of(id), r, g, b, a);
                Ok(())
            },
        )?,
    )?;
    globals.set(
        "ui_set_text_align",
        lua.create_function(|_, (ctx, id, align): (LuaCtx, i64, String)| {
            ctx.game().ui_set_text_align(id_of(id), &align);
            Ok(())
        })?,
    )?;

    // -- 显隐 / 交互 --

    globals.set(
        "ui_set_visible",
        lua.create_function(|_, (ctx, id, visible): (LuaCtx, i64, bool)| {
            ctx.game().ui_set_visible(id_of(id), visible);
            Ok(())
        })?,
    )?;
    globals.set(
        "ui_visible",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| Ok(ctx.game().ui_visible(id_of(id))))?,
    )?;
    globals.set(
        "ui_set_enabled",
        lua.create_function(|_, (ctx, id, enabled): (LuaCtx, i64, bool)| {
            ctx.game().ui_set_enabled(id_of(id), enabled);
            Ok(())
        })?,
    )?;
    globals.set(
        "ui_enabled",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| Ok(ctx.game().ui_enabled(id_of(id))))?,
    )?;
    globals.set(
        "ui_has",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| Ok(ctx.game().ui_has(id_of(id))))?,
    )?;
    globals.set(
        "ui_clicked",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| Ok(ctx.game().ui_clicked(id_of(id))))?,
    )?;
    globals.set(
        "ui_hovered",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| Ok(ctx.game().ui_hovered(id_of(id))))?,
    )?;
    globals.set(
        "ui_focused",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| Ok(ctx.game().ui_focused(id_of(id))))?,
    )?;
    globals.set(
        "ui_set_focus",
        lua.create_function(|_, (ctx, id): (LuaCtx, i64)| {
            ctx.game().ui_set_focus(id_of(id));
            Ok(())
        })?,
    )?;
    globals.set(
        "ui_clear_focus",
        lua.create_function(|_, ctx: LuaCtx| {
            ctx.game().ui_clear_focus();
            Ok(())
        })?,
    )?;

    // -- 外观 --

    globals.set(
        "ui_set_bg_color",
        lua.create_function(
            |_, (ctx, id, r, g, b, a): (LuaCtx, i64, f32, f32, f32, f32)| {
                ctx.game().ui_set_bg_color(id_of(id), r, g, b, a);
                Ok(())
            },
        )?,
    )?;
    globals.set(
        "ui_set_radius",
        lua.create_function(|_, (ctx, id, radius): (LuaCtx, i64, f32)| {
            ctx.game().ui_set_radius(id_of(id), radius);
            Ok(())
        })?,
    )?;
    globals.set(
        "ui_set_image",
        lua.create_function(|_, (ctx, id, path): (LuaCtx, i64, String)| {
            ctx.game().ui_set_image(id_of(id), &path);
            Ok(())
        })?,
    )?;
    globals.set(
        "ui_set_image_tint",
        lua.create_function(
            |_, (ctx, id, r, g, b, a): (LuaCtx, i64, f32, f32, f32, f32)| {
                ctx.game().ui_set_image_tint(id_of(id), r, g, b, a);
                Ok(())
            },
        )?,
    )?;
    globals.set(
        "ui_set_fill",
        lua.create_function(|_, (ctx, id, fill): (LuaCtx, i64, f32)| {
            ctx.game().ui_set_fill(id_of(id), fill);
            Ok(())
        })?,
    )?;

    // -- 布局 --

    globals.set(
        "ui_set_rect",
        lua.create_function(
            |_,
             (ctx, id, ax0, ay0, ax1, ay1, ox0, oy0, ox1, oy1): (
                LuaCtx,
                i64,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
            )| {
                ctx.game()
                    .ui_set_rect(id_of(id), [ax0, ay0], [ax1, ay1], [ox0, oy0], [ox1, oy1]);
                Ok(())
            },
        )?,
    )?;
    globals.set(
        "ui_set_anchor",
        lua.create_function(
            |_, (ctx, id, ax0, ay0, ax1, ay1): (LuaCtx, i64, f32, f32, f32, f32)| {
                ctx.game().ui_set_anchor(id_of(id), [ax0, ay0], [ax1, ay1]);
                Ok(())
            },
        )?,
    )?;
    globals.set(
        "ui_set_offset",
        lua.create_function(
            |_, (ctx, id, ox0, oy0, ox1, oy1): (LuaCtx, i64, f32, f32, f32, f32)| {
                ctx.game().ui_set_offset(id_of(id), [ox0, oy0], [ox1, oy1]);
                Ok(())
            },
        )?,
    )?;
    globals.set(
        "ui_set_size",
        lua.create_function(|_, (ctx, id, w, h): (LuaCtx, i64, f32, f32)| {
            ctx.game().ui_set_size(id_of(id), w, h);
            Ok(())
        })?,
    )?;
    globals.set(
        "ui_set_pos",
        lua.create_function(|_, (ctx, id, x, y): (LuaCtx, i64, f32, f32)| {
            ctx.game().ui_set_pos(id_of(id), x, y);
            Ok(())
        })?,
    )?;

    // -- 运行时创建（返回新实体 id，0 = 失败）--

    globals.set(
        "ui_spawn_label",
        lua.create_function(
            |_, (ctx, text, x, y, size): (LuaCtx, String, f32, f32, f32)| {
                let id = ctx.game().ui_spawn_label(&text, x, y, size, None);
                Ok(id.raw() as i64)
            },
        )?,
    )?;
    globals.set(
        "ui_spawn_button",
        lua.create_function(
            |_, (ctx, text, x, y, w, h): (LuaCtx, String, f32, f32, f32, f32)| {
                let id = ctx.game().ui_spawn_button(&text, x, y, w, h, None);
                Ok(id.raw() as i64)
            },
        )?,
    )?;
    globals.set(
        "ui_spawn_panel",
        lua.create_function(|_, (ctx, x, y, w, h): (LuaCtx, f32, f32, f32, f32)| {
            let id = ctx.game().ui_spawn_panel(x, y, w, h, None);
            Ok(id.raw() as i64)
        })?,
    )?;
    globals.set(
        "ui_spawn_image",
        lua.create_function(
            |_, (ctx, path, x, y, w, h): (LuaCtx, String, f32, f32, f32, f32)| {
                let id = ctx.game().ui_spawn_image(&path, x, y, w, h, None);
                Ok(id.raw() as i64)
            },
        )?,
    )?;
    Ok(())
}
