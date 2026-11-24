//! 动画事件钩子：`on_anim_event(ctx, name)` 必须收到剪辑事件名。

use std::path::PathBuf;

use xgc_script::ScriptRuntime;
use xgc_world::{FiredAnimEvent, Script, Transform, Velocity, World};

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
fn animation_events_reach_the_script_hook() {
    let dir = temp_dir("anim_event");
    std::fs::write(
        dir.join("anim.lua"),
        "function on_anim_event(ctx, name)\n  if name == 'hit' then\n    set_velocity(ctx, self_id(ctx), 5.0, 0.0, 0.0)\n  end\nend\n",
    )
    .unwrap();
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        Velocity::default(),
        Script {
            path: "anim.lua".to_string(),
        },
    ));
    let id = world.entity(entity).unwrap();

    let mut runtime = ScriptRuntime::new(&dir);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    world.push_animation_event(FiredAnimEvent {
        entity: id,
        name: "hit".to_string(),
    });
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert!(runtime.take_error().is_none());
    assert_eq!(
        world.get::<Velocity>(entity).unwrap().linear,
        [5.0, 0.0, 0.0]
    );

    // 名字不匹配时钩子仍然被调用，但脚本自己不写速度。
    world.push_animation_event(FiredAnimEvent {
        entity: id,
        name: "miss".to_string(),
    });
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert!(runtime.take_error().is_none());
    assert_eq!(
        world.get::<Velocity>(entity).unwrap().linear,
        [5.0, 0.0, 0.0]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
