//! 预制体实例标记：记录来源资产与源实体 ID。

use xgc_reflect::{AssetKind, Component, ComponentId, FieldDescriptor, FieldType, Value};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrefabInstance {
    /// 来源预制体路径（项目相对路径）。
    pub source: String,
    /// 预制体文件里的源实体 ID。
    pub source_id: u64,
    /// 是否为实例根。
    pub root: bool,
}

impl Component for PrefabInstance {
    const ID: ComponentId = ComponentId(0x0802);
    const NAME: &'static str = "prefab.instance";

    fn default_value() -> Self {
        Self::default()
    }

    fn fields() -> Vec<FieldDescriptor> {
        vec![
            FieldDescriptor::new("source", FieldType::String, Value::String(String::new()))
                .asset(AssetKind::Prefab)
                .doc("来源预制体路径或 GUID"),
            FieldDescriptor::new("source_id", FieldType::Entity, Value::Entity(0))
                .doc("预制体内的源实体 ID"),
            FieldDescriptor::new("root", FieldType::Bool, Value::Bool(false))
                .doc("是否为预制体实例根"),
        ]
    }

    fn get_field(&self, field: &str) -> Option<Value> {
        match field {
            "source" => Some(Value::String(self.source.clone())),
            "source_id" => Some(Value::Entity(self.source_id)),
            "root" => Some(Value::Bool(self.root)),
            _ => None,
        }
    }

    fn set_field(&mut self, field: &str, value: &Value) -> Result<(), String> {
        match field {
            "source" => match value {
                Value::String(value) => {
                    self.source = value.clone();
                    Ok(())
                }
                other => Err(format!(
                    "prefab.instance.source 期望 string，得到 {}",
                    other.type_name()
                )),
            },
            "source_id" => match value {
                Value::Entity(value) => {
                    self.source_id = *value;
                    Ok(())
                }
                other => Err(format!(
                    "prefab.instance.source_id 期望 entity，得到 {}",
                    other.type_name()
                )),
            },
            "root" => match value {
                Value::Bool(value) => {
                    self.root = *value;
                    Ok(())
                }
                other => Err(format!(
                    "prefab.instance.root 期望 bool，得到 {}",
                    other.type_name()
                )),
            },
            _ => Err(format!("prefab.instance 没有字段 {field}")),
        }
    }
}
