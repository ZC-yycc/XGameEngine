//! 真实项目迁移的 Lua 审计必须与 dry-run 共用口径，且不改脚本原文。

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn migrate(source: &Path, target: &Path, dry_run: bool) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xgc"));
    command.arg("migrate").args([source, target]);
    if dry_run {
        command.arg("--dry-run");
    }
    let Output {
        status,
        stdout,
        stderr,
    } = command.output().unwrap();
    assert!(
        status.success(),
        "{status}: {}",
        String::from_utf8_lossy(&stderr)
    );
    assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(&stderr));
    String::from_utf8(stdout).unwrap()
}

fn lua_report(report: &str) -> &str {
    let (summary, audit) = report.split_once("\n  lua-api: ").expect(report);
    assert!(
        summary.starts_with("migrate: project=XGame scenes=4 entities=35 "),
        "{summary}"
    );
    assert_eq!(
        summary.lines().nth(1).unwrap(),
        "  entity ID map: entity-id-map.json"
    );
    audit
}

#[test]
fn project_migration_reports_installed_lua_api_and_settlement() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../projects/XGame");
    let scratch = std::env::temp_dir().join(format!(
        "xgc-cli-migration-lua-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let target = scratch.join("project");
    let script = "Scripts/Jumper.lua";
    let original = fs::read(source.join(script)).unwrap();
    let dry_run = migrate(&source, &target, true);
    assert!(!scratch.exists(), "dry-run 不应创建输出目录");
    let audit = lua_report(&dry_run);
    assert_eq!(dry_run, migrate(&source, &target, true));
    let migrated = migrate(&source, &target, false);
    assert_eq!(audit, lua_report(&migrated));
    assert_eq!(fs::read(target.join(script)).unwrap(), original);
    assert_eq!(fs::read(source.join(script)).unwrap(), original);
    fs::remove_dir_all(scratch).unwrap();

    assert!(
        audit.starts_with("legacy=182 registered=68 missing=114 standard_only=2 extra=0\n"),
        "{audit}"
    );
    for evidence in [
        "standard_only=[error,warn]",
        "xgc=[add,log,remove,set]",
        "registered=[action_down,action_pressed,action_released,axis]",
        "absent=[spawn_ball,spawn_entity,spawn_entity_at,spawn_prefab,spawn_prefab_under,spawn_static_box]",
        "timing [hooks]",
        "timing [physics]",
        "timing [collision]",
        "timing [spawn]",
        "timing [transform]",
        "timing [animation]",
    ] {
        assert!(audit.contains(evidence), "缺少 {evidence}：{audit}");
    }
}
