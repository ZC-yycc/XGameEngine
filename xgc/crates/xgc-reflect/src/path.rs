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
}

impl FieldPath {
    /// 字段名以字母或下划线开头，后接字母、数字或下划线；索引为十进制非负整数。
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
                let (index, tail) = tail.split_once(']').ok_or_else(invalid)?;
                if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(invalid());
                }
                steps.push(Step::Index(index.parse().map_err(|_| invalid())?));
                rest = tail;
            } else {
                return Err(invalid());
            }
        }
        Ok(Self { root, steps })
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
                (Step::Index(_), "list") => current.element.as_deref(),
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
            Self::Field(field) => format!("访问字段 {field:?} 需要结构体，实际为 {actual}"),
            Self::Index(index) => format!("访问索引 [{index}] 需要列表，实际为 {actual}"),
        }
    }

    fn missing(&self) -> String {
        match self {
            Self::Field(field) => format!("不存在字段 {field:?}"),
            Self::Index(index) => format!("列表索引 [{index}] 不可访问"),
        }
    }
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
