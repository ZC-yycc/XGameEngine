//! 读路径的物理位置、优先级和错误口径须让宿主与打包看到同一结果。

use std::fs;
use std::path::PathBuf;

use xgc_asset::{resolve_read_path, scan, ReadPathError, SearchPaths};

struct Fixture(PathBuf);

impl Fixture {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("xgc-read-path-{}-{label}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn write(&self, path: &str, bytes: &[u8]) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn direct_file_then_configured_search_roots() {
    let fixture = Fixture::new("priority");
    let root = fixture.0.join("project");
    let reference = r" .\Scripts\main.lua ";
    let search_paths = SearchPaths::new(&[
        "../outside".into(),
        "Missing".into(),
        r".\ZFirst".into(),
        "ASecond".into(),
    ]);
    fixture.write("outside/Scripts/main.lua", b"outside");
    fixture.write("project/Scripts/main.lua", b"direct");
    fixture.write("project/ZFirst/Scripts/main.lua", b"first");
    fixture.write("project/ASecond/Scripts/main.lua", b"second");

    let direct = root.join("Scripts/main.lua");
    assert_eq!(
        resolve_read_path(&root, reference, &SearchPaths::default(), None).unwrap(),
        direct
    );
    assert_eq!(
        resolve_read_path(&root, reference, &search_paths, None).unwrap(),
        direct
    );
    fs::remove_file(&direct).unwrap();
    // 同名目录不是可读文件，仍应继续探测兜底根。
    fs::create_dir(&direct).unwrap();
    let first = root.join("ZFirst/Scripts/main.lua");
    assert_eq!(
        resolve_read_path(&root, reference, &search_paths, None).unwrap(),
        first
    );
    fs::remove_file(&first).unwrap();
    let second = root.join("ASecond/Scripts/main.lua");
    assert_eq!(
        resolve_read_path(&root, reference, &search_paths, None).unwrap(),
        second
    );
    fs::remove_file(&second).unwrap();
    assert_eq!(
        resolve_read_path(&root, reference, &search_paths, None).unwrap_err(),
        ReadPathError::Missing {
            relative: "Scripts/main.lua".into(),
            tried: [
                "Scripts/main.lua",
                "Missing/Scripts/main.lua",
                "ZFirst/Scripts/main.lua",
                "ASecond/Scripts/main.lua"
            ]
            .map(String::from)
            .into(),
        }
    );
}

#[test]
fn rejects_unsafe_logical_paths_before_probing() {
    let fixture = Fixture::new("unsafe");
    let root = fixture.0.join("project");
    fixture.write("outside.lua", b"outside");
    fixture.write("project/Fallback/main.lua", b"inside");
    let search_paths = SearchPaths::new(&["Fallback".into()]);
    for reference in [
        "../outside.lua",
        "/main.lua",
        r"C:\main.lua",
        r"\\host\share\main.lua",
        "Scripts/CON.lua",
        "Scripts/main.lua:stream",
        "",
    ] {
        assert!(
            matches!(
                resolve_read_path(&root, reference, &search_paths, None),
                Err(ReadPathError::Invalid(_))
            ),
            "{reference}"
        );
    }
}

#[test]
fn guid_reads_only_the_registered_file() {
    let fixture = Fixture::new("guid");
    let root = &fixture.0;
    fixture.write("Physical/mesh.glb", b"registered");
    fixture.write("Physical/mesh.glb.meta", br#"(guid: "abcd1234")"#);
    fixture.write("Fallback/Physical/mesh.glb", b"shadow");
    fixture.write("Fallback/abcd1234", b"plain path");
    let search_paths = SearchPaths::new(&["Fallback".into()]);
    let catalog = scan(root).unwrap();
    let path = root.join("Physical/mesh.glb");
    let guid = "guid://abcd1234";
    assert_eq!(
        resolve_read_path(root, guid, &search_paths, Some(&catalog)).unwrap(),
        path
    );
    assert_eq!(
        resolve_read_path(root, "abcd1234", &search_paths, Some(&catalog)).unwrap(),
        root.join("Fallback/abcd1234")
    );
    assert_eq!(
        resolve_read_path(root, guid, &search_paths, None).unwrap_err(),
        ReadPathError::GuidRequiresCatalog
    );
    assert_eq!(
        resolve_read_path(root, "guid://cafe", &search_paths, Some(&catalog)).unwrap_err(),
        ReadPathError::UnknownGuid("guid://cafe".into())
    );
    fs::remove_file(&path).unwrap();
    assert_eq!(
        resolve_read_path(root, guid, &search_paths, Some(&catalog)).unwrap_err(),
        ReadPathError::MissingGuidFile {
            reference: guid.into(),
            path
        }
    );
}
