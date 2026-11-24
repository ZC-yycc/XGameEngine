//! 项目内相对路径的**唯一**校验入口。
//!
//! 旧端把这条规则放在 `xgame-core::project::normalize_project_relative`，场景 /
//! 预制体 / 资产 / 脚本四条读路径共用。XGC 之前只有资产目录扫描，没有这条纪律；
//! 项目级 `check` 一接旧工程，`Scenes/Level:1.ron` 这种写法就会**通过**检查、
//! 但盘上根本不存在（Windows 把 `:1` 当成 NTFS 备用数据流）。所以先在
//! `xgc-base` 落一份，所有宿主的读 / 写边界都从这里过。
//!
//! 规则分两类，理由都是"Windows 会**静默**改写，写下的字符串和盘上的文件不是
//! 同一个"：
//! - **越界**：绝对路径 / 盘符 / UNC / `..`。`root.join(..)` 遇到绝对路径会整个
//!   替换掉根，读文件变成读机器上的任意位置；
//! - **Windows 不认**：冒号、结尾空格与点、控制字符与 `< > " | ? *`、保留设备名
//!   （`CON` / `NUL` / `COM1` …，`File::create("CON.ron")` 打开的是控制台）。
//!
//! 返回正斜杠、已去掉开头 `./` 与空组件的相对路径。**不检查**扩展名与目录白名单
//! —— 那是场景 / 预制体 / 资产各自的事。

/// 项目相对路径不合法时**具体哪里不合法**。
///
/// 分成枚举而不是直接给字符串，是为了让 `--check` 能输出纯 ASCII 的短标签
/// （控制台代码页不是 UTF-8，中文原因会在 CI 日志里变成乱码），而报错仍能拿到
/// 一句能读的中文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathDefect {
    /// 空路径（或只有 `./`）。
    Empty,
    /// 绝对路径 / 盘符 / UNC 前缀。
    Absolute,
    /// `..`（会跳到项目外）。
    ParentEscape,
    /// 组件含冒号（Windows 会当成 NTFS 备用数据流）。
    Colon { part: String },
    /// 组件以空格或点结尾（Windows 会静默吃掉）。
    TrailingSpaceOrDot { part: String },
    /// 组件含控制字符或 `< > " | ? *`。
    IllegalChar { part: String },
    /// 组件是 Windows 保留设备名（`CON` / `NUL` / `COM1` …）。
    ReservedName { part: String },
}

impl PathDefect {
    /// 纯 ASCII 的短标签（`--check` 报告用，别改成中文）。
    pub fn ascii(&self) -> &'static str {
        match self {
            Self::Empty => "empty path",
            Self::Absolute => "absolute path or drive prefix",
            Self::ParentEscape => "`..` escapes the project",
            Self::Colon { .. } => "colon (NTFS alternate data stream)",
            Self::TrailingSpaceOrDot { .. } => "trailing space or dot",
            Self::IllegalChar { .. } => "illegal character (control or < > \" | ? *)",
            Self::ReservedName { .. } => "windows reserved device name",
        }
    }
}

impl std::fmt::Display for PathDefect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "路径不能为空"),
            Self::Absolute => write!(f, "必须是项目相对路径（不能以 `/`、盘符或 UNC 开头）"),
            Self::ParentEscape => write!(f, "路径不能包含 `..`"),
            Self::Colon { part } => write!(
                f,
                "路径组件 `{part}` 含冒号（Windows 会把 `名字:后缀` 当成备用数据流）"
            ),
            Self::TrailingSpaceOrDot { part } => write!(
                f,
                "路径组件 `{part}` 以空格或点结尾（Windows 会静默吃掉这两个字符）"
            ),
            Self::IllegalChar { part } => {
                write!(f, "路径组件 `{part}` 含非法字符（控制字符或 < > \" | ? *）")
            }
            Self::ReservedName { part } => write!(
                f,
                "路径组件 `{part}` 是 Windows 保留设备名（打开的是设备，不是文件）"
            ),
        }
    }
}

impl std::error::Error for PathDefect {}

/// 校验并规范化一条项目相对路径。
///
/// 接受 `Scenes\showcase.ron`（Windows 写法）、`./Scenes//showcase.ron`；
/// 输出固定是 `Scenes/showcase.ron`。空路径、越界、Windows 不认的写法一律拒绝，
/// 错误里带**具体组件**，方便 `--check` 点名到文件。
pub fn normalize_project_relative(raw: &str) -> Result<String, PathDefect> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(PathDefect::Empty);
    }

    let rel = trimmed.replace('\\', "/");
    let rel = rel.strip_prefix("./").unwrap_or(&rel);
    if rel.is_empty() {
        return Err(PathDefect::Empty);
    }
    // 显式挡盘符与 UNC：非 Windows 的 `Path` 不一定会把 `C:` 认成绝对路径，
    // 但项目最终要在 Windows 上打开，不能靠平台差异放行。
    if rel.starts_with('/') || rel.as_bytes().get(1) == Some(&b':') {
        return Err(PathDefect::Absolute);
    }

    let mut parts = Vec::new();
    for part in rel.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err(PathDefect::ParentEscape);
        }
        validate_path_component(part)?;
        parts.push(part);
    }
    if parts.is_empty() {
        return Err(PathDefect::Empty);
    }
    Ok(parts.join("/"))
}

/// Windows 保留设备名：这些名字（**带不带扩展名都一样**）在 Win32 上打开的是
/// 设备而不是文件。
const RESERVED_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 这个名字是不是 Windows 保留设备名（只看第一个 `.` 之前的部分）。
///
/// `CON` / `con.ron` 都算；`CONSOLE` / `console.ron` / `COM10` 不算 —— 保留的是
/// **整名相等**（且大小写不敏感），别按前缀匹配，否则正常的词会被误杀。
fn is_reserved_device_name(part: &str) -> bool {
    let base = part.split('.').next().unwrap_or(part);
    RESERVED_DEVICE_NAMES
        .iter()
        .any(|name| base.eq_ignore_ascii_case(name))
}

/// 单个路径组件的字符级校验。
fn validate_path_component(part: &str) -> Result<(), PathDefect> {
    if part.contains(':') {
        return Err(PathDefect::Colon {
            part: part.to_string(),
        });
    }
    if part.ends_with([' ', '.']) {
        return Err(PathDefect::TrailingSpaceOrDot {
            part: part.to_string(),
        });
    }
    if part
        .chars()
        .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Err(PathDefect::IllegalChar {
            part: part.to_string(),
        });
    }
    if is_reserved_device_name(part) {
        return Err(PathDefect::ReservedName {
            part: part.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_normal_relative_path() {
        assert_eq!(
            normalize_project_relative("Scenes/showcase.ron").unwrap(),
            "Scenes/showcase.ron"
        );
    }

    #[test]
    fn normalizes_backslashes_and_redundant_dots() {
        assert_eq!(
            normalize_project_relative(r".\Scenes\\showcase.ron").unwrap(),
            "Scenes/showcase.ron"
        );
        assert_eq!(
            normalize_project_relative("Scenes/./showcase.ron").unwrap(),
            "Scenes/showcase.ron"
        );
        assert_eq!(
            normalize_project_relative("Scenes//showcase.ron").unwrap(),
            "Scenes/showcase.ron"
        );
    }

    #[test]
    fn rejects_absolute_paths_and_drive_prefixes() {
        assert_eq!(
            normalize_project_relative("/Scenes/showcase.ron").unwrap_err(),
            PathDefect::Absolute
        );
        assert_eq!(
            normalize_project_relative(r"C:\Scenes\showcase.ron").unwrap_err(),
            PathDefect::Absolute
        );
        assert_eq!(
            normalize_project_relative(r"\\server\share\showcase.ron").unwrap_err(),
            PathDefect::Absolute
        );
    }

    #[test]
    fn rejects_parent_escape() {
        assert_eq!(
            normalize_project_relative("../showcase.ron").unwrap_err(),
            PathDefect::ParentEscape
        );
        assert_eq!(
            normalize_project_relative("Scenes/../../showcase.ron").unwrap_err(),
            PathDefect::ParentEscape
        );
    }

    #[test]
    fn rejects_windows_silent_rewrites() {
        assert!(matches!(
            normalize_project_relative("Scenes/Level:1.ron").unwrap_err(),
            PathDefect::Colon { .. }
        ));
        assert!(matches!(
            normalize_project_relative("Scenes/Level /x.ron").unwrap_err(),
            PathDefect::TrailingSpaceOrDot { .. }
        ));
        assert!(matches!(
            normalize_project_relative("Scenes/Level.ron.").unwrap_err(),
            PathDefect::TrailingSpaceOrDot { .. }
        ));
        assert!(matches!(
            normalize_project_relative("Scenes/Level?.ron").unwrap_err(),
            PathDefect::IllegalChar { .. }
        ));
        assert!(matches!(
            normalize_project_relative("Scenes/CON.ron").unwrap_err(),
            PathDefect::ReservedName { .. }
        ));
        assert!(matches!(
            normalize_project_relative("Scenes/con").unwrap_err(),
            PathDefect::ReservedName { .. }
        ));
    }

    #[test]
    fn rejects_empty_paths() {
        assert_eq!(
            normalize_project_relative("").unwrap_err(),
            PathDefect::Empty
        );
        assert_eq!(
            normalize_project_relative("   ").unwrap_err(),
            PathDefect::Empty
        );
        assert_eq!(
            normalize_project_relative("./").unwrap_err(),
            PathDefect::Empty
        );
    }

    #[test]
    fn ascii_labels_are_ascii() {
        let labels = [
            PathDefect::Empty,
            PathDefect::Absolute,
            PathDefect::ParentEscape,
            PathDefect::Colon { part: "a:b".into() },
            PathDefect::TrailingSpaceOrDot { part: "a ".into() },
            PathDefect::IllegalChar { part: "a?".into() },
            PathDefect::ReservedName { part: "CON".into() },
        ];
        for label in labels {
            assert!(label.ascii().is_ascii(), "{} 不是 ASCII", label.ascii());
        }
    }
}
