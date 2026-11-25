//! 原生项目验收走与宿主相同的清单及世界读取边界。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use xgc_reflect::{Component, Value};
use xgc_store::{save_prefab, save_project, save_world, ProjectContent, ProjectManifest};
use xgc_world::{MeshRenderer, Parent, PrefabInstance, Script, World};

struct Temp(PathBuf);

impl Temp {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("xgc-native-check-{}-{label}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn xgc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_xgc"))
}

fn check(path: &Path, code: i32) -> Output {
    let output = xgc().arg("check").arg(path).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn manifest(root: &Path) -> ProjectManifest {
    fs::create_dir(root.join("scenes")).unwrap();
    fs::create_dir_all(root.join("Prefabs/Nested")).unwrap();
    let manifest = ProjectManifest {
        format_version: xgc_store::PROJECT_FORMAT_VERSION,
        name: "Fixture".into(),
        entry_scene: "scenes/main.json".into(),
        scenes: vec!["scenes/main.json".into(), "scenes/other.json".into()],
        content: ProjectContent {
            models: "Resources".into(),
            textures: "Resources".into(),
            audio: "Resources".into(),
            shaders: None,
            prefabs: "Prefabs".into(),
            scripts: "Scripts".into(),
        },
        search_paths: Vec::new(),
        input: Some("Controls/input.ron".into()),
    };
    save_project(&root.join("project.xgc"), &manifest).unwrap();
    manifest
}

#[test]
fn checks_real_migration_by_directory_and_manifest_without_writing() {
    let root = Temp::new("migrated");
    let original = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../projects/XGame");
    let out = root.0.join("project");
    let migration = xgc()
        .arg("migrate")
        .arg(original)
        .arg(&out)
        .output()
        .unwrap();
    assert!(migration.status.success(), "{:?}", migration);
    let manifest_path = out.join("project.xgc");
    let before = snapshot(&out);
    for path in [&out, &manifest_path] {
        let output = check(path, 0);
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.contains("scenes=4 entities=35 prefabs=1 scripts=8 assets=18 problems=0"),
            "{text}"
        );
    }
    let output = xgc()
        .current_dir(&out)
        .args(["check", "project.xgc"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(snapshot(&out), before);
}

fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            files.extend(snapshot(&entry.path()));
        } else {
            files.push((entry.path(), fs::read(entry.path()).unwrap()));
        }
    }
    files.sort();
    files
}

#[test]
fn reports_bad_children_assets_and_input_as_project_problems() {
    let root = Temp::new("problems");
    let manifest = manifest(&root.0);
    // GUID 已登记但文件丢失时，不能把资产目录命中当作引用有效。
    fs::write(root.0.join("missing.glb.meta"), "(guid: \"aabbccdd\")").unwrap();
    let mut scene = World::new();
    scene.spawn((
        Parent {
            entity: xgc_base::EntityId::from_raw(999),
        },
        MeshRenderer {
            model: "guid://aabbccdd".into(),
        },
        Script {
            path: "Scripts/missing.lua".into(),
        },
        PrefabInstance {
            source: "Prefabs/Nested/widget.prefab.json".into(),
            source_id: 999,
            root: true,
        },
    ));
    save_world(&root.0.join(&manifest.entry_scene), &scene.world_data()).unwrap();
    let mut prefab = World::new();
    prefab.spawn((MeshRenderer {
        model: "Resources/missing.glb".into(),
    },));
    save_prefab(
        &root.0.join("Prefabs/Nested/widget.prefab.json"),
        &prefab.world_data(),
    )
    .unwrap();
    fs::write(root.0.join("Prefabs/broken.prefab.json"), "bad JSON").unwrap();
    let output = check(&root.0, 2);
    let text = String::from_utf8(output.stdout).unwrap();
    for expected in [
        "[scene.missing]",
        "[scene.parent_missing]",
        "[asset.missing]",
        "guid://aabbccdd",
        "Resources/missing.glb",
        "[script.missing]",
        "[prefab.source_id]",
        "[prefab.parse]",
        "[input.read]",
    ] {
        assert!(text.contains(expected), "缺少诊断 {expected}\n{text}");
    }
}

#[test]
fn entry_read_failure_is_fatal_and_manifest_selection_does_not_fall_back() {
    let root = Temp::new("bad-entry");
    let manifest = manifest(&root.0);
    fs::write(
        root.0.join("project.ron"),
        "(format_version:1,name:\"old\",entry_scene:\"old.ron\")",
    )
    .unwrap();
    fs::write(
        root.0.join("old.ron"),
        "(format_version:2,name:\"old\",entities:[])",
    )
    .unwrap();
    let output = check(&root.0, 1);
    assert!(String::from_utf8_lossy(&output.stderr).contains(&manifest.entry_scene));
    fs::write(root.0.join("project.xgc"), "bad manifest").unwrap();
    let output = check(&root.0, 1);
    assert!(String::from_utf8_lossy(&output.stderr).contains("project.xgc"));
}

#[test]
fn validates_native_world_schema_after_loading() {
    let root = Temp::new("invalid-world");
    let mut manifest = manifest(&root.0);
    manifest.scenes.truncate(1);
    manifest.input = None;
    save_project(&root.0.join("project.xgc"), &manifest).unwrap();
    let mut scene = World::new();
    scene.spawn((Script::default(),));
    let mut data = scene.world_data();
    let script = data.entities[0]
        .components
        .iter_mut()
        .find(|item| item.component == Script::ID)
        .unwrap();
    script.fields.push(("bad_field".into(), Value::Bool(true)));
    save_world(&root.0.join(&manifest.entry_scene), &data).unwrap();
    let output = check(&root.0, 2);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("bad_field"), "{text}");
}

#[test]
fn reports_cross_file_ids_and_cycles_without_rewriting_worlds() {
    let root = Temp::new("ids-and-cycles");
    let mut manifest = manifest(&root.0);
    manifest.input = None;
    save_project(&root.0.join("project.xgc"), &manifest).unwrap();
    let mut world = World::new();
    world.spawn((Parent {
        entity: xgc_base::EntityId::from_raw(2),
    },));
    world.spawn((Parent {
        entity: xgc_base::EntityId::from_raw(1),
    },));
    let data = world.world_data();
    for scene in &manifest.scenes {
        save_world(&root.0.join(scene), &data).unwrap();
    }
    let before = snapshot(&root.0);
    let output = check(&root.0, 2);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("[project.duplicate_id] scenes/other.json"),
        "{text}"
    );
    assert_eq!(text.matches("[scene.parent_cycle]").count(), 2, "{text}");
    assert_eq!(snapshot(&root.0), before);
}
