//! 脚本字段读写与反射值之间的转换。

use mlua::{Lua, Value as LuaValue};
use xgc_world::{FieldSchema, Value};

pub(super) fn value_to_lua(lua: &Lua, value: &Value) -> mlua::Result<mlua::Value> {
    Ok(match value {
        Value::Bool(value) => mlua::Value::Boolean(*value),
        Value::I64(value) => mlua::Value::Integer(*value),
        Value::F64(value) => mlua::Value::Number(*value),
        Value::String(value) => mlua::Value::String(lua.create_string(value)?),
        Value::Vec2(value) => vec_table(lua, value)?,
        Value::Vec3(value) => vec_table(lua, value)?,
        Value::Vec4(value) | Value::Quat(value) => vec_table(lua, value)?,
        Value::Entity(value) => mlua::Value::Integer(*value as i64),
        Value::List(items) => {
            let table = lua.create_table_with_capacity(items.len(), 0)?;
            for (index, item) in items.iter().enumerate() {
                table.raw_set(index + 1, value_to_lua(lua, item)?)?;
            }
            mlua::Value::Table(table)
        }
        Value::Struct(fields) | Value::Map(fields) => {
            let table = lua.create_table_with_capacity(0, fields.len())?;
            for (name, value) in fields {
                table.raw_set(name.as_str(), value_to_lua(lua, value)?)?;
            }
            mlua::Value::Table(table)
        }
        Value::Enum { variant, payload } => {
            let table = lua.create_table_with_capacity(0, 2)?;
            table.raw_set("variant", variant.as_str())?;
            table.raw_set("payload", value_to_lua(lua, payload)?)?;
            mlua::Value::Table(table)
        }
    })
}

fn vec_table(lua: &Lua, values: &[f64]) -> mlua::Result<mlua::Value> {
    let table = lua.create_table()?;
    for (name, value) in ["x", "y", "z", "w"].into_iter().zip(values) {
        table.set(name, *value)?;
    }
    Ok(mlua::Value::Table(table))
}

pub(super) fn lua_to_value(value: &mlua::Value) -> mlua::Result<Value> {
    match value {
        mlua::Value::Boolean(value) => Ok(Value::Bool(*value)),
        mlua::Value::Integer(value) => Ok(Value::I64(*value)),
        mlua::Value::Number(value) => Ok(Value::F64(*value)),
        mlua::Value::String(value) => Ok(Value::String(value.to_str()?.to_string())),
        mlua::Value::Table(table) => {
            let x = table.get::<Option<f64>>("x")?;
            let y = table.get::<Option<f64>>("y")?;
            let z = table.get::<Option<f64>>("z")?;
            let w = table.get::<Option<f64>>("w")?;
            match (x, y, z, w) {
                (Some(x), Some(y), Some(z), Some(w)) => Ok(Value::Vec4([x, y, z, w])),
                (Some(x), Some(y), Some(z), None) => Ok(Value::Vec3([x, y, z])),
                (Some(x), Some(y), None, None) => Ok(Value::Vec2([x, y])),
                _ => Err(mlua::Error::RuntimeError(
                    "向量表需要 x/y[/z[/w]] 字段".to_string(),
                )),
            }
        }
        other => Err(mlua::Error::RuntimeError(format!(
            "不支持的 Lua 值：{}",
            other.type_name()
        ))),
    }
}

/// 批字段跟随注册表解释容器、四元数、实体 ID 和 Lua 整数相对于浮点字段的含义。
pub(super) fn lua_to_field_value(value: LuaValue, schema: &FieldSchema) -> mlua::Result<Value> {
    let mismatch = || mlua::Error::RuntimeError(format!("字段 {} 需要 {}", schema.name, schema.ty));
    let converted = match (schema.ty.as_str(), value) {
        ("f32" | "f64", LuaValue::Integer(value)) => Value::F64(value as f64),
        ("entity", LuaValue::Integer(value)) => Value::Entity(value as u64),
        ("quat", value) => match lua_to_value(&value)? {
            Value::Vec4(value) => Value::Quat(value),
            _ => return Err(mismatch()),
        },
        ("list", LuaValue::Table(table)) => {
            let element = schema.element.as_deref().ok_or_else(mismatch)?;
            let length = table.raw_len();
            let mut values = Vec::new();
            for index in 1..=length {
                values.push(lua_to_field_value(table.raw_get(index)?, element)?);
            }
            for pair in table.pairs::<LuaValue, LuaValue>() {
                if !matches!(pair?.0, LuaValue::Integer(index) if (1..=length).contains(&(index as usize)))
                {
                    return Err(mismatch());
                }
            }
            Value::List(values)
        }
        ("struct", LuaValue::Table(table)) => {
            let mut fields = std::collections::BTreeMap::new();
            for pair in table.pairs::<String, LuaValue>() {
                let (name, value) = pair?;
                let field = schema
                    .fields
                    .iter()
                    .find(|field| field.name == name)
                    .ok_or_else(mismatch)?;
                fields.insert(name, lua_to_field_value(value, field)?);
            }
            if fields.len() != schema.fields.len() {
                return Err(mismatch());
            }
            Value::Struct(fields)
        }
        ("map", LuaValue::Table(table)) => {
            let element = schema.element.as_deref().ok_or_else(mismatch)?;
            let mut fields = std::collections::BTreeMap::new();
            for pair in table.pairs::<String, LuaValue>() {
                let (key, value) = pair?;
                fields.insert(key, lua_to_field_value(value, element)?);
            }
            Value::Map(fields)
        }
        ("enum", LuaValue::Table(table)) => {
            let variant = table.raw_get::<String>("variant")?;
            let branch = schema.variant(&variant).ok_or_else(mismatch)?;
            let payload = lua_to_field_value(table.raw_get("payload")?, branch)?;
            Value::Enum {
                variant,
                payload: Box::new(payload),
            }
        }
        (_, value) => lua_to_value(&value)?,
    };
    if !schema.fits(&converted) {
        return Err(mismatch());
    }
    Ok(converted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_and_enum_are_plain_lua_tables() -> mlua::Result<()> {
        let lua = Lua::new();
        let value = Value::Map(
            [(
                "variant".into(),
                Value::Enum {
                    variant: "walk".into(),
                    payload: Box::new(Value::List(vec![Value::Bool(true)])),
                },
            )]
            .into(),
        );
        lua.globals().set("value", value_to_lua(&lua, &value)?)?;
        lua.load(
            r#"
            assert(getmetatable(value) == nil)
            assert(getmetatable(value.variant) == nil)
            assert(value.variant.variant == 'walk')
            assert(value.variant.payload[1] == true)
            "#,
        )
        .exec()
    }
}
