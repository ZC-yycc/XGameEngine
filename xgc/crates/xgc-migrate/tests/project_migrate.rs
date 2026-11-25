//! 项目级迁移回归：清单驱动、事务输出、dry-run 不写盘、旧工程不被改。
//!
//! fixture 全在系统临时目录，没有 `tempfile` 依赖；输出目录也建在临时目录里。

use std::fs;
use std::path::{Path, PathBuf};

use xgc_migrate::{migrate_old_project, ProjectMigrationError};
use xgc_store::{load_project, load_world};

fn scene(entities: &str) -> String {
    format!("(\n    format_version: 2,\n    name: \"T\",\n    entities: [{entities}],\n)\n")
}

fn entity(id: u64) -> String {
    format!(
        "(id: ({id}), name: \"E{id}\", parent: None, layer: 0, prefab: None, \
         transform: (position: (0.0, 0.0, {id}.0), rotation: (0.0, 0.0, 0.0, 1.0), scale: (1.0, 1.0, 1.0)), \
         components: (entries: []))"
    )
}

fn project(entry: &str, build: &[&str]) -> String {
    let scenes = build
        .iter()
        .map(|scene| format!("\"{scene}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "(\n    format_version: 1,\n    name: \"Fixture\",\n    entry_scene: \"{entry}\",\n    \
         content: (scenes_dir: \"Scenes\", models_dir: \"Resources\", textures_dir: \"Resources\", \
         audio_dir: \"Resources\", shaders_dir: \"Shaders\", prefabs_dir: \"Prefabs\", \
         scripts_dir: \"Scripts\", search_paths: []),\n    build: (scenes: [{scenes}]),\n)\n"
    )
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

fn write_meta(root: &Path, rel: &str, guid: &str) {
    write(
        root,
        &format!("{rel}.meta"),
        &format!("(guid: \"{guid}\")\n"),
    );
}

fn setup(label: &str, project_ron: &str, scenes: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "xgc-project-migrate-{}-{label}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("Scenes")).unwrap();
    fs::create_dir_all(root.join("Prefabs")).unwrap();
    fs::create_dir_all(root.join("Scripts")).unwrap();
    fs::create_dir_all(root.join("Resources")).unwrap();
    write(&root, "project.ron", project_ron);
    for (rel, text) in scenes {
        write(&root, rel, text);
    }
    root
}

fn standard_project(label: &str) -> (PathBuf, PathBuf) {
    let root = setup(
        label,
        &project("Scenes/a.ron", &["Scenes/a.ron", "Scenes/b.ron"]),
        &[
            (
                "Scenes/a.ron",
                &scene(&format!("{},{}", entity(1), entity(2))),
            ),
            ("Scenes/b.ron", &scene(&entity(3))),
            ("Prefabs/P.prefab.ron", &scene(&entity(7))),
            ("Scripts/main.lua", "function update(ctx) end\n"),
            ("Resources/model.bin", "mesh"),
            ("input.ron", "(actions: {})\n"),
        ],
    );
    write_meta(&root, "Resources/model.bin", "g1");
    let out = std::env::temp_dir().join(format!(
        "xgc-project-migrate-out-{}-{label}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&out);
    (root, out)
}

fn staging_of(out: &Path) -> PathBuf {
    out.with_file_name(format!(
        ".{}.staging-{}",
        out.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ))
}

#[test]
fn dry_run_reports_without_writing() {
    let (root, out) = standard_project("dry");
    let report = migrate_old_project(&root, &out, true).unwrap();
    assert!(report.dry_run);
    assert_eq!(report.scenes, 2);
    assert_eq!(report.entities, 3);
    assert!(report.copied_files >= 5, "{}", report.copied_files);
    assert!(!out.exists());
    assert!(!staging_of(&out).exists());
    assert!(report.render().contains("dry-run"));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn migrates_listed_scenes_and_content() {
    let (root, out) = standard_project("full");
    let report = migrate_old_project(&root, &out, false).unwrap();
    assert_eq!(report.scenes, 2);
    assert_eq!(report.entities, 3);

    let manifest = load_project(&out.join("project.xgc")).unwrap();
    assert_eq!(manifest.name, "Fixture");
    assert_eq!(manifest.entry_scene, "scenes/a.json");
    assert_eq!(manifest.scenes, vec!["scenes/a.json", "scenes/b.json"]);
    assert_eq!(manifest.content.models, "Resources");
    assert_eq!(manifest.content.prefabs, "Prefabs");
    assert_eq!(manifest.input.as_deref(), Some("input.ron"));
    assert_eq!(manifest.content.shaders, None);

    assert_eq!(
        load_world(&out.join("scenes/a.json"))
            .unwrap()
            .entities
            .len(),
        2
    );
    assert_eq!(
        load_world(&out.join("scenes/b.json"))
            .unwrap()
            .entities
            .len(),
        1
    );
    assert_eq!(
        fs::read_to_string(out.join("Resources/model.bin")).unwrap(),
        "mesh"
    );
    assert!(out.join("Resources/model.bin.meta").is_file());
    assert!(out.join("Scripts/main.lua").is_file());
    assert!(out.join("Prefabs/P.prefab.ron").is_file());
    assert!(out.join("input.ron").is_file());
    // 输出目录里只有清单、场景和内容，不再出现旧 project.ron。
    assert!(!out.join("project.ron").exists());
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn refuses_existing_output() {
    let (root, out) = standard_project("exists");
    fs::create_dir_all(&out).unwrap();
    let err = migrate_old_project(&root, &out, false).unwrap_err();
    assert!(
        matches!(err, ProjectMigrationError::OutputExists(_)),
        "{err}"
    );
    assert!(!staging_of(&out).exists());
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn preserves_another_migrations_staging_directory() {
    let (root, out) = standard_project("busy");
    let staging = staging_of(&out);
    let _ = fs::remove_dir_all(&staging);
    write(&staging, "marker", "in progress");

    let result = migrate_old_project(&root, &out, false);
    assert!(result.is_err(), "暂存目录已被占用，不能开始写入");
    assert_eq!(
        fs::read_to_string(staging.join("marker")).unwrap(),
        "in progress"
    );
    assert!(!out.exists());
    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(&staging).unwrap();
}

#[test]
fn refuses_a_project_with_check_problems() {
    let root = setup(
        "bad",
        &project("Scenes/a.ron", &["Scenes/a.ron", "Scenes/missing.ron"]),
        &[("Scenes/a.ron", &scene(&entity(1)))],
    );
    let out = std::env::temp_dir().join(format!(
        "xgc-project-migrate-bad-out-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&out);
    let err = migrate_old_project(&root, &out, false).unwrap_err();
    assert!(matches!(err, ProjectMigrationError::Problems(_)), "{err}");
    assert!(!out.exists());
    assert!(!staging_of(&out).exists());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn prepends_entry_when_build_omits_it() {
    let root = setup(
        "entry",
        &project("Scenes/a.ron", &["Scenes/b.ron"]),
        &[
            ("Scenes/a.ron", &scene(&entity(1))),
            ("Scenes/b.ron", &scene(&entity(2))),
        ],
    );
    let out = std::env::temp_dir().join(format!(
        "xgc-project-migrate-entry-out-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&out);
    let report = migrate_old_project(&root, &out, false).unwrap();
    assert_eq!(report.scenes, 2);
    let manifest = load_project(&out.join("project.xgc")).unwrap();
    assert_eq!(manifest.scenes, vec!["scenes/a.json", "scenes/b.json"]);
    assert_eq!(manifest.entry_scene, "scenes/a.json");
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn keeps_old_project_bytes_untouched() {
    let (root, out) = standard_project("untouched");
    let project_before = fs::read(root.join("project.ron")).unwrap();
    let scene_before = fs::read(root.join("Scenes/a.ron")).unwrap();
    migrate_old_project(&root, &out, false).unwrap();
    assert_eq!(fs::read(root.join("project.ron")).unwrap(), project_before);
    assert_eq!(fs::read(root.join("Scenes/a.ron")).unwrap(), scene_before);
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn remaps_reused_scene_ids_and_records_file_scoped_references() {
    let (root, out) = standard_project("ids");
    let a = scene(&format!(
        "{},{}",
        entity(42).replace("parent: None", "parent: Some((12))"),
        entity(12)
    ));
    let b = scene(&format!(
        "{},{}",
        entity(12).replace("parent: None", "parent: Some((42))"),
        entity(42)
    ));
    write(&root, "Scenes/a.ron", &a);
    write(&root, "Scenes/b.ron", &b);

    // 清单顺序负责关卡索引，不能影响 ID 分配顺序。
    write(
        &root,
        "project.ron",
        &project("Scenes/b.ron", &["Scenes/b.ron", "Scenes/a.ron"]),
    );
    let dry = migrate_old_project(&root, &out, true).unwrap();
    assert_eq!(dry.entities, 4);
    assert!(!out.exists());
    let report = migrate_old_project(&root, &out, false).unwrap();
    assert_eq!(report.entities, dry.entities);
    let session = xgc_store::ProjectSession::open(&out).unwrap();
    assert_eq!(
        session.manifest().scenes,
        ["scenes/b.json", "scenes/a.json"]
    );
    let a_data = session.load_scene("scenes/a.json").unwrap();
    let b_data = session.load_entry().unwrap();
    let ids = |world: &xgc_world::WorldData| {
        world
            .entities
            .iter()
            .map(|entity| entity.id.raw())
            .collect::<Vec<_>>()
    };
    assert!(
        ids(&a_data).iter().all(|id| !ids(&b_data).contains(id)),
        "不同场景复用了旧 ID"
    );
    assert_eq!(ids(&a_data), [2, 1]);
    assert_eq!(ids(&b_data), [3, 4]);
    for (data, child, parent) in [(&a_data, 2, 1), (&b_data, 3, 4)] {
        use xgc_base::EntityId;
        let mut world = xgc_world::World::new();
        world.restore_world_data(data).unwrap();
        let key = world.key(EntityId::from_raw(child)).unwrap();
        assert_eq!(
            world.get::<xgc_world::Parent>(key).unwrap().entity.raw(),
            parent
        );
        let added = world.spawn((xgc_world::Name::default(),));
        assert_eq!(world.entity(added).unwrap().raw(), 5);
    }

    let map_path = out.join("entity-id-map.json");
    let map_bytes = fs::read(&map_path).unwrap();
    let map: serde_json::Value = serde_json::from_slice(&map_bytes).unwrap();
    assert_eq!(
        map,
        serde_json::json!({
            "format_version": 1,
            "files": [
                {"source": "Scenes/a.ron", "target": "scenes/a.json", "ids": {"12": "1", "42": "2"}},
                {"source": "Scenes/b.ron", "target": "scenes/b.json", "ids": {"12": "3", "42": "4"}}
            ]
        })
    );
    assert_eq!(fs::read_to_string(root.join("Scenes/a.ron")).unwrap(), a);
    assert_eq!(fs::read_to_string(root.join("Scenes/b.ron")).unwrap(), b);

    // 关卡清单改序后，在另一个目录重新迁移，映射和两个世界仍逐字节相同。
    write(
        &root,
        "project.ron",
        &project("Scenes/b.ron", &["Scenes/a.ron", "Scenes/b.ron"]),
    );
    let reordered = out.with_extension("reordered");
    migrate_old_project(&root, &reordered, false).unwrap();
    assert_eq!(
        fs::read(reordered.join("entity-id-map.json")).unwrap(),
        map_bytes
    );
    for scene in ["scenes/a.json", "scenes/b.json"] {
        assert_eq!(
            fs::read(out.join(scene)).unwrap(),
            fs::read(reordered.join(scene)).unwrap()
        );
    }
    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(&out).unwrap();
    fs::remove_dir_all(&reordered).unwrap();
}

#[test]
fn reports_id_map_conflict_without_publishing_migration() {
    let (root, out) = standard_project("map-write-failure");
    let manifest = project("Scenes/a.ron", &["Scenes/a.ron", "Scenes/b.ron"]).replace(
        "models_dir: \"Resources\"",
        "models_dir: \"entity-id-map.json\"",
    );
    write(&root, "project.ron", &manifest);

    // 即使模型目录尚未创建，计划也不能让映射文件占据它的位置。
    let error = migrate_old_project(&root, &out, true).unwrap_err();
    assert!(error.to_string().contains("entity-id-map.json"));
    assert!(!root.join("entity-id-map.json").exists());

    write(&root, "entity-id-map.json/marker.bin", "asset");
    for dry_run in [true, false] {
        let error = migrate_old_project(&root, &out, dry_run).unwrap_err();
        assert!(error.to_string().contains("entity-id-map.json"));
        assert!(!out.exists());
        assert!(!staging_of(&out).exists());
    }
    assert_eq!(
        fs::read_to_string(root.join("entity-id-map.json/marker.bin")).unwrap(),
        "asset"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[cfg(windows)]
#[test]
fn rejects_two_spellings_of_the_same_scene_file() {
    let (root, out) = standard_project("scene-path-alias");
    // Windows 的常规目录两种拼法都能打开同一个文件。
    if root.join("scenes/a.RON").is_file() {
        write(
            &root,
            "project.ron",
            &project("Scenes/a.ron", &["Scenes/a.ron", "scenes/a.RON"]),
        );
        let error = migrate_old_project(&root, &out, true).unwrap_err();
        assert!(error.to_string().contains("同一个源文件"));
        assert!(!out.exists());
    }
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn nested_scene_keeps_subdirectory() {
    let root = setup(
        "nested",
        &project("Scenes/Levels/a.ron", &["Scenes/Levels/a.ron"]),
        &[("Scenes/Levels/a.ron", &scene(&entity(1)))],
    );
    let out = std::env::temp_dir().join(format!(
        "xgc-project-migrate-nested-out-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&out);
    migrate_old_project(&root, &out, false).unwrap();
    assert!(out.join("scenes/Levels/a.json").is_file());
    let manifest = load_project(&out.join("project.xgc")).unwrap();
    assert_eq!(manifest.entry_scene, "scenes/Levels/a.json");
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&out);
}
