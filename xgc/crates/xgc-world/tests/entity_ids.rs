//! 持久实体 ID 边界：装载、事务、实体快照与分配共用上限。

use std::panic::{catch_unwind, AssertUnwindSafe};

use xgc_base::EntityId;
use xgc_world::{builtin_registry, Command, EntityData, Journal, Name, Plan, World, WorldData};

fn world_with_name() -> World {
    let mut world = World::new();
    world.spawn((Name {
        value: "原有实体".into(),
    },));
    world
}

#[test]
fn reserved_id_in_world_data_leaves_live_entities_untouched() {
    let mut world = world_with_name();
    let before = world.world_data();
    let original = before.entities[0].id;
    let key = world.key(original).unwrap();
    let hash = world.state_hash();
    let invalid = EntityId::from_raw(u64::MAX);
    let incoming = WorldData {
        revision: 100,
        next_id: u64::MAX,
        entities: vec![
            EntityData::new(EntityId::from_raw(10)),
            EntityData::new(invalid),
        ],
        ..Default::default()
    };

    let error = world.restore_world_data(&incoming).unwrap_err();
    assert!(error.contains(&invalid.to_string()), "{error}");
    assert_eq!(world.world_data(), before);
    assert_eq!(world.key(original), Some(key));
    assert_eq!(world.state_hash(), hash);
    let diagnostics = World::validate_data(&incoming, &builtin_registry());
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].message, error);

    let invalid = world.capture_entity(original).unwrap().with_id(invalid);
    assert_eq!(world.restore_entity(invalid).unwrap_err(), error);
    assert_eq!(world.world_data(), before);
}

#[test]
fn duplicate_incoming_id_leaves_current_world_untouched() {
    let mut world = world_with_name();
    let before = world.world_data();
    let entity = EntityData::new(EntityId::from_raw(10));
    let incoming = WorldData {
        entities: vec![entity.clone(), entity],
        ..Default::default()
    };

    assert!(world.restore_world_data(&incoming).is_err());
    assert_eq!(world.world_data(), before);
}

#[test]
fn transaction_rejects_reserved_id_in_spawn_and_replace() {
    let mut world = world_with_name();
    let before = world.world_data();
    let original = before.entities[0].id;
    let key = world.key(original).unwrap();
    let data = EntityData::new(EntityId::from_raw(u64::MAX));
    assert!(world.snapshot_from_data(&data).is_err());
    let mut journal = Journal::new();
    for command in [
        Command::Spawn { data: data.clone() },
        Command::Replace { data },
    ] {
        let commands = vec![
            Command::Replace {
                data: EntityData::new(original),
            },
            Command::Spawn {
                data: EntityData::new(EntityId::from_raw(10)),
            },
            command,
        ];
        let plan = world.plan(commands.clone());
        assert_eq!(plan.diagnostics.len(), 1);
        assert!(plan.diagnostics[0].message.contains(&u64::MAX.to_string()));
        assert!(world.apply_logged(&plan, &mut journal).is_err());

        // 程序可直接构造 Plan。回滚 Replace 只能还原数据，所以必须提前拒绝坏 ID。
        let forged = Plan {
            base_revision: world.revision(),
            commands,
            diagnostics: Vec::new(),
        };
        let errors = world.apply_logged(&forged, &mut journal).unwrap_err();
        assert_eq!(world.world_data(), before);
        assert_eq!(world.key(original), Some(key));
        assert!(journal.is_empty());
        assert_eq!(errors.as_slice(), plan.diagnostics.as_slice());
    }
}

#[test]
fn final_entity_id_is_usable_and_allocation_never_wraps() {
    let mut world = world_with_name();
    let mut near_exhaustion = world.world_data();
    near_exhaustion.next_id = u64::MAX - 1;
    world.restore_world_data(&near_exhaustion).unwrap();

    let too_many = catch_unwind(AssertUnwindSafe(|| world.reserve_entity_ids(2)));
    assert!(too_many.is_err(), "超出剩余 ID 的预留应在改动前失败");
    assert_eq!(world.world_data(), near_exhaustion);

    let key = world.spawn((Name::default(),));
    let final_id = world.entity(key).unwrap();
    assert_eq!(final_id.raw(), u64::MAX - 1);
    assert_eq!(world.world_data().next_id, u64::MAX);
    let before = world.world_data();
    assert!(catch_unwind(AssertUnwindSafe(|| world.spawn((Name::default(),)))).is_err());
    assert!(catch_unwind(AssertUnwindSafe(|| world.reserve_entity_ids(1))).is_err());
    assert_eq!(world.world_data(), before);
    assert_eq!(world.key(final_id), Some(key));

    // 游标耗尽仍可编辑现存实体，撤销时也能恢复原有身份。
    let replacement = EntityData::new(final_id);
    let plan = world.plan(vec![Command::Replace { data: replacement }]);
    let applied = world.apply(&plan).unwrap();
    world.revert(&applied).unwrap();
    assert_eq!(world.world_data().next_id, u64::MAX);
    assert!(world.get::<Name>(world.key(final_id).unwrap()).is_some());
}
