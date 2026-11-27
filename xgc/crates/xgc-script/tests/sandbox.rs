use std::cell::RefCell;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use xgc_script::{LuaHost, ScriptRuntime, ScriptView};
use xgc_world::{Script, Transform, Velocity, World};

#[test]
fn runtime_limits_each_hook_and_reuses_the_vm_after_a_failure() {
    let dir = std::env::temp_dir().join(format!(
        "xgc_lua_sandbox_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("budget.lua");
    std::fs::write(
        &source,
        r#"
        local calls = 0
        function update(ctx)
            calls = calls + 1
            if calls == 1 then
                local total = 0
                for i = 1, 200000 do total = total + i end
            end
            xgc.set(ctx.entity, 'core.transform', 'position', {x=calls, y=0, z=0})
        end
        function fixed_update(ctx)
            local total = 0
            for i = 1, 20000 do total = total + i end
            xgc.set(ctx.entity, 'core.velocity', 'linear', {x=3, y=0, z=0})
        end
        "#,
    )
    .unwrap();
    let mut world = World::new();
    let key = world.spawn((
        Transform::default(),
        Velocity::default(),
        Script {
            paths: vec!["budget.lua".into()],
        },
    ));
    let mut runtime = ScriptRuntime::new(&dir);
    let dt = 1.0 / 60.0;
    runtime.run_frame(&mut world, dt);
    let first_error = runtime.take_error();
    let first_position = world.get::<Transform>(key).unwrap().position;
    let first_velocity = world.get::<Velocity>(key).unwrap().linear;
    runtime.run_frame(&mut world, dt);
    std::fs::remove_dir_all(dir).unwrap();

    let error = first_error.expect("项目脚本的 update 应有默认指令预算");
    assert!(
        error.contains("budget.lua") && error.contains("update") && error.contains("预算超限"),
        "{error}"
    );
    assert_eq!(first_position, [0.0; 3]);
    assert_eq!(first_velocity, [3.0, 0.0, 0.0]);
    assert!(runtime.take_error().is_none());
    assert_eq!(
        world.get::<Transform>(key).unwrap().position,
        [2.0, 0.0, 0.0]
    );
}

#[test]
fn coroutine_work_counts_toward_the_call_budget() {
    let host = LuaHost::new().unwrap();
    let error = host
        .run(
            "coroutine.lua",
            r#"
            local worker = coroutine.wrap(function()
                local total = 0
                for i = 1, 200000 do total = total + i end
                return total
            end)
            worker()
            "#,
        )
        .unwrap_err();
    assert!(error.contains("预算超限"), "{error}");
    assert_eq!(host.eval::<i32>("return 4 + 5").unwrap(), 9);
}

#[test]
fn coroutine_resume_and_protected_calls_share_the_same_budget() {
    let host = LuaHost::new().unwrap();
    host.run(
        "yield.lua",
        r#"
        worker = coroutine.create(function(value)
            local resumed = coroutine.yield(value + 1)
            return resumed * 2
        end)
        success, yielded = coroutine.resume(worker, 4)
        assert(success and yielded == 5)
        "#,
    )
    .unwrap();
    assert_eq!(
        host.eval::<(bool, i32)>("return coroutine.resume(worker, 6)")
            .unwrap(),
        (true, 12)
    );
    let error = host
        .run(
            "shared.lua",
            r#"
            local function work()
                local total = 0
                for i = 1, 60000 do total = total + i end
                return total
            end
            assert(coroutine.resume(coroutine.create(work)))
            pcall(function() coroutine.resume(coroutine.create(work)) end)
            continued_after_limit = true
            "#,
        )
        .unwrap_err();
    assert!(error.contains("预算超限"), "{error}");
    assert!(host.global_is_nil("continued_after_limit").unwrap());
    assert_eq!(host.eval::<i32>("return 4 + 5").unwrap(), 9);
}

#[test]
fn base_library_limits_dynamic_loading_to_text_without_file_access() {
    let host = LuaHost::new().unwrap();
    assert!(host.global_is_nil("dofile").unwrap());
    assert!(host.global_is_nil("loadfile").unwrap());
    host.run(
        "text.lua",
        r#"
        local binary = string.dump(function() return 7 end)
        assert(load(binary) == nil)
        assert(load(binary, 'binary', 'b') == nil)
        local reader = coroutine.wrap(function()
            coroutine.yield('return ')
            coroutine.yield('value')
        end)
        assert(load(reader, 'inline', 't', {value = 9})() == 9)
        assert(load('return math.abs(-7)')() == 7)
        "#,
    )
    .unwrap();
}

#[test]
fn project_vm_installs_memory_limit_and_recovers_after_rejection() {
    let host = LuaHost::with_commands_and_view(
        Rc::new(RefCell::new(Vec::new())),
        Rc::new(RefCell::new(ScriptView::default())),
        0,
    )
    .unwrap();
    let previous = host.set_memory_limit(512 * 1024).unwrap();
    assert_eq!(previous, 128 * 1024 * 1024);
    host.run(
        "memory.lua",
        r#"
        local calls = 0
        function update(ctx)
            calls = calls + 1
            if calls == 1 then
                local oversized = string.rep('x', 2 * 1024 * 1024)
            end
        end
        function count() return calls end
        "#,
    )
    .unwrap();
    let error = host.update_legacy(1.0 / 60.0).unwrap_err();
    assert!(
        error.contains("脚本内存超限") && error.contains("512 KiB"),
        "{error}"
    );
    assert!(host.update_legacy(1.0 / 60.0).unwrap());
    assert_eq!(host.call::<u32>("count", ()).unwrap(), 2);
}
