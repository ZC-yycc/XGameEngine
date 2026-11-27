use std::{cell::RefCell, rc::Rc};

use xgc_base::EntityId;
use xgc_script::{EntityView, LuaHost, ScriptView};
use xgc_world::{Transform, World};

#[test]
fn direct_lua_spawn_reports_exhaustion_without_changing_the_world() {
    let mut world = World::new();
    world.spawn((Transform::default(),));
    let mut saved = world.world_data();
    saved.next_id = u64::MAX;
    world.restore_world_data(&saved).unwrap();
    let world = Rc::new(RefCell::new(world));
    let host = LuaHost::with_world(Rc::clone(&world)).unwrap();

    let error = host.run("spawn", "xgc.spawn()").unwrap_err();
    assert!(error.contains("实体 ID"), "{error}");
    assert_eq!(world.borrow().world_data(), saved);
}

#[test]
fn ui_spawn_returns_zero_after_the_last_assignable_id() {
    let view = Rc::new(RefCell::new(ScriptView::default()));
    view.borrow_mut().spawn_cursor = EntityId::MAX_ASSIGNABLE_RAW;
    let host =
        LuaHost::with_commands_and_view(Rc::new(RefCell::new(Vec::new())), Rc::clone(&view), 0)
            .unwrap();
    let (last, rejected) = host
        .eval::<(i64, i64)>(
            "local last = ui_spawn_label(nil, 'last', 0, 0, 20); \
             return last, ui_spawn_label(nil, 'rejected', 0, 0, 20)",
        )
        .unwrap();
    assert_eq!(last as u64, EntityId::MAX_ASSIGNABLE_RAW);
    assert_eq!(rejected, 0);
    assert_eq!(view.borrow().pending_spawns.len(), 1);
    assert_eq!(view.borrow().spawn_cursor, u64::MAX);

    let mut world = World::new();
    let plan = world.plan(host.take_commands());
    world.apply(&plan).unwrap();
    assert_eq!(world.len(), 1);
    assert_eq!(world.next_entity_id().raw(), u64::MAX);
}

#[test]
fn caller_supplied_views_keep_first_match_in_input_order() {
    let view = Rc::new(RefCell::new(ScriptView::default()));
    view.borrow_mut().entities.extend(
        [(40, "first", 1.0), (7, "other", 2.0), (40, "last", 3.0)]
            .into_iter()
            .map(|(id, name, x)| EntityView {
                id,
                name: name.into(),
                position: Some([x, 0.0, 0.0]),
                ..EntityView::default()
            }),
    );
    let host =
        LuaHost::with_commands_and_view(Rc::new(RefCell::new(Vec::new())), Rc::clone(&view), 40)
            .unwrap();
    let result = host
        .eval::<(String, f32, f32)>(
            r#"
local name = entity_name(nil, 40)
local other = entity_pos(nil, 7).x
set_pos(nil, 10, 0, 0)
return name, other, entity_pos(nil, 40).x
"#,
        )
        .unwrap();
    assert_eq!(result, ("first".into(), 2.0, 10.0));
    let view = view.borrow();
    assert_eq!(view.entities[0].position, Some([10.0, 0.0, 0.0]));
    assert_eq!(view.entities[2].position, Some([3.0, 0.0, 0.0]));
}
