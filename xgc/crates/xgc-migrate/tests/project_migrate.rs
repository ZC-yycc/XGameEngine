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
    write(root, &format!("{rel}.meta"), &format!("(guid: \"{guid}\")\n"));
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
            ("Scenes/a.ron", &scene(&format!("{},{}", entity(1), entity(2)))),
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

    assert_eq!(load_world(&out.join("scenes/a.json")).unwrap().entities.len(), 2);
    assert_eq!(load_world(&out.join("scenes/b.json")).unwrap().entities.len(), 1);
    assert_eq!(fs::read_to_string(out.join("Resources/model.bin")).unwrap(), "mesh");
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
    assert!(matches!(err, ProjectMigrationError::OutputExists(_)), "{err}");
    assert!(!staging_of(&out).exists());
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn refuses_a_project_with_check_problems() {
    let root = setup(
        "bad",
        &project("Scenes/a.ron", &["Scenes/a.ron", "Scenes/missing.ron"]),
        &[("Scenes/a.ron", &scene(&entity(1)))],
    );
    let out = std::env::temp_dir().join(format!("xgc-project-migrate-bad-out-{}", std::process::id()));
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
    let out = std::env::temp_dir().join(format!("xgc-project-migrate-entry-out-{}", std::process::id()));
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
fn nested_scene_keeps_subdirectory() {
    let root = setup(
        "nested",
        &project("Scenes/Levels/a.ron", &["Scenes/Levels/a.ron"]),
        &[("Scenes/Levels/a.ron", &scene(&entity(1)))],
    );
    let out = std::env::temp_dir().join(format!("xgc-project-migrate-nested-out-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out);
    migrate_old_project(&root, &out, false).unwrap();
    assert!(out.join("scenes/Levels/a.json").is_file());
    let manifest = load_project(&out.join("project.xgc")).unwrap();
    assert_eq!(manifest.entry_scene, "scenes/Levels/a.json");
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&out);
}
