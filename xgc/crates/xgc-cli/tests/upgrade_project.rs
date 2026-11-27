//! 原生项目升级走真实命令行，源项目和所有未改写的文件逐字节核对。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use xgc_base::EntityId;
use xgc_store::{load_project, load_world, save_prefab, save_world};
use xgc_world::{Name, PrefabInstance, World};

const MANIFEST: &str = r#"
// 测试带注释、选项与枚举的原生清单。
(
    format_version: 1,
    name: "Fixture",
    entry_scene: "scenes/main.json",
    scenes: ["scenes/main.json", "scenes/other.json"],
    content: (
        models: "Resources", textures: "Resources", audio: "Resources",
        prefabs: "Prefabs", scripts: "Scripts",
    ),
    search_paths: ["Fallback"],
    quality: (preset: Some(High)),
)
"#;

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "xgc_upgrade_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn fixture(root: &Path) {
    fs::create_dir_all(root.join("Resources/empty")).unwrap();
    fs::write(root.join("project.xgc"), MANIFEST).unwrap();
    fs::write(root.join("Resources/data.bin"), [0, 255, 46]).unwrap();
    fs::write(root.join("notes.txt"), "保留项目里的任意普通文件\n").unwrap();

    let mut main = World::new();
    main.spawn((PrefabInstance {
        source: "guid://aabbccdd".into(),
        source_id: 2,
        root: true,
    },));
    save_world(&root.join("scenes/main.json"), &main.world_data()).unwrap();
    let mut other = World::new();
    other.reserve_entity_ids(1);
    other.spawn((Name {
        value: "other".into(),
    },));
    save_world(&root.join("scenes/other.json"), &other.world_data()).unwrap();

    let mut prefab = World::new();
    prefab.reserve_entity_ids(2);
    prefab.spawn((PrefabInstance {
        source: "Extras/deep.prefab.json".into(),
        source_id: 70,
        root: true,
    },));
    let prefab_path = root.join("Prefabs/top.prefab.json");
    save_prefab(&prefab_path, &prefab.world_data()).unwrap();
    fs::write(
        root.join("Prefabs/top.prefab.json.meta"),
        "(guid: \"aabbccdd\")",
    )
    .unwrap();

    // 只被另一个资产引用的预制体也被扫描，水位还要记住删除实体留过的 ID。
    let mut nested = World::new();
    nested.reserve_entity_ids(70);
    nested.spawn((Name {
        value: "nested".into(),
    },));
    nested.reserve_entity_ids(20);
    save_prefab(&root.join("Extras/deep.prefab.json"), &nested.world_data()).unwrap();
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                dirs.push(entry.path());
            } else {
                let path = entry.path();
                files.insert(
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    files
}

fn migrate(source: &Path, target: &Path, dry_run: bool) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_xgc"));
    cmd.arg("migrate").arg(source).arg(target);
    if dry_run {
        cmd.arg("--dry-run");
    }
    cmd.output().unwrap()
}

fn check(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xgc"))
        .arg("check")
        .arg(root)
        .output()
        .unwrap()
}

fn status(output: &Output, expected: i32) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn upgrades_native_project_without_rewriting_worlds_or_guids() {
    let temp = Temp::new();
    let source = temp.0.join("source");
    let target = temp.0.join("new-parent/upgraded");
    fixture(&source);
    let source_lock = if cfg!(windows) {
        ".XGC-PROJECT.LOCK"
    } else {
        ".xgc-project.lock"
    };
    fs::write(source.join(source_lock), "锁文件只属于源项目").unwrap();
    let before = snapshot(&source);

    let dry_run = migrate(&source, &target, true);
    status(&dry_run, 0);
    assert!(String::from_utf8_lossy(&dry_run.stdout).contains("next_entity_id=91"));
    assert!(!target.parent().unwrap().exists());
    assert_eq!(snapshot(&source), before);

    status(&migrate(&source, &target, false), 0);
    status(&check(&target), 0);
    let mut copied = snapshot(&target);
    let manifest = copied.remove(Path::new("project.xgc")).unwrap();
    assert_eq!(
        copied.remove(Path::new(".xgc-project.lock")),
        Some(Vec::new())
    );
    let manifest_text = String::from_utf8(manifest).unwrap();
    assert!(
        manifest_text.contains("format_version: 2"),
        "{manifest_text}"
    );
    assert!(
        manifest_text.contains("next_entity_id: 91"),
        "{manifest_text}"
    );
    let manifest = load_project(&target.join("project.xgc")).unwrap();
    assert_eq!(manifest.search_paths, ["Fallback"]);
    assert_eq!(
        manifest.quality.preset,
        Some(xgc_base::render_settings::QualityPreset::High)
    );
    assert!(target.join("Resources/empty").is_dir());

    let mut source_files = before.clone();
    source_files.remove(Path::new("project.xgc"));
    source_files.remove(Path::new(source_lock));
    assert_eq!(copied, source_files);
    assert_eq!(snapshot(&source), before);
    status(&check(&source), 1);

    // 升级输出已是当前格式，不能当 v1 再跑一次。
    let repeat = temp.0.join("repeat");
    status(&migrate(&target, &repeat, false), 2);
    assert!(!repeat.exists());
}

#[test]
fn check_reports_entity_ids_and_document_cursors_above_project_watermark() {
    let temp = Temp::new();
    let root = temp.0.join("project");
    fixture(&root);
    // 手写一个低于文件水位的清单，实体 ID 恰好等于项目上界。
    let manifest = MANIFEST.replace(
        "format_version: 1,",
        "format_version: 2,\n    next_entity_id: 70,",
    );
    fs::write(root.join("project.xgc"), manifest).unwrap();
    let before = snapshot(&root);

    let result = check(&root);
    status(&result, 2);
    let stdout = String::from_utf8(result.stdout).unwrap();
    let mut problems = stdout
        .lines()
        .filter(|line| line.contains("[project.id_watermark]"));
    assert_eq!(problems.clone().count(), 2, "{stdout}");
    assert!(problems.all(|line| line.contains("deep.prefab.json")));
    assert!(stdout.contains("文件 ID 水位 91 超出项目 next_entity_id 70"));
    assert!(stdout.contains("实体 ID 70 未低于项目 next_entity_id 70"));
    assert_eq!(snapshot(&root), before);
}

#[test]
fn rejects_duplicate_ids_and_unsafe_output_paths_without_writes() {
    let temp = Temp::new();
    let source = temp.0.join("source");
    fixture(&source);
    let before = snapshot(&source);
    let target = source.join("not-created").join("..").join("target");
    status(&migrate(&source, &target, false), 2);
    assert!(!source.join("not-created").exists());
    status(&migrate(&source, &temp.0, false), 2);
    let existing = temp.0.join("existing");
    fs::create_dir(&existing).unwrap();
    status(&migrate(&source, &existing, false), 2);
    assert_eq!(fs::read_dir(&existing).unwrap().count(), 0);
    assert_eq!(snapshot(&source), before);

    let other = source.join("scenes/other.json");
    let mut data = load_world(&other).unwrap();
    data.entities[0].id = EntityId::from_raw(0);
    save_world(&other, &data).unwrap();
    let before = snapshot(&source);
    let target = temp.0.join("out");
    for dry_run in [true, false] {
        let rejected = migrate(&source, &target, dry_run);
        status(&rejected, 2);
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("[project.duplicate_id]"));
        assert!(!target.exists());
        assert_eq!(snapshot(&source), before);
    }
}
