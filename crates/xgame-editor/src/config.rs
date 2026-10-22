//! 编辑器用户配置目录。
//!
//! 这里放的是**跨项目**的编辑器状态（最近项目、偏好、面板布局），不是项目内容：
//! 项目仍然完全由磁盘上的目录决定，换一台机器打开同一个项目不依赖这些文件。
//!
//! 路径规则（Windows）：`%APPDATA%\XGameEngine\`；其它平台依次退到
//! `$XDG_CONFIG_HOME` / `~/.config`。
//!
//! `XGAME_CONFIG_DIR` 可以**整体覆盖**这个目录（直接当成配置目录，不再拼
//! `XGameEngine`）。它是给无头验证和便携运行用的：测试可以在临时目录里跑，
//! 不会碰用户真实的布局与最近项目。

use std::path::PathBuf;

/// 用户配置目录名（位于平台的配置根目录下）。
pub const DIR_NAME: &str = "XGameEngine";

/// 返回配置目录；取不到平台配置根目录且没有覆盖时返回 `None`
/// （此时持久化功能静默关闭，编辑器照常运行）。
pub fn dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XGAME_CONFIG_DIR") {
        let dir = PathBuf::from(dir);
        if !dir.as_os_str().is_empty() {
            return Some(dir);
        }
    }

    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;

    Some(base.join(DIR_NAME))
}

/// 配置目录下的某个文件。目录不可用时返回 `None`。
pub fn file(name: &str) -> Option<PathBuf> {
    dir().map(|dir| dir.join(name))
}
