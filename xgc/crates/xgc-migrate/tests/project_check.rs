//! 项目级检查回归：能打开真实旧工程，也要能点名坏场景 / 坏脚本 / 悬空 GUID。
//!
//! 所有 fixture 都写在系统临时目录里，不碰仓库；没有 `tempfile` 依赖，清理靠
//! 每个用例末尾的 `remove_dir_all`。

use std::fs;
use std::path::{Path, PathBuf};

use xgc_migrate::{check_old_project, ProjectCheckError};

fn scene(entities: &str) -> String {
    format!("(\n    format_version: 2,\n    name: \"T\",\n    entities: [{entities}],\n)\n")
}

fn entity_full(id: u64, parent: Option<u64>, layer: u8, prefab: &str, components: &str) -> String {
    let parent = parent
        .map(|parent| format!("Some(({parent}))"))
        .unwrap_or_else(|| "None".to_string());
    // `components.entries` 是 `Vec<OldEntry>`，每个条目是 `(data: …)`；只有空列表
    // 才能写成 `[]`，否则 RON 会把 `data` 当成 `OldEntry` 的未知字段。
    let components = if components.is_empty() {
        String::new()
    } else {
        format!("({components})")
    };
    format!(
        "(id: ({id}), name: \"E{id}\", parent: {parent}, layer: {layer}, prefab: {prefab}, \
         transform: (position: (0.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0), scale: (1.0, 1.0, 1.0)), \
         components: (entries: [{components}]))"
    )
}

fn entity(id: u64, parent: Option<u64>, layer: u8, components: &str) -> String {
    entity_full(id, parent, layer, "None", components)
}

fn project(entry: &str, build: &[&str]) -> String {
    let scenes = build
        .iter()
        .map(|scene| format!("\"{scene}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "(\n    format_version: 1,\n    name: \"Fixture\",\n    entry_scene: \"{entry}\",\n    \
         content: (scenes_dir: \"Scenes\", prefabs_dir: \"Prefabs\", scripts_dir: \"Scripts\"),\n    \
         build: (scenes: [{scenes}]),\n)\n"
    )
}

fn setup(label: &str, project_ron: &str, files: &[(&str, &str)]) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("xgc-project-check-{}-{label}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("Scenes")).unwrap();
    fs::create_dir_all(root.join("Prefabs")).unwrap();
    fs::write(root.join("project.ron"), project_ron).unwrap();
    for (rel, text) in files {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }
    root
}

fn write_meta(root: &Path, rel: &str, guid: &str) {
    write_file(
        root,
        &format!("{rel}.meta"),
        &format!("(guid: \"{guid}\")\n"),
    );
}

fn write_file(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, text).unwrap();
}

#[test]
fn accepts_a_minimal_project() {
    let root = setup(
        "ok",
        &project("Scenes/a.ron", &["Scenes/a.ron"]),
        &[("Scenes/a.ron", &scene(&entity(1, None, 0, "")))],
    );
    let report = check_old_project(&root).unwrap();
    assert_eq!(report.scenes, 1);
    assert_eq!(report.entities, 1);
    assert!(!report.has_problems(), "{:?}", report.problems);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn reports_a_missing_build_scene() {
    let root = setup(
        "missing-build",
        &project("Scenes/a.ron", &["Scenes/a.ron", "Scenes/b.ron"]),
        &[("Scenes/a.ron", &scene(&entity(1, None, 0, "")))],
    );
    let report = check_old_project(&root).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("[build.missing]")),
        "{:?}",
        report.problems
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn reports_duplicate_ids_and_dangling_parent() {
    let root = setup(
        "ids",
        &project("Scenes/a.ron", &["Scenes/a.ron"]),
        &[(
            "Scenes/a.ron",
            &scene(&format!(
                "{},{}",
                entity(1, None, 0, ""),
                entity(1, Some(9), 0, "")
            )),
        )],
    );
    let report = check_old_project(&root).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("[scene.duplicate_id]")),
        "{:?}",
        report.problems
    );
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("[scene.parent_missing]")),
        "{:?}",
        report.problems
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn reports_a_missing_script() {
    let root = setup(
        "script",
        &project("Scenes/a.ron", &["Scenes/a.ron"]),
        &[(
            "Scenes/a.ron",
            &scene(&entity(1, None, 0, "data: Lua(path: \"Scripts/nope.lua\")")),
        )],
    );
    let report = check_old_project(&root).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("[script.missing]")),
        "{:?}",
        report.problems
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn resolves_guid_script_and_prefab() {
    let prefab = scene(&entity(7, None, 0, ""));
    let root = setup(
        "guid-refs",
        &project("Scenes/a.ron", &["Scenes/a.ron"]),
        &[
            ("Scripts/foo.lua", "function update(ctx) end\n"),
            ("Prefabs/P.prefab.ron", &prefab),
            (
                "Scenes/a.ron",
                &scene(&format!(
                    "{},{}",
                    entity(1, None, 0, "data: Lua(path: \"guid://s1\")"),
                    entity_full(
                        2,
                        None,
                        0,
                        "Some((source: \"guid://p1\", source_id: (7)))",
                        ""
                    )
                )),
            ),
        ],
    );
    write_meta(&root, "Scripts/foo.lua", "s1");
    write_meta(&root, "Prefabs/P.prefab.ron", "p1");
    let report = check_old_project(&root).unwrap();
    assert_eq!(report.scripts, 1);
    assert_eq!(report.prefabs, 1);
    assert!(!report.has_problems(), "{:?}", report.problems);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn reports_a_dangling_guid() {
    let root = setup(
        "dangling",
        &project("Scenes/a.ron", &["Scenes/a.ron"]),
        &[(
            "Scenes/a.ron",
            &scene(&entity(
                1,
                None,
                0,
                "data: Mesh((model: \"guid://deadbeef\"))",
            )),
        )],
    );
    let report = check_old_project(&root).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("[asset.dangling]")),
        "{:?}",
        report.problems
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn rejects_an_unsafe_entry_path() {
    let root = setup("unsafe-entry", &project("../a.ron", &[]), &[]);
    match check_old_project(&root) {
        Err(ProjectCheckError::InvalidEntry(_)) => {}
        other => panic!("期望 InvalidEntry，得到 {other:?}"),
    }
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn reports_a_prefab_source_id_that_does_not_exist() {
    let prefab = scene(&entity(7, None, 0, ""));
    let root = setup(
        "prefab-id",
        &project("Scenes/a.ron", &["Scenes/a.ron"]),
        &[
            ("Prefabs/P.prefab.ron", &prefab),
            (
                "Scenes/a.ron",
                &scene(&entity_full(
                    1,
                    None,
                    0,
                    "Some((source: \"guid://p1\", source_id: (9)))",
                    "",
                )),
            ),
        ],
    );
    write_meta(&root, "Prefabs/P.prefab.ron", "p1");
    let report = check_old_project(&root).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("[prefab.source_id]")),
        "{:?}",
        report.problems
    );
    let _ = fs::remove_dir_all(&root);
}
