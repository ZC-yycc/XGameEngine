//! 旧 headless 统计与迁移世界对照。

use std::path::Path;

use xgc_migrate::{compare_headless, migrate_str};

const OLD_STATS: &str = r#"{
    "entities": 24,
    "bodies": 10,
    "colliders": 10,
    "particles": {"emitters": 2},
    "audio": {"active": 1}
}"#;

#[test]
fn showcase_stats_match_old_headless() {
    let path = Path::new("../../../projects/XGame/Scenes/showcase.ron");
    if !path.exists() {
        eprintln!("skip: 没有旧项目 showcase.ron");
        return;
    }
    let (data, _) = migrate_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let report = compare_headless(&format!("\u{feff}{OLD_STATS}"), &data).unwrap();
    assert!(report.differences.is_empty(), "{:?}", report.differences);
    assert_eq!(report.new.entities, 24);
    assert_eq!(report.new.bodies, 10);
    assert_eq!(report.new.emitters, 2);
    assert_eq!(report.new.audio_active, 1);
}

#[test]
fn reports_entity_count_difference() {
    let path = Path::new("../../../projects/XGame/Scenes/showcase.ron");
    if !path.exists() {
        eprintln!("skip: 没有旧项目 showcase.ron");
        return;
    }
    let (data, _) = migrate_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let report = compare_headless(
        r#"{"entities": 23, "bodies": 10, "colliders": 10, "particles": {"emitters": 2}, "audio": {"active": 1}}"#,
        &data,
    )
    .unwrap();
    assert!(report
        .differences
        .iter()
        .any(|line| line.contains("entities")));
}
