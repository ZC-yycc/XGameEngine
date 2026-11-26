//! 组件注册表与 schema 生成。
//!
//! 一个组件 = 一个稳定 `ComponentId` + 一组字段描述 + 默认值。注册表是
//! schema、Inspector、Agent 工具、序列化的唯一描述源。派生宏从 Rust 字段声明
//! 生成本文的接口，读取、写入和容器值沿用同一套反射模型。

#![deny(unsafe_code)]

use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::fmt;

use serde::{Deserialize, Serialize};
use xgc_base::{Diagnostic, Severity, Subject};

mod path;
mod schema;
mod typed;
mod value;

pub use path::FieldPath;
pub use schema::FieldSchema;
pub use typed::ReflectValue;
pub use xgc_reflect_derive::{Component, ReflectValue};

/// 组件类型的稳定编号。落盘、协议、archetype 匹配都用它，不用类型名。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct ComponentId(pub u32);

impl fmt::Display for ComponentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 纯数据组件。具名结构体可通过同名派生宏生成描述与字段访问。
pub trait Component: Any + Clone + Send + Sync {
    const ID: ComponentId;
    const NAME: &'static str;

    fn default_value() -> Self;
    fn fields() -> Vec<FieldDescriptor> {
        Vec::new()
    }

    /// 参与 `World::state_hash` 的字段（缺省 = 全部字段）。
    ///
    /// 作者数据（例如 `animation.animator.events`）不是运行时状态：把它从
    /// 这里排掉，才能让「只加配置、不改运行时」的改动不移动验收参考哈希。
    /// 序列化 / schema / 校验仍然走 [`Component::fields`]。
    fn state_fields() -> Vec<FieldDescriptor> {
        Self::fields()
    }

    fn doc() -> &'static str {
        ""
    }

    /// 动态读字段；M1 的命令 / Agent 协议用。
    fn get_field(&self, _field: &str) -> Option<Value> {
        None
    }

    /// 混入字段的值，等价于读取该字段后对 `Value` 哈希。字段名由调用方混入。
    fn hash_field_into(&self, field: &str, hash: &mut u64) {
        if let Some(value) = self.get_field(field) {
            value.hash_into(hash);
        }
    }

    /// 动态写字段；实现方负责类型校验，错误必须是给人看的原因。
    fn set_field(&mut self, _field: &str, _value: &Value) -> Result<(), String> {
        Err(format!("{} 不支持字段写入", Self::NAME))
    }

    /// 计划阶段用：克隆一份试写，不碰真实世界。
    fn validate_field(&self, field: &str, value: &Value) -> Result<(), String> {
        let mut copy = self.clone();
        copy.set_field(field, value)
    }
}

#[derive(Clone, PartialEq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Bool,
    I64,
    U32,
    F32,
    F64,
    String,
    Vec2,
    Vec3,
    Vec4,
    Quat,
    Entity,
    List(Box<FieldDescriptor>),
    Struct(Vec<FieldDescriptor>),
    /// 键是字符串，所有值共用同一份描述。
    Map(Box<FieldDescriptor>),
    /// 每个分支的名字和载荷由描述节点提供；无载荷用空结构体。
    Enum(Vec<FieldDescriptor>),
}

impl FieldType {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::I64 => "i64",
            Self::U32 => "u32",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::String => "string",
            Self::Vec2 => "vec2",
            Self::Vec3 => "vec3",
            Self::Vec4 => "vec4",
            Self::Quat => "quat",
            Self::Entity => "entity",
            Self::List(_) => "list",
            Self::Struct(_) => "struct",
            Self::Map(_) => "map",
            Self::Enum(_) => "enum",
        }
    }

    /// 标量可直接从类型名还原，容器的类型还取决于子字段。
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "bool" => Self::Bool,
            "i64" => Self::I64,
            "u32" => Self::U32,
            "f32" => Self::F32,
            "f64" => Self::F64,
            "string" => Self::String,
            "vec2" => Self::Vec2,
            "vec3" => Self::Vec3,
            "vec4" => Self::Vec4,
            "quat" => Self::Quat,
            "entity" => Self::Entity,
            _ => return None,
        })
    }
}

/// 动态值：schema 默认值、命令参数、Agent 协议共用。
///
/// 带 `kind` 标签的 JSON，避免 `quat` / `vec4` 同为 4 元数组而无法反序列化。
/// `Entity` 用 u64 原始值；JSON 协议层负责把 64 位 ID 写成十进制字符串。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Value {
    Bool(bool),
    I64(i64),
    F64(f64),
    String(String),
    Vec2([f64; 2]),
    Vec3([f64; 3]),
    Vec4([f64; 4]),
    Quat([f64; 4]),
    Entity(u64),
    List(Vec<Value>),
    Struct(BTreeMap<String, Value>),
    Map(BTreeMap<String, Value>),
    Enum {
        variant: String,
        payload: Box<Value>,
    },
}

/// 字段描述同时供校验、schema 导出和生成式编辑器使用。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FieldDescriptor {
    pub name: &'static str,
    pub ty: FieldType,
    pub default: Value,
    pub unit: Option<&'static str>,
    pub range: Option<(f64, f64)>,
    /// 数值控件每个方向键步进或拖动一个逻辑像素的增量。
    pub step: Option<f64>,
    /// 生成式编辑器只显示该值及其子树，持久化和模拟仍读写同一字段。
    pub read_only: bool,
    /// 同层字段的显示分组。
    pub group: Option<&'static str>,
    pub doc: &'static str,
}

impl FieldDescriptor {
    pub fn new(name: &'static str, ty: FieldType, default: Value) -> Self {
        Self {
            name,
            ty,
            default,
            unit: None,
            range: None,
            step: None,
            read_only: false,
            group: None,
            doc: "",
        }
    }

    pub fn unit(mut self, unit: &'static str) -> Self {
        self.unit = Some(unit);
        self
    }

    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.range = Some((min, max));
        self
    }

    pub fn step(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }

    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    pub fn group(mut self, group: &'static str) -> Self {
        self.group = Some(group);
        self
    }

    pub fn doc(mut self, doc: &'static str) -> Self {
        self.doc = doc;
        self
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ComponentSchema {
    pub id: u32,
    pub name: String,
    pub summary: String,
    pub fields: Vec<FieldSchema>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SchemaDoc {
    pub format_version: u32,
    pub engine: &'static str,
    pub types: Vec<ComponentSchema>,
}

#[derive(Debug)]
pub enum RegistryError {
    DuplicateId(u32),
    DuplicateName(String),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateId(id) => write!(f, "组件 ID 重复：{id}"),
            Self::DuplicateName(name) => write!(f, "组件名重复：{name}"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// 组件注册表。注册顺序不影响输出：`schema()` 按 ID 排序。
#[derive(Default, Clone, Debug)]
pub struct Registry {
    components: Vec<ComponentSchema>,
    by_id: HashMap<u32, usize>,
    by_name: HashMap<String, usize>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T: Component>(&mut self) -> Result<(), RegistryError> {
        let id = T::ID.0;
        if self.by_id.contains_key(&id) {
            return Err(RegistryError::DuplicateId(id));
        }
        if self.by_name.contains_key(T::NAME) {
            return Err(RegistryError::DuplicateName(T::NAME.to_string()));
        }

        let fields = T::fields().into_iter().map(FieldSchema::from).collect();

        let index = self.components.len();
        self.by_id.insert(id, index);
        self.by_name.insert(T::NAME.to_string(), index);
        self.components.push(ComponentSchema {
            id,
            name: T::NAME.to_string(),
            summary: T::doc().to_string(),
            fields,
        });
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.components.len()
    }

    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    pub fn get(&self, id: ComponentId) -> Option<&ComponentSchema> {
        self.by_id.get(&id.0).map(|&i| &self.components[i])
    }

    pub fn by_name(&self, name: &str) -> Option<&ComponentSchema> {
        self.by_name.get(name).map(|&i| &self.components[i])
    }

    /// 确定性 schema：按 ID 排序，无时间戳 / 随机值。
    pub fn schema(&self) -> SchemaDoc {
        let mut types = self.components.clone();
        types.sort_by_key(|c| c.id);
        SchemaDoc {
            format_version: 1,
            engine: "xgc",
            types,
        }
    }

    /// 检查注册表自身的语义：字段名非空 / 不重复、默认值类型匹配。
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        for component in &self.components {
            if component.name.is_empty() {
                diagnostics.push(Diagnostic::new(
                    "SCHEMA.EMPTY_NAME",
                    Severity::Error,
                    Subject::Field(component.id.to_string()),
                    "组件名为空",
                ));
            }
            schema::validate_fields(&component.fields, &component.name, &mut diagnostics);
        }
        diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct Health {
        hp: f32,
        alive: bool,
    }

    impl Component for Health {
        const ID: ComponentId = ComponentId(0x0201);
        const NAME: &'static str = "game.health";

        fn default_value() -> Self {
            Self {
                hp: 100.0,
                alive: true,
            }
        }

        fn doc() -> &'static str {
            "生命值"
        }

        fn fields() -> Vec<FieldDescriptor> {
            vec![
                FieldDescriptor::new("hp", FieldType::F32, Value::F64(100.0))
                    .range(0.0, 1000.0)
                    .unit("hp")
                    .doc("当前生命"),
                FieldDescriptor::new("alive", FieldType::Bool, Value::Bool(true)),
            ]
        }
    }

    #[derive(Clone, Debug)]
    struct Other;

    impl Component for Other {
        const ID: ComponentId = ComponentId(0x0201);
        const NAME: &'static str = "game.other";
        fn default_value() -> Self {
            Self
        }
    }

    #[test]
    fn schema_is_sorted_and_deterministic() {
        let mut registry = Registry::new();
        registry.register::<Health>().unwrap();
        let a = serde_json::to_string_pretty(&registry.schema()).unwrap();
        let b = serde_json::to_string_pretty(&registry.schema()).unwrap();
        assert_eq!(a, b);
        assert!(a.contains("\"game.health\""));
        assert!(!a.contains("\"vec3\""));
        assert!(a.contains("\"hp\""));
        assert!(a.contains("\"min\": 0.0"));
    }

    #[test]
    fn duplicate_id_is_rejected() {
        let mut registry = Registry::new();
        registry.register::<Health>().unwrap();
        let err = registry.register::<Other>().unwrap_err();
        assert!(matches!(err, RegistryError::DuplicateId(0x0201)));
    }

    #[test]
    fn lookup_by_id_and_name() {
        let mut registry = Registry::new();
        registry.register::<Health>().unwrap();
        assert_eq!(
            registry.get(ComponentId(0x0201)).unwrap().name,
            "game.health"
        );
        assert_eq!(registry.by_name("game.health").unwrap().id, 0x0201);
        assert!(registry.by_name("nope").is_none());
    }

    #[derive(Clone, Debug)]
    struct Broken;

    impl Component for Broken {
        const ID: ComponentId = ComponentId(0x0301);
        const NAME: &'static str = "test.broken";

        fn default_value() -> Self {
            Self
        }

        fn fields() -> Vec<FieldDescriptor> {
            vec![
                FieldDescriptor::new("a", FieldType::F32, Value::String("bad".to_string())),
                FieldDescriptor::new("a", FieldType::Bool, Value::Bool(true)),
            ]
        }
    }

    #[test]
    fn validate_reports_duplicate_fields_and_bad_defaults() {
        let mut registry = Registry::new();
        registry.register::<Health>().unwrap();
        assert!(registry.validate().is_empty());

        registry.register::<Broken>().unwrap();
        let diagnostics = registry.validate();
        let codes: Vec<_> = diagnostics.iter().map(|diag| diag.code).collect();
        assert!(codes.contains(&"SCHEMA.DUP_FIELD"));
        assert!(codes.contains(&"SCHEMA.DEFAULT_TYPE"));
    }
}
