//! 编辑器状态模型回归：选择 / Inspector / 编辑 / 保存 / Play 快照恢复。

use xgc_base::EntityId;
use xgc_editor::EditorState;
use xgc_reflect::Value;
use xgc_world::{Name, Transform, World};

fn demo() -> (EditorState, EntityId) {
    let mut world = World::new();
    let entity = world.spawn((
        Name {
            value: "hero".to_string(),
        },
        Transform::default(),
    ));
    let id = world.entity(entity).unwrap();
    (EditorState::new(world), id)
}

#[test]
fn lists_names_and_inspects_components() {
    let (mut editor, id) = demo();
    let rows = editor.entities();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "hero");
    editor.select(Some(id));
    let inspector = editor.inspector();
    assert!(inspector.iter().any(|row| row.name == "core.transform"));
    assert!(inspector.iter().any(|row| row.name == "core.name"));
}

#[test]
fn edits_fields_and_saves_roundtrip() {
    let (mut editor, id) = demo();
    editor
        .edit_field(id, 0x0101, "position", Value::Vec3([1.0, 2.0, 3.0]))
        .unwrap();
    assert!(editor.is_dirty());
    let dir = std::env::temp_dir().join(format!(
        "xgc_editor_{}_{}",
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
    let loaded = EditorState::open(&path).unwrap();
    assert_eq!(loaded.entities()[0].name, "hero");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn play_runs_and_stop_restores_snapshot() {
    let (mut editor, id) = demo();
    editor.play().unwrap();
    assert!(editor.is_playing());
    assert_eq!(editor.step().unwrap().unwrap().frame, 0);
    assert!(editor
        .edit_field(id, 0x0101, "position", Value::Vec3([9.0, 0.0, 0.0]))
        .is_err());
    editor.stop().unwrap();
    assert!(!editor.is_playing());
    assert_eq!(editor.selected(), None);
}

#[test]
fn spawns_and_deletes_entities() {
    let mut editor = EditorState::default();
    let id = editor.spawn_entity("box").unwrap();
    assert_eq!(editor.entities().len(), 1);
    assert_eq!(editor.selected(), Some(id));
    editor.delete_selected().unwrap();
    assert_eq!(editor.entities().len(), 0);
    assert_eq!(editor.selected(), None);
}

#[test]
fn adds_and_removes_components() {
    let mut editor = EditorState::default();
    let id = editor.spawn_entity("box").unwrap();
    editor.add_component(id, "core.velocity").unwrap();
    let names: Vec<_> = editor
        .inspector()
        .into_iter()
        .map(|component| component.name)
        .collect();
    assert!(names.contains(&"core.velocity".to_string()));
    editor.remove_component(id, 0x0102).unwrap();
    let names: Vec<_> = editor
        .inspector()
        .into_iter()
        .map(|component| component.name)
        .collect();
    assert!(!names.contains(&"core.velocity".to_string()));
}

#[test]
fn undo_redo_restores_field_edits() {
    let (mut editor, id) = demo();
    editor
        .edit_field(id, 0x0101, "position", Value::Vec3([5.0, 6.0, 7.0]))
        .unwrap();
    assert!(editor.undo().unwrap());
    assert_eq!(
        editor
            .world()
            .get::<Transform>(editor.world().key(id).unwrap())
            .unwrap()
            .position,
        [0.0, 0.0, 0.0]
    );
    assert!(editor.redo().unwrap());
    assert_eq!(
        editor
            .world()
            .get::<Transform>(editor.world().key(id).unwrap())
            .unwrap()
            .position,
        [5.0, 6.0, 7.0]
    );
}

#[test]
fn undo_restores_spawned_entities() {
    let mut editor = EditorState::default();
    let id = editor.spawn_entity("box").unwrap();
    assert!(editor.undo().unwrap());
    assert!(editor.world().key(id).is_none());
    assert!(editor.redo().unwrap());
    assert!(editor.world().key(id).is_some());
}

#[test]
fn undo_is_rejected_during_play() {
    let (mut editor, _) = demo();
    editor.play().unwrap();
    assert!(editor.undo().is_err());
    assert!(editor.redo().is_err());
}
