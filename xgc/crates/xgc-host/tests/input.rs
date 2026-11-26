use xgc_host::ScriptedRunner;
use xgc_sim::{InputMap, InputState, KeyCode};
use xgc_world::{Script, Transform, Velocity, World};

#[test]
fn applies_actions_and_axes_until_the_keys_are_released() {
    let dir = std::env::temp_dir().join(format!(
        "xgc_host_input_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("move.lua"),
        "function update(ctx)\n  local x = axis(ctx, 'MoveX')\n  local jump = action_pressed(ctx, 'Jump')\n  set_velocity(ctx, self_id(ctx), x, jump and 9.0 or 0.0, 0.0)\nend\n",
    )
    .unwrap();
    let map = InputMap::parse_ron(
        r#"InputMap(actions: {"Jump": [Key(Space)]}, axes: {"MoveX": (positive: [Key(KeyD)], negative: [Key(KeyA)])})"#,
    )
    .unwrap();
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        Velocity::default(),
        Script {
            path: "move.lua".to_string(),
        },
    ));
    let (mut runner, _) =
        ScriptedRunner::with_assets(world, xgc_host::asset_paths(Some(&dir), &[]));
    let mut state = InputState::default();
    state.keys.extend([KeyCode::KeyD, KeyCode::Space]);
    runner.apply_input(&map, &state);
    runner.step().unwrap();
    assert_eq!(
        runner.world().get::<Velocity>(entity).unwrap().linear,
        [1.0, 9.0, 0.0]
    );
    state.keys.clear();
    runner.apply_input(&map, &state);
    runner.step().unwrap();
    assert_eq!(
        runner.world().get::<Velocity>(entity).unwrap().linear,
        [0.0; 3]
    );
    std::fs::remove_dir_all(dir).unwrap();
}
