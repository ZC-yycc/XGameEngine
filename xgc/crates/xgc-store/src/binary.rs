//! 二进制 cooked 世界：`XGCB` 固定小端格式，省掉 JSON 解析。

use std::path::Path;

use xgc_base::EntityId;
use xgc_world::{
    ComponentData, ComponentId, EntityData, LightKind, SceneEnvironment, SceneLight, Value,
    WorldData,
};

use crate::{write_atomic, Result, StoreError};

const MAGIC: &[u8; 4] = b"XGCB";
/// 2：`next_id` 之后追加场景环境（光 / 环境光）。
const VERSION: u32 = 2;

pub fn encode_world(data: &WorldData) -> Vec<u8> {
    let mut writer = Writer::default();
    writer.bytes.extend_from_slice(MAGIC);
    writer.u32(VERSION);
    writer.u64(data.revision);
    writer.u64(data.next_id);
    write_environment(&mut writer, &data.environment);
    writer.u32(data.entities.len() as u32);
    for entity in &data.entities {
        writer.u64(entity.id.raw());
        writer.u64(entity.revision);
        writer.u32(entity.components.len() as u32);
        for component in &entity.components {
            writer.u32(component.component.0);
            writer.u32(component.fields.len() as u32);
            for (name, value) in &component.fields {
                writer.string(name);
                writer.value(value);
            }
        }
    }
    writer.bytes
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
    let entity_count = reader.u32()? as usize;
    let mut entities = Vec::with_capacity(entity_count);
    for _ in 0..entity_count {
        let id = EntityId::from_raw(reader.u64()?);
        let revision = reader.u64()?;
        let component_count = reader.u32()? as usize;
        let mut components = Vec::with_capacity(component_count);
        for _ in 0..component_count {
            let component = ComponentId(reader.u32()?);
            let field_count = reader.u32()? as usize;
            let mut fields = Vec::with_capacity(field_count);
            for _ in 0..field_count {
                let name = reader.string()?;
                fields.push((name, reader.value()?));
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

fn write_environment(writer: &mut Writer, environment: &SceneEnvironment) {
    for value in environment.ambient {
        writer.f64(f64::from(value));
    }
    writer.u32(environment.lights.len() as u32);
    for light in &environment.lights {
        writer.string(&light.name);
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
}

fn read_environment(reader: &mut Reader<'_>) -> std::result::Result<SceneEnvironment, String> {
    let ambient = [
        reader.f64()? as f32,
        reader.f64()? as f32,
        reader.f64()? as f32,
    ];
    let light_count = reader.u32()? as usize;
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
    write_atomic(path, &encode_world(data))
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

    fn string(&mut self, value: &str) {
        self.u32(value.len() as u32);
        self.bytes(value.as_bytes());
    }

    fn value(&mut self, value: &Value) {
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
                self.string(value);
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
        }
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

    fn value(&mut self) -> std::result::Result<Value, String> {
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
            tag => return Err(format!("XGCB 未知值标签：{tag}")),
        })
    }
}
