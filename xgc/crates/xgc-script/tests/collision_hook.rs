//! 碰撞钩子：`on_collision_enter/exit(ctx, other)` 必须收到数字 ID，`entity_name` 可用。

use std::path::PathBuf;

use xgc_script::ScriptRuntime;
use xgc_world::{CollisionEvent, Script, Transform, Velocity, World};

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
fn collision_hooks_receive_the_other_entity_and_entity_name() {
    let dir = temp_dir("collision");
    std::fs::write(
        dir.join("hit.lua"),
        "function on_collision_enter(ctx, other)\n  local name = entity_name(ctx, other)\n  if name ~= 'trigger' then error('bad other: ' .. tostring(other)) end\n  set_velocity(ctx, self_id(ctx), 7.0, 0.0, 0.0)\nend\nfunction on_collision_exit(ctx, other)\n  set_velocity(ctx, self_id(ctx), 0.0, 0.0, 0.0)\nend\n",
    )
    .unwrap();
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        Velocity::default(),
        Script {
            paths: vec!["hit.lua".into()],
        },
    ));
    let id = world.entity(entity).unwrap();
    let trigger = world.spawn((
        Transform::default(),
        xgc_world::Name {
            value: "trigger".to_string(),
        },
    ));
    let other = world.entity(trigger).unwrap();

    let mut runtime = ScriptRuntime::new(&dir);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    world.push_collision_event(CollisionEvent {
        entity: id,
        other,
        enter: true,
    });
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert!(runtime.take_error().is_none());
    assert_eq!(
        world.get::<Velocity>(entity).unwrap().linear,
        [7.0, 0.0, 0.0]
    );

    world.push_collision_event(CollisionEvent {
        entity: id,
        other,
        enter: false,
    });
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert!(runtime.take_error().is_none());
    assert_eq!(
        world.get::<Velocity>(entity).unwrap().linear,
        [0.0, 0.0, 0.0]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
