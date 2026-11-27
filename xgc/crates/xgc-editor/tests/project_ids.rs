//! 项目级实体身份跨场景、编辑会话和撤销持久化。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use xgc_base::EntityId;
use xgc_editor::EditorState;
use xgc_store::{
    load_prefab, load_world, save_prefab, save_project, save_world, ProjectContent, ProjectManifest,
};
use xgc_world::{
    Component, ComponentData, EntityData, Name, Parent, PrefabInstance, Transform, Value, WorldData,
};

const FIRST_SCENE: &str = "scenes/a.json";
const SECOND_SCENE: &str = "scenes/b.json";
const PREFAB: &str = "Prefabs/root.prefab.json";
const GUID: &str = "0123456789abcdef0123456789abcdef";

#[test]
fn authoring_allocates_project_ids_across_scenes_and_undo() {
    let root = Fixture::new();
    let a = root.0.join(FIRST_SCENE);
    let b = root.0.join(SECOND_SCENE);
    let prefab = root.0.join(PREFAB);
    let meta = root.0.join(format!("{PREFAB}.meta"));
    save_world(&a, &document(1)).unwrap();
    save_world(&b, &document(2)).unwrap();
    save_prefab(&prefab, &document(3)).unwrap();
    fs::write(&meta, format!("(guid: \"{GUID}\")")).unwrap();
    root.save_manifest();
    let scene_bytes = [fs::read(&a).unwrap(), fs::read(&b).unwrap()];
    let prefab_bytes = fs::read(&prefab).unwrap();
    let meta_bytes = fs::read(&meta).unwrap();

    let mut first_scene = EditorState::open(&root.0).unwrap();
    let mut second_scene = EditorState::open(&root.0).unwrap();
    second_scene.open_scene(SECOND_SCENE).unwrap();
    let first = first_scene.spawn_entity("撤销这次新建").unwrap();
    assert!(first_scene.undo().unwrap());
    assert!(first_scene.world().key(first).is_none());
    let second = first_scene.spawn_entity("这次新建保留").unwrap();
    let instance = second_scene
        .instantiate_prefab_file(Path::new(PREFAB))
        .unwrap();
    assert_eq!(
        [first.raw(), second.raw(), instance.raw()],
        [10, 11, 12],
        "撤销或切到另一个场景后，项目已发放的 ID 不能重用"
    );

    let key = second_scene.world().key(instance).unwrap();
    let link = second_scene.world().get::<PrefabInstance>(key).unwrap();
    assert_eq!(link.source_id, 3);
    assert_eq!(link.source, format!("guid://{GUID}"));
    assert_eq!(fs::read(&a).unwrap(), scene_bytes[0]);
    assert_eq!(fs::read(&b).unwrap(), scene_bytes[1]);
    assert_eq!(fs::read(prefab).unwrap(), prefab_bytes);
    assert_eq!(fs::read(meta).unwrap(), meta_bytes);

    first_scene.save().unwrap();
    second_scene.save().unwrap();
    drop((first_scene, second_scene));
    let mut reopened = EditorState::open(&b).unwrap();
    assert!(reopened.world().key(instance).is_some());
    assert_eq!(
        reopened
            .spawn_entity("直接重开场景后继续发号")
            .unwrap()
            .raw(),
        13
    );
    assert_eq!(
        xgc_store::load_project(&root.0.join("project.xgc"))
            .unwrap()
            .next_entity_id,
        14
    );
}

#[test]
fn revert_reserves_ids_only_for_new_prefab_members() {
    let fixture = Fixture::new();
    let path = fixture.0.join(PREFAB);
    let manifest = fixture.0.join("project.xgc");
    save_world(&fixture.0.join(FIRST_SCENE), &document(1)).unwrap();
    save_world(&fixture.0.join(SECOND_SCENE), &document(2)).unwrap();
    let mut asset = document(3);
    asset.entities[0].components = vec![ComponentData::from_component(&Transform::default())];
    save_prefab(&path, &asset).unwrap();
    fixture.save_manifest();

    let mut editor = EditorState::open(&fixture.0).unwrap();
    let root = editor.instantiate_prefab_file(Path::new(PREFAB)).unwrap();
    assert_eq!(root.raw(), 10);
    editor.set_entity_position(root, [7.0, 8.0, 9.0]).unwrap();
    let before = fs::read(&manifest).unwrap();
    assert_eq!(editor.revert_selected_prefab().unwrap(), 1);
    assert_eq!(fs::read(&manifest).unwrap(), before);

    let mut other = EditorState::open(fixture.0.join(SECOND_SCENE)).unwrap();
    let occupied = other.spawn_entity("其他场景已领号").unwrap();
    assert_eq!(occupied.raw(), 11);
    asset.entities.push(EntityData {
        id: EntityId::from_raw(4),
        revision: 0,
        components: vec![ComponentData::from_component(&Parent {
            entity: EntityId::from_raw(3),
        })],
    });
    save_prefab(&path, &asset).unwrap();

    assert_eq!(editor.revert_selected_prefab().unwrap(), 2);
    let child = EntityId::from_raw(12);
    let key = editor.world().key(child).unwrap();
    assert_eq!(editor.world().get::<Parent>(key).unwrap().entity, root);
    assert_eq!(
        editor.world().get::<PrefabInstance>(key).unwrap().source_id,
        4
    );
    assert_eq!(editor.entity_position(root), Some([7.0, 8.0, 9.0]));
    assert_eq!(editor.selected(), Some(root));
    assert!(editor.world().key(occupied).is_none());
    assert!(editor.undo().unwrap());
    assert!(editor.world().key(child).is_none());
    editor.select(Some(root));
    assert_eq!(editor.revert_selected_prefab().unwrap(), 2);
    assert!(editor.world().key(child).is_none());
    assert!(editor.world().key(EntityId::from_raw(13)).is_some());
    assert_eq!(
        xgc_store::load_project(&manifest).unwrap().next_entity_id,
        14
    );
}

#[test]
fn scene_save_rejects_a_stale_editor_and_keeps_its_draft() {
    let fixture = Fixture::new();
    let a = fixture.0.join(FIRST_SCENE);
    let b = fixture.0.join(SECOND_SCENE);
    save_world(&a, &document(1)).unwrap();
    save_world(&b, &document(2)).unwrap();
    fixture.save_manifest();

    let mut first = EditorState::open(&a).unwrap();
    let mut stale = EditorState::open(&a).unwrap();
    first.spawn_entity("先保存的会话").unwrap();
    stale.spawn_entity("未保存的草稿").unwrap();
    first.save().unwrap();
    let before = fs::read(&a).unwrap();
    assert!(stale.save().unwrap_err().contains("外部修改"));
    assert!(stale.is_dirty());
    assert!(stale.world().key(EntityId::from_raw(11)).is_some());
    assert_eq!(fs::read(&a).unwrap(), before);

    let other = fs::read(&b).unwrap();
    assert!(stale.save_as(&b).is_err());
    assert_eq!(fs::read(&b).unwrap(), other);
    let export = fixture.0.join("draft.json");
    stale.save_as(&export).unwrap();
    stale.spawn_entity("继续保存独立草稿").unwrap();
    stale.save().unwrap();
    assert!(xgc_store::load_world(&export)
        .unwrap()
        .entities
        .iter()
        .any(|entity| entity.id.raw() == 12));
    assert_eq!(fs::read(&a).unwrap(), before);
}

#[test]
fn structural_apply_detaches_removed_source_members_across_registered_scenes() {
    let fixture = Fixture::new();
    let a = fixture.0.join(FIRST_SCENE);
    let b = fixture.0.join(SECOND_SCENE);
    let path = fixture.0.join(PREFAB);
    let meta = fixture.0.join(format!("{PREFAB}.meta"));
    let manifest = fixture.0.join("project.xgc");
    save_world(&a, &document(1)).unwrap();
    save_world(&b, &document(2)).unwrap();
    let mut asset = document(3);
    asset.entities[0].components = vec![ComponentData::from_component(&Name {
        value: "原资产根".into(),
    })];
    let source_child = EntityId::from_raw(4);
    asset.entities.push(EntityData {
        id: source_child,
        revision: 0,
        components: vec![
            ComponentData::from_component(&Name {
                value: "资产子节点".into(),
            }),
            ComponentData::from_component(&Parent {
                entity: EntityId::from_raw(3),
            }),
        ],
    });
    save_prefab(&path, &asset).unwrap();
    let meta_bytes = format!("(guid: \"{GUID}\")");
    fs::write(&meta, &meta_bytes).unwrap();
    fixture.save_manifest();

    let mut first = EditorState::open(&a).unwrap();
    let mut second = EditorState::open(&b).unwrap();
    let root = first.instantiate_prefab_file(Path::new(PREFAB)).unwrap();
    let other_root = second.instantiate_prefab_file(Path::new(PREFAB)).unwrap();
    let child = EntityId::from_raw(root.raw() + 1);
    let other_child = EntityId::from_raw(other_root.raw() + 1);
    second
        .edit_field(
            other_child,
            Name::ID.0,
            "value",
            Value::String("另一场景自行改名".into()),
        )
        .unwrap();
    second
        .edit_field(
            other_child,
            PrefabInstance::ID.0,
            "source",
            Value::String(PREFAB.into()),
        )
        .unwrap();
    first.save().unwrap();
    second.save().unwrap();
    let later = second.world().entity_data(other_child).unwrap();
    let high_water = fs::read(&manifest).unwrap();
    first
        .edit_field(
            root,
            Name::ID.0,
            "value",
            Value::String("写入资产的草稿名称".into()),
        )
        .unwrap();
    first.select(Some(child));
    first.delete_selected().unwrap();
    first.select(Some(root));

    let dependent_path = fixture.0.join("Prefabs/dependent.prefab.json");
    let mut dependent = document(5);
    dependent.entities[0].components = vec![ComponentData::from_component(&PrefabInstance {
        source: format!("guid://{GUID}"),
        source_id: source_child.raw(),
        root: true,
    })];
    save_prefab(&dependent_path, &dependent).unwrap();
    let files = [&path, &a, &b, &dependent_path];
    let before = files.map(|path| fs::read(path).unwrap());
    let draft = first.world().world_data();
    let error = first.apply_selected_prefab().unwrap_err();
    assert!(error.contains("dependent.prefab.json"), "{error}");
    assert_eq!(files.map(|path| fs::read(path).unwrap()), before);
    assert_eq!(first.world().world_data(), draft);
    assert!(!fixture.0.join(".xgc-prefab-apply.pending").exists());
    fs::remove_file(dependent_path).unwrap();

    assert_eq!(first.apply_selected_prefab().unwrap(), 1);
    let applied = load_prefab(&path).unwrap();
    assert_eq!(applied.next_id, asset.next_id);
    assert_eq!(applied.entities[0].id, asset.entities[0].id);
    assert_eq!(
        applied.entities[0].components[0].fields[0].1.as_string(),
        Some("写入资产的草稿名称")
    );
    assert_eq!(fs::read(&manifest).unwrap(), high_water);
    assert_eq!(fs::read_to_string(&meta).unwrap(), meta_bytes);

    let first_saved = load_world(&a).unwrap();
    let saved_root = first_saved
        .entities
        .iter()
        .find(|entity| entity.id == root)
        .unwrap();
    let saved_name = saved_root
        .components
        .iter()
        .find(|component| component.component == Name::ID)
        .unwrap();
    assert_eq!(saved_name.fields[0].1.as_string(), Some("原资产根"));
    assert!(first_saved.entities.iter().any(|entity| entity.id == child
        && entity
            .components
            .iter()
            .all(|component| component.component != PrefabInstance::ID)));
    let second_saved = load_world(&b).unwrap();
    let other_after = second_saved
        .entities
        .iter()
        .find(|entity| entity.id == other_child)
        .unwrap();
    assert_eq!(
        other_after.components,
        later
            .components
            .into_iter()
            .filter(|component| component.component != PrefabInstance::ID)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        second_saved
            .entities
            .iter()
            .find(|entity| entity.id == other_root)
            .unwrap(),
        &second.world().entity_data(other_root).unwrap()
    );

    assert!(first.is_dirty());
    assert!(first.world().key(child).is_none());
    assert!(first.undo().unwrap());
    assert!(first
        .world()
        .get::<PrefabInstance>(first.world().key(child).unwrap())
        .is_none());
    first.save().unwrap();
    assert!(second.save().unwrap_err().contains("外部修改"));
}

#[test]
fn structural_apply_adds_one_leaf_and_undo_redo_changes_only_its_draft_link() {
    let fixture = Fixture::new();
    let a = fixture.0.join(FIRST_SCENE);
    let b = fixture.0.join(SECOND_SCENE);
    let path = fixture.0.join(PREFAB);
    let meta = fixture.0.join(format!("{PREFAB}.meta"));
    let unrelated_asset = fixture.0.join("Prefabs/other.prefab.json");
    let manifest = fixture.0.join("project.xgc");
    let scene_parent = EntityId::from_raw(1);
    let source_root = EntityId::from_raw(3);
    let mut saved_scene = document(scene_parent.raw());
    saved_scene.entities[0]
        .components
        .push(ComponentData::from_component(&Name {
            value: "已保存的场景实体".into(),
        }));
    save_world(&a, &saved_scene).unwrap();
    save_world(&b, &document(2)).unwrap();
    let mut asset = document(source_root.raw());
    asset.entities[0]
        .components
        .push(ComponentData::from_component(&Transform {
            position: [1.0, 2.0, 3.0],
            ..Transform::default()
        }));
    save_prefab(&path, &asset).unwrap();
    save_prefab(&unrelated_asset, &document(4)).unwrap();
    let source = format!("guid://{GUID}");
    fs::write(&meta, format!("(guid: \"{GUID}\")")).unwrap();
    fixture.save_manifest();

    let mut first = EditorState::open(&a).unwrap();
    let mut second = EditorState::open(&b).unwrap();
    let root = first.instantiate_prefab_file(Path::new(PREFAB)).unwrap();
    let other_root = second.instantiate_prefab_file(Path::new(PREFAB)).unwrap();
    let placed_at = [7.0, 8.0, 9.0];
    first.set_entity_position(root, placed_at).unwrap();
    first.add_component(root, Parent::NAME).unwrap();
    first
        .edit_field(
            root,
            Parent::ID.0,
            "entity",
            Value::Entity(scene_parent.raw()),
        )
        .unwrap();
    second
        .edit_field(
            other_root,
            PrefabInstance::ID.0,
            "source",
            Value::String(PREFAB.into()),
        )
        .unwrap();
    first.save().unwrap();
    second.save().unwrap();

    let child = first.spawn_entity("发布到资产的叶子").unwrap();
    first.add_component(child, Parent::NAME).unwrap();
    first
        .edit_field(child, Parent::ID.0, "entity", Value::Entity(root.raw()))
        .unwrap();
    first
        .edit_field(
            scene_parent,
            Name::ID.0,
            "value",
            Value::String("只保留在当前草稿".into()),
        )
        .unwrap();
    let later = second.spawn_entity("另一会话随后领号").unwrap();
    assert_eq!(
        [root.raw(), other_root.raw(), child.raw(), later.raw()],
        [10, 11, 12, 13]
    );
    first.select(Some(root));
    let guarded_paths = [&a, &b, &meta, &unrelated_asset];
    let guarded_before = guarded_paths.map(|file| fs::read(file).unwrap());
    let draft_before = first.world().world_data();

    assert_eq!(first.apply_selected_prefab().unwrap(), 2);
    let source_child = EntityId::from_raw(14);
    let draft_after = first.world().world_data();
    let mut unbound = draft_after.clone();
    let adopted = unbound
        .entities
        .iter_mut()
        .find(|entity| entity.id == child)
        .unwrap();
    let previous = draft_before
        .entities
        .iter()
        .find(|entity| entity.id == child)
        .unwrap();
    let binding = ComponentData::from_component(&PrefabInstance {
        source,
        source_id: source_child.raw(),
        root: false,
    });
    assert!(adopted.components.contains(&binding));
    adopted
        .components
        .retain(|component| component.component != PrefabInstance::ID);
    adopted.revision = previous.revision;
    unbound.revision = draft_before.revision;
    assert_eq!(unbound, draft_before);
    assert_eq!(first.entity_position(root), Some(placed_at));
    assert_eq!(first.selected(), Some(root));
    assert!(first.is_dirty());

    let applied = load_prefab(&path).unwrap();
    assert_eq!(applied.next_id, 15);
    assert_eq!(applied.entities[0], asset.entities[0]);
    let mut expected_components = previous.components.clone();
    for component in &mut expected_components {
        if component.component == Parent::ID {
            *component = ComponentData::from_component(&Parent {
                entity: source_root,
            });
        }
    }
    assert_eq!(applied.entities.len(), 2);
    let adopted = &applied.entities[1];
    assert_eq!(adopted.id, source_child);
    assert_eq!(adopted.components, expected_components);
    assert_eq!(
        xgc_store::load_project(&manifest).unwrap().next_entity_id,
        15
    );
    assert_eq!(
        guarded_paths.map(|file| fs::read(file).unwrap()),
        guarded_before
    );
    let asset_bytes = fs::read(&path).unwrap();
    let manifest_bytes = fs::read(&manifest).unwrap();

    assert_eq!(first.apply_selected_prefab().unwrap(), 2);
    // 撤销只移除草稿关联。发布后的叶子既不能凭名字认回，也不能再领一个来源 ID。
    let assert_undo_redo = |editor: &mut EditorState| {
        assert!(editor.undo().unwrap());
        assert_eq!(editor.world().world_data(), draft_before);
        assert!(editor.apply_selected_prefab().is_err());
        assert_eq!(editor.world().world_data(), draft_before);
        assert_eq!(fs::read(&path).unwrap(), asset_bytes);
        assert_eq!(fs::read(&manifest).unwrap(), manifest_bytes);
        assert!(editor.redo().unwrap());
        assert_eq!(editor.world().world_data(), draft_after);
        assert!(editor.is_dirty());
    };
    assert_undo_redo(&mut first);
    assert_eq!(
        guarded_paths.map(|file| fs::read(file).unwrap()),
        guarded_before
    );

    first.save().unwrap();
    assert_eq!(load_world(&a).unwrap(), draft_after);
    let saved_bytes = fs::read(&a).unwrap();
    assert_undo_redo(&mut first);
    assert_eq!(fs::read(&a).unwrap(), saved_bytes);
    assert_eq!(fs::read(&b).unwrap(), guarded_before[1]);

    assert!(first.undo().unwrap());
    first
        .edit_field(
            scene_parent,
            Name::ID.0,
            "value",
            Value::String("清空重做后仍保留草稿".into()),
        )
        .unwrap();
    assert!(!first.can_redo());
    let branched_draft = first.world().world_data();
    assert!(first.apply_selected_prefab().is_err());
    assert_eq!(first.world().world_data(), branched_draft);
    assert_eq!(fs::read(&path).unwrap(), asset_bytes);
    assert_eq!(fs::read(&manifest).unwrap(), manifest_bytes);
    assert_eq!(fs::read(a).unwrap(), saved_bytes);
    assert_eq!(fs::read(b).unwrap(), guarded_before[1]);
}

fn document(id: u64) -> WorldData {
    WorldData {
        next_id: 10,
        entities: vec![EntityData {
            id: EntityId::from_raw(id),
            revision: 0,
            components: Vec::new(),
        }],
        ..Default::default()
    }
}

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "xgc-author-ids-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn save_manifest(&self) {
        save_project(
            &self.0.join("project.xgc"),
            &ProjectManifest {
                format_version: xgc_store::PROJECT_FORMAT_VERSION,
                next_entity_id: 10,
                name: "持久 ID".into(),
                entry_scene: FIRST_SCENE.into(),
                scenes: vec![FIRST_SCENE.into(), SECOND_SCENE.into()],
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
                layers: Default::default(),
            },
        )
        .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
