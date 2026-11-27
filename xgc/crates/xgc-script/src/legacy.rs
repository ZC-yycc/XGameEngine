//! 旧引擎脚本 API 的最小兼容层：`update(ctx)` + `rot/set_rot/delta` 等。
//!
//! 只读数据来自每帧快照 `ScriptView`，写操作一律进命令队列。

use std::cell::RefCell;
use std::rc::Rc;

use mlua::{Lua, Table};
use xgc_base::EntityId;
use xgc_world::{
    AnimationLayers, AnimationState, Animator, BodyKind, Command, Component, Transform,
    Value as FieldValue, Velocity,
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

pub(crate) fn register(
    lua: &Lua,
    commands: &Commands,
    view: &View,
    self_id: u64,
) -> Result<Rc<RefCell<Table>>, String> {
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
    let set_velocity_view = Rc::clone(view);
    globals
        .set(
            "set_velocity",
            lua.create_function(
                move |_, (_ctx, id, x, y, z): (mlua::Value, u64, f32, f32, f32)| {
                    let mut view = set_velocity_view.borrow_mut();
                    match view.entity(id).and_then(|row| row.body_kind) {
                        Some(BodyKind::Static) => {}
                        Some(_) => {
                            let linear = [x, y, z];
                            if !linear.iter().all(|value| value.is_finite()) {
                                return Ok(());
                            }
                            // 同一钩子通过 velocity 可读回命令，固定步之后由后端回写结果。
                            view.entity_mut(id).expect("已找到刚体").velocity = linear;
                            view.body_velocities.push((EntityId::from_raw(id), linear));
                        }
                        None => push_set(
                            &set_velocity_commands,
                            id,
                            Velocity::ID,
                            "linear",
                            FieldValue::Vec3([f64::from(x), f64::from(y), f64::from(z)]),
                        ),
                    }
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

    crate::audio_api::register(lua, view).map_err(|err| err.to_string())?;

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
                    edit_animation_layer(
                        &layer_weight_commands,
                        &mut layer_weight_view.borrow_mut(),
                        self_id,
                        &layer,
                        |fields| {
                            *fields.get_mut("weight")? =
                                FieldValue::F64(f64::from(weight.clamp(0.0, 1.0)));
                            Some(())
                        },
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
                    edit_animation_layer(
                        &layer_param_commands,
                        &mut layer_param_view.borrow_mut(),
                        self_id,
                        &layer,
                        |fields| {
                            // 层机器关闭时没有转移条件消费这些参数。
                            let FieldValue::Enum { variant, .. } = fields.get("machine")? else {
                                return None;
                            };
                            if variant != "Enabled" {
                                return None;
                            }
                            let FieldValue::Map(parameters) = fields.get_mut("parameters")? else {
                                return None;
                            };
                            parameters.insert(name, FieldValue::F64(f64::from(value)));
                            Some(())
                        },
                    );
                    Ok(())
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;
    crate::ui_api::register(lua, commands, view)?;
    type ReadAction = fn(&crate::view::ActionState) -> bool;
    let action_fields: [(&str, ReadAction); 3] = [
        ("action_down", |state| state.down),
        ("action_pressed", |state| state.pressed),
        ("action_released", |state| state.released),
    ];
    for (name, read) in action_fields {
        let view = Rc::clone(view);
        globals
            .set(
                name,
                lua.create_function(move |_, (_ctx, name): (mlua::Value, String)| {
                    Ok(view.borrow().actions.get(&name).is_some_and(read))
                })
                .map_err(|err| err.to_string())?,
            )
            .map_err(|err| err.to_string())?;
    }

    let state = register_script_state(lua, view, self_id)?;
    crate::character_api::register(lua, commands, view, self_id)?;
    Ok(state)
}

/// 每个挂载持有同一份持久状态，候选模块的顶层初始化操作临时表。
fn register_script_state(
    lua: &Lua,
    view: &View,
    self_id: u64,
) -> Result<Rc<RefCell<Table>>, String> {
    let globals = lua.globals();
    let state = Rc::new(RefCell::new(
        lua.create_table().map_err(|err| err.to_string())?,
    ));

    let get_state = Rc::clone(&state);
    globals
        .set(
            "state_get",
            lua.create_function(move |_, (_ctx, key): (mlua::Value, String)| {
                get_state.borrow().raw_get::<mlua::Value>(key)
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let get_or_state = Rc::clone(&state);
    globals
        .set(
            "state_get_or",
            lua.create_function(
                move |_, (_ctx, key, default): (mlua::Value, String, mlua::Value)| {
                    let value = get_or_state.borrow().raw_get::<mlua::Value>(key)?;
                    Ok(match value {
                        mlua::Value::Nil => default,
                        value => value,
                    })
                },
            )
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let set_state = Rc::clone(&state);
    globals
        .set(
            "state_set",
            lua.create_function(
                move |_, (_ctx, key, value): (mlua::Value, String, mlua::Value)| {
                    set_state.borrow().raw_set(key, value)
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
            lua.create_function(|lua, (_ctx, message): (mlua::Value, mlua::String)| {
                crate::sandbox::log(lua, message)
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
    Ok(state)
}

pub(crate) fn vector_table(lua: &Lua, value: [f32; 3]) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("x", value[0])?;
    table.set("y", value[1])?;
    table.set("z", value[2])?;
    Ok(table)
}

/// 按名选第一层，沿用动态路径写入更新过的帧内视图。
fn edit_animation_layer(
    commands: &Commands,
    view: &mut ScriptView,
    id: u64,
    name: &str,
    edit: impl FnOnce(&mut std::collections::BTreeMap<String, FieldValue>) -> Option<()>,
) {
    let Some(FieldValue::List(layers)) =
        view.entity_mut(id).and_then(|row| row.anim_layers.as_mut())
    else {
        return;
    };
    let Some(fields) = layers.iter_mut().find_map(|layer| match layer {
        FieldValue::Struct(fields)
            if matches!(fields.get("name"), Some(FieldValue::String(value)) if value == name) =>
        {
            Some(fields)
        }
        _ => None,
    }) else {
        return;
    };
    if edit(fields).is_none() {
        return;
    }
    let entity = EntityId::from_raw(id);
    let updated = FieldValue::List(layers.clone());
    let mut queue = commands.borrow_mut();
    // 连续的整栈写入可以合并。遇到动态路径命令时保留它在事务中的顺序。
    if let Some(Command::SetField {
        entity: target,
        component: AnimationLayers::ID,
        field,
        value,
    }) = queue.last_mut()
    {
        if *target == entity && field == "layers" {
            *value = updated;
            return;
        }
    }
    queue.push(Command::SetField {
        entity,
        component: AnimationLayers::ID,
        field: "layers".to_string(),
        value: updated,
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
