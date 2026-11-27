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
    assert_eq!(&first[..8], b"XGCB\x07\0\0\0");
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
fn binary_world_distinguishes_the_final_id_from_the_exhausted_cursor() {
    use xgc_base::EntityId;
    use xgc_world::EntityData;

    let data = WorldData {
        next_id: u64::MAX,
        entities: vec![EntityData::new(EntityId::from_raw(u64::MAX - 1))],
        ..Default::default()
    };
    let mut bytes = xgc_store::encode_world(&data).unwrap();
    assert_eq!(xgc_store::decode_world(&bytes).unwrap(), data);

    // 最后一行只有 8 字节 ID、8 字节 revision 和 4 字节组件数。
    let id_start = bytes.len() - 20;
    assert_eq!(
        &bytes[id_start..id_start + 8],
        &(u64::MAX - 1).to_le_bytes()
    );
    bytes[id_start..id_start + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    let error = xgc_store::decode_world(&bytes).unwrap_err();
    assert!(error.contains(&u64::MAX.to_string()), "{error}");

    let mut invalid = data;
    invalid.entities[0].id = EntityId::from_raw(u64::MAX);
    assert_eq!(xgc_store::encode_world(&invalid).unwrap_err(), error);
}

#[test]
fn binary_world_rejects_previous_version() {
    let mut bytes = xgc_store::encode_world(&World::new().world_data()).unwrap();
    bytes[4..8].copy_from_slice(&6_u32.to_le_bytes());
    let err = xgc_store::decode_world(&bytes).unwrap_err();
    assert!(err.contains("不支持的 XGCB 版本：6"), "{err}");
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
fn binary_world_roundtrips_mixed_containers() {
    let bytes = cooked_with_raw_value(&[
        9, 2, 0, 0, 0, // 包含结构体和映射的列表
        10, 0, 0, 0, 0, // 空结构体
        11, 2, 0, 0, 0, // 映射按键排序
        1, 0, 0, 0, b'a', 12, 1, 0, 0, 0, b'v', // a 是带列表载荷的枚举
        9, 1, 0, 0, 0, 0, 1, // 枚举载荷是 [true]
        1, 0, 0, 0, b'z', 11, 0, 0, 0, 0, // z 是空映射
    ]);
    let decoded = xgc_store::decode_world(&bytes).unwrap();
    let expected = serde_json::json!({"kind": "list", "value": [
        {"kind": "struct", "value": {}},
        {"kind": "map", "value": {
            "a": {"kind": "enum", "value": {"variant": "v", "payload": {
                "kind": "list", "value": [{"kind": "bool", "value": true}]
            }}},
            "z": {"kind": "map", "value": {}}
        }}
    ]});
    let data = world_data_with_value(serde_json::from_value(expected.clone()).unwrap());
    assert_eq!(decoded, data);
    assert_eq!(xgc_store::encode_world(&data).unwrap(), bytes);
    assert_eq!(
        serde_json::to_value(&decoded.entities[0].components[0].fields[0].1).unwrap(),
        expected
    );
}

#[test]
fn binary_world_rejects_invalid_nested_lengths_and_duplicate_keys() {
    for tag in [9, 10, 11] {
        let mut value = vec![tag];
        value.extend_from_slice(&u32::MAX.to_le_bytes());
        let err = xgc_store::decode_world(&cooked_with_raw_value(&value)).unwrap_err();
        assert!(err.contains("条目数量超过"), "{err}");
    }

    for (tag, diagnostic) in [(10, "结构字段重复：x"), (11, "映射键重复：x")] {
        let mut value = vec![tag];
        value.extend_from_slice(&2_u32.to_le_bytes());
        for boolean in [0, 1] {
            value.extend_from_slice(&1_u32.to_le_bytes());
            value.extend_from_slice(&[b'x', 0, boolean]);
        }
        let err = xgc_store::decode_world(&cooked_with_raw_value(&value)).unwrap_err();
        assert!(err.contains(diagnostic), "{err}");
    }
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

#[test]
fn binary_world_counts_map_entries_and_enum_payloads_toward_depth_limit() {
    let mut raw = Vec::new();
    for _ in 0..31 {
        raw.extend_from_slice(&[11, 1, 0, 0, 0, 1, 0, 0, 0, b'k']);
        raw.extend_from_slice(&[12, 1, 0, 0, 0, b'v']);
    }
    raw.extend_from_slice(&[12, 1, 0, 0, 0, b'v', 0, 0]);
    let bytes = cooked_with_raw_value(&raw);
    let mut decoded = xgc_store::decode_world(&bytes).unwrap();
    assert_eq!(xgc_store::encode_world(&decoded).unwrap(), bytes);

    let value = &mut decoded.entities[0].components[0].fields[0].1;
    *value = Value::List(vec![std::mem::replace(value, Value::Bool(false))]);
    let err = xgc_store::encode_world(&decoded).unwrap_err();
    assert!(err.contains("嵌套超过 64 层"), "{err}");

    let mut too_deep = vec![9, 1, 0, 0, 0];
    too_deep.extend(raw);
    let err = xgc_store::decode_world(&cooked_with_raw_value(&too_deep)).unwrap_err();
    assert!(err.contains("嵌套超过 64 层"), "{err}");
}
