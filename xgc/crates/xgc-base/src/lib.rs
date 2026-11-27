//! XGame Core 的基础类型。
//!
//! 这里只放**跨子系统共享、且不依赖任何子系统**的东西：持久实体身份、运行时
//! 句柄、诊断，以及各宿主共用的项目设置。渲染 / 物理 / 脚本的运行时类型
//! 不进入这个 crate，保证配置读写不依赖任何后端。

#![deny(unsafe_code)]

pub mod diagnostics;
pub mod layer;
pub mod path;
pub mod render_settings;

pub use diagnostics::{catalog as diagnostic_catalog, DiagnosticCode};

use std::fmt;

use serde::de::{Deserializer, Error as DeError};
use serde::ser::{Error as SerError, Serializer};
use serde::{Deserialize, Serialize};

/// 持久实体身份：项目级单调分配，永不复用。
///
/// 磁盘、存档、Agent 协议、跨文件引用都用它。运行时热路径用 [`EntityKey`]。
/// JSON 里固定写成十进制字符串，避免 2^53 精度丢失。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityId(u64);

impl EntityId {
    /// 游标可以走到 `u64::MAX` 表示已耗尽，持久身份必须留出这最后一格。
    pub const MAX_ASSIGNABLE_RAW: u64 = u64::MAX - 1;

    /// 仅包装原值，供运行时代码和分配游标使用。
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    /// 写入或接收持久身份时，拒绝留给分配游标的哨兵。
    pub const fn validate(self) -> Result<(), InvalidEntityId> {
        if self.0 <= Self::MAX_ASSIGNABLE_RAW {
            Ok(())
        } else {
            Err(InvalidEntityId(self.0))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidEntityId(u64);

impl fmt::Display for InvalidEntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "实体 ID {} 超出持久 ID 上限 {}",
            self.0,
            EntityId::MAX_ASSIGNABLE_RAW
        )
    }
}

impl std::error::Error for InvalidEntityId {}

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

impl Serialize for EntityId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(S::Error::custom)?;
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for EntityId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let id = text
            .parse::<u64>()
            .map(Self)
            .map_err(|err| D::Error::custom(format!("实体 ID 不是十进制 u64：{err}")))?;
        id.validate().map_err(D::Error::custom)?;
        Ok(id)
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
