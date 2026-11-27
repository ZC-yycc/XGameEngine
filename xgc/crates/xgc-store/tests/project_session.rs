//! 项目清单会话：只按登记过的相对路径读场景。

use xgc_base::EntityId;
use xgc_store::{
    load_project, save_project, save_world, ProjectContent, ProjectManifest, ProjectSession,
    PROJECT_FORMAT_VERSION,
};
use xgc_world::{Name, Transform, World};

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "xgc_session_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn manifest() -> ProjectManifest {
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        next_entity_id: 2,
        name: "demo".to_string(),
        entry_scene: "scenes/main.json".to_string(),
        scenes: vec!["scenes/main.json".to_string()],
        content: ProjectContent {
            models: "Resources".to_string(),
            textures: "Resources".to_string(),
            audio: "Resources".to_string(),
            shaders: None,
            prefabs: "Prefabs".to_string(),
            scripts: "Scripts".to_string(),
        },
        search_paths: Vec::new(),
        input: None,
        render: Default::default(),
        quality: Default::default(),
        layers: Default::default(),
    }
}

#[test]
fn opens_a_directory_and_rejects_unregistered_scenes() {
    let dir = temp("open");
    let mut world = World::new();
    world.spawn((
        Name {
            value: "hero".to_string(),
        },
        Transform::default(),
    ));
    save_world(&dir.join("scenes/main.json"), &world.world_data()).unwrap();
    save_project(&dir.join("project.xgc"), &manifest()).unwrap();

    let session = ProjectSession::open(&dir).unwrap();
    assert_eq!(session.manifest().name, "demo");
    assert_eq!(session.root(), dir.as_path());
    assert_eq!(session.load_entry().unwrap().entities.len(), 1);

    let err = session.load_scene("scenes/other.json").unwrap_err();
    assert!(err.to_string().contains("没有登记"), "{err}");
    let err = session.scene_path("../escape.json").unwrap_err();
    assert!(err.to_string().contains("不合法"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn authoring_load_checks_the_latest_scene_registration() {
    let dir = temp("authoring-registration");
    let path = dir.join("project.xgc");
    let mut project = manifest();
    let main = project.entry_scene.clone();
    let other = "scenes/other.json";
    project.scenes.push(other.into());
    save_project(&path, &project).unwrap();
    let world = World::new().world_data();
    save_world(&dir.join(&main), &world).unwrap();
    save_world(&dir.join(other), &world).unwrap();
    let session = ProjectSession::open(&dir).unwrap();
    let (_, _, original) = session.load_scene_for_authoring(&main).unwrap();

    project.entry_scene = other.into();
    project.scenes = vec![other.into()];
    save_project(&path, &project).unwrap();
    let error = session.load_scene_for_authoring(&main).unwrap_err();
    assert!(error.to_string().contains("场景登记已变"), "{error}");
    assert_eq!(std::fs::read(dir.join(&main)).unwrap(), original);
    assert_eq!(session.load_scene_for_authoring(other).unwrap().1, world);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn reservations_from_concurrent_sessions_are_disjoint_and_preserve_project_changes() {
    let dir = temp("concurrent-ids");
    let path = dir.join("project.xgc");
    let original = manifest();
    save_project(&path, &original).unwrap();
    let mut first = ProjectSession::open(&dir).unwrap();
    let mut second = ProjectSession::open(&path).unwrap();
    let mut edited = original.clone();
    edited.name = "在发号前改名".into();
    save_project(&path, &edited).unwrap();
    let barrier = std::sync::Barrier::new(2);
    let mut ranges = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            first.reserve_entity_ids(8, EntityId::from_raw(2)).unwrap()
        });
        let b = scope.spawn(|| {
            barrier.wait();
            second.reserve_entity_ids(8, EntityId::from_raw(2)).unwrap()
        });
        [a.join().unwrap(), b.join().unwrap()]
    });
    ranges.sort_by_key(|range| range.start);
    assert_eq!(ranges, [2..10, 10..18]);
    let mut on_disk = edited;
    on_disk.next_entity_id = 18;
    assert_eq!(load_project(&path).unwrap(), on_disk);
    let bytes = std::fs::read(&path).unwrap();
    assert!(save_project(&path, &original)
        .unwrap_err()
        .to_string()
        .contains("不能回退"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn reserves_last_id_and_rejects_exhaustion_without_advancing() {
    let dir = temp("exhausted-ids");
    let path = dir.join("project.xgc");
    let mut manifest = manifest();
    manifest.next_entity_id = EntityId::MAX_ASSIGNABLE_RAW;
    save_project(&path, &manifest).unwrap();
    let mut session = ProjectSession::open(&dir).unwrap();
    let last = EntityId::from_raw(EntityId::MAX_ASSIGNABLE_RAW);
    let original = std::fs::read(&path).unwrap();
    assert!(session
        .reserve_entity_ids(2, last)
        .unwrap_err()
        .to_string()
        .contains("耗尽"));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(
        session.reserve_entity_ids(1, last).unwrap(),
        last.raw()..u64::MAX
    );
    let exhausted = std::fs::read(&path).unwrap();
    assert!(session
        .reserve_entity_ids(1, last)
        .unwrap_err()
        .to_string()
        .contains("耗尽"));
    assert_eq!(std::fs::read(&path).unwrap(), exhausted);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn manifest_roundtrips_through_disk() {
    let dir = temp("roundtrip");
    let manifest = manifest();
    let path = dir.join("project.xgc");
    save_project(&path, &manifest).unwrap();
    assert_eq!(load_project(&path).unwrap(), manifest);
    assert!(!std::fs::read_to_string(path).unwrap().contains("layers:"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn manifest_preserves_collision_overrides() {
    let dir = temp("layer-roundtrip");
    let path = dir.join("project.xgc");
    let original = ron::ser::to_string(&manifest()).unwrap();
    let fields = original.strip_prefix('(').unwrap();
    let source = format!(
        r#"(layers:(names:{{3:"Pickup",31:"Ghost"}},masks:{{3:2147483648,31:0}}),{fields}"#
    );
    std::fs::write(&path, source).unwrap();
    let parsed = load_project(&path).unwrap();
    save_project(&path, &parsed).unwrap();
    assert_eq!(load_project(&path).unwrap(), parsed);
    assert_eq!(
        serde_json::to_value(parsed).unwrap()["layers"],
        serde_json::json!({
            "names": {"3": "Pickup", "31": "Ghost"},
            "masks": {"3": 2147483648u32, "31": 0},
        })
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rejects_out_of_range_layers_at_manifest_boundaries() {
    let dir = temp("bad-layers");
    let path = dir.join("project.xgc");
    let original = ron::ser::to_string(&manifest()).unwrap();
    let fields = original.strip_prefix('(').unwrap();
    for invalid in [r#"names:{32:"Unknown"}"#, "masks:{32:0}"] {
        let source = format!("(layers:({invalid}),{fields}");
        let parsed = ron::from_str(&source).unwrap();
        std::fs::write(&path, &source).unwrap();
        let read = load_project(&path).unwrap_err().to_string();
        let write = save_project(&path, &parsed).unwrap_err().to_string();
        for message in [read, write] {
            assert!(
                message.contains("layers") && message.contains("32"),
                "{message}"
            );
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
