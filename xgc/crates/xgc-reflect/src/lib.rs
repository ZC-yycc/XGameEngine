//! 组件注册表与 schema 生成。
//!
//! 一个组件 = 一个稳定 `ComponentId` + 一组字段描述 + 默认值。注册表是
//! schema、Inspector、Agent 工具、序列化的**唯一描述源**；这里先做 M0 需要的
//! 最小版本，derive 宏与命令 / 查询生成在 M1 接上。

#![deny(unsafe_code)]

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use xgc_base::{Diagnostic, Severity, Subject};

mod value;

/// 组件类型的稳定编号。落盘、协议、archetype 匹配都用它，不用类型名。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct ComponentId(pub u32);

impl fmt::Display for ComponentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 纯数据组件。
///
/// `fields()` 目前手写；M1 用 derive 生成，并加编译期穷举测试防漂移。
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

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
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
}

impl FieldType {
    pub const fn as_str(self) -> &'static str {
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
        }
    }

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
}

impl Value {
    /// schema 声明类型与动态值是否相容；校验 / 迁移 / Agent 共用这一份判断。
    pub fn fits(&self, ty: FieldType) -> bool {
        match (self, ty) {
            (Self::Bool(_), FieldType::Bool) => true,
            (Self::I64(_), FieldType::I64) => true,
            (Self::I64(value), FieldType::U32) => (0..=u32::MAX as i64).contains(value),
            (Self::F64(_), FieldType::F32 | FieldType::F64) => true,
            (Self::String(_), FieldType::String) => true,
            (Self::Vec2(_), FieldType::Vec2) => true,
            (Self::Vec3(_), FieldType::Vec3) => true,
            (Self::Vec4(_), FieldType::Vec4) => true,
            (Self::Quat(_), FieldType::Quat) => true,
            (Self::Entity(_), FieldType::Entity) => true,
            _ => false,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Bool(_) => "bool",
            Self::I64(_) => "i64",
            Self::F64(_) => "f64",
            Self::String(_) => "string",
            Self::Vec2(_) => "vec2",
            Self::Vec3(_) => "vec3",
            Self::Vec4(_) => "vec4",
            Self::Quat(_) => "quat",
            Self::Entity(_) => "entity",
        }
    }
}

/// 字段描述：字段名、类型、默认值、单位、范围、文档。
#[derive(Clone, Debug)]
pub struct FieldDescriptor {
    pub name: &'static str,
    pub ty: FieldType,
    pub default: Value,
    pub unit: Option<&'static str>,
    pub range: Option<(f64, f64)>,
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

    pub fn doc(mut self, doc: &'static str) -> Self {
        self.doc = doc;
        self
    }
}

/// 可序列化的字段 schema。
#[derive(Clone, Debug, Serialize)]
pub struct FieldSchema {
    pub name: String,
    pub ty: String,
    pub default: Value,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    pub doc: String,
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

        let fields = T::fields()
            .into_iter()
            .map(|f| {
                let (min, max) = match f.range {
                    Some((min, max)) => (Some(min), Some(max)),
                    None => (None, None),
                };
                FieldSchema {
                    name: f.name.to_string(),
                    ty: f.ty.as_str().to_string(),
                    default: f.default,
                    required: true,
                    unit: f.unit.map(str::to_string),
                    min,
                    max,
                    doc: f.doc.to_string(),
                }
            })
            .collect();

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
            let mut names = HashSet::new();
            for field in &component.fields {
                let subject = Subject::Field(format!("{}.{}", component.name, field.name));
                if field.name.is_empty() {
                    diagnostics.push(Diagnostic::new(
                        "SCHEMA.EMPTY_FIELD",
                        Severity::Error,
                        subject,
                        "字段名为空",
                    ));
                }
                if !names.insert(field.name.as_str()) {
                    diagnostics.push(Diagnostic::new(
                        "SCHEMA.DUP_FIELD",
                        Severity::Error,
                        Subject::Field(format!("{}.{}", component.name, field.name)),
                        "字段名重复",
                    ));
                }
                if !default_matches(&field.default, &field.ty) {
                    diagnostics.push(Diagnostic::new(
                        "SCHEMA.DEFAULT_TYPE",
                        Severity::Error,
                        Subject::Field(format!("{}.{}", component.name, field.name)),
                        format!(
                            "默认值类型 {} 与字段类型 {} 不一致",
                            field.default.type_name(),
                            field.ty
                        ),
                    ));
                }
            }
        }
        diagnostics
    }
}

fn default_matches(value: &Value, ty: &str) -> bool {
    match ty {
        "bool" => matches!(value, Value::Bool(_)),
        "i64" | "u32" => matches!(value, Value::I64(_)),
        "f32" | "f64" => matches!(value, Value::F64(_)),
        "string" => matches!(value, Value::String(_)),
        "vec2" => matches!(value, Value::Vec2(_)),
        "vec3" => matches!(value, Value::Vec3(_)),
        "vec4" => matches!(value, Value::Vec4(_)),
        "quat" => matches!(value, Value::Quat(_)),
        "entity" => matches!(value, Value::Entity(_)),
        _ => false,
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
