//! 用可控时钟检查窗口 Play 对外部脚本保存的处理。

use std::time::{SystemTime, UNIX_EPOCH};

use super::*;
use xgc_world::{Component, Script, Transform, Value, World};

#[test]
fn playing_editor_reloads_changed_scripts_between_steps() {
    let root = std::env::temp_dir().join(format!(
        "xgc_play_reload_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let script = root.join("counter.lua");
    std::fs::write(root.join("counter.lua.meta"), r#"(guid: "counter")"#).unwrap();
    std::fs::write(
        &script,
        r#"
        function start(ctx) state_set(ctx, 'count', 0) end
        function update(ctx)
            local n = state_get(ctx, 'count') + 1
            state_set(ctx, 'count', n)
            set_pos(ctx, n, 0, 0)
        end
        "#,
    )
    .unwrap();
    let mut world = World::new();
    let key = world.spawn((
        Transform::default(),
        Script {
            paths: vec!["guid://counter".into()],
        },
    ));
    let entity = world.entity(key).unwrap();
    let scene = root.join("scene.json");
    xgc_store::save_world(&scene, &world.world_data()).unwrap();
    let original = std::fs::read(&scene).unwrap();
    let mut editor = EditorState::open(&scene).unwrap();
    editor.select(Some(entity));
    editor
        .edit_field(
            entity,
            Transform::ID.0,
            "position",
            Value::Vec3([7.0, 0.0, 0.0]),
        )
        .unwrap();
    let edit_world = editor.world().world_data();
    let mut app = EditorApp::new(editor);
    app.state.play().unwrap();

    let ctx = egui::Context::default();
    let start = Instant::now();
    let fixed = Duration::from_secs_f32(1.0 / 60.0);
    let repaint = |app: &mut EditorApp, elapsed| {
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            let now = start + elapsed;
            app.poll_play_scripts(ctx, now);
            app.tick_play(ctx, now, None);
        });
    };
    let x = |app: &EditorApp| app.state.entity_position(entity).unwrap()[0];
    repaint(&mut app, Duration::ZERO);
    repaint(&mut app, fixed);
    assert_eq!(x(&app), 1.0);

    std::fs::write(&script, "error('fixture reload error')").unwrap();
    repaint(&mut app, fixed * 2);
    assert!(app.status.is_none(), "扫描间隔内不读脚本");
    assert_eq!(x(&app), 2.0);
    repaint(&mut app, Duration::from_secs(1));
    let error = app.status.as_deref().unwrap_or_default();
    assert!(
        error.contains("guid://counter") && error.contains("fixture reload error"),
        "{error}"
    );
    assert!(!app.state.is_paused());
    assert_eq!(x(&app), 6.0);

    app.status = None;
    repaint(&mut app, Duration::from_secs(2));
    assert!(app.status.is_none(), "同一失败版本只报告一次");
    assert_eq!(x(&app), 10.0);

    std::fs::write(
        &script,
        r#"
        function start(ctx) state_set(ctx, 'count', 999) end
        function update(ctx)
            local n = state_get(ctx, 'count') + 10
            state_set(ctx, 'count', n)
            set_pos(ctx, n, 0, 0)
        end
        "#,
    )
    .unwrap();
    app.state.pause();
    repaint(&mut app, Duration::from_secs(3));
    assert!(app.state.is_paused());
    assert_eq!(x(&app), 10.0);
    app.state.step().unwrap().unwrap();
    assert_eq!(x(&app), 20.0, "下一步使用新版钩子与原有状态");
    app.state.stop();
    assert_eq!(app.state.world().world_data(), edit_world);
    assert!(app.state.is_dirty());
    assert_eq!(app.state.selected(), Some(entity));
    assert_eq!(std::fs::read(&scene).unwrap(), original);
    std::fs::remove_dir_all(root).unwrap();
}
