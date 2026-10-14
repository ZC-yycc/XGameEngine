//! 项目构建器——把当前打开的项目打包成一款可以独立运行、直接分发的游戏。
//!
//! # 产物长什么样
//!
//! ```text
//! <项目根>/Build/<项目名>/
//! ├── <项目名>.exe        # xgame-game 播放器的副本（改了名，看起来就是这款游戏）
//! ├── project.ron         # 项目清单
//! ├── Scenes/             # 场景
//! └── Resources/          # 资产
//! ```
//!
//! 这个布局不是随便定的：`xgame-game` 在没有命令行参数时，会优先去找**自己
//! 可执行文件旁边**的 `project.ron`（见 `xgame-game::resolve_project`）。所以
//! 构建出来的游戏**双击 exe 就能跑**，不需要知道项目在哪，也不需要任何启动脚本。
//!
//! # 播放器从哪来
//!
//! 本模块**不编译** Rust 代码——它只是把已经存在的 `xgame-game` 可执行文件
//! 复制过去。开发时它就在编辑器旁边（两者同属 `target/<profile>/`），发布包
//! （`dist/`）里也是放在一起的。找不到就报 [`BuildError::PlayerNotFound`]，
//! 并在提示里写清要先构建哪个包。

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use xgame_core::project::{ProjectHandle, PROJECT_FILE_NAME};

/// 构建产物所在的目录名（相对项目根）。
pub const BUILD_DIR: &str = "Build";

/// 播放器（游戏运行时）可执行文件名。
#[cfg(windows)]
const PLAYER_EXE: &str = "xgame-game.exe";
#[cfg(not(windows))]
const PLAYER_EXE: &str = "xgame-game";

// ---------------------------------------------------------------------------
// 结果与错误
// ---------------------------------------------------------------------------

/// 一次成功构建的结果。
#[derive(Debug, Clone)]
pub struct BuildReport {
    /// 产物目录。
    pub output_dir: PathBuf,
    /// 生成的游戏可执行文件。
    pub executable: PathBuf,
    /// 复制过去的资产文件数（不含 exe 与清单）。
    pub asset_count: usize,
    /// 实际打包的目录（相对项目根），用于让用户看清「都带了什么」。
    pub packed_dirs: Vec<String>,
    /// 构建过程中的警告（不致命，但**必须让人看见**）。
    ///
    /// 走报告而不是 `log::warn!`：那条路只到 stderr（终端），而打包按钮是在
    /// 图形界面里点的 —— 最典型的警告"有 Cargo.toml 却没编译过插件"正是要在
    /// Console 里喊的那一声。
    pub warnings: Vec<String>,
}

impl BuildReport {
    /// 供控制台显示的一行摘要。
    pub fn summary(&self) -> String {
        format!(
            "构建完成 → {}  （{} 个资产文件{}）",
            self.output_dir.display(),
            self.asset_count,
            if self.packed_dirs.is_empty() {
                String::new()
            } else {
                format!("，目录: {}", self.packed_dirs.join(", "))
            }
        )
    }
}

/// 构建失败的原因。
///
/// 这里不用 `thiserror`——编辑器多引一个依赖不值得，错误种类也就这么几种。
#[derive(Debug)]
pub enum BuildError {
    /// 没找到播放器可执行文件。
    PlayerNotFound { searched: Vec<PathBuf> },
    /// 项目清单里写的入口场景在磁盘上不存在。
    EntrySceneMissing(PathBuf),
    /// 文件操作失败。
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PlayerNotFound { searched } => {
                write!(
                    f,
                    "找不到播放器 '{}'。请先构建它：cargo build -p xgame-game\n\
                     已查找过：{}",
                    PLAYER_EXE,
                    searched
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(" | ")
                )
            }
            Self::EntrySceneMissing(path) => {
                write!(f, "入口场景不存在: {}", path.display())
            }
            Self::Io { path, source } => {
                write!(f, "文件操作失败 ({}): {source}", path.display())
            }
        }
    }
}

impl std::error::Error for BuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// 把 `io::Error` 绑上出错的路径。
fn io_err(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> BuildError {
    let path = path.into();
    move |source| BuildError::Io { path, source }
}

// ---------------------------------------------------------------------------
// 构建
// ---------------------------------------------------------------------------

/// 把项目打包到 `<项目根>/Build/<项目名>/`。
///
/// 会先清空上一次的构建结果（只删这个输出目录，不碰项目里的任何东西）。
pub fn build(project: &ProjectHandle) -> Result<BuildReport, BuildError> {
    // 快速失败：入口场景都不在，构建出来也是个空游戏。
    // 放在动手之前，避免留下一个半成品目录。
    let entry_scene = project.entry_scene_path();
    if !entry_scene.is_file() {
        return Err(BuildError::EntrySceneMissing(entry_scene));
    }

    let player = locate_player()?;

    let output_dir = project.root().join(BUILD_DIR).join(safe_file_name(project.name()));
    prepare_output_dir(&output_dir)?;

    // 1) 播放器 → <项目名>.exe
    //    改名的用意：分发出去的成品看起来就是这款游戏本身，
    //    而不是一个叫 xgame-game 的通用运行时。
    let executable = output_dir.join(format!(
        "{}{}",
        safe_file_name(project.name()),
        std::env::consts::EXE_SUFFIX
    ));
    std::fs::copy(&player, &executable).map_err(io_err(&executable))?;

    // 2) 项目清单——播放器就是靠它认出「旁边有项目」
    let manifest_src = project.root().join(PROJECT_FILE_NAME);
    let manifest_dst = output_dir.join(PROJECT_FILE_NAME);
    std::fs::copy(&manifest_src, &manifest_dst).map_err(io_err(&manifest_src))?;

    // 3) 场景与资产目录
    let mut asset_count = 0;
    let mut packed_dirs = Vec::new();
    for relative in content_dirs(project) {
        let src = project.path(&relative);
        if !src.is_dir() {
            continue;
        }
        let dst = output_dir.join(&relative);
        asset_count += copy_dir(&src, &dst)?;
        packed_dirs.push(relative);
    }

    // 4) Rust 插件（P0.5）——固定目录，不由清单声明。
    let plugins = pack_plugins(project.root(), &output_dir)?;
    asset_count += plugins.asset_count;
    if let Some(dir) = plugins.dir {
        packed_dirs.push(dir);
    }
    let warnings = plugins.warnings;

    Ok(BuildReport {
        output_dir,
        executable,
        asset_count,
        packed_dirs,
        warnings,
    })
}

// ---------------------------------------------------------------------------
// 播放器定位
// ---------------------------------------------------------------------------

/// 定位 `xgame-game` 播放器可执行文件。
///
/// 查找顺序：编辑器同目录 → 兄弟 profile 目录（`target/debug` ↔ `target/release`）。
/// 后一条是为了照顾「编辑器用 debug 跑、播放器只构建过 release」这种开发期常见情况。
fn locate_player() -> Result<PathBuf, BuildError> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(PLAYER_EXE));
            if let Some(target_dir) = dir.parent() {
                for profile in ["release", "debug"] {
                    candidates.push(target_dir.join(profile).join(PLAYER_EXE));
                }
            }
        }
    }

    if let Some(found) = candidates.iter().find(|p| p.is_file()) {
        return Ok(found.clone());
    }

    Err(BuildError::PlayerNotFound { searched: candidates })
}

// ---------------------------------------------------------------------------
// 路径处理
// ---------------------------------------------------------------------------

/// 项目里需要打进产物的目录（去重后按名称排序）。
///
/// 取清单里声明的各个内容目录（默认配置下 `textures_dir` / `audio_dir` 与
/// `models_dir` 同为 `Resources`，去重后只留一份），外加**入口场景所在的目录**——
/// 否则把入口场景放在 `Levels/main.ron` 这种自定义位置的项目会漏掉它。
///
/// 清单里的路径是用户可改的，所以这里会过滤掉越界（`..`、绝对路径）的写法。
fn content_dirs(project: &ProjectHandle) -> Vec<String> {
    let manifest = project.manifest();
    let content = &manifest.content;

    let mut dirs: BTreeSet<String> = BTreeSet::new();
    for raw in [
        content.scenes_dir.as_str(),
        content.models_dir.as_str(),
        content.textures_dir.as_str(),
        content.audio_dir.as_str(),
        content.shaders_dir.as_str(),
        content.prefabs_dir.as_str(),
    ] {
        if let Some(dir) = normalize_relative(raw) {
            dirs.insert(dir);
        }
    }

    // 入口场景的父目录
    if let Some(parent) = Path::new(&manifest.entry_scene).parent() {
        let parent = parent.to_string_lossy();
        if let Some(dir) = normalize_relative(parent.as_ref()) {
            dirs.insert(dir);
        }
    }

    dirs.into_iter().collect()
}

/// 把清单里写的相对目录整理成可用的形式；非法或越界的写法返回 `None`。
fn normalize_relative(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches(['/', '\\']);
    if trimmed.is_empty() {
        return None;
    }
    let path = Path::new(trimmed);
    // 只接受纯普通路径段——挡掉 `..`（越界到项目外）和绝对路径（`C:\`、`\`）
    if path
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
    {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// 把项目名收拾成安全的目录名 / 文件名。
fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.').to_string();
    if cleaned.is_empty() {
        // 名字全被过滤光了也得有个能用的东西
        "Game".to_string()
    } else {
        cleaned
    }
}

/// 清空并重建输出目录。
fn prepare_output_dir(dir: &Path) -> Result<(), BuildError> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(io_err(dir))?;
    }
    std::fs::create_dir_all(dir).map_err(io_err(dir))
}

/// 递归复制目录，返回复制的文件数。
fn copy_dir(src: &Path, dst: &Path) -> Result<usize, BuildError> {
    std::fs::create_dir_all(dst).map_err(io_err(dst))?;

    let mut count = 0;
    for entry in std::fs::read_dir(src).map_err(io_err(src))? {
        let entry = entry.map_err(io_err(src))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());

        if entry.file_type().map_err(io_err(&from))?.is_dir() {
            count += copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(io_err(&from))?;
            count += 1;
        }
    }
    Ok(count)
}

/// [`pack_plugins`] 的结果。拆成独立类型只为一件事：让"插件打包"这条逻辑能
/// 脱离播放器可执行文件单独测 —— 完整的 `build()` 要求磁盘上真有 xgame-game，
/// 测试环境里没有。
struct PackedPlugins {
    asset_count: usize,
    /// 实际打包了 `Plugins/` 时是它的名字；没有插件时为 `None`（不进 packed_dirs）。
    dir: Option<String>,
    warnings: Vec<String>,
}

/// 把 `<项目>/Plugins/` 复制进产物目录。
///
/// 播放器启动时只认 `<项目>/Plugins/`，漏了它 = 打包出去的游戏里 Rust 组件全部
/// **静默**失效（场景里挂着 Behaviors 却纹丝不动，没有任何报错）。所以除了打包，
/// 还要在"项目是插件项目却一个 DLL 都没有"时大声警告：那几乎一定是忘了按 ▶ Play
/// 编译，或者编译失败了 —— 一个打包出来的"缺胳膊少腿"的成品，比一次明确的失败
/// 更糟糕（失败你会当场知道，缺件要等玩家反馈）。
///
/// 警告走 [`BuildReport::warnings`] 而不是 `log::warn!`：后者只到 stderr，
/// 而这个警告恰恰是点图形界面按钮的人需要看见的。
fn pack_plugins(project_root: &Path, output_dir: &Path) -> Result<PackedPlugins, BuildError> {
    let plugins_dir = crate::plugin_build::PLUGINS_DIR;
    let src = project_root.join(plugins_dir);
    if src.is_dir() {
        let dst = output_dir.join(plugins_dir);
        return Ok(PackedPlugins {
            asset_count: copy_dir(&src, &dst)?,
            dir: Some(plugins_dir.to_owned()),
            warnings: Vec::new(),
        });
    }
    Ok(PackedPlugins {
        asset_count: 0,
        dir: None,
        warnings: if crate::plugin_build::is_plugin_project(project_root) {
            vec![
                "项目有 Cargo.toml（Rust 插件项目）但 Plugins/ 里没有 DLL —— \
                 打包出去的游戏不会有任何 Rust 组件。回编辑器按 ▶ Play 编译一次再构建。"
                    .to_owned(),
            ]
        } else {
            Vec::new()
        },
    })
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_file_name_strips_illegal_chars() {
        assert_eq!(safe_file_name("My Game"), "My Game");
        assert_eq!(safe_file_name("a/b:c*d"), "abcd");
        assert_eq!(safe_file_name("   "), "Game");
        assert_eq!(safe_file_name("..."), "Game");
    }

    #[test]
    fn normalize_relative_rejects_escapes() {
        assert_eq!(normalize_relative("Scenes"), Some("Scenes".into()));
        assert_eq!(normalize_relative("/Resources/"), Some("Resources".into()));
        assert_eq!(normalize_relative("../outside"), None);
        assert_eq!(normalize_relative("a/../b"), None);
        assert_eq!(normalize_relative(""), None);
        // `C:/Windows` 在 Windows 上是 Prefix + RootDir 段，必须被拒。
        // 非 Windows 上它只是两个普通目录段，这条断言不成立，所以按平台限定。
        #[cfg(windows)]
        assert_eq!(normalize_relative("C:/Windows"), None, "绝对路径不应被接受");
    }

    #[test]
    fn content_dirs_dedupes_and_includes_entry_scene() {
        let base = std::env::temp_dir().join(format!("xgame_build_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        // 造一个真实项目：默认配置下 Shaders 不存在、textures/audio 与 Resources 重合，
        // 正好用来验证去重。
        let mut handle = ProjectHandle::create(&base, "demo").expect("create temp project");
        // 把入口场景挪到自定义目录，验证 content_dirs 会把它带上
        handle.manifest_mut().entry_scene = "Levels/main.ron".into();
        std::fs::create_dir_all(handle.root().join("Levels")).unwrap();

        let dirs = content_dirs(&handle);
        assert!(dirs.contains(&"Scenes".to_string()));
        assert!(dirs.contains(&"Resources".to_string()));
        assert!(dirs.contains(&"Levels".to_string()));
        assert_eq!(
            dirs.iter().filter(|d| d.as_str() == "Resources").count(),
            1,
            "重复的内容目录应被去重"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 三个分支各自成立：有插件就打包、纯数据项目安静跳过、插件项目缺产物要警告。
    ///
    /// 第三条是这条测试的重点 —— "打包出来的成品少了 Rust 组件"这件事本身不会
    /// 报任何错（播放器照常启动、场景照常渲染、组件静默不生效），所以唯一的
    /// 机会就是在这里喊一声。旧实现根本没有这一步，打包出来的游戏会少整个目录。
    #[test]
    fn pack_plugins_copies_dir_and_warns_when_missing() {
        let base = std::env::temp_dir().join(format!("xgame_plugins_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let project = base.join("project");
        let out = base.join("out");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&out).unwrap();

        // 1) 纯数据项目：没有 Cargo.toml、也没有 Plugins/ ⇒ 静默跳过。
        let got = pack_plugins(&project, &out).unwrap();
        assert_eq!(got.asset_count, 0);
        assert!(got.dir.is_none(), "纯数据项目不该出现 Plugins/ 条目");
        assert!(got.warnings.is_empty(), "没有 Cargo.toml 就不该有警告");

        // 2) 插件项目但还没编译：必须给出警告（这是最容易漏的那一步）。
        std::fs::write(project.join("Cargo.toml"), b"[package]\nname=\"x\"\n").unwrap();
        let got = pack_plugins(&project, &out).unwrap();
        assert_eq!(got.warnings.len(), 1, "有 Cargo.toml 却没 DLL 必须警告");
        assert!(got.warnings[0].contains("Plugins/"), "{}", got.warnings[0]);

        // 3) 编译过之后：DLL 被复制进产物目录，警告消失。
        let plugins = project.join(crate::plugin_build::PLUGINS_DIR);
        std::fs::create_dir_all(&plugins).unwrap();
        std::fs::write(plugins.join("demo.dll"), b"bin").unwrap();
        let got = pack_plugins(&project, &out).unwrap();
        assert_eq!(got.asset_count, 1, "应当复制一个 DLL");
        assert_eq!(got.dir.as_deref(), Some(crate::plugin_build::PLUGINS_DIR));
        assert!(got.warnings.is_empty(), "有产物就不该再警告");
        assert!(
            out.join(crate::plugin_build::PLUGINS_DIR)
                .join("demo.dll")
                .is_file(),
            "DLL 必须真的落到产物目录里"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
