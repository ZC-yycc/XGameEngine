//! 项目级实体身份跨场景、编辑会话和撤销持久化。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use xgc_base::EntityId;
use xgc_editor::EditorState;
use xgc_store::{save_prefab, save_project, save_world, ProjectContent, ProjectManifest};
use xgc_world::{ComponentData, EntityData, Parent, PrefabInstance, Transform, WorldData};

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
