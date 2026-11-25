//! 脚本字段读写与反射值之间的转换。

use mlua::Lua;
use xgc_world::Value;

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
