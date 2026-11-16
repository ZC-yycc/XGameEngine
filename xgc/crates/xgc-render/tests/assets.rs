//! MeshCache 回归：builtin、缺失、GUID 拒绝、真实 GLB 缓存。

use std::path::Path;

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
