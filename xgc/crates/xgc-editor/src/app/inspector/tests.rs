use std::collections::BTreeMap;

use egui::epaint::Shape;
use xgc_base::EntityId;
use xgc_reflect::{Component, ComponentId, FieldDescriptor, FieldPath, FieldType};
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
            blend_elapsed: 42.75,
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
    // 对齐整值，让断言测步长，而不是 egui 的智能舍入。
    let end = start + egui::vec2(35.0, 0.0);
    inspector.pointer_button(start, true);
    inspector.render(vec![egui::Event::PointerMoved(end)]);
    inspector.pointer_button(end, false);

    assert_eq!(inspector.events()[0].name, "footstep");
    assert_eq!(inspector.events()[0].time, 2.0);
    assert_eq!(inspector.app.state.world().revision(), before + 1);
    assert!(inspector.app.state.undo().unwrap());
    assert_eq!(inspector.events(), &[AnimEvent::new("footstep", 0.25)]);
    assert!(!inspector.app.state.can_undo());
}

#[test]
fn inspector_clamps_dragged_event_time_to_the_schema_range() {
    let mut inspector = Inspector::seeded();
    let output = inspector.render(Vec::new());
    let start = text_position(&output, "0.25");
    let end = start - egui::vec2(80.0, 0.0);
    inspector.pointer_button(start, true);
    inspector.render(vec![egui::Event::PointerMoved(end)]);
    inspector.pointer_button(end, false);

    assert_eq!(inspector.events()[0].time, 0.0);
    assert!(inspector.app.state.undo().unwrap());
    assert_eq!(inspector.events()[0].time, 0.25);
}

#[test]
fn inspector_does_not_rewrite_out_of_range_snapshot_values() {
    let ctx = egui::Context::default();
    let schema = FieldSchema::from(
        FieldDescriptor::new("weight", FieldType::F64, Value::F64(0.0)).range(0.0, 1.0),
    );
    render(&ctx, vec![], |ui| {
        assert!(field_row(ui, &schema, &Value::F64(1.5), "weight", "weight", true).is_none());
    });
}

#[test]
fn inspector_clamps_keyboard_edits_to_the_schema_range() {
    let ctx = egui::Context::default();
    let schema = FieldSchema::from(
        FieldDescriptor::new("weight", FieldType::F64, Value::F64(0.0))
            .range(0.0, 1.0)
            .step(0.05),
    );
    let mut edited = None;
    let mut draw = |ui: &mut egui::Ui| {
        edited = value_editor(ui, &Value::F64(0.02), &schema);
    };
    let initial = render(&ctx, vec![], &mut draw);
    let pos = text_position(&initial, "0.02");
    render(&ctx, button_events(pos, true), &mut draw);
    render(&ctx, button_events(pos, false), &mut draw);
    render(
        &ctx,
        vec![egui::Event::Key {
            key: egui::Key::ArrowDown,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        &mut draw,
    );
    assert_eq!(edited, Some(Value::F64(0.0)));
}

#[test]
fn inspector_leaves_runtime_blend_state_read_only() {
    let mut inspector = Inspector::seeded();
    let before = inspector.app.state.world().world_data();
    let output = inspector.render(Vec::new());
    let start = text_position(&output, "42.75");
    let end = start + egui::vec2(30.0, 0.0);
    inspector.pointer_button(start, true);
    inspector.render(vec![egui::Event::PointerMoved(end)]);
    inspector.pointer_button(end, false);

    assert_eq!(inspector.app.state.world().world_data(), before);
    assert!(!inspector.app.state.can_undo());
    let headings = output.shapes.iter().filter(
        |shape| matches!(&shape.shape, Shape::Text(text) if text.galley.text() == "Blend State"),
    );
    assert_eq!(headings.count(), 1);
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

const ESCAPED_KEY: &str = "a.\"b\\c]";

#[derive(Clone)]
struct Containers {
    entries: Value,
    mode: Value,
}

impl Component for Containers {
    const ID: ComponentId = ComponentId(0xffff);
    const NAME: &'static str = "test.containers";

    fn default_value() -> Self {
        Self {
            entries: Value::Map(BTreeMap::from([(ESCAPED_KEY.to_string(), weight(4.55))])),
            mode: mode("Disabled", Value::Struct(BTreeMap::new())),
        }
    }

    fn fields() -> Vec<FieldDescriptor> {
        let weight_field = FieldDescriptor::new("weight", FieldType::F64, Value::F64(0.25));
        let entry =
            FieldDescriptor::new("entry", FieldType::Struct(vec![weight_field]), weight(0.25));
        let disabled = FieldDescriptor::new(
            "Disabled",
            FieldType::Struct(vec![]),
            Value::Struct(BTreeMap::new()),
        );
        let threshold = FieldDescriptor::new("threshold", FieldType::F64, Value::F64(0.75));
        let tuned = FieldDescriptor::new("Tuned", FieldType::Struct(vec![threshold]), tuning(0.75));
        let default = Self::default_value();
        vec![
            FieldDescriptor::new("entries", FieldType::Map(Box::new(entry)), default.entries),
            FieldDescriptor::new("mode", FieldType::Enum(vec![disabled, tuned]), default.mode),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "entries" => Some(self.entries.clone()),
            "mode" => Some(self.mode.clone()),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match (field, value) {
            ("entries", Value::Map(_)) => self.entries = value.clone(),
            ("mode", Value::Enum { .. }) => self.mode = value.clone(),
            _ => return Err(format!("无效的测试字段：{field}")),
        }
        Ok(())
    }
}

fn weight(value: f64) -> Value {
    Value::Struct(BTreeMap::from([("weight".into(), Value::F64(value))]))
}

fn tuning(value: f64) -> Value {
    Value::Struct(BTreeMap::from([("threshold".into(), Value::F64(value))]))
}

fn mode(variant: &str, payload: Value) -> Value {
    Value::Enum {
        variant: variant.to_string(),
        payload: Box::new(payload),
    }
}

impl Inspector {
    fn with_containers() -> Self {
        let mut world = World::new();
        world.register_component::<Containers>();
        let key = world.spawn((Containers::default_value(),));
        let entity = world.entity(key).unwrap();
        let mut state = EditorState::new(world);
        state.registry.register::<Containers>().unwrap();
        state.select(Some(entity));
        let ctx = egui::Context::default();
        ctx.style_mut(|style| style.animation_time = 0.0);
        Self {
            app: EditorApp::new(state),
            ctx,
            entity,
        }
    }

    fn container_value(&self, path: &str) -> Value {
        let world = self.app.state.world();
        world.get_field(self.entity, Containers::ID, path).unwrap()
    }

    fn click_text(&mut self, text: &str) {
        let output = self.render(Vec::new());
        self.click(text_position(&output, text));
    }

    fn drag_right(&mut self, text: &str) {
        let output = self.render(Vec::new());
        let start = text_position(&output, text);
        let end = start + egui::vec2(30.0, 0.0);
        self.pointer_button(start, true);
        self.render(vec![egui::Event::PointerMoved(end)]);
        self.pointer_button(end, false);
    }
}

#[test]
fn inspector_map_edits_escaped_key_and_adds_or_removes_entries() {
    let mut inspector = Inspector::with_containers();
    let before = inspector.app.state.world().world_data();
    let revision = inspector.app.state.world().revision();
    let escaped = FieldPath::key_segment(ESCAPED_KEY);

    inspector.drag_right("4.55");
    let updated = inspector.container_value(&format!("entries{escaped}.weight"));
    assert!(matches!(updated, Value::F64(value) if value > 4.55));

    inspector.click_text("Key");
    inspector.render(vec![egui::Event::Text("second".into())]);
    inspector.click_text("Add entry");
    assert_eq!(
        inspector.container_value("entries[\"second\"]"),
        weight(0.25)
    );

    inspector.click_text("Key");
    inspector.render(vec![egui::Event::Text("second".into())]);
    inspector.click_text("Add entry");
    assert_eq!(inspector.app.state.world().revision(), revision + 2);

    inspector.click_text(&format!("Remove {escaped}"));
    assert_eq!(
        inspector.container_value("entries"),
        Value::Map(BTreeMap::from([("second".into(), weight(0.25))])),
    );
    assert_eq!(inspector.app.state.world().revision(), revision + 3);
    assert_eq!(inspector.app.state.undo.len(), 3);
    assert_eq!(inspector.app.state.undo.first(), Some(&before));
}

#[test]
fn inspector_enum_switches_and_edits_active_payload() {
    let mut inspector = Inspector::with_containers();
    let revision = inspector.app.state.world().revision();

    inspector.click_text("Disabled");
    inspector.click_text("Tuned");
    assert_eq!(
        inspector.container_value("mode"),
        mode("Tuned", tuning(0.75))
    );

    inspector.drag_right("0.75");
    let updated = inspector.container_value("mode.Tuned.threshold");
    assert!(matches!(updated, Value::F64(value) if value > 0.75));

    inspector.click_text("Tuned");
    inspector.click_text("Disabled");
    assert_eq!(
        inspector.container_value("mode"),
        Containers::default_value().mode
    );

    inspector.click_text("Disabled");
    inspector.click_text("Tuned");
    assert_eq!(
        inspector.container_value("mode"),
        mode("Tuned", tuning(0.75))
    );
    assert_eq!(inspector.app.state.world().revision(), revision + 4);
    assert_eq!(inspector.app.state.undo.len(), 4);
}

#[test]
fn inspector_container_controls_are_disabled_in_play() {
    let mut inspector = Inspector::with_containers();
    let state = &mut inspector.app.state;
    let world = std::mem::take(&mut state.world);
    let before = world.world_data();
    // 测试组件只注册在夹具世界里，直接用该世界构造播放副本。
    state.play = Some(crate::PlaySession {
        selected: state.selected,
        simulation: xgc_sim::Simulation::new(world),
        particles: xgc_sim::ParticleSystem::new(),
        runtime: xgc_script::ScriptRuntime::new("."),
        input: None,
        paused: false,
        elapsed: std::time::Duration::ZERO,
        root_motion: xgc_world::RootMotionLibrary::new(),
    });

    inspector.drag_right("4.55");
    inspector.click_text("Key");
    inspector.render(vec![egui::Event::Text("second".into())]);
    inspector.click_text("Add entry");
    inspector.click_text(&format!("Remove {}", FieldPath::key_segment(ESCAPED_KEY)));
    inspector.click_text("Disabled");
    let output = inspector.render(Vec::new());
    assert!(!output.shapes.iter().any(|shape| {
        matches!(&shape.shape, Shape::Text(text) if text.galley.text() == "Tuned")
    }));
    assert_eq!(inspector.app.state.world().world_data(), before);
    assert!(!inspector.app.state.can_undo());
    assert!(inspector.app.status.is_none());
}
