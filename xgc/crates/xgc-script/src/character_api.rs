//! 角色控制器脚本 API：`character_simple_move` / `character_jump` / 读数。
//!
//! 写操作全部走命令队列：`character_simple_move` 只写 `Velocity.linear` 的
//! 水平两轴，竖直分量保留（重力由物理侧维护）；`character_jump` 写
//! `CharacterController.jump`，真正的离地发生在下一次 KCC 移动。
//!
//! 读数来自每帧快照 `ScriptView`，所以同一帧内先 `character_jump` 再读
//! `character_is_grounded` 仍是跳跃前的值——与旧端 `LuaCtx` 的「读运行时、
//! 写队列」时序一致。

use std::rc::Rc;

use mlua::{Lua, Value as LuaValue};
use xgc_base::EntityId;
use xgc_world::{CharacterController, Command, Component, Velocity};

use crate::legacy::{push_set, vector_table, Commands, View};

pub(crate) fn register(
    lua: &Lua,
    commands: &Commands,
    view: &View,
    self_id: u64,
) -> Result<(), String> {
    let globals = lua.globals();

    let axis_view = Rc::clone(view);
    globals
        .set(
            "axis",
            lua.create_function(move |_, (_ctx, name): (LuaValue, String)| {
                Ok(axis_view.borrow().axes.get(&name).copied().unwrap_or(0.0))
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let grounded_view = Rc::clone(view);
    globals
        .set(
            "character_is_grounded",
            lua.create_function(move |_, _ctx: LuaValue| {
                Ok(grounded_view
                    .borrow()
                    .entity(self_id)
                    .is_some_and(|entity| entity.grounded))
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let velocity_view = Rc::clone(view);
    globals
        .set(
            "character_velocity",
            lua.create_function(move |lua, _ctx: LuaValue| {
                let velocity = velocity_view
                    .borrow()
                    .entity(self_id)
                    .map(|entity| entity.velocity)
                    .unwrap_or([0.0, 0.0, 0.0]);
                vector_table(lua, velocity)
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let move_commands = Rc::clone(commands);
    let move_view = Rc::clone(view);
    globals
        .set(
            "character_simple_move",
            lua.create_function(move |_, (_ctx, x, _y, z): (LuaValue, f32, f32, f32)| {
                // 旧端签名是 `(ctx, vx, vy, vz)`，但 `vy` 被忽略：竖直方向
                // 只由重力 / 跳跃维护。这里保留参数位以避免脚本调用错位。
                let (has_velocity, current_y) = move_view
                    .borrow()
                    .entity(self_id)
                    .map(|entity| (entity.has_velocity, entity.velocity[1]))
                    .unwrap_or((false, 0.0));
                // 旧场景里的 Player 只有 `physics.character`，没有 `core.velocity`；
                // 旧引擎的 `character_simple_move` 会自己建运动状态，XGC 这里补一次组件。
                if !has_velocity {
                    // 同一帧第二次 `character_simple_move` 会看到旧快照仍没有 Velocity，
                    // 先查队列，避免重复 AddComponent 让整条计划变无效。
                    let pending_add = move_commands.borrow().iter().any(|command| {
                        matches!(
                            command,
                            Command::AddComponent { entity, component }
                                if *entity == EntityId::from_raw(self_id)
                                    && component.component == Velocity::ID
                        )
                    });
                    if !pending_add {
                        let component = crate::component_data::default_component("core.velocity")?;
                        move_commands.borrow_mut().push(Command::AddComponent {
                            entity: EntityId::from_raw(self_id),
                            component,
                        });
                    }
                }
                push_set(
                    &move_commands,
                    self_id,
                    Velocity::ID,
                    "linear",
                    xgc_world::Value::Vec3([f64::from(x), f64::from(current_y), f64::from(z)]),
                );
                Ok(true)
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    let jump_commands = Rc::clone(commands);
    let jump_view = Rc::clone(view);
    globals
        .set(
            "character_jump",
            lua.create_function(move |_, (_ctx, speed): (LuaValue, f32)| {
                let grounded = jump_view
                    .borrow()
                    .entity(self_id)
                    .is_some_and(|entity| entity.grounded);
                if !grounded {
                    return Ok(false);
                }
                push_set(
                    &jump_commands,
                    self_id,
                    CharacterController::ID,
                    "jump_velocity",
                    xgc_world::Value::F64(f64::from(speed)),
                );
                push_set(
                    &jump_commands,
                    self_id,
                    CharacterController::ID,
                    "jump",
                    xgc_world::Value::Bool(true),
                );
                Ok(true)
            })
            .map_err(|err| err.to_string())?,
        )
        .map_err(|err| err.to_string())?;

    Ok(())
}
