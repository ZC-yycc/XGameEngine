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
