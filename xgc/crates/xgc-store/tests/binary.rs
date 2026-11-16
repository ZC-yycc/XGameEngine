//! XGCB 二进制世界：往返、确定性、坏数据拒绝。

use xgc_world::{Name, Transform, Velocity, World};

#[test]
fn binary_world_roundtrips_and_is_deterministic() {
    let mut world = World::new();
    world.spawn((
        Transform {
            position: [1.5, -2.0, 3.25],
            ..Default::default()
        },
        Velocity {
            linear: [0.5, 0.0, -1.0],
        },
        Name {
            value: "hero".to_string(),
        },
    ));
    let data = world.world_data();
    let first = xgc_store::encode_world(&data);
    let second = xgc_store::encode_world(&data);
    assert_eq!(first, second);
    assert_eq!(&first[..4], b"XGCB");
    let decoded = xgc_store::decode_world(&first).unwrap();
    assert_eq!(decoded, data);

    let dir = std::env::temp_dir().join(format!(
        "xgc_binary_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("world.xgcb");
    xgc_store::save_world_binary(&path, &data).unwrap();
    let loaded = xgc_store::load_world_binary(&path).unwrap();
    assert_eq!(loaded, data);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn binary_world_rejects_corruption() {
    let mut world = World::new();
    world.spawn((Transform::default(),));
    let data = world.world_data();
    let mut bytes = xgc_store::encode_world(&data);
    bytes[0] = b'Z';
    assert!(xgc_store::decode_world(&bytes).is_err());
    let good = xgc_store::encode_world(&data);
    assert!(xgc_store::decode_world(&good[..good.len() - 1]).is_err());
}
