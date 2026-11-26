//! 资产烹饪：复制需要的源资产、忽略构建目录、manifest 确定。

use std::path::Path;

use xgc_asset::cook_project;

#[test]
fn cooks_referenced_sources_and_stale_files() {
    let root = std::env::temp_dir().join(format!(
        "xgc_cook_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let out = root.join("cooked");
    std::fs::create_dir_all(root.join("Resources")).unwrap();
    std::fs::create_dir_all(root.join("Scripts")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("Resources/mesh.glb"), b"glb").unwrap();
    std::fs::write(root.join("Scripts/a.lua"), b"function update() end").unwrap();
    std::fs::write(root.join("input.ron"), b"InputMap()").unwrap();
    std::fs::write(root.join("target/ignored.txt"), b"x").unwrap();
    std::fs::write(root.join("README.md"), b"x").unwrap();

    let manifest = cook_project(&root, &out).unwrap();
    let paths: Vec<_> = manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec!["Resources/mesh.glb", "Scripts/a.lua", "input.ron"]
    );
    assert_eq!(
        std::fs::read(out.join("Resources/mesh.glb")).unwrap(),
        b"glb"
    );
    assert!(!out.join("target/ignored.txt").exists());

    std::fs::write(
        root.join("Scripts/a.lua"),
        b"function update() return 1 end",
    )
    .unwrap();
    std::fs::write(root.join("Scripts/b.lua"), b"return 2").unwrap();
    let manifest = cook_project(&root, &out).unwrap();
    assert_eq!(manifest.files.len(), 4);
    assert_eq!(
        std::fs::read(out.join("Scripts/b.lua")).unwrap(),
        b"return 2"
    );
    let manifest_text = std::fs::read_to_string(out.join("cook.json")).unwrap();
    assert!(manifest_text.contains("Scripts/b.lua"));
    assert_eq!(
        manifest
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "Resources/mesh.glb",
            "Scripts/a.lua",
            "Scripts/b.lua",
            "input.ron"
        ]
    );
    std::fs::remove_dir_all(Path::new(&root)).unwrap();
}

#[test]
fn cooks_only_referenced_assets_and_sidecars() {
    let fixture = std::env::temp_dir().join(format!(
        "xgc_prune_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let root = fixture.join("source");
    let out = fixture.join("cooked");
    std::fs::create_dir_all(root.join("models/data")).unwrap();
    std::fs::create_dir_all(root.join("textures")).unwrap();
    std::fs::write(fixture.join("outside.png"), b"outside").unwrap();
    std::fs::write(root.join("a.lua"), b"a").unwrap();
    std::fs::write(root.join("a.lua.meta"), b"meta").unwrap();
    std::fs::write(root.join("b.lua"), b"b").unwrap();
    std::fs::write(
        root.join("models/a.gltf"),
        br#"{
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": 3, "uri": "data/vertices.bin"}],
            "images": [
                {"uri": "file://../textures/albedo.png"},
                {"uri": "../textures/albedo.png"},
                {"uri": "../../outside.png"},
                {"uri": "../../source/textures/unused.png"},
                {"uri": "data:image/png;base64,AAAA"}
            ]
        }"#,
    )
    .unwrap();
    std::fs::write(root.join("models/data/vertices.bin"), b"bin").unwrap();
    std::fs::write(root.join("models/a.bin"), b"unused").unwrap();
    std::fs::write(root.join("models/b.glb"), b"glb").unwrap();
    std::fs::write(root.join("textures/albedo.png"), b"png").unwrap();
    std::fs::write(root.join("textures/albedo.png.meta"), b"meta").unwrap();
    std::fs::write(root.join("textures/unused.png"), b"unused").unwrap();
    std::fs::write(root.join("input.ron"), b"InputMap()").unwrap();
    let keep = ["a.lua".to_string(), "models/a.gltf".to_string()]
        .into_iter()
        .collect();
    let manifest = xgc_asset::cook_project_filtered(&root, &out, Some(&keep)).unwrap();
    let paths: Vec<_> = manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec![
            "a.lua",
            "a.lua.meta",
            "input.ron",
            "models/a.gltf",
            "models/data/vertices.bin",
            "textures/albedo.png",
            "textures/albedo.png.meta"
        ]
    );
    for unused in [
        "b.lua",
        "models/a.bin",
        "models/b.glb",
        "textures/unused.png",
    ] {
        assert!(!out.join(unused).exists(), "不应烹饪 {unused}");
    }
    std::fs::remove_dir_all(fixture).unwrap();
}
