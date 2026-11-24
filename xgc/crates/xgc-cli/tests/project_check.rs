//! `xgc check <项目目录>` 的端到端回归。
//!
//! 真实旧工程必须 0 问题（这是 M6 门禁「迁移后 check 0 问题」的入口）；坏工程
//! 必须按 0/1/2 契约给退出码，而不是把目录当世界 JSON 读出一句「拒绝访问」。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn xgc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_xgc"))
}

fn real_project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../projects/XGame")
        .canonicalize()
        .expect("找不到 projects/XGame")
}

fn fixture(label: &str, project_ron: Option<&str>) -> PathBuf {
    let root = std::env::temp_dir().join(format!("xgc-cli-check-{}-{label}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("Scenes")).unwrap();
    fs::create_dir_all(root.join("Prefabs")).unwrap();
    if let Some(project_ron) = project_ron {
        fs::write(root.join("project.ron"), project_ron).unwrap();
    }
    root
}

#[test]
fn real_project_passes() {
    let output = xgc().arg("check").arg(real_project()).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "xgc check 失败：code={:?}\nstdout={stdout}",
        output.status.code()
    );
    assert!(stdout.contains("scenes=4"), "{stdout}");
    assert!(stdout.contains("problems=0"), "{stdout}");
    assert!(stdout.contains("check: OK"), "{stdout}");
}

#[test]
fn missing_scene_exits_2() {
    let project = "(\n  format_version: 1,\n  name: \"F\",\n  entry_scene: \"Scenes/a.ron\",\n  content: (scenes_dir: \"Scenes\", prefabs_dir: \"Prefabs\"),\n  build: (scenes: [\"Scenes/a.ron\", \"Scenes/b.ron\"]),\n)\n";
    let root = fixture("missing-scene", Some(project));
    let scene = "(\n  format_version: 2,\n  name: \"A\",\n  entities: [],\n)\n";
    fs::write(root.join("Scenes/a.ron"), scene).unwrap();

    let output = xgc().arg("check").arg(&root).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(2), "stdout={stdout}");
    assert!(stdout.contains("[build.missing]"), "{stdout}");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn missing_project_ron_exits_1() {
    let root = fixture("no-project", None);
    let output = xgc().arg("check").arg(&root).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let _ = fs::remove_dir_all(&root);
}
