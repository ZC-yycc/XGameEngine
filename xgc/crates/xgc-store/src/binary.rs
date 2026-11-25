//! 二进制 cooked 世界：`XGCB` 固定小端格式，省掉 JSON 解析。

use std::collections::{btree_map::Entry, BTreeMap};
use std::path::Path;

use xgc_base::EntityId;
use xgc_world::{
    ComponentData, ComponentId, EntityData, LightKind, SceneEnvironment, SceneLight, Value,
    WorldData,
};

use crate::{write_atomic, Result, StoreError};

const MAGIC: &[u8; 4] = b"XGCB";
/// 4：LOD 层级改为结构化列表。
const VERSION: u32 = 4;
// 读写共用限制，防止深层反射值耗尽线程栈。
const MAX_VALUE_DEPTH: usize = 64;

pub fn encode_world(data: &WorldData) -> std::result::Result<Vec<u8>, String> {
    let mut writer = Writer::default();
    writer.bytes.extend_from_slice(MAGIC);
    writer.u32(VERSION);
    writer.u64(data.revision);
    writer.u64(data.next_id);
    write_environment(&mut writer, &data.environment)?;
    writer.length(data.entities.len())?;
    for entity in &data.entities {
        writer.u64(entity.id.raw());
        writer.u64(entity.revision);
        writer.length(entity.components.len())?;
        for component in &entity.components {
            writer.u32(component.component.0);
            writer.length(component.fields.len())?;
            for (name, value) in &component.fields {
                writer.string(name)?;
                writer.value(value, MAX_VALUE_DEPTH)?;
            }
        }
    }
    Ok(writer.bytes)
}

pub fn decode_world(bytes: &[u8]) -> std::result::Result<WorldData, String> {
    let mut reader = Reader { bytes, cursor: 0 };
    if reader.take(4)? != MAGIC {
        return Err("magic 不是 XGCB".to_string());
    }
    let version = reader.u32()?;
    if version != VERSION {
        return Err(format!("不支持的 XGCB 版本：{version}"));
    }
    let revision = reader.u64()?;
    let next_id = reader.u64()?;
    let environment = read_environment(&mut reader)?;
    let entity_count = reader.count(8 + 8 + 4)?;
    let mut entities = Vec::with_capacity(entity_count);
    for _ in 0..entity_count {
        let id = EntityId::from_raw(reader.u64()?);
        let revision = reader.u64()?;
        let component_count = reader.count(4 + 4)?;
        let mut components = Vec::with_capacity(component_count);
        for _ in 0..component_count {
            let component = ComponentId(reader.u32()?);
            let field_count = reader.count(4 + 2)?;
            let mut fields = Vec::with_capacity(field_count);
            for _ in 0..field_count {
                let name = reader.string()?;
                fields.push((name, reader.value(MAX_VALUE_DEPTH)?));
            }
            components.push(ComponentData { component, fields });
        }
        entities.push(EntityData {
            id,
            revision,
            components,
        });
    }
    if reader.cursor != bytes.len() {
        return Err(format!(
            "XGCB 尾部有 {} 字节多余数据",
            bytes.len() - reader.cursor
        ));
    }
    Ok(WorldData {
        revision,
        next_id,
        entities,
        environment,
    })
}

fn write_environment(
    writer: &mut Writer,
    environment: &SceneEnvironment,
) -> std::result::Result<(), String> {
    for value in environment.ambient {
        writer.f64(f64::from(value));
    }
    writer.length(environment.lights.len())?;
    for light in &environment.lights {
        writer.string(&light.name)?;
        writer.u8(match light.kind {
            LightKind::Directional => 0,
            LightKind::Point => 1,
        });
        for value in light.direction {
            writer.f64(f64::from(value));
        }
        for value in light.position {
            writer.f64(f64::from(value));
        }
        for value in light.color {
            writer.f64(f64::from(value));
        }
        writer.f64(f64::from(light.intensity));
        writer.f64(f64::from(light.range));
        writer.u8(u8::from(light.casts_shadow));
        writer.u32(light.shadow_resolution);
    }
    Ok(())
}

fn read_environment(reader: &mut Reader<'_>) -> std::result::Result<SceneEnvironment, String> {
    let ambient = [
        reader.f64()? as f32,
        reader.f64()? as f32,
        reader.f64()? as f32,
    ];
    let light_count = reader.count(4 + 1 + 11 * 8 + 1 + 4)?;
    let mut lights = Vec::with_capacity(light_count);
    for _ in 0..light_count {
        let name = reader.string()?;
        let kind = match reader.u8()? {
            0 => LightKind::Directional,
            1 => LightKind::Point,
            other => return Err(format!("XGCB 未知光源类型：{other}")),
        };
        let direction = [
            reader.f64()? as f32,
            reader.f64()? as f32,
            reader.f64()? as f32,
        ];
        let position = [
            reader.f64()? as f32,
            reader.f64()? as f32,
            reader.f64()? as f32,
        ];
        let color = [
            reader.f64()? as f32,
            reader.f64()? as f32,
            reader.f64()? as f32,
        ];
        let intensity = reader.f64()? as f32;
        let range = reader.f64()? as f32;
        let casts_shadow = reader.u8()? != 0;
        let shadow_resolution = reader.u32()?;
        lights.push(SceneLight {
            name,
            kind,
            direction,
            position,
            color,
            intensity,
            range,
            casts_shadow,
            shadow_resolution,
        });
    }
    Ok(SceneEnvironment { lights, ambient })
}

pub fn save_world_binary(path: &Path, data: &WorldData) -> Result<()> {
    let bytes = encode_world(data).map_err(StoreError::Format)?;
    write_atomic(path, &bytes)
}

pub fn load_world_binary(path: &Path) -> Result<WorldData> {
    let bytes = std::fs::read(path)?;
    decode_world(&bytes).map_err(StoreError::Format)
}

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn bytes(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(value);
    }

    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    fn u32(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn f64(&mut self, value: f64) {
        self.u64(value.to_bits());
    }

    fn length(&mut self, length: usize) -> std::result::Result<(), String> {
        self.u32(u32::try_from(length).map_err(|_| "XGCB 长度溢出".to_string())?);
        Ok(())
    }

    fn string(&mut self, value: &str) -> std::result::Result<(), String> {
        self.length(value.len())?;
        self.bytes(value.as_bytes());
        Ok(())
    }

    fn value(&mut self, value: &Value, levels: usize) -> std::result::Result<(), String> {
        if levels == 0 {
            return Err(format!("XGCB 值嵌套超过 {MAX_VALUE_DEPTH} 层"));
        }
        match value {
            Value::Bool(value) => {
                self.u8(0);
                self.u8(u8::from(*value));
            }
            Value::I64(value) => {
                self.u8(1);
                self.u64(*value as u64);
            }
            Value::F64(value) => {
                self.u8(2);
                self.f64(*value);
            }
            Value::String(value) => {
                self.u8(3);
                self.string(value)?;
            }
            Value::Vec2(value) => {
                self.u8(4);
                self.f64(value[0]);
                self.f64(value[1]);
            }
            Value::Vec3(value) => {
                self.u8(5);
                for value in value {
                    self.f64(*value);
                }
            }
            Value::Vec4(value) => {
                self.u8(6);
                for value in value {
                    self.f64(*value);
                }
            }
            Value::Quat(value) => {
                self.u8(7);
                for value in value {
                    self.f64(*value);
                }
            }
            Value::Entity(value) => {
                self.u8(8);
                self.u64(*value);
            }
            Value::List(values) => {
                self.u8(9);
                self.length(values.len())?;
                for value in values {
                    self.value(value, levels - 1)?;
                }
            }
            Value::Struct(fields) => {
                self.u8(10);
                self.length(fields.len())?;
                for (name, value) in fields {
                    self.string(name)?;
                    self.value(value, levels - 1)?;
                }
            }
        }
        Ok(())
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> std::result::Result<&'a [u8], String> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or_else(|| "XGCB 长度溢出".to_string())?;
        if end > self.bytes.len() {
            return Err("XGCB 数据被截断".to_string());
        }
        let slice = &self.bytes[self.cursor..end];
        self.cursor = end;
        Ok(slice)
    }

    fn u8(&mut self) -> std::result::Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> std::result::Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().expect("4 字节")))
    }

    fn u64(&mut self) -> std::result::Result<u64, String> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().expect("8 字节")))
    }

    fn f64(&mut self) -> std::result::Result<f64, String> {
        Ok(f64::from_bits(self.u64()?))
    }

    fn string(&mut self) -> std::result::Result<String, String> {
        let length = self.u32()? as usize;
        let bytes = self.take(length)?;
        String::from_utf8(bytes.to_vec()).map_err(|err| format!("XGCB 字符串不是 UTF-8：{err}"))
    }

    fn count(&mut self, min_entry_size: usize) -> std::result::Result<usize, String> {
        let count = self.u32()? as usize;
        // 在分配前确认剩余数据至少能容纳声明的条目数。
        if count > (self.bytes.len() - self.cursor) / min_entry_size {
            return Err("XGCB 条目数量超过剩余数据长度".to_string());
        }
        Ok(count)
    }

    fn value(&mut self, levels: usize) -> std::result::Result<Value, String> {
        if levels == 0 {
            return Err(format!("XGCB 值嵌套超过 {MAX_VALUE_DEPTH} 层"));
        }
        Ok(match self.u8()? {
            0 => Value::Bool(self.u8()? != 0),
            1 => Value::I64(self.u64()? as i64),
            2 => Value::F64(self.f64()?),
            3 => Value::String(self.string()?),
            4 => Value::Vec2([self.f64()?, self.f64()?]),
            5 => Value::Vec3([self.f64()?, self.f64()?, self.f64()?]),
            6 => Value::Vec4([self.f64()?, self.f64()?, self.f64()?, self.f64()?]),
            7 => Value::Quat([self.f64()?, self.f64()?, self.f64()?, self.f64()?]),
            8 => Value::Entity(self.u64()?),
            9 => {
                let count = self.count(2)?;
                let mut values = Vec::new();
                for _ in 0..count {
                    values.push(self.value(levels - 1)?);
                }
                Value::List(values)
            }
            10 => {
                let count = self.count(4 + 2)?;
                let mut fields = BTreeMap::new();
                for _ in 0..count {
                    let name = self.string()?;
                    match fields.entry(name) {
                        Entry::Vacant(entry) => {
                            entry.insert(self.value(levels - 1)?);
                        }
                        Entry::Occupied(entry) => {
                            return Err(format!("XGCB 结构字段重复：{}", entry.key()));
                        }
                    }
                }
                Value::Struct(fields)
            }
            tag => return Err(format!("XGCB 未知值标签：{tag}")),
        })
    }
}
