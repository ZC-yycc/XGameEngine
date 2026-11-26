//! 原生预制体编辑的文件和场景边界回归。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use xgc_editor::EditorState;
use xgc_store::{
    load_prefab, save_prefab, save_project, save_world, ProjectContent, ProjectManifest,
};
use xgc_world::{Component, Name, PrefabInstance, Transform, Value, World, WorldData};

#[test]
fn applies_native_prefab_values_to_moved_guid_asset() {
    let temp = Temp::new();
    let scene_path = temp.0.join("scenes/main.json");
    let original = temp.0.join("Prefabs/pillar.prefab.json");
    let moved = temp.0.join("Prefabs/moved.prefab.json");
    let original_meta = temp.0.join("Prefabs/pillar.prefab.json.meta");
    let moved_meta = temp.0.join("Prefabs/moved.prefab.json.meta");
    let guid = "0123456789abcdef0123456789abcdef";
    let original_meta_bytes = format!("(guid: \"{guid}\")").into_bytes();
    write_asset(&original);
    fs::write(&original_meta, &original_meta_bytes).unwrap();
    save_world(&scene_path, &WorldData::default()).unwrap();
    save_project(
        &temp.0.join("project.xgc"),
        &ProjectManifest {
            format_version: xgc_store::PROJECT_FORMAT_VERSION,
            name: "demo".into(),
            entry_scene: "scenes/main.json".into(),
            scenes: vec!["scenes/main.json".into()],
            content: ProjectContent {
                models: "Resources".into(),
                textures: "Resources".into(),
                audio: "Resources".into(),
                shaders: None,
                prefabs: "Prefabs".into(),
                scripts: "Scripts".into(),
            },
            search_paths: Vec::new(),
            input: None,
            render: Default::default(),
            quality: Default::default(),
        },
    )
    .unwrap();
    let mut editor = EditorState::open(&temp.0).unwrap();
    let root = editor
        .instantiate_prefab_file(Path::new("Prefabs/pillar.prefab.json"))
        .unwrap();
    editor.save().unwrap();
    let scene_before = fs::read(&scene_path).unwrap();
    let prefab_before = load_prefab(&original).unwrap();
    fs::rename(&original, &moved).unwrap();
    fs::rename(&original_meta, &moved_meta).unwrap();
    editor.set_entity_position(root, [9.0, 8.0, 7.0]).unwrap();
    editor
        .edit_field(root, Name::ID.0, "value", Value::String("changed".into()))
        .unwrap();
    let before = editor.world().world_data();

    assert_eq!(editor.apply_selected_prefab().unwrap(), 1);

    assert_eq!(editor.world().world_data(), before);
    assert!(editor.is_dirty());
    assert_eq!(editor.selected(), Some(root));
    assert_eq!(fs::read(&scene_path).unwrap(), scene_before);
    assert_eq!(fs::read(&moved_meta).unwrap(), original_meta_bytes);
    assert!(!original.exists());
    let prefab_after = load_prefab(&moved).unwrap();
    assert_eq!(prefab_after.entities[0].id, prefab_before.entities[0].id);
    let mut asset = World::new();
    asset.restore_world_data(&prefab_after).unwrap();
    let key = asset.key(prefab_after.entities[0].id).unwrap();
    assert_eq!(asset.get::<Name>(key).unwrap().value, "changed");
    assert_eq!(
        asset.get::<Transform>(key).unwrap().position,
        [2.0, 0.0, 0.0]
    );
    assert!(asset.get::<PrefabInstance>(key).is_none());
    let applied_bytes = fs::read(&moved).unwrap();
    assert_eq!(editor.apply_selected_prefab().unwrap(), 1);
    assert_eq!(fs::read(&moved).unwrap(), applied_bytes);
    assert!(editor.undo().unwrap());
    assert_eq!(editor.entities()[0].name, "Pillar");
    assert_eq!(fs::read(&moved).unwrap(), applied_bytes);
}

#[test]
fn apply_rejects_stale_scene_and_structural_or_play_changes() {
    let temp = Temp::new();
    let asset = temp.0.join("pillar.prefab.json");
    let scene = temp.0.join("scene.json");
    write_asset(&asset);
    let mut editor = EditorState::default();
    let root = editor.instantiate_prefab_file(&asset).unwrap();
    editor.save_as(&scene).unwrap();
    let original = fs::read(&asset).unwrap();
    let scene_original = fs::read(&scene).unwrap();
    let mut changed = scene_original.clone();
    changed.push(b'\n');
    fs::write(&scene, &changed).unwrap();

    let error = editor.apply_selected_prefab().unwrap_err();
    assert!(error.contains("已被外部修改"), "{error}");
    assert_eq!(fs::read(&scene).unwrap(), changed);
    assert_eq!(fs::read(&asset).unwrap(), original);

    // 重新打开会采用最新文件基准，即使变化只是排版。
    let mut editor = EditorState::open(&scene).unwrap();
    editor.select(Some(root));
    assert_eq!(editor.apply_selected_prefab().unwrap(), 1);
    assert_eq!(fs::read(&asset).unwrap(), original);
    editor.add_component(root, "core.velocity").unwrap();
    let before = editor.world().world_data();
    assert!(editor.apply_selected_prefab().is_err());
    assert_eq!(editor.world().world_data(), before);
    assert_eq!(fs::read(&asset).unwrap(), original);
    assert_eq!(fs::read(&scene).unwrap(), changed);
    editor.play().unwrap();
    assert!(editor.apply_selected_prefab().unwrap_err().contains("Play"));
    assert_eq!(fs::read(&asset).unwrap(), original);
}

#[test]
fn conditional_prefab_save_preserves_external_edits_and_rejects_bad_values() {
    let temp = Temp::new();
    let path = temp.0.join("pillar.prefab.json");
    write_asset(&path);
    let (mut proposed, expected) = xgc_store::load_prefab_with_bytes(&path).unwrap();
    let mut edited = expected.clone();
    edited.push(b'\n');
    fs::write(&path, &edited).unwrap();
    proposed.revision += 1;

    let error = xgc_store::save_prefab_if_unchanged(&path, &proposed, &expected).unwrap_err();
    assert!(error.to_string().contains("已被外部修改"), "{error}");
    assert_eq!(fs::read(&path).unwrap(), edited);

    let transform = proposed.entities[0]
        .components
        .iter_mut()
        .find(|component| component.component == Transform::ID)
        .unwrap();
    let (_, position) = transform
        .fields
        .iter_mut()
        .find(|(name, _)| name == "position")
        .unwrap();
    *position = Value::Vec3([f64::NAN, 0.0, 0.0]);
    assert!(xgc_store::save_prefab_if_unchanged(&path, &proposed, &edited).is_err());
    assert_eq!(fs::read(&path).unwrap(), edited);
}

fn write_asset(path: &Path) {
    let mut world = World::new();
    world.spawn((
        Name {
            value: "Pillar".into(),
        },
        Transform {
            position: [2.0, 0.0, 0.0],
            ..Transform::default()
        },
    ));
    save_prefab(path, &world.world_data()).unwrap();
}

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "xgc-editor-prefab-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

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
