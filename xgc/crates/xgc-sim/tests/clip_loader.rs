//! 剪辑 RON 加载 / 库回归。

use xgc_sim::{load_clip_ron, AnimationLibrary};

const CLIP: &str = r#"(
    name: "Walk",
    duration: 1.0,
    translation: [
        (time: 0.0, value: (0.0, 0.0, 0.0)),
        (time: 1.0, value: (1.0, 2.0, 3.0)),
    ],
)"#;

#[test]
fn loads_clip_from_ron() {
    let clip = load_clip_ron(CLIP).unwrap();
    assert_eq!(clip.name, "Walk");
    assert_eq!(clip.sample_looped(0.5), [0.5, 1.0, 1.5]);
}

#[test]
fn library_indexes_by_name() {
    let clip = load_clip_ron(CLIP).unwrap();
    let mut library = AnimationLibrary::new();
    library.insert(clip);
    assert_eq!(library.len(), 1);
    assert_eq!(library.sample_looped("Walk", 0.25), Some([0.25, 0.5, 0.75]));
    assert!(library.sample_looped("Missing", 0.0).is_none());
}

#[test]
fn malformed_clip_is_reported() {
    assert!(load_clip_ron("(name:").is_err());
}

#[test]
fn loads_clip_directory_and_reports_bad_files() {
    let dir = std::env::temp_dir().join(format!("xgc-clips-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("walk.ron"), CLIP).unwrap();
    std::fs::write(dir.join("bad.ron"), "(name:").unwrap();
    let (library, issues) = AnimationLibrary::load_dir(&dir).unwrap();
    assert_eq!(library.len(), 1);
    assert_eq!(issues.len(), 1);
    assert_eq!(library.names().collect::<Vec<_>>(), vec!["Walk"]);
    std::fs::remove_dir_all(&dir).unwrap();
}
