//! 迁移验收：重新迁移结果逐实体 / 逐字段一致。

use std::path::Path;

use xgc_migrate::{migrate_str, verify_migration};

#[test]
fn showcase_migration_matches_json_roundtrip() {
    let path = Path::new("../../../projects/XGame/Scenes/showcase.ron");
    if !path.exists() {
        eprintln!("skip: 没有旧项目 showcase.ron");
        return;
    }
    let text = std::fs::read_to_string(path).unwrap();
    let (data, _) = migrate_str(&text).unwrap();
    verify_migration(&text, &data).expect("内存迁移应一致");

    let dir = std::env::temp_dir().join(format!(
        "xgc_verify_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("world.json");
    xgc_store::save_world(&file, &data).unwrap();
    let loaded = xgc_store::load_world(&file).unwrap();
    verify_migration(&text, &loaded).expect("JSON 往返后应一致");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn verify_reports_changed_field() {
    let path = Path::new("../../../projects/XGame/Scenes/showcase.ron");
    if !path.exists() {
        eprintln!("skip: 没有旧项目 showcase.ron");
        return;
    }
    let text = std::fs::read_to_string(path).unwrap();
    let (mut data, _) = migrate_str(&text).unwrap();
    let component = data
        .entities
        .iter_mut()
        .flat_map(|entity| &mut entity.components)
        .find(|component| component.component.0 == 0x0101)
        .unwrap();
    for (name, value) in &mut component.fields {
        if name == "position" {
            *value = xgc_world::Value::Vec3([123.0, 0.0, 0.0]);
        }
    }
    let differences = verify_migration(&text, &data).unwrap_err();
    assert!(differences.iter().any(|line| line.contains("position")));
}

/// 环境不挂在实体上：丢光源 / 改环境光必须被单独抓到，否则静默降级成"没光照"。
#[test]
fn verify_reports_changed_light_and_ambient() {
    let path = Path::new("../../../projects/XGame/Scenes/showcase.ron");
    if !path.exists() {
        eprintln!("skip: 没有旧项目 showcase.ron");
        return;
    }
    let text = std::fs::read_to_string(path).unwrap();
    let (mut data, _) = migrate_str(&text).unwrap();
    let sun = data
        .environment
        .lights
        .first_mut()
        .expect("showcase 应迁移出场景光源");
    assert_eq!(sun.name, "Sun");
    sun.intensity = 123.0;
    data.environment.ambient = [0.5, 0.5, 0.5];

    let differences = verify_migration(&text, &data).unwrap_err();
    assert!(
        differences.iter().any(|line| line.contains("ambient")),
        "环境光差异必须点名：{differences:?}"
    );
    assert!(
        differences
            .iter()
            .any(|line| line.contains("光源 0") && line.contains("intensity")),
        "光源字段差异必须点名：{differences:?}"
    );
}
