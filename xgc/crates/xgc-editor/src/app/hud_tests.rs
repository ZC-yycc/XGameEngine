use super::*;
use xgc_world::{Script, UiElement, UiInteraction, UiLayout, World};

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
        root.join("button.lua"),
        "local clicks = 0\nfunction update(ctx)\n  local id = self_id(ctx)\n  if ui_clicked(ctx, id) then\n    clicks = clicks + 1\n    ui_set_text(ctx, id, tostring(clicks))\n  end\nend\n",
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
    let click = |pressed| egui::Event::PointerButton {
        pos: cursor,
        button: egui::PointerButton::Primary,
        pressed,
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
        vec![egui::Event::PointerMoved(cursor), click(true)],
    );
    repaint(
        &mut app,
        start + Duration::from_millis(4),
        vec![click(false)],
    );
    assert_eq!(text(&app), "0");
    repaint(&mut app, start + fixed * 3, Vec::new());
    assert_eq!(
        text(&app),
        "1",
        "固定步应消费且只消费一次待处理点击，状态：{:?}",
        app.status
    );
    assert_eq!(app.state.world().len(), 1);
    app.state.stop();
    assert_eq!(text(&app), "0");
    std::fs::remove_dir_all(root).unwrap();
}
