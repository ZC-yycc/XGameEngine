use super::*;
use xgc_world::{
    Command, Component, ComponentData, Script, Transform, UiElement, UiInteraction, UiLayout, World,
};

#[test]
fn hud_click_between_fixed_steps_reaches_the_script_once() {
    let root = std::env::temp_dir().join(format!(
        "xgc_play_hud_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("input.ron"),
        r#"InputMap(actions: {"Jump": [Key(KeyJ)]})"#,
    )
    .unwrap();
    std::fs::write(
        root.join("button.lua"),
        r#"
        local clicks, pressed, released, held = 0, 0, 0, 0
        function update(ctx)
            local id = self_id(ctx)
            if action_pressed(ctx, 'Jump') then pressed = pressed + 1 end
            if action_released(ctx, 'Jump') then released = released + 1 end
            if action_down(ctx, 'Jump') then held = held + 1 end
            if ui_clicked(ctx, id) then
                clicks = clicks + 1
                ui_set_text(ctx, id, tostring(clicks))
            end
            set_pos(ctx, pressed, released, held)
        end
        "#,
    )
    .unwrap();
    let mut world = World::new();
    let key = world.spawn((
        UiElement {
            kind: "button".into(),
            text: "0".into(),
            anchor_min: [0.5, 0.5],
            anchor_max: [0.5, 0.5],
            offset_min: [-40.0, -40.0],
            offset_max: [40.0, 40.0],
            ..UiElement::default()
        },
        UiLayout::default(),
        UiInteraction::default(),
        Script {
            path: "button.lua".into(),
        },
    ));
    let entity = world.entity(key).unwrap();
    let plan = world.plan(vec![Command::AddComponent {
        entity,
        component: ComponentData {
            component: Transform::ID,
            fields: Vec::new(),
        },
    }]);
    world.apply(&plan).unwrap();
    let scene = root.join("scene.json");
    xgc_store::save_world(&scene, &world.world_data()).unwrap();
    let mut app = EditorApp::new(EditorState::open(&scene).unwrap());
    app.state.play().unwrap();

    let ctx = egui::Context::default();
    let start = Instant::now();
    let fixed = Duration::from_secs_f32(1.0 / 60.0);
    let viewport = egui::Rect::from_min_size(egui::pos2(250.0, 70.0), egui::vec2(640.0, 360.0));
    let cursor = viewport.center();
    let repaint = |app: &mut EditorApp, time, events| {
        let _ = ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| app.tick_play(ctx, time, Some(viewport)),
        );
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let jump = |pressed| egui::Event::Key {
        key: egui::Key::J,
        physical_key: Some(egui::Key::J),
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let text = |app: &EditorApp| {
        let world = app.state.world();
        world
            .get::<UiElement>(world.key(entity).unwrap())
            .unwrap()
            .text
            .clone()
    };
    repaint(&mut app, start, Vec::new());
    repaint(
        &mut app,
        start + Duration::from_millis(2),
        vec![
            egui::Event::PointerMoved(cursor),
            button(cursor, true),
            jump(true),
        ],
    );
    repaint(
        &mut app,
        start + Duration::from_millis(4),
        vec![button(cursor, false), jump(false)],
    );
    assert_eq!(text(&app), "0");
    assert_eq!(app.state.entity_position(entity), Some([0.0; 3]));
    repaint(&mut app, start + fixed * 3, Vec::new());
    assert_eq!(
        text(&app),
        "1",
        "固定步应消费且只消费一次待处理点击，状态：{:?}",
        app.status
    );
    assert_eq!(app.state.entity_position(entity), Some([1.0, 1.0, 0.0]));
    let interaction = |app: &EditorApp| {
        let world = app.state.world();
        *world
            .get::<UiInteraction>(world.key(entity).unwrap())
            .unwrap()
    };
    let captured = |app: &EditorApp| app.state.world().ui_pointer_capture().is_some();
    let outside = viewport.right_bottom() + egui::vec2(12.0, 12.0);
    repaint(&mut app, start + fixed * 4, vec![button(cursor, true)]);
    assert!(interaction(&app).pressed, "固定步应接收原生按下边沿");
    assert!(captured(&app));
    repaint(
        &mut app,
        start + fixed * 5,
        vec![egui::Event::PointerMoved(outside)],
    );
    assert!(interaction(&app).pressed);
    assert!(!interaction(&app).hovered);
    repaint(&mut app, start + fixed * 6, vec![button(outside, false)]);
    assert!(!captured(&app), "在视口外释放必须清掉捕获句柄");
    assert!(!interaction(&app).pressed);
    assert!(!interaction(&app).clicked);
    assert_eq!(text(&app), "1");
    repaint(
        &mut app,
        start + fixed * 7,
        vec![egui::Event::PointerMoved(cursor)],
    );
    assert!(interaction(&app).hovered);
    assert!(!interaction(&app).pressed);

    // 已被 egui 识别为拖动，也应允许回到捕获目标并在原目标上松开。
    repaint(
        &mut app,
        start + fixed * 8,
        vec![
            button(cursor, true),
            egui::Event::PointerMoved(outside),
            egui::Event::PointerMoved(cursor),
            button(cursor, false),
        ],
    );
    assert_eq!(text(&app), "2");

    repaint(&mut app, start + fixed * 9, vec![button(cursor, true)]);
    assert!(captured(&app));
    repaint(
        &mut app,
        start + fixed * 10,
        vec![egui::Event::WindowFocused(false)],
    );
    assert!(!captured(&app));
    repaint(
        &mut app,
        start + fixed * 11,
        vec![egui::Event::WindowFocused(true), button(cursor, false)],
    );
    assert_eq!(text(&app), "2", "失焦前的按下不能组成点击");

    repaint(
        &mut app,
        start + fixed * 12,
        vec![button(cursor, true), jump(true)],
    );
    assert!(captured(&app));
    let before_pause = app.state.entity_position(entity).unwrap();
    assert_eq!(before_pause, [2.0, 1.0, 1.0]);
    app.state.pause();
    repaint(
        &mut app,
        start + fixed * 13,
        vec![button(cursor, false), jump(false), jump(true)],
    );
    assert!(app.state.step().unwrap().is_some());
    assert!(!captured(&app), "暂停后的手动步进必须清掉旧捕获");
    assert!(!interaction(&app).pressed);
    assert_eq!(text(&app), "2");
    assert_eq!(app.state.entity_position(entity), Some(before_pause));
    app.state.resume();
    repaint(&mut app, start + fixed * 14, Vec::new());
    repaint(&mut app, start + fixed * 15, Vec::new());
    assert_eq!(app.state.entity_position(entity), Some([2.0, 1.0, 2.0]));
    assert_eq!(app.state.world().len(), 1);
    app.state.stop();
    repaint(&mut app, start + fixed * 16, Vec::new());
    assert_eq!(text(&app), "0");
    assert_eq!(app.state.entity_position(entity), Some([0.0; 3]));
    std::fs::remove_dir_all(root).unwrap();
}
