//! Inspector 字段编辑走世界事务，历史快照和落盘都保留原生事件。

use xgc_base::EntityId;
use xgc_editor::EditorState;
use xgc_reflect::{Component, Value};
use xgc_world::{AnimEvent, Animator, World};

fn seeded_animator() -> (EditorState, EntityId) {
    let mut world = World::new();
    let key = world.spawn((Animator {
        events: vec![AnimEvent::new("footstep", 0.25)],
        ..Animator::default()
    },));
    let entity = world.entity(key).unwrap();
    let mut editor = EditorState::new(world);
    editor.select(Some(entity));
    (editor, entity)
}

fn events(editor: &EditorState, entity: EntityId) -> &[AnimEvent] {
    let world = editor.world();
    &world
        .get::<Animator>(world.key(entity).unwrap())
        .unwrap()
        .events
}

#[test]
fn animator_event_edit_undo_redo_and_reopen() {
    let (mut editor, entity) = seeded_animator();
    let before = editor.world().world_data();
    editor
        .edit_field(entity, Animator::ID.0, "events[0].time", Value::F64(0.75))
        .unwrap();
    assert_eq!(events(&editor, entity), &[AnimEvent::new("footstep", 0.75)]);
    assert!(editor.is_dirty());
    assert_eq!(editor.selected(), Some(entity));

    assert!(editor.undo().unwrap());
    assert_eq!(editor.world().world_data(), before);
    assert!(!editor.can_undo(), "一次字段编辑只记录一个撤销点");
    assert!(editor
        .edit_field(entity, Animator::ID.0, "events[0].time", Value::Bool(false))
        .is_err());
    assert_eq!(editor.world().world_data(), before);
    assert!(!editor.can_undo());
    assert!(editor.redo().unwrap(), "失败的编辑不能丢掉重做记录");
    assert_eq!(events(&editor, entity), &[AnimEvent::new("footstep", 0.75)]);

    let dir = std::env::temp_dir().join(format!(
        "xgc_editor_inspector_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("world.json");
    editor.save_as(&path).unwrap();
    assert!(!editor.is_dirty());
    let mut reopened = EditorState::open(&path).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    reopened.select(Some(entity));
    assert_eq!(
        events(&reopened, entity),
        &[AnimEvent::new("footstep", 0.75)]
    );
}

#[test]
fn animator_event_invalid_edit_preserves_world_and_history() {
    let (mut editor, entity) = seeded_animator();
    let before = editor.world().world_data();
    assert!(editor
        .edit_field(entity, Animator::ID.0, "events[1].time", Value::F64(1.0))
        .is_err());
    assert_eq!(editor.world().world_data(), before);
    assert_eq!(editor.selected(), Some(entity));
    assert!(!editor.is_dirty());
    assert!(!editor.undo().unwrap(), "失败的编辑不能记录撤销点");
    assert!(!editor.can_redo());
}
