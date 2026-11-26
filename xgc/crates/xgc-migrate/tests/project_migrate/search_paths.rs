use std::fs;
use std::path::Path;

use xgc_migrate::{check_old_project, migrate_old_project, ProjectMigrationError};
use xgc_store::{load_project, load_world};
use xgc_world::{AudioSource, MeshRenderer, Script, World};

use super::{entity, project, scene, staging_of, standard_project, write, write_meta};

fn set_search_paths(root: &Path, paths: &str) -> String {
    let text = project("Scenes/a.ron", &["Scenes/a.ron", "Scenes/b.ron"])
        .replace("search_paths: []", &format!("search_paths: {paths}"));
    write(root, "project.ron", &text);
    text
}

#[test]
fn copies_search_roots_in_place_and_preserves_asset_references() {
    let (root, out) = standard_project("search-paths");
    let old_manifest = set_search_paths(
        &root,
        r#"["Missing", ".\\ZPreferred", "AAlternate", "ZPreferred/Resources", "./ZPreferred", "Resources"]"#,
    );
    let direct = entity(1).replace(
        "entries: []",
        r#"entries: [(data: Lua(path: "Scripts/main.lua")),
                     (data: Mesh((model: "Resources/Fox.gltf")))]"#,
    );
    let fallback = entity(2).replace(
        "entries: []",
        r#"entries: [(data: Lua(path: "Scripts/fallback.lua")),
                     (data: Mesh((model: "guid://abcd1234"))),
                     (data: Audio((clip: "Resources/Audio/Beep.wav")))]"#,
    );
    let old_scene = scene(&format!("{direct},{fallback}"));
    write(&root, "Scenes/a.ron", &old_scene);
    let scripts = [
        (
            "ZPreferred/Scripts/main.lua",
            "function update(ctx) return 1 end\n",
        ),
        (
            "ZPreferred/Scripts/fallback.lua",
            "function update(ctx) return 2 end\n",
        ),
        (
            "AAlternate/Scripts/fallback.lua",
            "function update(ctx) return 3 end\n",
        ),
    ];
    for (path, text) in scripts {
        write(&root, path, text);
    }
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../projects/XGame/Resources");
    let models_and_audio = [
        ("Fox.gltf", "ZPreferred/Resources/Fox.gltf"),
        ("Fox.bin", "ZPreferred/Resources/Fox.bin"),
        ("Texture.png", "ZPreferred/Resources/Texture.png"),
        ("Box.glb", "AAlternate/Resources/Box.glb"),
        ("Audio/Beep.wav", "AAlternate/Resources/Audio/Beep.wav"),
    ];
    for (source, path) in models_and_audio {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::copy(assets.join(source), path).unwrap();
    }
    let guid_model = "AAlternate/Resources/Box.glb";
    write_meta(&root, guid_model, "abcd1234");

    let check = check_old_project(&root).unwrap();
    assert!(!check.has_problems(), "{}", check.render());
    assert_eq!(check.scripts, 2);
    let dry = migrate_old_project(&root, &out, true).unwrap();
    assert!(!out.exists());
    assert!(!staging_of(&out).exists());
    assert_eq!(
        dry.copied_files,
        4 + scripts.len() + models_and_audio.len() + 1,
        "搜索根文件及 .meta 应进入干跑计划，重叠目录只计一次"
    );
    let written = migrate_old_project(&root, &out, false).unwrap();
    assert_eq!(dry.copied_files, written.copied_files);
    assert!(!staging_of(&out).exists());
    let manifest = load_project(&out.join("project.xgc")).unwrap();
    assert_eq!(
        manifest.search_paths,
        [
            "Missing",
            "ZPreferred",
            "AAlternate",
            "ZPreferred/Resources",
            "Resources"
        ]
    );
    assert!(!out.join("Missing").exists());
    assert!(!out.join("Resources/Fox.gltf").exists());
    assert!(!out.join("Scripts/fallback.lua").exists());
    assert_eq!(
        fs::read(out.join("Scripts/main.lua")).unwrap(),
        fs::read(root.join("Scripts/main.lua")).unwrap()
    );
    for (path, text) in scripts {
        assert_eq!(fs::read_to_string(out.join(path)).unwrap(), text);
    }
    for (source, path) in models_and_audio {
        assert_eq!(
            fs::read(out.join(path)).unwrap(),
            fs::read(assets.join(source)).unwrap()
        );
    }
    let catalog = xgc_asset::scan(&out).unwrap();
    assert_eq!(
        catalog.resolve("guid://abcd1234"),
        Some(out.join(guid_model).as_path())
    );

    let data = load_world(&out.join(&manifest.entry_scene)).unwrap();
    let mut world = World::new();
    world.restore_world_data(&data).unwrap();
    let direct = world.key(data.entities[0].id).unwrap();
    let fallback = world.key(data.entities[1].id).unwrap();
    assert_eq!(
        world.get::<Script>(direct).unwrap().path,
        "Scripts/main.lua"
    );
    assert_eq!(
        world.get::<Script>(fallback).unwrap().path,
        "Scripts/fallback.lua"
    );
    assert_eq!(
        world.get::<MeshRenderer>(direct).unwrap().model,
        "Resources/Fox.gltf"
    );
    assert_eq!(
        world.get::<MeshRenderer>(fallback).unwrap().model,
        "guid://abcd1234"
    );
    assert_eq!(
        world.get::<AudioSource>(fallback).unwrap().clip,
        "Resources/Audio/Beep.wav"
    );
    assert_eq!(
        fs::read_to_string(root.join("project.ron")).unwrap(),
        old_manifest
    );
    assert_eq!(
        fs::read_to_string(root.join("Scenes/a.ron")).unwrap(),
        old_scene
    );
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(out).unwrap();
}

#[test]
fn rejects_search_root_that_escapes_project() {
    let (root, out) = standard_project("search-path-escape");
    let old_manifest = set_search_paths(&root, r#"["../outside"]"#);
    for dry_run in [true, false] {
        let error = migrate_old_project(&root, &out, dry_run).unwrap_err();
        assert!(
            matches!(error, ProjectMigrationError::Problems(ref problems) if problems.iter().any(|line| {
                line.contains("[content.search_path]") && line.contains("../outside")
            })),
            "{error}"
        );
        assert!(!out.exists());
        assert!(!staging_of(&out).exists());
    }
    assert_eq!(
        fs::read_to_string(root.join("project.ron")).unwrap(),
        old_manifest
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_search_root_file_collision_before_writing() {
    let (root, out) = standard_project("search-path-conflict");
    set_search_paths(&root, r#"["scenes"]"#);
    write(&root, "scenes/a.json", "existing asset");
    for dry_run in [true, false] {
        let error = migrate_old_project(&root, &out, dry_run).unwrap_err();
        assert!(
            matches!(error, ProjectMigrationError::Manifest(ref message) if message.contains("scenes/a.json")),
            "{error}"
        );
        assert!(!out.exists());
        assert!(!staging_of(&out).exists());
    }
    assert_eq!(
        fs::read_to_string(root.join("scenes/a.json")).unwrap(),
        "existing asset"
    );
    fs::remove_dir_all(root).unwrap();
}
