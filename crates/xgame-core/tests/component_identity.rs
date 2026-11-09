use xgame_core::component_id::{ComponentId, ComponentIdSequence, ComponentIdentity};

#[test]
fn persisted_high_watermark_survives_deletion_and_reorder() {
    let mut seq = ComponentIdSequence::default();
    let a = seq.allocate().unwrap();
    let b = seq.allocate().unwrap();
    let c = seq.allocate().unwrap();
    seq.validate([c, a, b]).unwrap();

    let text = ron::to_string(&seq).unwrap();
    let mut restored: ComponentIdSequence = ron::from_str(&text).unwrap();
    let d = restored.allocate().unwrap();
    assert_eq!(d.raw(), 4);
    restored.validate([c, a, d]).unwrap();
    assert!(restored.validate([c, c]).is_err());
    assert!(restored.validate([ComponentId::from_raw(5).unwrap()]).is_err());
    assert!(ComponentIdSequence::from_next(0).is_err());
    assert!(ron::from_str::<ComponentIdSequence>("(next_id: 0)").is_err());
    let mut exhausted = ComponentIdSequence::from_next(u64::MAX).unwrap();
    assert!(exhausted.allocate().is_err());
    assert_eq!(exhausted.next_id(), u64::MAX);
}

#[test]
fn prefab_source_is_separate_from_local_component_id() {
    let identity = ComponentIdentity {
        id: ComponentId::from_raw(2).unwrap(),
        source_component_id: Some(ComponentId::from_raw(17).unwrap()),
    };
    let text = ron::to_string(&identity).unwrap();
    assert_eq!(ron::from_str::<ComponentIdentity>(&text).unwrap(), identity);
    assert_eq!(identity.id.raw(), 2);
    assert_eq!(identity.source_component_id.unwrap().raw(), 17);
    for raw in [0, u64::MAX] {
        assert!(ron::from_str::<ComponentId>(&raw.to_string()).is_err());
    }
}
