//! 旧脚本生命周期与角色 API：`fixed_update`、`character_*`、`axis`、`state_*`。

use std::path::PathBuf;

use xgc_script::ScriptRuntime;
use xgc_world::{CharacterController, Script, Transform, Velocity, World};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "xgc_lua_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn fixed_update_runs_once_per_step_with_a_context() {
    let dir = temp_dir("fixed");
    std::fs::write(
        dir.join("counter.lua"),
        "function fixed_update(ctx)\n  if ctx.entity ~= self_id(ctx) then error('bad ctx') end\n  fixed_calls = (fixed_calls or 0) + 1\n  set_pos(ctx, fixed_calls, 0.0, 0.0)\nend\n",
    )
    .unwrap();
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        Script {
            paths: vec!["counter.lua".into()],
        },
    ));
    let mut runtime = ScriptRuntime::new(&dir);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert_eq!(world.get::<Transform>(entity).unwrap().position[0], 2.0);
    assert!(runtime.take_error().is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn character_api_drives_velocity_and_jump() {
    let dir = temp_dir("character");
    std::fs::write(
        dir.join("player.lua"),
        "function fixed_update(ctx)\n  local x = axis(ctx, 'MoveX')\n  character_simple_move(ctx, x * 2.0, 0.0, 1.0)\n  character_simple_move(ctx, x * 2.0, 0.0, 1.0)\n  if character_is_grounded(ctx) then character_jump(ctx, 5.5) end\n  state_set(ctx, 't', state_get_or(ctx, 't', 0.0) + delta(ctx))\n  set_pos(ctx, state_get_or(ctx, 't', 0.0), character_velocity(ctx).y, pos(ctx).y)\nend\n",
    )
    .unwrap();
    let mut world = World::new();
    // 旧 character 场景的 Player 只有 Character + Lua，没有 core.velocity；
    // `character_simple_move` 必须自己补组件，否则第一帧就会 EDIT.UNKNOWN_COMPONENT。
    let entity = world.spawn((
        Transform::default(),
        CharacterController {
            grounded: true,
            ..Default::default()
        },
        Script {
            paths: vec!["player.lua".into()],
        },
    ));
    let mut runtime = ScriptRuntime::new(&dir);
    let id = world.entity(entity).unwrap();
    runtime.set_axis("MoveX", 1.0);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert!(runtime.take_error().is_none());
    // `AddComponent` 走 replace 路径，旧 EntityKey 会失效，按 EntityId 重新取。
    let key = world.key(id).unwrap();
    assert_eq!(world.get::<Velocity>(key).unwrap().linear, [2.0, 0.0, 1.0]);
    let character = world.get::<CharacterController>(key).unwrap();
    assert!(character.jump);
    assert_eq!(character.jump_velocity, 5.5);
    assert!(runtime.take_error().is_none());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn state_survives_between_frames_and_axis_defaults_to_zero() {
    let dir = temp_dir("state");
    std::fs::write(
        dir.join("state.lua"),
        "function fixed_update(ctx)\n  local t = state_get_or(ctx, 't', 0.0) + delta(ctx)\n  state_set(ctx, 't', t)\n  set_pos(ctx, t, axis(ctx, 'MoveX'), 0.0)\nend\n",
    )
    .unwrap();
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        Script {
            paths: vec!["state.lua".into()],
        },
    ));
    let mut runtime = ScriptRuntime::new(&dir);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    let position = world.get::<Transform>(entity).unwrap().position;
    assert!((position[0] - 2.0 / 60.0).abs() < 1e-6);
    assert_eq!(position[1], 0.0);
    std::fs::remove_dir_all(&dir).unwrap();
}
