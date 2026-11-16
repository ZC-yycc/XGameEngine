//! 事务日志回归：JSON 往返、revision 校验、从初始快照重放。

use xgc_base::EntityId;
use xgc_reflect::{ComponentId, Value};
use xgc_world::{Command, Journal, Transform, Velocity, World};

fn set_position(id: EntityId, x: f32) -> Command {
    Command::SetField {
        entity: id,
        component: ComponentId(0x0101),
        field: "position".to_string(),
        value: Value::Vec3([f64::from(x), 0.0, 0.0]),
    }
}

#[test]
fn journal_json_roundtrip_and_replay() {
    let mut world = World::new();
    let key = world.spawn((Transform::default(), Velocity::default()));
    let id = world.entity(key).unwrap();
    let initial = world.snapshot();
    let mut journal = Journal::new();

    let plan = world.plan(vec![set_position(id, 3.0)]);
    let applied = world.apply_logged(&plan, &mut journal).unwrap();
    assert_eq!(journal.len(), 1);
    assert_eq!(journal.records()[0].tx_id, 0);
    assert_eq!(journal.records()[0].revision, applied.revision);

    let new_id = EntityId::from_raw(1_000);
    let data = world.entity_data(id).unwrap().with_id(new_id);
    let plan = world.plan(vec![Command::Spawn { data }]);
    world.apply_logged(&plan, &mut journal).unwrap();
    assert_eq!(journal.len(), 2);

    let json = journal.to_json().unwrap();
    let decoded = Journal::from_json(&json).unwrap();
    assert_eq!(decoded, journal);

    let mut replay = World::new();
    let hash = decoded.replay(&mut replay, &initial).unwrap();
    assert_eq!(hash, world.state_hash());
    assert_eq!(replay.len(), 2);
    assert!(replay.key(id).is_some());
    assert!(replay.key(new_id).is_some());
    assert_eq!(
        replay.get_field(id, ComponentId(0x0101), "position"),
        Some(Value::Vec3([3.0, 0.0, 0.0]))
    );
}

#[test]
fn replay_rejects_revision_drift() {
    let mut world = World::new();
    let key = world.spawn((Transform::default(),));
    let id = world.entity(key).unwrap();
    let initial = world.snapshot();
    let mut journal = Journal::new();

    let plan = world.plan(vec![set_position(id, 1.0)]);
    world.apply_logged(&plan, &mut journal).unwrap();
    let mut json = serde_json::Value::from_str(&journal.to_json().unwrap()).unwrap();
    json["records"][0]["base_revision"] = serde_json::Value::from(999);
    let tampered = Journal::from_json(&json.to_string()).unwrap();

    let mut replay = World::new();
    let err = tampered.replay(&mut replay, &initial).unwrap_err();
    assert!(err.contains("base_revision"));
}

use std::str::FromStr;
