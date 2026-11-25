//! XGCB 二进制世界：往返、确定性、坏数据拒绝。

use xgc_world::{Name, Transform, Value, Velocity, World, WorldData};

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
    let first = xgc_store::encode_world(&data).unwrap();
    let second = xgc_store::encode_world(&data).unwrap();
    assert_eq!(first, second);
    assert_eq!(&first[..8], b"XGCB\x04\0\0\0");
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
fn binary_world_rejects_previous_version() {
    let mut bytes = xgc_store::encode_world(&World::new().world_data()).unwrap();
    bytes[4..8].copy_from_slice(&3_u32.to_le_bytes());
    let err = xgc_store::decode_world(&bytes).unwrap_err();
    assert!(err.contains("不支持的 XGCB 版本：3"), "{err}");
}

#[test]
fn binary_world_rejects_corruption() {
    let mut world = World::new();
    world.spawn((Transform::default(),));
    let data = world.world_data();
    let mut bytes = xgc_store::encode_world(&data).unwrap();
    bytes[0] = b'Z';
    assert!(xgc_store::decode_world(&bytes).is_err());
    let good = xgc_store::encode_world(&data).unwrap();
    assert!(xgc_store::decode_world(&good[..good.len() - 1]).is_err());
}

fn world_data_with_value(value: Value) -> WorldData {
    let mut world = World::new();
    world.spawn((Name::default(),));
    let mut data = world.world_data();
    data.entities[0].components[0].fields[0].1 = value;
    data
}

fn cooked_with_raw_value(value: &[u8]) -> Vec<u8> {
    let data = world_data_with_value(Value::Bool(false));
    let mut bytes = xgc_store::encode_world(&data).unwrap();
    // 最后两个字节是测试字段的布尔标签和值。
    assert_eq!(&bytes[bytes.len() - 2..], &[0, 0]);
    bytes.truncate(bytes.len() - 2);
    bytes.extend_from_slice(value);
    bytes
}

#[test]
fn binary_world_roundtrips_nested_values() {
    let data = world_data_with_value(Value::List(vec![
        Value::Struct(
            [
                ("z".into(), Value::List(vec![Value::I64(-42)])),
                ("a".into(), Value::String("脚步".into())),
            ]
            .into(),
        ),
        Value::List(vec![]),
        Value::Struct(Default::default()),
    ]));
    let bytes = xgc_store::encode_world(&data).unwrap();
    let decoded = xgc_store::decode_world(&bytes).unwrap();
    assert_eq!(decoded, data);
    assert_eq!(xgc_store::encode_world(&decoded).unwrap(), bytes);
}

#[test]
fn binary_world_rejects_invalid_nested_lengths_and_duplicate_keys() {
    for tag in [9, 10] {
        let mut value = vec![tag];
        value.extend_from_slice(&u32::MAX.to_le_bytes());
        let err = xgc_store::decode_world(&cooked_with_raw_value(&value)).unwrap_err();
        assert!(err.contains("条目数量超过"), "{err}");
    }

    let mut value = vec![10];
    value.extend_from_slice(&2_u32.to_le_bytes());
    for boolean in [0, 1] {
        value.extend_from_slice(&1_u32.to_le_bytes());
        value.extend_from_slice(&[b'x', 0, boolean]);
    }
    let err = xgc_store::decode_world(&cooked_with_raw_value(&value)).unwrap_err();
    assert!(err.contains("结构字段重复：x"), "{err}");
}

#[test]
fn binary_world_bounds_nested_values_on_read_and_write() {
    let mut value = Value::Bool(false);
    let mut raw = Vec::new();
    for _ in 0..63 {
        value = Value::List(vec![value]);
        raw.extend_from_slice(&[9, 1, 0, 0, 0]);
    }
    let data = world_data_with_value(value.clone());
    let bytes = xgc_store::encode_world(&data).unwrap();
    assert_eq!(xgc_store::decode_world(&bytes).unwrap(), data);

    let data = world_data_with_value(Value::List(vec![value]));
    let err = xgc_store::encode_world(&data).unwrap_err();
    assert!(err.contains("嵌套超过 64 层"), "{err}");

    raw.extend_from_slice(&[9, 1, 0, 0, 0, 0, 0]);
    let err = xgc_store::decode_world(&cooked_with_raw_value(&raw)).unwrap_err();
    assert!(err.contains("嵌套超过 64 层"), "{err}");
}
