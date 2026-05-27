//! 资产标识与引用。
//!
//! 提供轻量级资产句柄，替代原始字符串路径。未来版本将支持资产数据库、
//! 引用计数和热重载。

use serde::{Deserialize, Serialize};
use std::path::Path;

/// 资产标识符——轻量的字符串包装，唯一标识一个资产。
///
/// 当前使用文件系统路径作为标识，未来可切换到 UUID 或内容哈希。
///
/// # Example
///
/// ```ignore
/// let id = AssetId::new("Resources/Box.glb");
/// assert_eq!(id.path(), "Resources/Box.glb");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetId(String);

impl AssetId {
    /// 从路径字符串创建资产标识。
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    /// 获取资产路径。
    pub fn path(&self) -> &str {
        &self.0
    }

    /// 获取文件扩展名（小写）。
    pub fn extension(&self) -> Option<&str> {
        Path::new(&self.0).extension().and_then(|e| e.to_str())
    }

    /// 获取文件名（包含扩展名）。
    pub fn file_name(&self) -> Option<&str> {
        Path::new(&self.0).file_name().and_then(|n| n.to_str())
    }

    /// 获取去掉扩展名的文件名。
    pub fn stem(&self) -> Option<&str> {
        Path::new(&self.0).file_stem().and_then(|s| s.to_str())
    }

    /// 是否为有效路径（非空）。
    pub fn is_valid(&self) -> bool {
        !self.0.is_empty()
    }
}

impl From<String> for AssetId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for AssetId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl std::fmt::Display for AssetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for AssetId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
