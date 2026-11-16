//! 资产目录回归：.meta 扫描、GUID → 路径、重复 GUID。

use std::path::Path;

use xgc_asset::scan;

#[test]
fn scans_meta_and_resolves_guid() {
    let dir = std::env::temp_dir().join(format!(
        "xgc_asset_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("Models")).unwrap();
    std::fs::write(dir.join("Models/Box.glb"), b"fake").unwrap();
    std::fs::write(
        dir.join("Models/Box.glb.meta"),
        "(guid: \"abc123\", kind: Model,)",
    )
    .unwrap();
    std::fs::write(
        dir.join("Models/Other.glb.meta"),
        "(guid: \"abc123\", kind: Model,)",
    )
    .unwrap();

    let catalog = scan(&dir).unwrap();
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog.issues().len(), 1);
    assert!(catalog
        .resolve("guid://abc123")
        .unwrap()
        .ends_with("Box.glb"));
    assert!(catalog.resolve("abc123").is_some());
    assert!(catalog.resolve("missing").is_none());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn scans_real_project_when_available() {
    let root = Path::new("../../../projects/XGame");
    if !root.exists() {
        eprintln!("skip: 没有 projects/XGame");
        return;
    }
    let catalog = scan(root).unwrap();
    assert!(catalog.len() >= 3);
    let resolved = catalog
        .resolve("guid://43b41a2008432f77ca7fbca93854f890")
        .unwrap();
    assert!(resolved.ends_with("Box.glb"), "实际 {}", resolved.display());
}
