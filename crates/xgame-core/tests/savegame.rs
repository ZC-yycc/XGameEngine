//! 存档格式与落盘纪律。
//!
//! 这一组钉的是四件事：
//! 1. **槽名不能当地路径用**（`../x` / 盘符 / `a:b` / `CON` 全部拒绝）；
//! 2. 写 → 读**逐字段相等**，且同状态两次写盘**逐字节相同**（可 diff、可断言）；
//! 3. 坏存档**拒绝加载**（版本不对 / 非有限数 / 非单位旋转），而不是"修一修" ——
//!    被修过的存档会让玩家看到自己没玩过的状态；
//! 4. 变量表（`SaveVars`）读档是**替换**不是合并。

use std::collections::BTreeMap;
use std::path::PathBuf;

use xgame_core::savegame::{
    delete_save, list_saves, read_save, save_exists, save_slot_relative, write_save, EntityState,
    SaveError, SaveGame, SaveValue, SaveVars, SAVE_DIR, SAVE_FORMAT_VERSION,
};
use xgame_core::EntityId;

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "xgame_save_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn sample_save() -> SaveGame {
    let mut save = SaveGame::new();
    save.scene = "Scenes/Level1.ron".into();
    save.game_time = 42.5;
    save.vars.insert("coins".into(), SaveValue::Number(12.0));
    save.vars.insert("checkpoint".into(), SaveValue::Text("gate".into()));
    // 故意**乱序**塞进去：写盘前必须排序，否则两次存档字节不同。
    save.entities.push(EntityState {
        id: 7,
        position: [1.0, 2.0, 3.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
        linear_velocity: [0.0, -1.5, 0.0],
        angular_velocity: [0.0, 0.0, 0.0],
        vars: BTreeMap::from([("hp".to_string(), SaveValue::Number(80.0))]),
    });
    save.entities.push(EntityState {
        id: 3,
        position: [-4.0, 0.5, 2.0],
        rotation: [0.0, 0.707_106_77, 0.0, 0.707_106_77],
        scale: [2.0, 2.0, 2.0],
        linear_velocity: [0.0, 0.0, 0.0],
        angular_velocity: [0.0, 0.0, 0.0],
        vars: BTreeMap::new(),
    });
    save
}

/// 槽名是**文件名**的一部分：越界 / Windows 不认的写法必须拒绝。
#[test]
fn slot_names_cannot_escape_or_use_windows_traps() {
    for bad in ["../escape", "a:b", "CON", "x/../../y", "", "/", "  "] {
        match save_slot_relative(bad) {
            Err(SaveError::InvalidSlot { .. }) => {}
            other => panic!("槽名 {bad:?} 必须被拒绝，实际: {other:?}"),
        }
    }
    // 合法写法：子目录、带不带 `.sav`、结尾斜杠，都落到同一个文件。
    assert_eq!(save_slot_relative("slot1").unwrap(), "Saves/slot1.sav");
    assert_eq!(save_slot_relative("slot1.sav").unwrap(), "Saves/slot1.sav");
    assert_eq!(save_slot_relative("chapter1/slot2").unwrap(), "Saves/chapter1/slot2.sav");
    assert_eq!(save_slot_relative("chapter/").unwrap(), "Saves/chapter.sav");
}

/// 写 → 读逐字段相等；同状态两次写盘**逐字节相同**（实体按 ID 排序）。
#[test]
fn roundtrip_is_lossless_and_deterministic() {
    let root = temp_root("roundtrip");
    let save = sample_save();
    let relative = write_save(&root, "slot1", &save).expect("写档");
    assert_eq!(relative, "Saves/slot1.sav");

    let loaded = read_save(&root, "slot1").expect("读档");
    assert_eq!(loaded.scene, "Scenes/Level1.ron");
    assert_eq!(loaded.game_time, 42.5);
    assert_eq!(loaded.entities.len(), 2);
    assert_eq!(loaded.entities[0].id, 3, "写盘时按 ID 升序");
    assert_eq!(loaded.entities[1].id, 7);
    assert_eq!(loaded.vars.get("coins"), Some(&SaveValue::Number(12.0)));
    assert_eq!(
        loaded.entity(EntityId::from_raw(7)).unwrap().vars.get("hp"),
        Some(&SaveValue::Number(80.0))
    );

    let first = std::fs::read(root.join("Saves/slot1.sav")).unwrap();
    // 同样的状态再写一次（这次实体顺序是倒着给的，排序保证字节不变）。
    let mut again = save.clone();
    again.entities.reverse();
    write_save(&root, "slot1", &again).expect("再写档");
    let second = std::fs::read(root.join("Saves/slot1.sav")).unwrap();
    assert_eq!(first, second, "同状态两次存档必须逐字节相同");

    std::fs::remove_dir_all(&root).unwrap();
}

/// 坏存档**拒绝加载**，并且报出具体哪一条。
#[test]
fn corrupt_saves_are_rejected_with_reasons() {
    let root = temp_root("reject");

    let mut future = SaveGame::new();
    future.format_version = SAVE_FORMAT_VERSION + 1;
    match write_save(&root, "future", &future) {
        Err(SaveError::Invalid { defects, .. }) => {
            assert!(defects[0].contains("format_version"), "{defects:?}")
        }
        other => panic!("版本不对必须拒绝，实际: {other:?}"),
    }

    let mut nan = sample_save();
    nan.entities[0].position[1] = f32::NAN;
    assert!(
        matches!(write_save(&root, "nan", &nan), Err(SaveError::Invalid { .. })),
        "非有限数不许进存档"
    );

    let mut tilted = sample_save();
    tilted.entities[0].rotation = [0.0, 0.3, 0.0, 0.95]; // |q|² = 0.9925
    match write_save(&root, "tilted", &tilted) {
        Err(SaveError::Invalid { defects, .. }) => {
            assert!(defects[0].contains("not unit"), "{defects:?}")
        }
        other => panic!("非单位旋转必须拒绝，实际: {other:?}"),
    }

    // 手改坏的文本：解析失败要说清是哪份文件。
    std::fs::create_dir_all(root.join(SAVE_DIR)).unwrap();
    std::fs::write(root.join("Saves/broken.sav"), "(format_version: 1, scene:").unwrap();
    match read_save(&root, "broken") {
        Err(SaveError::Parse { path, .. }) => assert_eq!(path, "Saves/broken.sav"),
        other => panic!("坏文本必须报解析失败，实际: {other:?}"),
    }

    std::fs::remove_dir_all(&root).unwrap();
}

/// 存在性 / 列表 / 删除：都不碰不存在的东西，删除幂等。
#[test]
fn exists_list_and_delete_are_total() {
    let root = temp_root("listing");
    assert!(!save_exists(&root, "slot1"), "没有 Saves/ 目录时也不能 panic");
    assert!(list_saves(&root).is_empty());

    write_save(&root, "slot2", &sample_save()).unwrap();
    write_save(&root, "slot1", &sample_save()).unwrap();
    write_save(&root, "chapter/slot3", &sample_save()).unwrap();
    assert!(save_exists(&root, "slot1"));
    assert!(save_exists(&root, "slot1.sav"), "两种写法指向同一个文件");
    assert_eq!(list_saves(&root), vec!["slot1", "slot2"], "升序，子目录不进列表");

    assert!(delete_save(&root, "slot1").unwrap(), "第一次删掉");
    assert!(!delete_save(&root, "slot1").unwrap(), "第二次就当不存在");
    assert!(!save_exists(&root, "slot1"));
    // 删不存在的槽**不是**错误（读档菜单点了个早已删掉的槽）。
    assert!(!delete_save(&root, "nope").unwrap());

    std::fs::remove_dir_all(&root).unwrap();
}

/// 变量表：摊进存档 / 读档**替换**（不是合并）。
#[test]
fn vars_are_snapshotted_and_replaced() {
    let root = temp_root("vars");
    let mut vars = SaveVars::default();
    let player = EntityId::from_raw(7);
    assert!(vars.set_global("coins", SaveValue::Number(5.0)));
    assert!(vars.set_entity(player, "hp", SaveValue::Number(80.0)));
    assert!(
        !vars.set_global("bad", SaveValue::Number(f64::NAN)),
        "非有限数不进变量表"
    );

    let mut save = sample_save();
    vars.write_into(&mut save);
    assert_eq!(save.vars.get("coins"), Some(&SaveValue::Number(5.0)));
    assert_eq!(save.entity(player).unwrap().vars.get("hp"), Some(&SaveValue::Number(80.0)));
    assert_eq!(save.entity(EntityId::from_raw(3)).unwrap().vars.len(), 0);

    // 读档前先塞一个"存档里没有"的键：读档之后它必须消失。
    vars.set_global("stale", SaveValue::Bool(true));
    vars.set_entity(EntityId::from_raw(3), "stale", SaveValue::Bool(true));
    let loaded = {
        write_save(&root, "slot1", &save).unwrap();
        read_save(&root, "slot1").unwrap()
    };
    vars.read_from(&loaded);
    assert!(vars.global("stale").is_none(), "读档是替换，不是合并");
    assert!(vars.entity(EntityId::from_raw(3), "stale").is_none());
    assert_eq!(vars.global("coins"), Some(&SaveValue::Number(5.0)));
    assert_eq!(vars.entity(player, "hp"), Some(&SaveValue::Number(80.0)));

    // 切场景清实体变量、全局变量保留（与脚本全局变量同款语义）。
    vars.clear_entities();
    assert!(vars.entity(player, "hp").is_none());
    assert_eq!(vars.global("coins"), Some(&SaveValue::Number(5.0)));

    std::fs::remove_dir_all(&root).unwrap();
}

/// 存档是纯文本 RON：能被人读懂、能被 grep。
#[test]
fn save_file_is_human_readable_ron() {
    let root = temp_root("readable");
    write_save(&root, "slot1", &sample_save()).unwrap();
    let text = std::fs::read_to_string(root.join("Saves/slot1.sav")).unwrap();
    assert!(text.contains("format_version: 1"), "{text}");
    assert!(text.contains("Scenes/Level1.ron"), "{text}");
    assert!(text.contains("Number(12.0)"), "{text}");
    // 零速度不写（存档里一堆 `linear_velocity: (0,0,0)` 只是噪音）：
    // 只有实体 7 有非零线速度，所以整份存档里这个字段只出现一次。
    assert_eq!(
        text.matches("linear_velocity").count(),
        1,
        "只有非零速度才写这一行: {text}"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

/// 落盘纪律：目标被占用时**保留旧存档**，且不留下半截文件。
///
/// 与场景保存同一条契约（`fsx::write_atomic`）—— 存档也必须是"要么旧内容、
/// 要么完整新内容"，否则玩家读到的是一份截断的档。
#[cfg(windows)]
#[test]
fn locked_target_keeps_the_previous_save() {
    use std::os::windows::fs::OpenOptionsExt;

    let root = temp_root("locked");
    let first = sample_save();
    write_save(&root, "slot1", &first).expect("首次写档");
    let before = std::fs::read(root.join("Saves/slot1.sav")).unwrap();

    // 允许读写但不允许删除 / 替换（share_mode = 3）。
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(root.join("Saves/slot1.sav"))
        .unwrap();

    let mut second = sample_save();
    second.game_time = 999.0;
    let result = write_save(&root, "slot1", &second);
    let after = std::fs::read(root.join("Saves/slot1.sav")).unwrap();
    drop(lock);

    assert!(result.is_err(), "被占用的目标必须报错，不能静默成功");
    assert_eq!(before, after, "旧存档必须原样保留");
    // 暂存文件不留在盘上（失败路径自己清）。
    let leftovers: Vec<_> = std::fs::read_dir(root.join("Saves"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
        .collect();
    assert!(leftovers.is_empty(), "失败后不该留下暂存文件");

    std::fs::remove_dir_all(&root).unwrap();
}

/// `Saves` 这个名字被一个**文件**占着：报错，不 panic，也不覆盖那个文件。
#[test]
fn saves_path_taken_by_a_file_is_an_error_not_a_panic() {
    let root = temp_root("blocked");
    std::fs::write(root.join(SAVE_DIR), b"not a directory").unwrap();

    let result = write_save(&root, "slot1", &sample_save());
    assert!(result.is_err(), "目录建不出来必须报错");
    assert_eq!(
        std::fs::read(root.join(SAVE_DIR)).unwrap(),
        b"not a directory",
        "那个文件不该被删掉或覆盖"
    );
    // 读 / 列 / 删在这种情况下都只是"没有"，不 panic。
    assert!(!save_exists(&root, "slot1"));
    assert!(list_saves(&root).is_empty());
    assert!(delete_save(&root, "slot1").is_err() || !delete_save(&root, "slot1").unwrap());

    std::fs::remove_dir_all(&root).unwrap();
}
