use egui::epaint::Shape;
use xgc_base::EntityId;
use xgc_reflect::{FieldDescriptor, FieldType};
use xgc_world::{AnimEvent, Animator, World};

use super::*;
use crate::EditorState;

struct Inspector {
    app: EditorApp,
    ctx: egui::Context,
    entity: EntityId,
}

impl Inspector {
    fn seeded() -> Self {
        let mut world = World::new();
        let key = world.spawn((Animator {
            events: vec![AnimEvent::new("footstep", 0.25)],
            ..Animator::default()
        },));
        let entity = world.entity(key).unwrap();
        let mut state = EditorState::new(world);
        state.select(Some(entity));
        let ctx = egui::Context::default();
        ctx.style_mut(|style| style.animation_time = 0.0);
        Self {
            app: EditorApp::new(state),
            ctx,
            entity,
        }
    }

    fn render(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
        render(&self.ctx, events, |ui| self.app.inspector(ui))
    }

    fn pointer_button(&mut self, pos: egui::Pos2, pressed: bool) {
        self.render(button_events(pos, pressed));
    }

    fn click(&mut self, pos: egui::Pos2) {
        self.pointer_button(pos, true);
        self.pointer_button(pos, false);
    }

    fn events(&self) -> &[AnimEvent] {
        let world = self.app.state.world();
        &world
            .get::<Animator>(world.key(self.entity).unwrap())
            .unwrap()
            .events
    }
}

fn button_events(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

fn render(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    mut draw: impl FnMut(&mut egui::Ui),
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(380.0, 720.0),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, &mut draw);
        },
    )
}

fn text_position(output: &egui::FullOutput, text: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Text(shape) if shape.galley.text() == text => {
                Some(shape.pos + shape.galley.size() * 0.5)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("缺少 Inspector 文本：{text}"))
}

#[test]
fn inspector_drags_nested_event_time() {
    let mut inspector = Inspector::seeded();
    let before = inspector.app.state.world().revision();
    let output = inspector.render(Vec::new());
    text_position(&output, "time (f32, s)");
    let start = text_position(&output, "0.25");
    let end = start + egui::vec2(30.0, 0.0);
    inspector.pointer_button(start, true);
    inspector.render(vec![egui::Event::PointerMoved(end)]);
    inspector.pointer_button(end, false);

    assert_eq!(inspector.events()[0].name, "footstep");
    assert!(inspector.events()[0].time > 0.25);
    assert_eq!(inspector.app.state.world().revision(), before + 1);
    assert!(inspector.app.state.undo().unwrap());
    assert_eq!(inspector.events(), &[AnimEvent::new("footstep", 0.25)]);
    assert!(!inspector.app.state.can_undo());
}

#[test]
fn inspector_list_buttons_replace_list_using_schema_default() {
    let mut inspector = Inspector::seeded();
    let before = inspector.app.state.world().world_data();
    let output = inspector.render(Vec::new());
    text_position(&output, "events (1)");
    inspector.click(text_position(&output, "Add item"));
    assert_eq!(
        inspector.events(),
        &[
            AnimEvent::new("footstep", 0.25),
            AnimEvent::new("event", 0.0)
        ]
    );
    let output = inspector.render(Vec::new());
    text_position(&output, "events (2)");
    inspector.click(text_position(&output, "Remove [0]"));
    assert_eq!(inspector.events(), &[AnimEvent::new("event", 0.0)]);

    assert!(inspector.app.state.undo().unwrap());
    assert_eq!(inspector.events().len(), 2);
    assert!(inspector.app.state.undo().unwrap());
    assert_eq!(inspector.app.state.world().world_data(), before);
    assert!(!inspector.app.state.can_undo());
}

#[test]
fn inspector_adds_a_typed_default_to_a_sublist() {
    let ctx = egui::Context::default();
    ctx.style_mut(|style| style.animation_time = 0.0);
    let flag = FieldDescriptor::new("flag", FieldType::Bool, Value::Bool(true));
    let row = FieldDescriptor::new("row", FieldType::List(Box::new(flag)), Value::List(vec![]));
    let value = Value::List(vec![Value::List(vec![])]);
    let schema = FieldSchema::from(FieldDescriptor::new(
        "flags",
        FieldType::List(Box::new(row)),
        Value::List(vec![]),
    ));
    let mut edit = None;
    let mut draw = |ui: &mut egui::Ui| {
        if let Some(next) = field_row(ui, &schema, &value, "flags", "flags", true) {
            assert!(edit.is_none(), "一次点击只能生成一个编辑命令");
            edit = Some(next);
        }
    };
    let output = render(&ctx, vec![], &mut draw);
    text_position(&output, "[0] (0)");
    // 内层列表的按钮在外层列表按钮之前绘制。
    let add = text_position(&output, "Add item");
    render(&ctx, button_events(add, true), &mut draw);
    render(&ctx, button_events(add, false), &mut draw);
    let edit = edit.unwrap();
    assert_eq!(edit.path, "flags[0]");
    assert_eq!(edit.value, Value::List(vec![Value::Bool(true)]));
}
