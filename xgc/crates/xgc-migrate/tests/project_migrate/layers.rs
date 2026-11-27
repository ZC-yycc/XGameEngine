use super::*;
use serde_json::{json, to_value};

#[test]
fn migrates_project_collision_layers() {
    let (root, out) = standard_project("layers");
    let path = root.join("project.ron");
    let text = fs::read_to_string(&path).unwrap();
    let prefix = text.trim_end().strip_suffix(')').unwrap();
    // 第 3 层允许与第 12 层碰撞，但第 12 层只允许第 31 层。
    // 零掩码有实际语义，全通掩码、空名和越界覆盖按旧项目的规则裁掉。
    let source = format!(
        r#"{prefix}
            layers: (
                names: {{3: "  Pickup  ", 12: "Ground", 30: "  ", 31: "Ghost", 32: "Invalid"}},
                masks: {{3: 4096, 12: 2147483648, 30: 4294967295, 31: 0, 32: 1}},
            ),
        )"#
    );
    fs::write(&path, &source).unwrap();
    write(
        &root,
        "Scenes/b.ron",
        &scene(&entity(3).replace("layer: 0", "layer: 31")),
    );

    let dry = migrate_old_project(&root, &out, true).unwrap();
    assert!(dry.dry_run);
    assert!(!out.exists());
    assert!(!staging_of(&out).exists());
    let written = migrate_old_project(&root, &out, false).unwrap();
    assert!(!written.dry_run);
    assert_eq!(
        dry.render().replace("（dry-run，未写盘）", ""),
        written.render()
    );

    let manifest_path = out.join("project.xgc");
    let manifest = load_project(&manifest_path).unwrap();
    assert_eq!(
        to_value(manifest).unwrap()["layers"],
        json!({
            "names": {"3": "Pickup", "12": "Ground", "31": "Ghost"},
            "masks": {"3": 4096, "12": 2147483648u32, "31": 0},
        }),
        "迁移后的项目矩阵应保留有效覆盖"
    );
    let report = written.render();
    assert!(
        report.contains("[migration.layers] names=3 masks=3"),
        "{report}"
    );
    assert!(written.unmatched.is_empty(), "{:?}", written.unmatched);
    assert_eq!(fs::read_to_string(&path).unwrap(), source);
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(out).unwrap();
}
