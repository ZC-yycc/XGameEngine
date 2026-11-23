//! 旧引擎脚本 API 的最小兼容层：`update(ctx)` + `rot/set_rot/delta` 等。
//!
//! 只读数据来自每帧快照 `ScriptView`，写操作一律进命令队列。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use mlua::{Lua, Table};
use xgc_base::EntityId;
use xgc_world::{
    AnimationLayers, AnimationState, Animator, Command, Component, Transform, UiElement,
    Value as FieldValue, Velocity,
};

/// 脚本 API 的颜色参数是 **sRGB 0..1**（美术 / CSS 的直觉：`0.5` 就是中灰），
/// UI 组件里存的是**线性**值。这一处是唯一入口转换点，与旧引擎
/// `xgame-script/src/ui_ctx.rs` 的 `color_srgb` 同一套公式与阈值。
///
/// 少了这一步，脚本给的颜色会被当成线性值再经 sRGB 目标编码一次：
/// HUD 血条 `(0.86, 0.33, 0.28)` 会从旧端的 `#DC5347` 变成 `#EF9A90`。
fn srgb_to_linear(channel: f32) -> f32 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

pub use crate::view::{EntityView, ScriptView};

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
                let current = {
                    let view = anim_view.borrow();
                    let Some(entity) = view.entity(self_id) else {
                        return Ok(());
                    };
                    let Some(parameters) = &entity.anim_parameters else {
                        return Ok(());
                    };
                    parameters.clone()
                };
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
                    // 脚本给的是 sRGB，组件存线性：与旧端同口径，见 `srgb_to_linear`。
                    let linear = |channel: f32| f64::from(srgb_to_linear(channel));
                    push_set(
                        &ui_color_commands,
                        id,
                        UiElement::ID,
                        "background",
                        FieldValue::Vec4([linear(r), linear(g), linear(b), f64::from(a)]),
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
