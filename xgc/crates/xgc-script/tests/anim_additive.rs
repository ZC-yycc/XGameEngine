//! 脚本侧叠加层权重：`set_anim_additive_weight` 写 `Animator.additive_weight`，
//! 超界值钳到 [0,1]，NaN 忽略（保留上一次的值）。
#[test]
fn set_anim_additive_weight_writes_the_animator_component() {
    use xgc_world::{Animator, Script, Transform, World};

    let dir = std::env::temp_dir().join(format!(
        "xgc_lua_additive_weight_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("additive.lua");
    std::fs::write(
        &path,
        "function update(ctx) set_anim_additive_weight(ctx, 0.45) end\n",
    )
    .unwrap();
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        Animator {
            additive_clip: "Run".to_string(),
            ..Animator::default()
        },
        Script {
            paths: vec!["additive.lua".into()],
        },
    ));
    let mut runtime = xgc_script::ScriptRuntime::new(&dir);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert!(
        (world.get::<Animator>(entity).unwrap().additive_weight - 0.45).abs() < 1e-6,
        "权重应写进 Animator 组件"
    );

    // 超界值钳到 [0,1]；NaN 直接忽略（保留上一次的值）。
    std::fs::write(
        &path,
        "function update(ctx) set_anim_additive_weight(ctx, 2.0) end\n",
    )
    .unwrap();
    runtime.reload_all().unwrap();
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert_eq!(world.get::<Animator>(entity).unwrap().additive_weight, 1.0);
    std::fs::write(
        &path,
        "function update(ctx) set_anim_additive_weight(ctx, 0.0/0.0) end\n",
    )
    .unwrap();
    runtime.reload_all().unwrap();
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert_eq!(world.get::<Animator>(entity).unwrap().additive_weight, 1.0);

    std::fs::remove_dir_all(&dir).unwrap();
}
