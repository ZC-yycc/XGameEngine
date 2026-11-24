//! `state_hash` 只认运行时状态：作者配置字段（动画事件）不参与。

use xgc_world::{AnimEvent, Animator, Component, Transform, World};

#[test]
fn animation_events_are_authoring_data_not_state() {
    let mut world = World::new();
    let key = world.spawn((Transform::default(), Animator::default()));
    let entity = world.entity(key).unwrap();
    let before = world.state_hash();

    let animator = world.get_mut::<Animator>(key).unwrap();
    animator.events.push(AnimEvent::new("footstep", 0.5));

    assert_eq!(
        world.state_hash(),
        before,
        "只改作者事件表不能让验收参考哈希漂移"
    );

    let data = world.world_data();
    let animator = data.entities[0]
        .components
        .iter()
        .find(|component| component.component == Animator::ID)
        .expect("实体应携带 Animator");
    assert!(
        animator.fields.iter().any(|(name, _)| name == "events"),
        "非空事件表必须落进 WorldData，否则保存 / 迁移会静默丢掉"
    );

    let mut restored = World::new();
    restored.restore_world_data(&data).unwrap();
    let key = restored.key(entity).expect("实体应恢复");
    assert_eq!(restored.get::<Animator>(key).unwrap().events.len(), 1);
    assert_eq!(restored.state_hash(), before);
}
