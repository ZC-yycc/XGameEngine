//! 字段路径只寻址已有槽位，避免编辑打字错误时隐式插入数据。

use super::{FieldSchema, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldPath {
    root: String,
    steps: Vec<Step>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    Field(String),
    Index(usize),
    Key(String),
}

impl FieldPath {
    /// 字段名和枚举分支使用 `.name`，列表使用 `[0]`，映射使用 `["key"]`（JSON 字符串）。
    pub fn parse(path: &str) -> Result<Self, String> {
        let invalid = || format!("字段路径语法无效：{path:?}");
        let mut rest = path;
        let root = take_field(&mut rest).ok_or_else(invalid)?.to_string();
        let mut steps = Vec::new();
        while !rest.is_empty() {
            if let Some(tail) = rest.strip_prefix('.') {
                rest = tail;
                let field = take_field(&mut rest).ok_or_else(invalid)?;
                steps.push(Step::Field(field.to_string()));
            } else if let Some(tail) = rest.strip_prefix('[') {
                let (step, tail) = bracket(tail).ok_or_else(invalid)?;
                steps.push(step);
                rest = tail;
            } else {
                return Err(invalid());
            }
        }
        Ok(Self { root, steps })
    }

    /// 生成可拼接的映射键段。路径生产方和解析方必须使用相同的转义规则。
    pub fn key_segment(key: &str) -> String {
        let quoted = serde_json::to_string(key).expect("字符串总能编码成 JSON");
        format!("[{quoted}]")
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn is_root(&self) -> bool {
        self.steps.is_empty()
    }

    /// `root_value` 是根字段的值，从其内部沿路径查找。
    pub fn read<'a>(&self, root_value: &'a Value) -> Result<&'a Value, String> {
        let mut current = root_value;
        for step in &self.steps {
            current = match (step, current) {
                (Step::Field(field), Value::Struct(fields)) => fields.get(field),
                (Step::Index(index), Value::List(values)) => values.get(*index),
                (Step::Key(key), Value::Map(values)) => values.get(key),
                (Step::Field(name), Value::Enum { variant, payload }) => {
                    (name == variant).then_some(payload.as_ref())
                }
                (_, value) => return Err(step.wrong_kind(value.kind())),
            }
            .ok_or_else(|| step.missing())?;
        }
        Ok(current)
    }

    /// `root_value` 是根字段的值。目标必须存在，成功时返回原值供撤销使用。
    pub fn replace(&self, root_value: &mut Value, value: Value) -> Result<Value, String> {
        let mut current = root_value;
        for step in &self.steps {
            current = match (step, current) {
                (Step::Field(field), Value::Struct(fields)) => fields.get_mut(field),
                (Step::Index(index), Value::List(values)) => values.get_mut(*index),
                (Step::Key(key), Value::Map(values)) => values.get_mut(key),
                (Step::Field(name), Value::Enum { variant, payload }) => {
                    (name == variant).then_some(payload.as_mut())
                }
                (_, value) => return Err(step.wrong_kind(value.kind())),
            }
            .ok_or_else(|| step.missing())?;
        }
        Ok(std::mem::replace(current, value))
    }

    /// `root` 是根字段的 schema。列表的所有索引共用元素描述，越界由实际值的遍历判定。
    pub fn schema<'a>(&self, root: &'a FieldSchema) -> Result<&'a FieldSchema, String> {
        let mut current = root;
        for step in &self.steps {
            current = match (step, current.ty.as_str()) {
                (Step::Field(name), "struct") => {
                    current.fields.iter().find(|field| field.name == *name)
                }
                (Step::Index(_), "list") | (Step::Key(_), "map") => current.element.as_deref(),
                (Step::Field(name), "enum") => current.variant(name),
                (_, kind) => return Err(step.wrong_kind(kind)),
            }
            .ok_or_else(|| step.missing())?;
        }
        Ok(current)
    }
}

impl Step {
    fn wrong_kind(&self, actual: &str) -> String {
        match self {
            Self::Field(field) => {
                format!("访问字段或分支 {field:?} 需要结构体或枚举，实际为 {actual}")
            }
            Self::Index(index) => format!("访问索引 [{index}] 需要列表，实际为 {actual}"),
            Self::Key(key) => format!("访问键 {key:?} 需要映射，实际为 {actual}"),
        }
    }

    fn missing(&self) -> String {
        match self {
            Self::Field(field) => format!("字段或枚举分支 {field:?} 不可访问"),
            Self::Index(index) => format!("列表索引 [{index}] 不可访问"),
            Self::Key(key) => format!("映射键 {key:?} 不存在"),
        }
    }
}

fn bracket(tail: &str) -> Option<(Step, &str)> {
    if tail.starts_with('"') {
        let mut json = serde_json::Deserializer::from_str(tail).into_iter::<String>();
        let key = json.next()?.ok()?;
        let rest = tail[json.byte_offset()..].strip_prefix(']')?;
        return Some((Step::Key(key), rest));
    }
    let (index, rest) = tail.split_once(']')?;
    if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((Step::Index(index.parse().ok()?), rest))
}

fn take_field<'a>(path: &mut &'a str) -> Option<&'a str> {
    let len = path.find(['.', '[']).unwrap_or(path.len());
    let (field, rest) = path.split_at(len);
    let mut chars = field.chars();
    if !chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_alphabetic())
        || !chars.all(|ch| ch == '_' || ch.is_alphanumeric())
    {
        return None;
    }
    *path = rest;
    Some(field)
}
