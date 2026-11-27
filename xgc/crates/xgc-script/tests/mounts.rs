use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use xgc_script::ScriptRuntime;
use xgc_world::{CollisionEvent, FiredAnimEvent, PhysicsBody, Script, Transform, UiElement, World};

struct Scripts(PathBuf);

impl Scripts {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("xgc_script_{label}_{}_{nonce}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn write(&self, path: &str, source: &str) {
        std::fs::write(self.0.join(path), source).unwrap();
    }

    fn runtime(&self) -> ScriptRuntime {
        ScriptRuntime::new(&self.0)
    }
}

impl Drop for Scripts {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn mounts(paths: &[&str]) -> Script {
    Script {
        paths: paths.iter().map(|path| (*path).into()).collect(),
    }
}

fn step(runtime: &mut ScriptRuntime, world: &mut World) {
    runtime.run_frame(world, 1.0 / 60.0);
    assert_eq!(runtime.take_error(), None);
}

#[test]
fn replacing_a_path_retires_the_old_vm_and_starts_the_new_one() {
    let scripts = Scripts::new("replace_mount");
    scripts.write("old.lua", "function update(ctx) set_pos(ctx, 1, 0, 0) end");
    scripts.write("new.lua", "function start(ctx) set_pos(ctx, 2, 0, 0) end");
    let mut world = World::new();
    let entity = world.spawn((Transform::default(), mounts(&["old.lua"])));
    let mut runtime = scripts.runtime();
    step(&mut runtime, &mut world);
    assert_eq!(world.get::<Transform>(entity).unwrap().position[0], 1.0);

    *world.get_mut::<Script>(entity).unwrap() = mounts(&["new.lua"]);
    step(&mut runtime, &mut world);
    assert_eq!(world.get::<Transform>(entity).unwrap().position[0], 2.0);
}

#[test]
fn destroy_uses_the_removed_entity_context_and_commits_commands() {
    let scripts = Scripts::new("destroy_mount");
    let mut world = World::new();
    let survivor = world.spawn((Transform::default(),));
    let target = world.entity(survivor).unwrap().raw();
    scripts.write(
        "destroy.lua",
        &format!(
            r#"
            function start(ctx) state_set(ctx, 'amount', 7) end
            function on_destroy(ctx)
                assert(ctx.entity == self_id(ctx))
                set_pos(ctx, 99, 0, 0)
                local amount = state_get(ctx, 'amount')
                xgc.set({target}, 'core.transform', 'position', {{x = amount, y = 0, z = 0}})
            end
            "#
        ),
    );
    let entity = world.spawn((mounts(&["destroy.lua"]),));
    let mut runtime = scripts.runtime();
    step(&mut runtime, &mut world);
    assert!(world.despawn(entity));
    step(&mut runtime, &mut world);
    assert_eq!(world.get::<Transform>(survivor).unwrap().position[0], 7.0);
    assert_eq!(runtime.entity_count(), 0);
    assert_eq!(runtime.script_count(), 0);
}

#[test]
fn mounts_dispatch_in_order_with_shared_pending_writes() {
    let scripts = Scripts::new("mount_order");
    for label in ["a", "b"] {
        scripts.write(
            &format!("{label}.lua"),
            &format!(
                r#"
                local function mark(ctx, stage)
                    local id = self_id(ctx)
                    ui_set_text(ctx, id, ui_text(ctx, id) .. '{label}' .. stage)
                end
                function start(ctx) mark(ctx, 'S') end
                function update(ctx) if time(ctx) == delta(ctx) then mark(ctx, 'U') end end
                function fixed_update(ctx) if time(ctx) == delta(ctx) then mark(ctx, 'F') end end
                function on_collision_enter(ctx, other)
                    assert(entity_name(ctx, other) == 'trigger')
                    mark(ctx, 'C')
                end
                function on_anim_event(ctx, name)
                    assert(name == 'hit')
                    mark(ctx, 'E')
                end
                function on_destroy(ctx) mark(ctx, 'D') end
                "#
            ),
        );
    }
    let mut world = World::new();
    let entity = world.spawn((UiElement::default(), mounts(&["a.lua", "", "b.lua"])));
    let trigger = world.spawn((xgc_world::Name {
        value: "trigger".into(),
    },));
    let id = world.entity(entity).unwrap();
    let other = world.entity(trigger).unwrap();
    let mut runtime = scripts.runtime();
    step(&mut runtime, &mut world);
    assert_eq!(runtime.entity_count(), 1);
    assert_eq!(runtime.script_count(), 2);
    assert_eq!(world.get::<UiElement>(entity).unwrap().text, "aSbSaUbUaFbF");

    world.push_collision_event(CollisionEvent {
        entity: id,
        other,
        enter: true,
    });
    step(&mut runtime, &mut world);
    assert_eq!(
        world.get::<UiElement>(entity).unwrap().text,
        "aSbSaUbUaFbFaCbC"
    );
    world.clear_collision_events();

    world.push_animation_event(FiredAnimEvent {
        entity: id,
        name: "hit".into(),
    });
    step(&mut runtime, &mut world);
    assert_eq!(
        world.get::<UiElement>(entity).unwrap().text,
        "aSbSaUbUaFbFaCbCaEbE"
    );
    world.clear_animation_events();

    *world.get_mut::<Script>(entity).unwrap() = mounts(&[""]);
    step(&mut runtime, &mut world);
    assert_eq!(
        world.get::<UiElement>(entity).unwrap().text,
        "aSbSaUbUaFbFaCbCaEbEaDbD"
    );
    assert_eq!(runtime.entity_count(), 0);
    assert_eq!(runtime.script_count(), 0);
    step(&mut runtime, &mut world);
    assert_eq!(
        world.get::<UiElement>(entity).unwrap().text,
        "aSbSaUbUaFbFaCbCaEbEaDbD"
    );
}

#[test]
fn reorder_keeps_vm_states_by_path_and_occurrence() {
    let scripts = Scripts::new("reorder_mounts");
    for label in ["a", "b", "c"] {
        scripts.write(
            &format!("{label}.lua"),
            &format!(
                r#"
                local tag
                function start(ctx)
                    local id = self_id(ctx)
                    local text = ui_text(ctx, id)
                    tag = '{label}' .. tostring(#text + 1)
                    ui_set_text(ctx, id, text .. tag)
                end
                function update(ctx)
                    local id = self_id(ctx)
                    ui_set_text(ctx, id, ui_text(ctx, id) .. '-' .. tag)
                end
                function on_destroy(ctx)
                    local id = self_id(ctx)
                    ui_set_text(ctx, id, ui_text(ctx, id) .. '!' .. tag)
                end
                "#
            ),
        );
    }
    let mut world = World::new();
    let entity = world.spawn((UiElement::default(), mounts(&["a.lua", "b.lua", "a.lua"])));
    let mut runtime = scripts.runtime();
    step(&mut runtime, &mut world);
    assert_eq!(runtime.entity_count(), 1);
    assert_eq!(runtime.script_count(), 3);
    assert_eq!(
        world.get::<UiElement>(entity).unwrap().text,
        "a1b3a5-a1-b3-a5"
    );

    *world.get_mut::<Script>(entity).unwrap() = mounts(&["a.lua", "a.lua", "b.lua"]);
    world.get_mut::<UiElement>(entity).unwrap().text.clear();
    step(&mut runtime, &mut world);
    assert_eq!(world.get::<UiElement>(entity).unwrap().text, "-a1-a5-b3");

    *world.get_mut::<Script>(entity).unwrap() = mounts(&["c.lua", "a.lua"]);
    world.get_mut::<UiElement>(entity).unwrap().text.clear();
    step(&mut runtime, &mut world);
    assert_eq!(
        world.get::<UiElement>(entity).unwrap().text,
        "!a5!b3c7-c7-a1"
    );
    assert_eq!(runtime.script_count(), 2);
}

#[test]
fn failed_reload_keeps_the_live_module_and_its_state() {
    let scripts = Scripts::new("failed_reload");
    scripts.write(
        "reload.lua",
        r#"
        function start(ctx) state_set(ctx, 'count', 0) end
        function update(ctx)
            local n = state_get(ctx, 'count') + 1
            state_set(ctx, 'count', n)
            set_pos(ctx, n, 0, 0)
        end
        "#,
    );
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        UiElement::default(),
        mounts(&["reload.lua"]),
    ));
    let mut runtime = scripts.runtime();
    step(&mut runtime, &mut world);
    assert_eq!(world.get::<Transform>(entity).unwrap().position[0], 1.0);

    scripts.write(
        "reload.lua",
        r#"
        state_set(nil, 'count', 999)
        ui_set_text(nil, self_id(nil), 'rejected')
        _G.update = function(ctx) set_pos(ctx, 99, 0, 0) end
        error('fixture reload failure')
        "#,
    );
    let error = runtime.reload_all().unwrap_err();
    assert!(
        error.contains("reload.lua") && error.contains("fixture reload failure"),
        "{error}"
    );
    step(&mut runtime, &mut world);
    assert_eq!(world.get::<Transform>(entity).unwrap().position[0], 2.0);
    assert!(world.get::<UiElement>(entity).unwrap().text.is_empty());
}

#[test]
fn successful_reload_removes_old_hooks_and_keeps_persistent_state() {
    let scripts = Scripts::new("remove_hooks");
    scripts.write(
        "reload.lua",
        r#"
        function start(ctx) state_set(ctx, 'count', 3) end
        function update(ctx) set_pos(ctx, state_get(ctx, 'count'), 0, 0) end
        function on_anim_event(ctx) ui_set_text(ctx, self_id(ctx), 'old hook') end
        "#,
    );
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        UiElement::default(),
        mounts(&["reload.lua"]),
    ));
    let id = world.entity(entity).unwrap();
    let mut runtime = scripts.runtime();
    step(&mut runtime, &mut world);
    assert_eq!(world.get::<Transform>(entity).unwrap().position[0], 3.0);

    scripts.write(
        "reload.lua",
        r#"
        local make = assert(load('return function(ctx) set_pos(ctx, state_get(ctx, "count") + 1, 0, 0) end'))
        function start(ctx) state_set(ctx, 'count', 100) end
        fixed_update = make()
        "#,
    );
    runtime.reload_all().unwrap();
    world.push_animation_event(FiredAnimEvent {
        entity: id,
        name: "fixture".into(),
    });
    step(&mut runtime, &mut world);
    assert_eq!(world.get::<Transform>(entity).unwrap().position[0], 4.0);
    assert!(world.get::<UiElement>(entity).unwrap().text.is_empty());
}

#[test]
fn reload_replaces_all_mounts_together_without_publishing_initializer_effects() {
    let scripts = Scripts::new("reload_mounts_together");
    for label in ["a", "b"] {
        scripts.write(
            &format!("{label}.lua"),
            &format!(
                "function update(ctx)\n\
                 local id = self_id(ctx)\n\
                 ui_set_text(ctx, id, table.concat({{ui_text(ctx, id), '{label}'}}))\n\
                 end"
            ),
        );
    }
    let mut world = World::new();
    let entity = world.spawn((
        UiElement::default(),
        Transform::default(),
        PhysicsBody::default(),
        mounts(&["a.lua", "b.lua"]),
    ));
    let mut runtime = scripts.runtime();
    step(&mut runtime, &mut world);
    assert_eq!(world.get::<UiElement>(entity).unwrap().text, "ab");

    scripts.write(
        "a.lua",
        r#"
        ui_spawn_label(nil, 'candidate', 0, 0, 20)
        ui_set_text(nil, self_id(nil), 'candidate text')
        set_velocity(nil, self_id(nil), 1, 2, 3)
        assert(velocity(nil, self_id(nil)).y == 2)
        set_bus_volume(nil, 'bgm', 0.125)
        play_sound(nil, 'candidate.wav', 1, 1)
        table.concat = function() return 'candidate library' end
        function update(ctx)
            local id = self_id(ctx)
            ui_set_text(ctx, id, ui_text(ctx, id) .. 'A')
        end
        "#,
    );
    scripts.write(
        "b.lua",
        "function update(ctx) end; error('last mount rejected')",
    );

    let assert_no_audio_effect = |runtime: &mut ScriptRuntime| {
        runtime.finish_audio_frame(xgc_world::AudioMixer::default(), |mix, _, commands| {
            assert_eq!(mix.bgm, 1.0);
            assert_eq!(commands.len(), 0);
        });
    };
    let error = runtime.reload_all().unwrap_err();
    assert!(error.contains("b.lua") && error.contains("last mount rejected"));
    assert_no_audio_effect(&mut runtime);
    step(&mut runtime, &mut world);
    assert_eq!(world.get::<UiElement>(entity).unwrap().text, "abab");
    assert_eq!(world.len(), 1);
    assert_eq!(world.drain_body_velocities().next(), None);

    scripts.write(
        "b.lua",
        r#"
        function update(ctx)
            local id = self_id(ctx)
            ui_set_text(ctx, id, ui_text(ctx, id) .. 'B')
        end
        "#,
    );
    runtime.reload_all().unwrap();
    assert_no_audio_effect(&mut runtime);
    step(&mut runtime, &mut world);
    assert_eq!(world.get::<UiElement>(entity).unwrap().text, "ababAB");
    assert_eq!(world.len(), 1);
    assert_eq!(world.drain_body_velocities().next(), None);
}

#[test]
fn changed_mounts_retry_together_and_keep_untouched_module_locals() {
    let scripts = Scripts::new("poll_reload");
    let append = |label: &str| {
        format!(
            "function update(ctx) local id = self_id(ctx); \
             ui_set_text(ctx, id, ui_text(ctx, id) .. '{label}') end"
        )
    };
    scripts.write("a.lua", &append("a"));
    scripts.write("b.lua", &append("b"));
    scripts.write(
        "counter.lua",
        r#"
        local count = 0
        function update(ctx)
            count = count + 1
            local id = self_id(ctx)
            ui_set_text(ctx, id, ui_text(ctx, id) .. tostring(count))
        end
        "#,
    );
    let mut world = World::new();
    let entity = world.spawn((
        UiElement::default(),
        mounts(&["a.lua", "b.lua", "counter.lua"]),
    ));
    let mut runtime = scripts.runtime();
    step(&mut runtime, &mut world);
    assert_eq!(runtime.poll_reload(), Ok(0));

    scripts.write("a.lua", &append("first"));
    std::fs::remove_file(scripts.0.join("b.lua")).unwrap();
    assert!(runtime.poll_reload().unwrap_err().contains("b.lua"));
    assert_eq!(runtime.poll_reload(), Ok(0));
    step(&mut runtime, &mut world);
    assert_eq!(world.get::<UiElement>(entity).unwrap().text, "ab1ab2");

    scripts.write("b.lua", &append("second"));
    assert_eq!(runtime.poll_reload(), Ok(2));
    assert_eq!(runtime.poll_reload(), Ok(0));
    step(&mut runtime, &mut world);
    assert_eq!(
        world.get::<UiElement>(entity).unwrap().text,
        "ab1ab2firstsecond3"
    );
}

#[test]
fn failed_mount_does_not_overwrite_the_other_mounts_commands() {
    let scripts = Scripts::new("failed_mount");
    scripts.write("first.lua", "ui_set_text(nil, self_id(nil), 'ready')");
    scripts.write(
        "bad.lua",
        "ui_set_text(nil, self_id(nil), 'wrong'); error('load error')",
    );
    scripts.write(
        "last.lua",
        "function update(ctx) assert(ui_text(ctx, self_id(ctx)) == 'ready') end",
    );
    let mut world = World::new();
    let entity = world.spawn((
        UiElement::default(),
        mounts(&["first.lua", "bad.lua", "last.lua"]),
    ));
    let id = world.entity(entity).unwrap();
    let mut runtime = scripts.runtime();
    runtime.run_frame(&mut world, 1.0 / 60.0);
    let error = runtime.take_error().unwrap();
    assert!(
        error.contains("bad.lua")
            && error.contains(&format!("实体 {id}"))
            && error.contains("load error"),
        "{error}"
    );
    assert_eq!(world.get::<UiElement>(entity).unwrap().text, "ready");
    assert_eq!(runtime.entity_count(), 1);
    assert_eq!(runtime.script_count(), 2);

    scripts.write(
        "bad.lua",
        "function start(ctx) ui_set_text(ctx, self_id(ctx), 'ready') end",
    );
    step(&mut runtime, &mut world);
    assert_eq!(runtime.script_count(), 3);
}
