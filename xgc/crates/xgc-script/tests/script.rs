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
