//! 旧端 `ui_spawn_*`：运行时创建 UI 实体。
//!
//! 旧端 `ui_ctx::ui_spawn` 直接往场景里塞实体、再发一条 `EntitySpawned` 命令；
//! XGC 没有"直接塞场景"的入口，走 `Command::Spawn`（帧末统一结算，与其它脚本
//! 写操作同一事务）。代价是"同帧看得见"要自己补：实体先记进
//! `ScriptView::pending_spawns`，于是同一帧接着调 `ui_has` / setter / `ui_set_focus`
//! 都能按 id 找到它；命令结算后下一帧从世界重建快照，pending 清空。
//!
//! ID 用 `WorldData::next_id` 派生的游标分配（旧端 `EntityId::new()` 同语义：
//! 单调递增、不复用）。四个函数返回 `id as i64`，失败返回 0（旧端 Lua 绑定同
//! 约定）。
//!
//! 组件集合固定六件、无父节点（旧端四函数传 `parent = None`）：
//! `core.transform` / `core.name` / `core.global_transform` / `ui.element` /
//! `ui.layout` / `ui.interaction`。默认值照旧端四个构造器逐字段抄：
//! `UiElement::text` 透明底 + 白字；`panel` 深灰底 `rgba8(28,30,36,216)` + 内边距 6；
//! `button` 蓝底 `rgba8(38,92,168,255)` + 1px 亮边 + 圆角 6 + 内边距 6；
//! `image` 透明底、`keep_aspect = false`（=`image_mode = "stretch"`）。
//! 注意 XGC 的 `ui.element.interactable` schema 缺省是 true，而旧端 panel / text /
//! image 构造器都是 false，所以这里必须显式写回。

use std::rc::Rc;

use mlua::Lua;
use xgc_base::EntityId;
use xgc_world::{Command, ComponentData, EntityData, Value as FieldValue};

use crate::legacy::{srgb_to_linear, Commands, View};
use crate::view::{EntityView, UiElementView};

/// 旧端 `ui_ctx.rs` 里交给 `ui_spawn` 的实体名（不是显示文本，是 `core.name`）。
const LABEL_NAME: &str = "Label";
const BUTTON_NAME: &str = "Button";
const PANEL_NAME: &str = "Panel";
const IMAGE_NAME: &str = "Image";

fn linear(channel: f32) -> f64 {
    f64::from(srgb_to_linear(channel))
}

/// 旧端 `Color::from_rgba8`：rgb 先归一化再 sRGB→线性，alpha 直接除 255。
fn color8(rgb: [f32; 3], alpha: f32) -> FieldValue {
    FieldValue::Vec4([
        linear(rgb[0] / 255.0),
        linear(rgb[1] / 255.0),
        linear(rgb[2] / 255.0),
        f64::from(alpha / 255.0),
    ])
}

fn vec2(value: [f32; 2]) -> FieldValue {
    FieldValue::Vec2([f64::from(value[0]), f64::from(value[1])])
}

/// 内建组件的默认值集合（`component_data::default_component` 与脚本挂组件共用一份）。
fn component(name: &str) -> ComponentData {
    crate::component_data::default_component(name).expect("内置组件名必须注册")
}

fn set_field(data: &mut ComponentData, field: &str, value: FieldValue) {
    if let Some(slot) = data.fields.iter_mut().find(|(name, _)| name == field) {
        slot.1 = value;
        return;
    }
    data.fields.push((field.to_string(), value));
}

/// 旧端 `UiElement::at([0,0], x, y, w, h)`：anchor 固定左上，offset 存左上 / 右下。
fn base_element(kind: &str, rect: [f32; 4]) -> ComponentData {
    let mut data = component("ui.element");
    set_field(&mut data, "kind", FieldValue::String(kind.to_string()));
    set_field(&mut data, "anchor_min", vec2([0.0, 0.0]));
    set_field(&mut data, "anchor_max", vec2([0.0, 0.0]));
    set_field(&mut data, "offset_min", vec2([rect[0], rect[1]]));
    set_field(
        &mut data,
        "offset_max",
        vec2([rect[0] + rect[2], rect[1] + rect[3]]),
    );
    // 旧端 text / panel / image 的 interactable 都从 panel 构造器继承 false。
    set_field(&mut data, "interactable", FieldValue::Bool(false));
    data
}

fn element_view(kind: &str, rect: [f32; 4], text: &str, interactable: bool) -> UiElementView {
    UiElementView {
        kind: kind.to_string(),
        anchor_min: [0.0, 0.0],
        anchor_max: [0.0, 0.0],
        offset_min: [rect[0], rect[1]],
        offset_max: [rect[0] + rect[2], rect[1] + rect[3]],
        text: text.to_string(),
        visible: true,
        interactable,
    }
}

/// 旧端 `UiElement::text` + `ui_spawn_label`：宽度按"一个字符一个字号宽"估，
/// 字高取 `size * 1.4`。`size` 只有写进 `text_size` 时钳到 1..=512，估算宽度 /
/// 高度用的是原始入参（与旧端一致）。
fn label(text: &str, x: f32, y: f32, size: f32) -> (ComponentData, UiElementView) {
    let width = (text.chars().count() as f32 * size).max(size);
    let rect = [x, y, width, size * 1.4];
    let mut data = base_element("text", rect);
    set_field(&mut data, "text", FieldValue::String(text.to_string()));
    set_field(
        &mut data,
        "text_size",
        FieldValue::F64(f64::from(size.clamp(1.0, 512.0))),
    );
    (data, element_view("text", rect, text, false))
}

/// 旧端 `UiElement::button`：蓝底 + 1px 亮边 + 圆角 6 + 内边距 6，文字居中。
fn button(text: &str, x: f32, y: f32, width: f32, height: f32) -> (ComponentData, UiElementView) {
    let rect = [x, y, width, height];
    let mut data = base_element("button", rect);
    set_field(&mut data, "text", FieldValue::String(text.to_string()));
    set_field(&mut data, "background", color8([38.0, 92.0, 168.0], 255.0));
    set_field(
        &mut data,
        "border_color",
        color8([96.0, 148.0, 214.0], 255.0),
    );
    set_field(&mut data, "border_width", FieldValue::F64(1.0));
    set_field(&mut data, "corner_radius", FieldValue::F64(6.0));
    set_field(&mut data, "padding", FieldValue::F64(6.0));
    set_field(
        &mut data,
        "text_align",
        FieldValue::String("center".to_string()),
    );
    set_field(
        &mut data,
        "text_valign",
        FieldValue::String("middle".to_string()),
    );
    set_field(&mut data, "interactable", FieldValue::Bool(true));
    (data, element_view("button", rect, text, true))
}

/// 旧端 `UiElement::panel`：深灰底 + 内边距 6。
fn panel(x: f32, y: f32, width: f32, height: f32) -> (ComponentData, UiElementView) {
    let rect = [x, y, width, height];
    let mut data = base_element("panel", rect);
    set_field(&mut data, "background", color8([28.0, 30.0, 36.0], 216.0));
    set_field(&mut data, "padding", FieldValue::F64(6.0));
    (data, element_view("panel", rect, "", false))
}

/// 旧端 `UiElement::image`：透明底、`keep_aspect = false`（XGC 的 `"stretch"`）。
fn image(path: &str, x: f32, y: f32, width: f32, height: f32) -> (ComponentData, UiElementView) {
    let rect = [x, y, width, height];
    let mut data = base_element("image", rect);
    set_field(
        &mut data,
        "image_path",
        FieldValue::String(path.to_string()),
    );
    set_field(
        &mut data,
        "image_mode",
        FieldValue::String("stretch".to_string()),
    );
    (data, element_view("image", rect, "", false))
}

/// 生成实体并下发 `Command::Spawn`；游标耗尽或撞号时不修改帧内视图。
fn spawn_entity(
    view: &View,
    commands: &Commands,
    name: &str,
    element: (ComponentData, UiElementView),
) -> Option<u64> {
    let (ui, ui_view) = element;
    let id = {
        let mut view = view.borrow_mut();
        let id = view.spawn_cursor;
        EntityId::from_raw(id).validate().ok()?;
        if view.entity(id).is_some() {
            return None;
        }
        view.spawn_cursor = id + 1;
        // 同帧可见：命令还没结算，setter / 焦点按 id 查这份快照。
        view.pending_spawns.insert(
            id,
            EntityView {
                id,
                name: name.to_string(),
                position: Some([0.0; 3]),
                rotation: [0.0, 0.0, 0.0, 1.0],
                ui: Some(ui_view),
                has_interaction: true,
                ..Default::default()
            },
        );
        id
    };

    let mut data = EntityData::new(EntityId::from_raw(id));
    data.components.push(component("core.transform"));
    let mut name_component = component("core.name");
    set_field(
        &mut name_component,
        "value",
        FieldValue::String(name.to_string()),
    );
    data.components.push(name_component);
    data.components.push(component("core.global_transform"));
    data.components.push(ui);
    data.components.push(component("ui.layout"));
    data.components.push(component("ui.interaction"));
    commands.borrow_mut().push(Command::Spawn { data });
    Some(id)
}

fn spawn_id(
    view: &View,
    commands: &Commands,
    name: &str,
    element: (ComponentData, UiElementView),
) -> i64 {
    match spawn_entity(view, commands, name, element) {
        Some(id) => id as i64,
        None => 0,
    }
}

/// 注册四个运行时创建函数。`ui_api::register` 末尾一行调用。
pub fn register(lua: &Lua, commands: &Commands, view: &View) -> Result<(), String> {
    let globals = lua.globals();

    let label_view = Rc::clone(view);
    let label_commands = Rc::clone(commands);
    globals
        .set(
            "ui_spawn_label",
            lua.create_function(
                move |_, (_ctx, text, x, y, size): (mlua::Value, String, f32, f32, f32)| {
                    Ok(spawn_id(
                        &label_view,
                        &label_commands,
                        LABEL_NAME,
                        label(&text, x, y, size),
                    ))
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let button_view = Rc::clone(view);
    let button_commands = Rc::clone(commands);
    globals
        .set(
            "ui_spawn_button",
            lua.create_function(
                move |_, (_ctx, text, x, y, w, h): (mlua::Value, String, f32, f32, f32, f32)| {
                    Ok(spawn_id(
                        &button_view,
                        &button_commands,
                        BUTTON_NAME,
                        button(&text, x, y, w, h),
                    ))
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let panel_view = Rc::clone(view);
    let panel_commands = Rc::clone(commands);
    globals
        .set(
            "ui_spawn_panel",
            lua.create_function(
                move |_, (_ctx, x, y, w, h): (mlua::Value, f32, f32, f32, f32)| {
                    Ok(spawn_id(
                        &panel_view,
                        &panel_commands,
                        PANEL_NAME,
                        panel(x, y, w, h),
                    ))
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let image_view = Rc::clone(view);
    let image_commands = Rc::clone(commands);
    globals
        .set(
            "ui_spawn_image",
            lua.create_function(
                move |_, (_ctx, path, x, y, w, h): (mlua::Value, String, f32, f32, f32, f32)| {
                    Ok(spawn_id(
                        &image_view,
                        &image_commands,
                        IMAGE_NAME,
                        image(&path, x, y, w, h),
                    ))
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    Ok(())
}
