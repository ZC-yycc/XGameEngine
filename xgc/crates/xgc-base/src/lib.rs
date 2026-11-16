//! XGame Core 的基础类型。
//!
//! 这里只放**跨子系统共享、且不依赖任何子系统**的东西：持久实体身份、运行时
//! 句柄、诊断。渲染 / 物理 / 脚本的类型不许进这个 crate，否则依赖方向会反转。

#![deny(unsafe_code)]

use std::fmt;

use serde::{Deserialize, Serialize};

/// 持久实体身份：项目级单调分配，永不复用。
///
/// 磁盘、存档、Agent 协议、跨文件引用都用它。运行时热路径用 [`EntityKey`]。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntityId(u64);

impl EntityId {
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Debug for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EntityId({})", self.0)
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 运行时实体句柄：O(1) 访问，`generation` 防悬垂。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct EntityKey {
    index: u32,
    generation: u32,
}

impl EntityKey {
    pub const fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    pub const fn index(self) -> u32 {
        self.index
    }

    pub const fn generation(self) -> u32 {
        self.generation
    }
}

/// 世界阶段计数；用于变更检测的位图。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Tick(u32);

impl Tick {
    pub const fn get(self) -> u32 {
        self.0
    }

    /// 前进一格；回绕是允许的，调用方不要把 tick 当成绝对时间。
    pub fn advance(&mut self) -> Tick {
        self.0 = self.0.wrapping_add(1);
        *self
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        };
        f.write_str(s)
    }
}

/// 诊断指向的对象；不是一坨自由文本。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Subject {
    Entity(EntityId),
    Asset(String),
    Scene(String),
    Field(String),
    System(String),
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Entity(id) => write!(f, "entity:{id}"),
            Self::Asset(path) => write!(f, "asset:{path}"),
            Self::Scene(path) => write!(f, "scene:{path}"),
            Self::Field(path) => write!(f, "field:{path}"),
            Self::System(name) => write!(f, "system:{name}"),
        }
    }
}

/// 源码位置；`file` 是项目相对路径的字符串形式。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Span {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

impl Span {
    pub fn new(file: impl Into<String>, line: u32, column: u32) -> Self {
        Self {
            file: file.into(),
            line,
            column,
        }
    }
}

/// 结构化诊断：稳定 code + 对象 + 位置 + 人读消息。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub subject: Subject,
    pub span: Option<Span>,
    pub message: String,
}

impl Diagnostic {
    pub fn new(
        code: &'static str,
        severity: Severity,
        subject: Subject,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity,
            subject,
            span: None,
            message: message.into(),
        }
    }

    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} [{}] {}", self.severity, self.code, self.subject)?;
        if let Some(span) = &self.span {
            write!(f, " at {}:{}:{}", span.file, span.line, span.column)?;
        }
        write!(f, ": {}", self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_id_is_stable_and_readable() {
        let id = EntityId::from_raw(42);
        assert_eq!(id.raw(), 42);
        assert_eq!(id.to_string(), "42");
        assert_eq!(format!("{id:?}"), "EntityId(42)");
    }

    #[test]
    fn tick_wraps_without_panicking() {
        let mut tick = Tick::default();
        assert_eq!(tick.advance().get(), 1);
        let mut full = Tick(u32::MAX);
        assert_eq!(full.advance().get(), 0);
    }

    #[test]
    fn diagnostic_display_includes_code_and_subject() {
        let diag = Diagnostic::new(
            "WORLD.DUP_ID",
            Severity::Error,
            Subject::Entity(EntityId::from_raw(7)),
            "实体 ID 重复",
        )
        .with_span(Span::new("scenes/a.scene.ron", 3, 9));
        let text = diag.to_string();
        assert!(text.contains("WORLD.DUP_ID"));
        assert!(text.contains("entity:7"));
        assert!(text.contains("scenes/a.scene.ron:3:9"));
    }
}
