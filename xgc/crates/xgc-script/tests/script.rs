//! Lua 宿主回归：安全标准库、执行、函数调用。

use xgc_script::LuaHost;

#[test]
fn evaluates_arithmetic_and_functions() {
    let host = LuaHost::new().unwrap();
    assert_eq!(host.eval::<i64>("return 1 + 2").unwrap(), 3);
    host.run("defs", "function add(a, b) return a + b end")
        .unwrap();
    assert_eq!(host.call::<i64>("add", (2, 5)).unwrap(), 7);
}

#[test]
fn sandbox_does_not_expose_io_or_os() {
    let host = LuaHost::new().unwrap();
    assert!(host.global_is_nil("io").unwrap());
    assert!(host.global_is_nil("os").unwrap());
    assert!(host.global_is_nil("package").unwrap());
}

#[test]
fn syntax_errors_are_reported() {
    let host = LuaHost::new().unwrap();
    let error = host.run("bad", "function (").unwrap_err();
    assert!(error.contains("bad"));
}

#[test]
fn world_api_reads_and_writes_transform() {
    use std::cell::RefCell;
    use std::rc::Rc;

    use xgc_world::{Transform, World};

    let mut world = World::new();
    let entity = world.spawn((Transform::default(),));
    let id = world.entity(entity).unwrap();
    let world = Rc::new(RefCell::new(world));
    let host = LuaHost::with_world(Rc::clone(&world)).unwrap();
    assert_eq!(host.eval::<i64>("return xgc.entity_count()").unwrap(), 1);
    host.run(
        "move",
        &format!("xgc.set_position({}, 4.0, 5.0, 6.0)", id.raw()),
    )
    .unwrap();
    let (x, y, z) = host
        .eval::<(f32, f32, f32)>(&format!(
            "local p = xgc.get_position({}); return p.x, p.y, p.z",
            id.raw()
        ))
        .unwrap();
    assert_eq!((x, y, z), (4.0, 5.0, 6.0));
    let position = world.borrow().get::<Transform>(entity).unwrap().position;
    assert_eq!(position, [4.0, 5.0, 6.0]);
}

#[test]
fn world_api_rejects_missing_entity() {
    use std::cell::RefCell;
    use std::rc::Rc;

    use xgc_world::World;

    let world = Rc::new(RefCell::new(World::new()));
    let host = LuaHost::with_world(world).unwrap();
    let error = host
        .run("bad", "xgc.set_position(99, 1, 2, 3)")
        .unwrap_err();
    assert!(error.contains("99"));
}

#[test]
fn generic_field_api_reads_vectors_and_names() {
    use std::cell::RefCell;
    use std::rc::Rc;

    use xgc_world::{Name, Transform, World};

    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        Name {
            value: "old".to_string(),
        },
    ));
    let id = world.entity(entity).unwrap();
    let world = Rc::new(RefCell::new(world));
    let host = LuaHost::with_world(Rc::clone(&world)).unwrap();
    host.run(
        "fields",
        &format!(
            "xgc.set({}, 'core.transform', 'position', {{x=1.0, y=2.0, z=3.0}}); \
             xgc.set({}, 'core.name', 'value', 'hero')",
            id.raw(),
            id.raw()
        ),
    )
    .unwrap();
    let x = host
        .eval::<f64>(&format!(
            "local p = xgc.get({}, 'core.transform', 'position'); return p.x",
            id.raw()
        ))
        .unwrap();
    assert_eq!(x, 1.0);
    assert_eq!(world.borrow().get::<Name>(entity).unwrap().value, "hero");
}

#[test]
fn unknown_component_is_reported() {
    use std::cell::RefCell;
    use std::rc::Rc;

    use xgc_world::World;

    let world = Rc::new(RefCell::new(World::new()));
    let host = LuaHost::with_world(world).unwrap();
    let error = host.run("bad", "xgc.get(1, 'nope', 'x')").unwrap_err();
    assert!(error.contains("未知组件"));
}

#[test]
fn instruction_budget_stops_infinite_loop() {
    let mut host = LuaHost::new().unwrap();
    host.set_instruction_limit(20_000);
    let error = host.run("spin", "while true do end").unwrap_err();
    assert!(error.contains("预算"), "{error}");
}

#[test]
fn scripts_can_be_loaded_and_reloaded() {
    let host = LuaHost::new().unwrap();
    host.load_script("math", "function value() return 1 end")
        .unwrap();
    assert_eq!(host.call::<i64>("value", ()).unwrap(), 1);
    host.load_script("math", "function value() return 2 end")
        .unwrap();
    assert_eq!(host.call::<i64>("value", ()).unwrap(), 2);
    assert_eq!(host.script_names(), vec!["math"]);
    assert!(host.reload("missing").is_err());
}

#[test]
fn lifecycle_hooks_run_once_and_update() {
    let host = LuaHost::new().unwrap();
    host.run(
        "life",
        "value = 0
         function on_start() value = 1 end
         function on_update(dt) value = value + dt end
         function on_destroy() value = -1 end",
    )
    .unwrap();
    assert!(host.start().unwrap());
    assert!(!host.start().unwrap(), "on_start 只触发一次");
    assert!(host.update(0.5).unwrap());
    assert_eq!(host.eval::<f64>("return value").unwrap(), 1.5);
    assert!(host.destroy().unwrap());
    assert_eq!(host.eval::<f64>("return value").unwrap(), -1.0);
}

#[test]
fn scripts_can_spawn_and_despawn_entities() {
    use std::cell::RefCell;
    use std::rc::Rc;

    use xgc_world::World;

    let world = Rc::new(RefCell::new(World::new()));
    let host = LuaHost::with_world(Rc::clone(&world)).unwrap();
    let _id = host
        .eval::<u64>("local id = xgc.spawn(); xgc.despawn(id); return id")
        .unwrap();
    assert_eq!(world.borrow().len(), 0);
}

#[test]
fn runtime_drives_script_entities_and_reloads() {
    use xgc_world::{Script, Transform, World};

    let dir = std::env::temp_dir().join(format!(
        "xgc_lua_runtime_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("move.lua");
    std::fs::write(
        &path,
        "function on_update(dt)\n  xgc.set(0, 'core.transform', 'position', {x=2.0, y=3.0, z=4.0})\nend\n",
    )
    .unwrap();
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        Script {
            path: "move.lua".to_string(),
        },
    ));
    let mut runtime = xgc_script::ScriptRuntime::new(&dir);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert_eq!(
        world.get::<Transform>(entity).unwrap().position,
        [2.0, 3.0, 4.0]
    );
    assert_eq!(runtime.entity_count(), 1);

    std::fs::write(
        &path,
        "function on_update(dt)\n  xgc.set(0, 'core.transform', 'position', {x=9.0, y=0.0, z=0.0})\nend\n",
    )
    .unwrap();
    runtime.reload_all().unwrap();
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert_eq!(
        world.get::<Transform>(entity).unwrap().position,
        [9.0, 0.0, 0.0]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn memory_limit_rejects_large_allocation() {
    let host = LuaHost::new().unwrap();
    let _previous = host.set_memory_limit(128 * 1024).unwrap();
    let error = host
        .run(
            "big",
            "local text = string.rep('x', 1024 * 1024); return text",
        )
        .unwrap_err();
    assert!(
        error.contains("memory") || error.contains("内存"),
        "{error}"
    );
}

#[test]
fn runtime_destroys_scripts_when_entity_is_removed() {
    use xgc_world::{Script, Transform, World};

    let dir = std::env::temp_dir().join(format!(
        "xgc_lua_destroy_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("idle.lua"),
        "destroyed = false\nfunction on_destroy() destroyed = true end\n",
    )
    .unwrap();
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        Script {
            path: "idle.lua".to_string(),
        },
    ));
    let id = world.entity(entity).unwrap();
    let mut runtime = xgc_script::ScriptRuntime::new(&dir);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert_eq!(runtime.entity_count(), 1);
    let key = world.key(id).unwrap();
    world.despawn(key);
    runtime.run_frame(&mut world, 1.0 / 60.0);
    assert_eq!(runtime.entity_count(), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}
