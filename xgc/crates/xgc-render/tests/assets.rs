//! MeshCache 回归：builtin、缺失、GUID 拒绝、真实 GLB 缓存。

use std::path::Path;

use xgc_asset::{scan, AssetPaths, SearchPaths};
use xgc_render::MeshCache;

#[test]
fn builtin_missing_and_guid_are_distinguished() {
    let mut cache = MeshCache::new(".");
    assert!(cache.get("builtin://quad").unwrap().is_some());
    assert!(cache.get("missing.glb").unwrap().is_none());
    assert!(cache.get("guid://abc").is_err());
    assert!(cache.get("../escape.glb").is_err());
    assert_eq!(cache.len(), 1);
}

#[test]
fn loads_and_caches_box_glb_when_available() {
    let root = Path::new("../../../projects/XGame/Resources");
    if !root.join("Box.glb").exists() {
        eprintln!("skip: 没有 Box.glb");
        return;
    }
    let mut cache = MeshCache::new(root);
    let mesh = cache.get("Box.glb").unwrap().unwrap();
    assert_eq!(mesh.positions.len(), 24);
    assert_eq!(mesh.indices.len(), 36);
    let cached = cache.get("Box.glb").unwrap().unwrap();
    assert_eq!(cached.positions.len(), 24);
    assert_eq!(cache.len(), 1);
}

#[test]
fn loads_model_from_the_first_matching_search_root() {
    let dir = std::env::temp_dir().join(format!("xgc-model-search-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("First")).unwrap();
    std::fs::create_dir_all(dir.join("Second")).unwrap();
    std::fs::copy(
        "../../../projects/XGame/Resources/Box.glb",
        dir.join("First/Box.glb"),
    )
    .unwrap();
    std::fs::write(dir.join("Second/Box.glb"), "invalid model").unwrap();
    let paths = AssetPaths::new(
        &dir,
        SearchPaths::new(&["First".into(), "Second".into()]),
        None,
    );
    let mut cache = MeshCache::with_assets(paths.clone());
    let mesh = cache.get("Box.glb").unwrap().unwrap();
    assert_eq!((mesh.positions.len(), mesh.indices.len()), (24, 36));
    // 直接命中的文件格式有错也应报错，不能静默换成 fallback 的不同模型。
    std::fs::write(dir.join("Box.glb"), "invalid direct model").unwrap();
    let mut cache = MeshCache::with_assets(paths);
    assert!(cache.get("Box.glb").is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn resolves_guid_through_asset_catalog() {
    let root = Path::new("../../../projects/XGame");
    if !root.exists() {
        eprintln!("skip: 没有 projects/XGame");
        return;
    }
    let catalog = scan(root).unwrap();
    let mut cache = MeshCache::with_catalog(".", catalog);
    let mesh = cache
        .get("guid://43b41a2008432f77ca7fbca93854f890")
        .unwrap()
        .unwrap();
    assert_eq!(mesh.positions.len(), 24);
    assert_eq!(mesh.indices.len(), 36);
}
