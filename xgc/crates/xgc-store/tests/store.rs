//! 持久化回归：WorldData / Journal 落盘往返、版本校验、无改动不重写。

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use xgc_base::EntityId;
use xgc_store::{load_journal, load_world, save_journal, save_world};
use xgc_world::{Command, ComponentId, Journal, Name, Transform, Value, World};

fn temp_dir(tag: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("xgc_store_{tag}_{}_{stamp}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn demo_world() -> World {
    let mut world = World::new();
    for i in 0..10 {
        world.spawn((
            Transform {
                position: [i as f32, 0.0, 0.0],
                ..Transform::default()
            },
            Name {
                value: format!("entity_{i}"),
            },
        ));
    }
    world
}

#[test]
fn world_data_roundtrip_and_no_rewrite() {
    let dir = temp_dir("world");
    let path = dir.join("world.json");
    let world = demo_world();
    let data = world.world_data();

    save_world(&path, &data).unwrap();
    let first = fs::read(&path).unwrap();
    save_world(&path, &data).unwrap();
    let second = fs::read(&path).unwrap();
    assert_eq!(first, second);

    let loaded = load_world(&path).unwrap();
    assert_eq!(loaded, data);
    let mut restored = World::new();
    restored.restore_world_data(&loaded).unwrap();
    assert_eq!(restored.state_hash(), world.state_hash());

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn journal_roundtrip_and_replay() {
    let dir = temp_dir("journal");
    let path = dir.join("journal.json");
    let mut world = demo_world();
    let initial = world.snapshot();
    let id = EntityId::from_raw(0);
    let mut journal = Journal::new();

    let plan = world.plan(vec![Command::SetField {
        entity: id,
        component: ComponentId(0x0101),
        field: "position".to_string(),
        value: Value::Vec3([9.0, 0.0, 0.0]),
    }]);
    world.apply_logged(&plan, &mut journal).unwrap();

    save_journal(&path, &journal).unwrap();
    let loaded = load_journal(&path).unwrap();
    assert_eq!(loaded, journal);

    let mut replay = World::new();
    let hash = loaded.replay(&mut replay, &initial).unwrap();
    assert_eq!(hash, world.state_hash());

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unsupported_version_is_rejected() {
    let dir = temp_dir("version");
    let path = dir.join("bad.json");
    fs::write(
        &path,
        r#"{"format_version":999,"kind":"world","payload":{"revision":0,"next_id":0,"entities":[]}}"#,
    )
    .unwrap();
    let err = load_world(&path).unwrap_err();
    assert!(err.to_string().contains("格式版本"));
    fs::remove_dir_all(dir).unwrap();
}
