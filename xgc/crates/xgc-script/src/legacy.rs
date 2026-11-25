//! 旧引擎脚本 API 的最小兼容层：`update(ctx)` + `rot/set_rot/delta` 等。
//!
//! 只读数据来自每帧快照 `ScriptView`，写操作一律进命令队列。

use std::cell::RefCell;
use std::rc::Rc;

use mlua::{Lua, Table};
use xgc_base::EntityId;
use xgc_world::{
    AnimationLayers, AnimationState, Animator, Command, Component, Transform, Value as FieldValue,
    Velocity,
};

/// 脚本 API 的颜色参数是 **sRGB 0..1**（美术 / CSS 的直觉：`0.5` 就是中灰），
/// UI 组件里存的是**线性**值。这一处是唯一入口转换点，与旧引擎
/// `xgame-script/src/ui_ctx.rs` 的 `color_srgb` 同一套公式与阈值。
///
/// 少了这一步，脚本给的颜色会被当成线性值再经 sRGB 目标编码一次：
/// HUD 血条 `(0.86, 0.33, 0.28)` 会从旧端的 `#DC5347` 变成 `#EF9A90`。
pub(crate) fn srgb_to_linear(channel: f32) -> f32 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

pub use crate::view::{EntityView, ScriptView};

pub(crate) type Commands = Rc<RefCell<Vec<Command>>>;
pub(crate) type View = Rc<RefCell<ScriptView>>;

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
                // `euler_to_quat` 的分量序是 [绕 X, 绕 Y, 绕 Z]，欧拉顺序 YXZ。
                // 俯仰必须放第一个分量：放第三个会变成绕自身视轴的**滚转**
                // （相机会「歪头」而不是低头，朝向也不指向目标）。
                let rotation = euler_to_quat([pitch, yaw, 0.0]);
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
                if !value.is_finite() {
                    return Ok(());
                }
                let mut view = anim_view.borrow_mut();
                let Some(parameters) = view
                    .entity_mut(self_id)
                    .and_then(|entity| entity.anim_parameters.as_mut())
                else {
                    return Ok(());
                };
                // 事件钩子运行前宿主会取走 update 的命令。把待结算的值留在本帧视图，
                // 后续钩子写新参数时才不会用帧首快照覆盖前一笔。
                let FieldValue::Map(entries) = parameters else {
                    return Ok(());
                };
                entries.insert(name, FieldValue::F64(f64::from(value)));
                push_set(
                    &anim_commands,
                    self_id,
                    AnimationState::ID,
                    "parameters",
                    parameters.clone(),
                );
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let additive_commands = Rc::clone(commands);
    globals
        .set(
            "set_anim_additive_weight",
            lua.create_function(move |_, (_ctx, weight): (mlua::Value, f32)| {
                // 旧端把权重钳在 [0,1]；NaN / ±inf 直接忽略，不许一次坏值污染姿势。
                let weight = if weight.is_finite() {
                    weight.clamp(0.0, 1.0)
                } else {
                    return Ok(());
                };
                push_set(
                    &additive_commands,
                    self_id,
                    Animator::ID,
                    "additive_weight",
                    FieldValue::F64(f64::from(weight)),
                );
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    // 旧脚本里出现但当前无实现的调用：静默 no-op，避免迁移脚本直接报错。
    let function = lua
        .create_function(|_, _: mlua::MultiValue| Ok(()))
        .map_err(|err| err.to_string())?;
    globals
        .set("play_sound", function)
        .map_err(|err| err.to_string())?;

    // 混合层：脚本读当前层栈 → 改权重 / 参数 → 写回组件。命令在宿主阶段结算，
    // `animation_layers` 系统排在宿主之后，所以本帧写的权重当帧生效。
    let layer_weight_commands = Rc::clone(commands);
    let layer_weight_view = Rc::clone(view);
    globals
        .set(
            "set_anim_layer_weight",
            lua.create_function(
                move |_, (_ctx, layer, weight): (mlua::Value, String, f32)| {
                    if !weight.is_finite() {
                        return Ok(());
                    }
                    // 同帧先 py 一次「读 → 改 → 写」：`current_layers_text` 会看到
                    // 本帧已排队但未结算的写入，权重与参数依次累积。
                    let current = current_layers_text(
                        &layer_weight_commands,
                        &layer_weight_view.borrow(),
                        self_id,
                    );
                    let Some(current) = current else {
                        return Ok(());
                    };
                    let Ok(mut parsed) = xgc_world::parse_layers(&current) else {
                        return Ok(());
                    };
                    let Some(target) = xgc_world::find_layer(&mut parsed, &layer) else {
                        return Ok(());
                    };
                    target.weight = weight.clamp(0.0, 1.0);
                    push_layers_text(
                        &layer_weight_commands,
                        self_id,
                        xgc_world::layers_to_ron(&parsed),
                    );
                    Ok(())
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let layer_param_commands = Rc::clone(commands);
    let layer_param_view = Rc::clone(view);
    globals
        .set(
            "set_anim_layer_param",
            lua.create_function(
                move |_, (_ctx, layer, name, value): (mlua::Value, String, String, f32)| {
                    if !value.is_finite() {
                        return Ok(());
                    }
                    let current = current_layers_text(
                        &layer_param_commands,
                        &layer_param_view.borrow(),
                        self_id,
                    );
                    let Some(current) = current else {
                        return Ok(());
                    };
                    let Ok(mut parsed) = xgc_world::parse_layers(&current) else {
                        return Ok(());
                    };
                    let Some(target) = xgc_world::find_layer(&mut parsed, &layer) else {
                        return Ok(());
                    };
                    // 旧端只对带机器的层写参数（层机器的转移条件读它）。
                    if target.machine.is_none() {
                        return Ok(());
                    }
                    xgc_world::set_layer_parameter(target, &name, value);
                    push_layers_text(
                        &layer_param_commands,
                        self_id,
                        xgc_world::layers_to_ron(&parsed),
                    );
                    Ok(())
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    crate::ui_api::register(lua, commands, view)?;
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

    register_script_state(lua, view, self_id)?;
    crate::character_api::register(lua, commands, view, self_id)?;
    Ok(())
}

/// 脚本私有状态表：`state_get` / `state_set` / `state_get_or`。
///
/// 状态挂在每个脚本各自的 Lua 环境里（旧端也是每脚本一份），重载脚本会重置。
fn register_script_state(lua: &Lua, view: &View, self_id: u64) -> Result<(), String> {
    let globals = lua.globals();
    let state = lua.create_table().map_err(|err| err.to_string())?;

    let get_state = state.clone();
    globals
        .set(
            "state_get",
            lua.create_function(move |_, (_ctx, key): (mlua::Value, String)| {
                get_state.get::<mlua::Value>(key)
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let get_or_state = state.clone();
    globals
        .set(
            "state_get_or",
            lua.create_function(
                move |_, (_ctx, key, default): (mlua::Value, String, mlua::Value)| {
                    let value = get_or_state.get::<mlua::Value>(key)?;
                    Ok(match value {
                        mlua::Value::Nil => default,
                        value => value,
                    })
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let set_state = state;
    globals
        .set(
            "state_set",
            lua.create_function(
                move |_, (_ctx, key, value): (mlua::Value, String, mlua::Value)| {
                    set_state.set(key, value)
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    // `pos(ctx)` 与 `log(ctx, msg)` 是旧脚本最常用的两个全局函数；`xgc.pos`
    // 不存在，别把新 API 的命名习惯带进来。
    let pos_view = Rc::clone(view);
    globals
        .set(
            "pos",
            lua.create_function(move |lua, _ctx: mlua::Value| {
                let view = pos_view.borrow();
                let position = view
                    .entity(self_id)
                    .map(|entity| entity.position)
                    .unwrap_or([0.0, 0.0, 0.0]);
                vector_table(lua, position)
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    globals
        .set(
            "log",
            lua.create_function(|_, (_ctx, message): (mlua::Value, String)| {
                println!("[lua] {message}");
                Ok(())
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    // `entity_name(ctx, id)`：`on_collision_*` 的 `other` 是数字 ID，旧脚本靠它认对面是谁。
    // 实体已销毁 / 没有名字组件时返回空串（旧端同口径，不抛错）。
    let name_view = Rc::clone(view);
    globals
        .set(
            "entity_name",
            lua.create_function(move |_, (_ctx, id): (mlua::Value, i64)| {
                let name = name_view
                    .borrow()
                    .entity(id as u64)
                    .map(|entity| entity.name.clone())
                    .unwrap_or_default();
                Ok(name)
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    Ok(())
}

pub(crate) fn vector_table(lua: &Lua, value: [f32; 3]) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("x", value[0])?;
    table.set("y", value[1])?;
    table.set("z", value[2])?;
    Ok(table)
}

/// 读当前层栈文本：先看命令队列里同实体 / 同字段尚未结算的写入（同帧多次改层
/// 必须依次累积），没有才退回帧首视图快照。
///
/// 少了这一步，`FoxLocomotion.lua` 同一帧里的 `set_anim_layer_param` +
/// `set_anim_layer_weight` 会各自从帧首快照构造整份 RON，后结算的那条把前一条
/// 覆盖掉（层权重永远回到 0）。旧端两个 setter 直接改运行时层结构，不存在这问题。
fn current_layers_text(commands: &Commands, view: &ScriptView, id: u64) -> Option<String> {
    let entity = EntityId::from_raw(id);
    let queued = commands
        .borrow()
        .iter()
        .rev()
        .find_map(|command| match command {
            Command::SetField {
                entity: target,
                component,
                field,
                value: FieldValue::String(text),
            } if *target == entity && *component == AnimationLayers::ID && field == "layers" => {
                Some(text.clone())
            }
            _ => None,
        });
    queued.or_else(|| view.entity(id).and_then(|row| row.anim_layers.clone()))
}

/// 写层栈文本：命令队列里已有同实体 / 同字段的写入就替换它的值（保持一条命令），
/// 否则追加。与 `current_layers_text` 配对，保证同帧多次修改只落一份最终文本。
fn push_layers_text(commands: &Commands, id: u64, text: String) {
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
            if *target == entity && *component == AnimationLayers::ID && field == "layers" {
                *value = FieldValue::String(text);
                return;
            }
        }
    }
    queue.push(Command::SetField {
        entity,
        component: AnimationLayers::ID,
        field: "layers".to_string(),
        value: FieldValue::String(text),
    });
}

pub(crate) fn push_set(
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
