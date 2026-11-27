//! 原生项目验收走与宿主相同的清单及世界读取边界。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use xgc_reflect::{Component, Value};
use xgc_store::{save_prefab, save_project, save_world, ProjectContent, ProjectManifest};
use xgc_world::{LodGroup, Material, MeshRenderer, Name, Parent, PrefabInstance, Script, World};

#[test]
fn pack_uses_schema_to_find_model_references() {
    let root = Temp::new("pack-lod");
    let assets = root.0.join("source");
    fs::create_dir_all(assets.join("Resources")).unwrap();
    fs::write(assets.join("Resources/far.glb"), "far").unwrap();
    fs::write(
        assets.join("Resources/far.glb.meta"),
        "(guid: \"aabbccdd\")",
    )
    .unwrap();
    fs::write(assets.join("Resources/near.glb"), "near").unwrap();
    fs::write(assets.join("Resources/base.glb"), "base").unwrap();
    fs::write(assets.join("Resources/unused.glb"), "unused").unwrap();
    fs::write(
        assets.join("Resources/unused.glb.meta"),
        "(guid: \"feedface\")",
    )
    .unwrap();

    let mut scene = World::new();
    scene.spawn((LodGroup::default(),));
    scene.spawn((
        MeshRenderer {
            model: "Resources/base.glb".into(),
        },
        Name {
            value: "guid://feedface".into(),
        },
    ));
    let mut data = scene.world_data();
    data.entities[0].components[0].fields[0].1 =
        lod_levels(&["Resources/near.glb", "guid://aabbccdd"]);
    let world = root.0.join("scene.json");
    save_world(&world, &data).unwrap();
    let out = root.0.join("cooked");
    let output = xgc()
        .arg("pack")
        .args([&world, &assets, &out])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    for path in [
        "Resources/near.glb",
        "Resources/far.glb",
        "Resources/far.glb.meta",
        "Resources/base.glb",
    ] {
        assert!(out.join(path).is_file(), "缺少 {path}");
    }
    assert!(!out.join("Resources/unused.glb").exists());
    assert!(!out.join("Resources/unused.glb.meta").exists());
    assert_eq!(
        xgc_store::load_world_binary(&out.join("world.xgcb")).unwrap(),
        data
    );
}

#[test]
fn pack_follows_native_prefabs_and_keeps_material_assets() {
    let root = Temp::new("pack-prefabs");
    let assets = root.0.join("source");
    fs::create_dir_all(assets.join("Prefabs/Nested")).unwrap();
    fs::create_dir_all(assets.join("Resources/Models/bytes")).unwrap();
    fs::create_dir_all(assets.join("Resources/Textures")).unwrap();
    let original = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../projects/XGame/Resources");
    let model = fs::read_to_string(original.join("Fox.gltf"))
        .unwrap()
        .replace("Fox.bin", "bytes/geometry.bin")
        .replace("Texture.png", "../Textures/coat.png");
    fs::write(assets.join("Resources/Models/Fox.gltf"), model).unwrap();
    fs::copy(
        original.join("Fox.bin"),
        assets.join("Resources/Models/bytes/geometry.bin"),
    )
    .unwrap();
    fs::copy(
        original.join("Texture.png"),
        assets.join("Resources/Textures/coat.png"),
    )
    .unwrap();
    fs::write(assets.join("Resources/unused.glb"), "unused").unwrap();
    fs::write(assets.join("Resources/shared.mat"), "(roughness: 0.3)").unwrap();
    fs::write(
        assets.join("Resources/shared.mat.meta"),
        "(guid: \"feedface\")",
    )
    .unwrap();
    fs::write(
        assets.join("Prefabs/first.prefab.json.meta"),
        "(guid: \"aabbccdd\")",
    )
    .unwrap();

    let mut first = World::new();
    first.spawn((PrefabInstance {
        source: "Prefabs/Nested/second.prefab.json".into(),
        source_id: 1,
        root: true,
    },));
    save_prefab(
        &assets.join("Prefabs/first.prefab.json"),
        &first.world_data(),
    )
    .unwrap();
    let mut second = World::new();
    second.reserve_entity_ids(1);
    second.spawn((
        PrefabInstance {
            source: "guid://aabbccdd".into(),
            source_id: 0,
            root: true,
        },
        Material {
            asset: "guid://feedface".into(),
            ..Material::default()
        },
        MeshRenderer {
            model: "Resources/Models/Fox.gltf".into(),
        },
    ));
    save_prefab(
        &assets.join("Prefabs/Nested/second.prefab.json"),
        &second.world_data(),
    )
    .unwrap();
    save_prefab(
        &assets.join("Prefabs/unused.prefab.json"),
        &World::new().world_data(),
    )
    .unwrap();

    let mut scene = World::new();
    scene.reserve_entity_ids(2);
    scene.spawn((PrefabInstance {
        source: "guid://aabbccdd".into(),
        source_id: 0,
        root: true,
    },));
    let world = root.0.join("scene.json");
    save_world(&world, &scene.world_data()).unwrap();
    let out = assets.join("A-cooked");
    let pack = || {
        xgc()
            .arg("pack")
            .args([&world, &assets, &out])
            .output()
            .unwrap()
    };
    let output = pack();
    assert!(output.status.success(), "{output:?}");
    for path in [
        "Prefabs/first.prefab.json",
        "Prefabs/first.prefab.json.meta",
        "Prefabs/Nested/second.prefab.json",
        "Resources/shared.mat",
        "Resources/shared.mat.meta",
        "Resources/Models/Fox.gltf",
        "Resources/Models/bytes/geometry.bin",
        "Resources/Textures/coat.png",
    ] {
        assert_eq!(
            fs::read(out.join(path)).unwrap_or_else(|err| panic!("缺少 {path}：{err}")),
            fs::read(assets.join(path)).unwrap(),
        );
    }
    for unused in ["Prefabs/unused.prefab.json", "Resources/unused.glb"] {
        assert!(!out.join(unused).exists(), "不应烹饪 {unused}");
    }
    xgc_asset::verify_manifest(&out).unwrap();
    assert_eq!(
        xgc_store::load_world_binary(&out.join("world.xgcb")).unwrap(),
        scene.world_data()
    );
    let mut meshes = xgc_render::MeshCache::with_catalog(&out, xgc_asset::scan(&out).unwrap());
    let mesh = meshes.get("Resources/Models/Fox.gltf").unwrap().unwrap();
    assert!(mesh.texture.is_some());
    assert!(!mesh.positions.is_empty());
    assert_eq!(
        meshes
            .material("guid://feedface")
            .unwrap()
            .unwrap()
            .roughness,
        0.3
    );

    let first_pack = snapshot(&out);
    let output = pack();
    assert!(output.status.success(), "{output:?}");
    assert!(snapshot(&out) == first_pack, "重复打包改变了输出文件");
}

#[test]
fn check_and_script_runtime_use_the_same_ordered_search_roots() {
    let root = Temp::new("search-order");
    let mut project = manifest(&root.0);
    project.scenes.truncate(1);
    project.input = None;
    project.search_paths = vec!["First".into(), "Second".into()];
    save_project(&root.0.join("project.xgc"), &project).unwrap();
    for dir in ["First", "Second"] {
        fs::create_dir(root.0.join(dir)).unwrap();
    }
    fs::write(
        root.0.join("First/move.lua"),
        "function update(ctx) set_pos(ctx, 3, 0, 0) end",
    )
    .unwrap();
    fs::write(root.0.join("Second/move.lua"), "local =").unwrap();
    fs::write(root.0.join("Second/model.glb"), "model fixture").unwrap();
    let mut world = World::new();
    let key = world.spawn((
        xgc_world::Transform::default(),
        MeshRenderer {
            model: "model.glb".into(),
        },
        Script {
            paths: vec!["move.lua".into()],
        },
    ));
    let entity = world.entity(key).unwrap();
    let scene = root.0.join(&project.entry_scene);
    save_world(&scene, &world.world_data()).unwrap();
    let result = root.0.join("result.json");
    let verify = |expected_x| {
        let before = snapshot(&root.0);
        let report = String::from_utf8(check(&root.0, 0).stdout).unwrap();
        assert!(report.contains("scripts=1 assets=0 problems=0"), "{report}");
        assert_eq!(snapshot(&root.0), before, "体检必须只读");
        let output = xgc()
            .arg("run-scripts")
            .args([&scene, &root.0])
            .args(["1", result.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let data = xgc_store::load_world(&result).unwrap();
        let mut after = World::new();
        after.restore_world_data(&data).unwrap();
        let transform = after
            .get::<xgc_world::Transform>(after.key(entity).unwrap())
            .unwrap();
        assert_eq!(transform.position, [expected_x, 0.0, 0.0]);
    };
    verify(3.0);
    fs::write(
        root.0.join("move.lua"),
        "function update(ctx) set_pos(ctx, 7, 0, 0) end",
    )
    .unwrap();
    verify(7.0);
}

#[test]
fn checks_models_at_lod_field_paths() {
    let root = Temp::new("lod-models");
    let mut manifest = manifest(&root.0);
    manifest.scenes.truncate(1);
    manifest.input = None;
    save_project(&root.0.join("project.xgc"), &manifest).unwrap();

    let mut scene = World::new();
    scene.spawn((LodGroup::default(),));
    let mut data = scene.world_data();
    data.entities[0].components[0].fields[0].1 =
        lod_levels(&["builtin://cube", "guid://aabbccdd", "Resources/missing.glb"]);
    save_world(&root.0.join(&manifest.entry_scene), &data).unwrap();
    let text = String::from_utf8(check(&root.0, 2).stdout).unwrap();
    assert!(text.contains("levels[1].model guid://aabbccdd"), "{text}");
    assert!(
        text.contains("levels[2].model Resources/missing.glb"),
        "{text}"
    );
    assert!(!text.contains("levels[0].model"), "{text}");
}

fn lod_levels(models: &[&str]) -> Value {
    Value::List(
        models
            .iter()
            .map(|model| {
                Value::Struct(
                    [
                        ("model".into(), Value::String((*model).into())),
                        ("max_distance".into(), Value::F64(30.0)),
                    ]
                    .into(),
                )
            })
            .collect(),
    )
}

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
        next_entity_id: 1_000,
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
        render: Default::default(),
        quality: Default::default(),
        layers: Default::default(),
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
            paths: vec!["Scripts/missing.lua".into()],
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
        "paths[0] Scripts/missing.lua",
        "[prefab.source_id]",
        "[prefab.parse]",
        "[input.read]",
    ] {
        assert!(text.contains(expected), "缺少诊断 {expected}\n{text}");
    }
}

#[test]
fn checks_each_referenced_lua_file_once_without_running_it() {
    let root = Temp::new("lua-syntax");
    let mut manifest = manifest(&root.0);
    manifest.scenes.truncate(1);
    manifest.input = None;
    save_project(&root.0.join("project.xgc"), &manifest).unwrap();
    fs::create_dir(root.0.join("Scripts")).unwrap();
    let broken = "Scripts/broken.lua";
    let valid = "Scripts/valid.lua";
    fs::write(root.0.join(broken), "local value = 1\nlocal =\n").unwrap();
    fs::write(
        root.0.join(format!("{broken}.meta")),
        "(guid: \"aabbccdd\")",
    )
    .unwrap();
    fs::write(root.0.join(valid), "error('检查时不该运行')\n").unwrap();
    let mut scene = World::new();
    scene.spawn((Script {
        paths: [broken, "guid://aabbccdd", valid]
            .map(str::to_owned)
            .to_vec(),
    },));
    save_world(&root.0.join(&manifest.entry_scene), &scene.world_data()).unwrap();

    let before = snapshot(&root.0);
    let text = String::from_utf8(check(&root.0, 2).stdout).unwrap();
    assert!(text.contains("scripts=2 assets=1 problems=1"), "{text}");
    assert_eq!(text.matches("[SCRIPT.COMPILE]").count(), 1, "{text}");
    assert!(text.contains("Scripts/broken.lua:2:0"), "{text}");
    for (path, expected) in [(broken, 2), (valid, 0), ("missing.lua", 1)] {
        let output = xgc()
            .current_dir(&root.0)
            .args(["script-check", path])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(expected), "{output:?}");
        if expected == 2 {
            let diagnostic = String::from_utf8(output.stderr).unwrap();
            assert!(text.contains(diagnostic.trim()), "{text}\n{diagnostic}");
        }
    }
    assert_eq!(snapshot(&root.0), before);
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
