//! 旧 Rust 插件组件 RON → `plugin.rust` 组件数据。

use xgc_world::{Component, ComponentData, PluginComponent, Value};

pub fn convert(
    registered_name: &str,
    fields: Option<&ron::Value>,
) -> Result<ComponentData, String> {
    let fields = match fields {
        Some(fields) => {
            ron::to_string(fields).map_err(|err| format!("plugin.rust 字段序列化失败：{err}"))?
        }
        None => String::new(),
    };
    Ok(ComponentData {
        component: PluginComponent::ID,
        fields: vec![
            (
                "registered_name".to_string(),
                Value::String(registered_name.to_string()),
            ),
            ("fields".to_string(), Value::String(fields)),
        ],
    })
}
