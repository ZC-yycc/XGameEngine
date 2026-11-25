//! 原生预制体在独立世界中的实例化和还原回归。

use xgc_editor::EditorState;
use xgc_world::{Name, Transform, World};

#[test]
fn instantiates_native_prefab_and_selects_root() {
    let path = std::env::temp_dir().join(format!(
        "xgc_editor_native_{}.prefab.json",
        std::process::id()
    ));
    let mut asset = World::new();
    asset.spawn(());
    xgc_store::save_prefab(&path, &asset.world_data()).unwrap();
    let mut editor = EditorState::demo();
    let before = editor.entities().len();
    let result = editor.instantiate_prefab_file(&path);
    std::fs::remove_file(path).unwrap();
    let root = result.unwrap();
    assert_eq!(editor.entities().len(), before + 1);
    assert_eq!(editor.selected(), Some(root));
    assert!(editor.is_dirty());
}

#[test]
fn reverts_standalone_prefab_from_its_local_path() {
    let path = std::env::temp_dir().join(format!(
        "xgc_editor_revert_{}.prefab.json",
        std::process::id()
    ));
    let mut asset = World::new();
    let asset_key = asset.spawn((
        Name {
            value: "Pillar".into(),
        },
        Transform::default(),
    ));
    xgc_store::save_prefab(&path, &asset.world_data()).unwrap();

    let mut editor = EditorState::demo();
    let root = editor.instantiate_prefab_file(&path).unwrap();
    editor.set_entity_position(root, [7.0, 8.0, 9.0]).unwrap();
    let count = editor.entities().len();
    asset.get_mut::<Name>(asset_key).unwrap().value = "Updated Pillar".into();
    xgc_store::save_prefab(&path, &asset.world_data()).unwrap();

    let reverted = editor.revert_selected_prefab();
    std::fs::remove_file(path).unwrap();
    assert_eq!(reverted.unwrap(), 1);
    assert_eq!(editor.entities().len(), count);
    assert_eq!(editor.selected(), Some(root));
    assert!(editor
        .entities()
        .iter()
        .any(|entity| entity.id == root && entity.name == "Updated Pillar"));
    assert_eq!(editor.entity_position(root), Some([7.0, 8.0, 9.0]));
}
