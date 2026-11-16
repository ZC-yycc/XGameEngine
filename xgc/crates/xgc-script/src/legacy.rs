//! 旧引擎脚本 API 的最小兼容层：`update(ctx)` + `rot/set_rot/delta` 等。
//!
//! 只读数据来自每帧快照 `ScriptView`，写操作一律进命令队列。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use mlua::{Lua, Table};
use xgc_base::EntityId;
use xgc_world::{
    AnimationState, Command, Component, Transform, UiElement, Value as FieldValue, Velocity,
};

#[derive(Clone, Debug, Default)]
pub struct EntityView {
    pub id: u64,
    pub name: String,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub velocity: [f32; 3],
    pub anim_parameters: String,
    pub clicked: bool,
    pub hovered: bool,
    pub pressed: bool,
}
#[derive(Clone, Debug, Default)]
pub struct ScriptView {
    pub delta: f32,
    pub time: f32,
    pub entities: Vec<EntityView>,
    pub actions: BTreeMap<String, bool>,
}

impl ScriptView {
    pub fn entity(&self, id: u64) -> Option<&EntityView> {
        self.entities.iter().find(|entity| entity.id == id)
    }

    pub fn by_name(&self, name: &str) -> Option<&EntityView> {
        self.entities.iter().find(|entity| entity.name == name)
    }
}

type Commands = Rc<RefCell<Vec<Command>>>;
type View = Rc<RefCell<ScriptView>>;

pub fn register(lua: &Lua, commands: &Commands, view: &View, self_id: u64) -> Result<(), String> {
    let globals = lua.globals();
    let delta_view = Rc::clone(view);
    globals
        .set(
            "delta",
            lua.create_function(move |_, _ctx: mlua::Value| Ok(delta_view.borrow().delta))
                .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    let time_view = Rc::clone(view);
    globals
        .set(
            "time",
            lua.create_function(move |_, _ctx: mlua::Value| Ok(time_view.borrow().time))
                .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    globals
        .set(
            "self_id",
            lua.create_function(move |_, _ctx: mlua::Value| Ok(self_id))
                .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let id_view = Rc::clone(view);
    globals
        .set(
            "entity_id",
            lua.create_function(move |_, (_ctx, name): (mlua::Value, String)| {
                Ok(id_view
                    .borrow()
                    .by_name(&name)
                    .map(|entity| entity.id)
                    .unwrap_or(0))
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let pos_view = Rc::clone(view);
    globals
        .set(
            "entity_pos",
            lua.create_function(move |lua, (_ctx, id): (mlua::Value, u64)| {
                let view = pos_view.borrow();
                let entity = view
                    .entity(id)
                    .ok_or_else(|| mlua::Error::RuntimeError(format!("实体 {id} 不存在")))?;
                vector_table(lua, entity.position)
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let set_pos_commands = Rc::clone(commands);
    globals
        .set(
            "set_pos",
            lua.create_function(move |_, (_ctx, x, y, z): (mlua::Value, f32, f32, f32)| {
                push_set(
                    &set_pos_commands,
                    self_id,
                    Transform::ID,
                    "position",
                    FieldValue::Vec3([f64::from(x), f64::from(y), f64::from(z)]),
                );
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let rot_view = Rc::clone(view);
    globals
        .set(
            "rot",
            lua.create_function(move |lua, _ctx: mlua::Value| {
                let view = rot_view.borrow();
                let rotation = view
                    .entity(self_id)
                    .map(|entity| entity.rotation)
                    .unwrap_or([0.0, 0.0, 0.0, 1.0]);
                let yaw = 2.0 * rotation[1].atan2(rotation[3]).to_degrees();
                vector_table(lua, [0.0, yaw, 0.0])
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let set_rot_commands = Rc::clone(commands);
    globals
        .set(
            "set_rot",
            lua.create_function(move |_, (_ctx, x, y, z): (mlua::Value, f32, f32, f32)| {
                let rotation = euler_to_quat([x, y, z]);
                push_set(
                    &set_rot_commands,
                    self_id,
                    Transform::ID,
                    "rotation",
                    FieldValue::Quat([
                        f64::from(rotation[0]),
                        f64::from(rotation[1]),
                        f64::from(rotation[2]),
                        f64::from(rotation[3]),
                    ]),
                );
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let look_view = Rc::clone(view);
    let look_commands = Rc::clone(commands);
    globals
        .set(
            "look_at",
            lua.create_function(move |_, (_ctx, tx, ty, tz): (mlua::Value, f32, f32, f32)| {
                let view = look_view.borrow();
                let position = view
                    .entity(self_id)
                    .map(|entity| entity.position)
                    .unwrap_or([0.0; 3]);
                let direction = [tx - position[0], ty - position[1], tz - position[2]];
                let length = (direction[0] * direction[0]
                    + direction[1] * direction[1]
                    + direction[2] * direction[2])
                    .sqrt();
                if length <= 1e-6 {
                    return Ok(());
                }
                let pitch = (direction[1] / length).asin().to_degrees();
                let yaw = (-direction[0]).atan2(-direction[2]).to_degrees();
                let rotation = euler_to_quat([0.0, yaw, pitch]);
                push_set(
                    &look_commands,
                    self_id,
                    Transform::ID,
                    "rotation",
                    FieldValue::Quat([
                        f64::from(rotation[0]),
                        f64::from(rotation[1]),
                        f64::from(rotation[2]),
                        f64::from(rotation[3]),
                    ]),
                );
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let velocity_view = Rc::clone(view);
    globals
        .set(
            "velocity",
            lua.create_function(move |lua, (_ctx, id): (mlua::Value, u64)| {
                let view = velocity_view.borrow();
                let entity = view
                    .entity(id)
                    .ok_or_else(|| mlua::Error::RuntimeError(format!("实体 {id} 不存在")))?;
                vector_table(lua, entity.velocity)
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let set_velocity_commands = Rc::clone(commands);
    globals
        .set(
            "set_velocity",
            lua.create_function(
                move |_, (_ctx, id, x, y, z): (mlua::Value, u64, f32, f32, f32)| {
                    push_set(
                        &set_velocity_commands,
                        id,
                        Velocity::ID,
                        "linear",
                        FieldValue::Vec3([f64::from(x), f64::from(y), f64::from(z)]),
                    );
                    Ok(())
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let anim_commands = Rc::clone(commands);
    let anim_view = Rc::clone(view);
    globals
        .set(
            "set_anim_param",
            lua.create_function(move |_, (_ctx, name, value): (mlua::Value, String, f32)| {
                let current = anim_view
                    .borrow()
                    .entity(self_id)
                    .map(|entity| entity.anim_parameters.clone())
                    .unwrap_or_else(|| "{}".to_string());
                let mut parameters: BTreeMap<String, f32> =
                    ron::from_str(&current).unwrap_or_default();
                parameters.insert(name, value);
                let text = ron::ser::to_string(&parameters).unwrap_or_else(|_| "{}".to_string());
                push_set(
                    &anim_commands,
                    self_id,
                    AnimationState::ID,
                    "parameters",
                    FieldValue::String(text),
                );
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    // 旧脚本里出现但当前无实现的调用：静默 no-op，避免迁移脚本直接报错。
    for name in [
        "set_anim_additive_weight",
        "set_anim_layer_param",
        "set_anim_layer_weight",
        "play_sound",
    ] {
        let function = lua
            .create_function(|_, _: mlua::MultiValue| Ok(()))
            .map_err(|err| err.to_string())?;
        globals.set(name, function).map_err(|err| err.to_string())?;
    }
    let ui_text_commands = Rc::clone(commands);
    globals
        .set(
            "ui_set_text",
            lua.create_function(move |_, (_ctx, id, text): (mlua::Value, u64, String)| {
                push_set(
                    &ui_text_commands,
                    id,
                    UiElement::ID,
                    "text",
                    FieldValue::String(text),
                );
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    let ui_size_commands = Rc::clone(commands);
    globals
        .set(
            "ui_set_size",
            lua.create_function(
                move |_, (_ctx, id, width, height): (mlua::Value, u64, f32, f32)| {
                    push_set(
                        &ui_size_commands,
                        id,
                        UiElement::ID,
                        "offset_max",
                        FieldValue::Vec2([f64::from(width), f64::from(height)]),
                    );
                    Ok(())
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    let ui_color_commands = Rc::clone(commands);
    globals
        .set(
            "ui_set_bg_color",
            lua.create_function(
                move |_, (_ctx, id, r, g, b, a): (mlua::Value, u64, f32, f32, f32, f32)| {
                    push_set(
                        &ui_color_commands,
                        id,
                        UiElement::ID,
                        "background",
                        FieldValue::Vec4([f64::from(r), f64::from(g), f64::from(b), f64::from(a)]),
                    );
                    Ok(())
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    let ui_visible_commands = Rc::clone(commands);
    globals
        .set(
            "ui_set_visible",
            lua.create_function(move |_, (_ctx, id, visible): (mlua::Value, u64, bool)| {
                push_set(
                    &ui_visible_commands,
                    id,
                    UiElement::ID,
                    "visible",
                    FieldValue::Bool(visible),
                );
                Ok(())
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
    let action_view = Rc::clone(view);
    globals
        .set(
            "action_pressed",
            lua.create_function(move |_, (_ctx, name): (mlua::Value, String)| {
                Ok(action_view
                    .borrow()
                    .actions
                    .get(&name)
                    .copied()
                    .unwrap_or(false))
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn vector_table(lua: &Lua, value: [f32; 3]) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("x", value[0])?;
    table.set("y", value[1])?;
    table.set("z", value[2])?;
    Ok(table)
}

fn push_set(
    commands: &Commands,
    id: u64,
    component: xgc_world::ComponentId,
    field: &str,
    value: FieldValue,
) {
    commands.borrow_mut().push(Command::SetField {
        entity: EntityId::from_raw(id),
        component,
        field: field.to_string(),
        value,
    });
}

/// YXZ 欧拉角（度）→ 四元数。
pub fn euler_to_quat(degrees: [f32; 3]) -> [f32; 4] {
    let [x, y, z] = [
        (degrees[0] * 0.5).to_radians(),
        (degrees[1] * 0.5).to_radians(),
        (degrees[2] * 0.5).to_radians(),
    ];
    let (sx, cx) = x.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sz, cz) = z.sin_cos();
    [
        cy * sx * cz + sy * cx * sz,
        sy * cx * cz - cy * sx * sz,
        cy * cx * sz - sy * sx * cz,
        cy * cx * cz + sy * sx * sz,
    ]
}
