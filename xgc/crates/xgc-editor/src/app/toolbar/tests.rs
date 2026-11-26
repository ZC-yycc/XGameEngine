//! 用 egui 事件走工具栏与运行时钟，回归真实按钮的状态转换。

use std::time::{Duration, Instant};

use super::*;
use crate::EditorState;
use xgc_world::{Transform, Velocity, World};

#[test]
fn toolbar_controls_drive_the_play_clock() {
    let mut world = World::new();
    let key = world.spawn((
        Transform::default(),
        Velocity {
            linear: [60.0, 0.0, 0.0],
        },
    ));
    let entity = world.entity(key).unwrap();
    let mut app = EditorApp::new(EditorState::new(world));
    let ctx = egui::Context::default();
    let render = |app: &mut EditorApp, now, events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 720.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::TopBottomPanel::top("xgc-toolbar").show(ctx, |ui| app.toolbar(ui));
                app.tick_play(ctx, now);
            },
        )
    };
    let click = |app: &mut EditorApp, label, now| {
        let output = render(app, now, Vec::new());
        let pos = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("工具栏没有 {label}"));
        for pressed in [true, false] {
            render(
                app,
                now,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    };
    let position = |app: &EditorApp| app.state.entity_position(entity).unwrap()[0];
    let fixed = Duration::from_secs_f32(1.0 / 60.0);
    let start = Instant::now();

    click(&mut app, "Play", start);
    assert!(app.state.is_playing());
    assert_eq!(position(&app), 0.0);
    let first_step = start + fixed;
    render(&mut app, first_step, Vec::new());
    assert_eq!(position(&app), 1.0);

    click(&mut app, "Pause", first_step);
    assert!(app.state.is_paused());
    assert!(app.last_play_update.is_none());
    let later = start + Duration::from_secs(10);
    render(&mut app, later, Vec::new());
    assert_eq!(position(&app), 1.0);
    click(&mut app, "Step", later);
    assert_eq!(position(&app), 2.0);

    click(&mut app, "Resume", later);
    assert!(!app.state.is_paused());
    assert_eq!(position(&app), 2.0);
    render(&mut app, later + fixed, Vec::new());
    assert_eq!(position(&app), 3.0);
    click(&mut app, "Stop", later + fixed);
    assert!(!app.state.is_playing());
    assert_eq!(position(&app), 0.0);
}
