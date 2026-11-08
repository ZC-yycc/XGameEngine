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
use std::path::{Path, PathBuf};

use xgame_core::project::{ProjectHandle, PROJECT_FILE_NAME};

/// 构建产物所在的目录名（相对项目根）。
pub const BUILD_DIR: &str = "Build";
/// 同一项目的最终发布锁；文件长期保留，锁随进程句柄自动释放。
const PUBLISH_LOCK_FILE: &str = ".xgame-publish.lock";
/// 发布事务的可恢复旧包；名字稳定，下一进程才能从目录状态判断上次停在哪一步。
const PUBLISH_PREVIOUS_PREFIX: &str = ".xgame-previous-";
/// 先放进旧输出再一起 rename；只有带此空目录的 `previous` 才能由恢复逻辑删除。
const PUBLISH_TRANSACTION_MARKER: &str = ".xgame-publish-transaction";

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
    /// 发布清单里的关卡数（`build.scenes`；未配置清单 = 0）。
    ///
    /// 报告的用途：确认"我配了 3 关，打出来的包里就是 3 关"。
    pub scene_count: usize,
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
            "构建完成 → {}  （{} 个资产文件{}{}）",
            self.output_dir.display(),
            self.asset_count,
            if self.scene_count == 0 {
                String::new()
            } else {
                format!("，{} 关", self.scene_count)
            },
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
    /// 发布清单（`build.scenes`）里有装载不起来的关卡。
    ///
    /// 收**全部**失败项而不是遇到第一个就退：作者一次改三处路径、只想跑一次构建。
    SceneUnloadable { scenes: Vec<String> },
    /// 内容源与构建目录互相包含，或存在无法安全复制的链接。
    PathConflict { source: PathBuf, output: PathBuf },
    /// 播放器存在，但和编辑器不是同一份引擎源码编出来的。
    ///
    /// 为什么直接拒绝而不是警告后继续：拷进去的是一整个引擎，源码不同的播放器
    /// 跑的是**旧的**物理 / 脚本 / 渲染行为。作者在编辑器里验过的效果、修过的 bug，
    /// 到了成品里可能全都不成立 —— 这种"看起来成功、实际是旧引擎"的成品比一次
    /// 明确的失败糟得多。
    PlayerStale { searched: Vec<PathBuf> },
    /// 文件操作失败。
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// 打包后的游戏**缺东西**：源码工程里有、包里没有。
    ///
    /// 为什么直接拒绝而不是警告后继续：复制目录不会报错，清单之外的引用会静默
    /// 落不进包里 —— 作者在编辑器里一切正常，玩家拿到的成品却少一整棵子树 /
    /// 一关黑屏。这种"看起来成功、实际残缺"的产物比一次明确的失败糟得多。
    PackageIncomplete { missing: Vec<String> },
    /// 打包后的产物自己读不出来（入口场景坏掉 / 清单丢了）。
    PackageUnreadable { reason: String },
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
            Self::SceneUnloadable { scenes } => {
                writeln!(
                    f,
                    "发布清单里有 {} 关装载不起来（构建已中止，没有产出半成品）:",
                    scenes.len()
                )?;
                for scene in scenes {
                    writeln!(f, "  - {scene}")?;
                }
                write!(
                    f,
                    "改 `project.ron` 的 `build.scenes`，或用 ⚙ 项目设置里的场景顺序修掉"
                )
            }
            Self::PathConflict { source, output } => write!(
                f,
                "构建路径冲突或不安全的目录链接：源 {}，输出 {}",
                source.display(),
                output.display()
            ),
            Self::PlayerStale { searched } => write!(
                f,
                "找到的播放器 '{}' 与编辑器不是同一份引擎源码编出来的（源码指纹不同），\
                 打包出去的游戏会跑旧引擎。请重新构建播放器：cargo build -p xgame-game\
                 （发布包用 一键构建.cmd）\n已检查过：{}",
                PLAYER_EXE,
                searched
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
            Self::Io { path, source } => {
                write!(f, "文件操作失败 ({}): {source}", path.display())
            }
            Self::PackageIncomplete { missing } => {
                writeln!(
                    f,
                    "打包后的游戏缺少 {} 项引用（源码工程里有、包里没有，玩家会看不到 / 玩不了）:",
                    missing.len()
                )?;
                for line in missing {
                    writeln!(f, "  - {line}")?;
                }
                write!(
                    f,
                    "把文件放进清单里的内容目录（Scenes / Resources / Prefabs / Scripts …），\
                     或在 project.ron 的 content 里声明它们所在的目录，然后重新构建"
                )
            }
            Self::PackageUnreadable { reason } => write!(
                f,
                "打包后的产物读不出来（构建已中止，没有覆盖上一版发布）: {reason}"
            ),
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
/// 先在同盘临时目录完成复制，再替换旧发布目录；复制失败时旧包保持不变。
pub fn build(project: &ProjectHandle) -> Result<BuildReport, BuildError> {
    build_with_player(project, locate_player)
}

fn build_with_player(
    project: &ProjectHandle,
    player: impl FnOnce() -> Result<PlayerPick, BuildError>,
) -> Result<BuildReport, BuildError> {
    let output_dir = project
        .root()
        .join(BUILD_DIR)
        .join(safe_file_name(project.name()));
    let content = &project.manifest().content;
    for raw in [
        &content.scenes_dir,
        &content.models_dir,
        &content.textures_dir,
        &content.audio_dir,
        &content.shaders_dir,
        &content.prefabs_dir,
        &content.scripts_dir,
    ] {
        if !raw.is_empty() && normalize_relative(raw).is_none() {
            return Err(BuildError::PathConflict {
                source: project.path(raw),
                output: output_dir.clone(),
            });
        }
    }
    for raw in &content.search_paths {
        if normalize_relative(raw).is_none() {
            return Err(BuildError::PathConflict {
                source: project.path(raw),
                output: output_dir.clone(),
            });
        }
    }
    let dirs = content_dirs(project);
    if let Some(relative) = dirs.iter().find(|relative| {
        Path::new(relative)
            .components()
            .next()
            .is_some_and(|component| {
                component
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(PUBLISH_TRANSACTION_MARKER)
            })
    }) {
        // 事务标记会短暂放在发布包根；若内容也占这个名字，下一次恢复无法区分
        // 用户资产与引擎状态。宁可在复制前明确拒绝，不在恢复时猜哪一边能删。
        return Err(BuildError::PathConflict {
            source: project.path(relative),
            output: output_dir,
        });
    }
    validate_output(project, &dirs, &output_dir)?;

    // 恢复必须早于播放器查找、场景试装与复制。否则上次恰好死在第一次 rename 后，
    // 下一次构建又因无关的输入错误快速失败，正式输出仍会一直缺席。
    let mut warnings = recover_interrupted_publish(&output_dir)?
        .into_iter()
        .collect::<Vec<_>>();

    // 快速失败：入口场景都不在，构建出来也是个空游戏。
    // 放在动手之前，避免留下一个半成品目录。
    let entry_scene = project.entry_scene_path();
    if !entry_scene.is_file() {
        return Err(BuildError::EntrySceneMissing(entry_scene));
    }

    // 发布清单：逐关**试装**一遍再动手。
    //
    // 为什么值得多读一遍盘：清单指向的关卡少一个文件，构建本身不会报错（它只是
    // 复制目录），玩家却会在切关卡那一刻撞上黑屏。读一遍 RON 是毫秒级的事，
    // 换掉"发布之后才发现"的返工。失败项一次全列出来 —— 作者往往一次改几处。
    let order = project.manifest().scene_order();
    let mut unloadable: Vec<String> = Vec::new();
    for raw in &order {
        // 语法先过一遍：`../Outside.ron` 这种在磁盘上**可能真的存在**，
        // 直接 `load_scene` 会成功，然后被打包进一个指到项目外的路径。
        match xgame_core::project::normalize_scene_relative(raw) {
            Err(e) => unloadable.push(format!("{raw} ({e})")),
            Ok(relative) => {
                if let Err(e) = project.load_scene(&relative) {
                    unloadable.push(format!("{relative} ({e})"));
                }
            }
        }
    }
    if !unloadable.is_empty() {
        return Err(BuildError::SceneUnloadable { scenes: unloadable });
    }

    let pick = player()?;
    let player = pick.path;
    if paths_overlap(&resolved_path(&player)?, &resolved_path(&output_dir)?) {
        return Err(BuildError::PathConflict {
            source: player,
            output: output_dir,
        });
    }
    let staging = Staging::new(output_dir.parent().expect("输出有父目录"))?;
    validate_output(project, &dirs, &staging.path)?;
    let package_dir = staging.path.join("package");
    std::fs::create_dir(&package_dir).map_err(io_err(&package_dir))?;

    // 1) 播放器 → <项目名>.exe
    //    改名的用意：分发出去的成品看起来就是这款游戏本身，
    //    而不是一个叫 xgame-game 的通用运行时。
    let executable = package_dir.join(format!(
        "{}{}",
        safe_file_name(project.name()),
        std::env::consts::EXE_SUFFIX
    ));
    std::fs::copy(&player, &executable).map_err(io_err(&executable))?;

    // 2) 项目清单——播放器就是靠它认出「旁边有项目」
    let manifest_src = project.root().join(PROJECT_FILE_NAME);
    let manifest_dst = package_dir.join(PROJECT_FILE_NAME);
    std::fs::copy(&manifest_src, &manifest_dst).map_err(io_err(&manifest_src))?;

    // 3) 场景与资产目录
    let mut asset_count = 0;
    let mut packed_dirs = Vec::new();
    let input = project.root().join("input.ron");
    match std::fs::symlink_metadata(&input) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                // 内容目录拒绝链接，根目录下的输入文件也不能跟到工程外读取。
                return Err(BuildError::PathConflict {
                    source: input,
                    output: package_dir.join("input.ron"),
                });
            }
            let packed_input = package_dir.join("input.ron");
            std::fs::copy(&input, &packed_input).map_err(io_err(&input))?;
            // 检查包内实际字节：复制前检查源文件仍可能在两步之间被编辑。
            // 运行时解析失败会退回默认绑定，成品能启动但自定义操作失效。
            match xgame_input::InputMap::load(&packed_input) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Err(BuildError::PackageUnreadable {
                        reason: "input.ron: 已复制的文件不存在".to_string(),
                    });
                }
                Err(error) => {
                    return Err(BuildError::PackageUnreadable {
                        reason: format!("input.ron: {error}"),
                    });
                }
            }
            asset_count += 1;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(&input)(e)),
    }
    for relative in dirs {
        let src = project.path(&relative);
        if !src.is_dir() {
            continue;
        }
        let dst = package_dir.join(&relative);
        asset_count += copy_dir(&src, &dst)?;
        packed_dirs.push(relative);
    }

    // 4) Rust 插件（P0.5）——固定目录，不由清单声明。
    let plugins = pack_plugins(project.root(), &package_dir)?;
    asset_count += plugins.asset_count;
    if let Some(dir) = plugins.dir {
        packed_dirs.push(dir);
    }
    warnings.extend(pick.warnings);
    warnings.extend(plugins.warnings);

    // 5) 发布前自检：包里不能比源码工程少引用。
    //    放在 `publish` 之前 —— 失败时上一版发布目录原封不动。
    audit_package(project, &package_dir)?;

    let executable = output_dir.join(executable.file_name().expect("播放器有文件名"));
    staging.publish(&package_dir, &output_dir, &mut warnings)?;

    Ok(BuildReport {
        output_dir,
        executable,
        asset_count,
        packed_dirs,
        scene_count: order.len(),
        warnings,
    })
}

/// 打包完成前的自检：**包里不能比源码工程少引用**。
///
/// 复制目录本身不会报错，所以清单之外的引用（预制体放在 `Assets/`、脚本放在
/// 自定义目录、关卡放在 `Extra/`）会静默落不进包里。前面几步已经补上了
/// "清单里每一关的父目录"，这条则是兜底 —— 任何形态的清单外依赖都在这里现形。
///
/// 判据是**构建有没有引入新问题**，不是"工程有没有问题"：打包前后一样缺的东西
/// 归 `--check` 管，一条早就存在的坏引用不该让作者连包都打不出来。
fn audit_package(project: &ProjectHandle, package_dir: &Path) -> Result<(), BuildError> {
    let packaged = xgame_core::check_project(package_dir)
        .map_err(|reason| BuildError::PackageUnreadable { reason })?;
    // 源码工程读不出来（`--check` 会硬失败那种）时不当基线：构建已经逐关试装过，
    // 硬把它当成"基线为空"只会把每一行都报成构建漏的。
    let baseline = xgame_core::check_project(project.root())
        .map(|report| report.reference_issues())
        .unwrap_or_default();
    let missing: Vec<String> = packaged
        .reference_issues()
        .into_iter()
        .filter(|line| !baseline.contains(line))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(BuildError::PackageIncomplete {
        missing: explain_guids(project, missing),
    })
}

/// 把报错里的 `guid://…` 补回"源码工程里它指向哪个文件"。
///
/// 场景存的是身份（GUID），玩家看不到的东西作者也看不出：只说"悬空 GUID
/// `cf757711…`"等于没说 —— 那句话既不能定位文件，也不能提示怎么修。这里按
/// **源码工程**的资产库翻译回路径，作者一眼就知道是哪个文件没进包。
fn explain_guids(project: &ProjectHandle, missing: Vec<String>) -> Vec<String> {
    use xgame_core::AssetGuid;

    let db = project.assets();
    missing
        .into_iter()
        .map(|line| {
            let Some(start) = line.find("guid://") else {
                return line;
            };
            let raw = &line[start + "guid://".len()..];
            let Some(guid) = AssetGuid::parse(raw.get(..32).unwrap_or(raw)) else {
                return line;
            };
            // `db` 只在这一行里用一次：拿到路径就放锁，别把 guard 带出闭包。
            match db.path_of(&guid).map(str::to_owned) {
                Some(path) => format!("{line}  （源码工程里是 {path}，它没进包）"),
                None => line,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 播放器定位
// ---------------------------------------------------------------------------

/// 挑中的播放器 + 打包时要一起报出去的提醒。
#[derive(Debug)]
pub(crate) struct PlayerPick {
    pub path: PathBuf,
    /// 例如"同目录那个是旧引擎，改用了 target/release 下的"。走
    /// [`BuildReport::warnings`]，因为点按钮的人看不到 stderr。
    pub warnings: Vec<String>,
}

impl PlayerPick {
    /// 测试用：认一个播放器，不产生提醒。
    #[cfg(test)]
    fn plain(path: PathBuf) -> Self {
        Self {
            path,
            warnings: Vec::new(),
        }
    }
}

/// 定位 `xgame-game` 播放器可执行文件。
///
/// 查找顺序：编辑器同目录 → 兄弟 profile 目录（`target/debug` ↔ `target/release`）。
/// 后一条是为了照顾「编辑器用 debug 跑、播放器只构建过 release」这种开发期常见情况。
fn locate_player() -> Result<PlayerPick, BuildError> {
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

    pick_player(&candidates)
}

/// 在候选里挑**引擎源码一致**的播放器。
///
/// 只看"文件存在"是不够的：`dist/xgame-game.exe` 与 `target/release` 里的那份都
/// 可能比当前源码旧。改了物理 / 脚本又只重编了编辑器，打包就会把一个旧引擎塞进
/// 成品，而且没有任何征兆。判据是播放器二进制里嵌着的源码指纹（见
/// [`xgame::plugin::SOURCE_FINGERPRINT`]），它随可执行文件走，拷到哪都不会失效。
///
/// 顺序仍然是"先找到的先用"，只是**跳过**源码不一致的：所以 debug 编辑器配
/// release 播放器照旧能打包，而旧引擎的那份会被越过并留一条提醒。
fn pick_player(candidates: &[PathBuf]) -> Result<PlayerPick, BuildError> {
    let existing: Vec<PathBuf> = candidates
        .iter()
        .filter(|path| path.is_file())
        .cloned()
        .collect();
    if existing.is_empty() {
        return Err(BuildError::PlayerNotFound {
            searched: candidates.to_vec(),
        });
    }

    let mut skipped: Vec<PathBuf> = Vec::new();
    for path in &existing {
        // 读不出来（权限 / 正在被占用）就当它不可用：宁可报"都不是同一份源码"，
        // 也不要凭文件存在就把一个来路不明的二进制打进成品。
        let matches = std::fs::read(path)
            .map(|bytes| xgame::plugin::embeds_source_fingerprint(&bytes))
            .unwrap_or(false);
        if matches {
            let warnings = if skipped.is_empty() {
                Vec::new()
            } else {
                vec![format!(
                    "跳过了与引擎源码不一致的播放器：{}（用的是 {}）",
                    skipped
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(" | "),
                    path.display()
                )]
            };
            return Ok(PlayerPick {
                path: path.clone(),
                warnings,
            });
        }
        skipped.push(path.clone());
    }

    Err(BuildError::PlayerStale { searched: existing })
}

// ---------------------------------------------------------------------------
// 路径处理
// ---------------------------------------------------------------------------

/// 项目里需要打进产物的目录（去重后按名称排序）。
///
/// 取清单里声明的各个内容目录和 `search_paths`（默认配置下 `textures_dir` /
/// `audio_dir` 与 `models_dir` 同为 `Resources`，去重后只留一份），外加**入口场景
/// 所在的目录**——否则把入口场景放在 `Levels/main.ron` 这种自定义位置的项目会漏掉它。
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
        content.scripts_dir.as_str(),
    ] {
        if let Some(dir) = normalize_relative(raw) {
            dirs.insert(dir);
        }
    }
    // 运行时能从兜底根读到的模型、脚本与音频也必须随包走；否则源码工程正常，
    // 发布产物却会在第一次读取时丢资产。非法条目由构建入口拒绝，这里仍只收合法项。
    for raw in &content.search_paths {
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

    // 发布清单（`build.scenes`）里每一关的父目录。
    //
    // 关卡可以放在 `scenes_dir` 之外（`Extra/boss.ron`）——`--check` 认这种写法，
    // 构建却不认的话就会出现"编辑器里能玩、成品里第二关黑屏"。
    for raw in &manifest.build.scenes {
        let normalized = raw.trim().replace('\\', "/");
        if let Some(parent) = Path::new(&normalized).parent() {
            let parent = parent.to_string_lossy();
            if let Some(dir) = normalize_relative(parent.as_ref()) {
                dirs.insert(dir);
            }
        }
    }

    dirs.into_iter().collect()
}

/// 把清单里写的相对目录整理成可用的形式；非法或越界的写法返回 `None`。
///
/// 判定**只有一处**：`xgame_core::normalize_project_relative`（场景 / 资产 /
/// 清单三条入口共用）。这里曾经是第四份手写的路径规则 —— 只挡 `..`、绝对路径
/// 和冒号，于是 `Scenes/foo ` 这种 Windows 会静默改名的目录照样进包。
fn normalize_relative(raw: &str) -> Option<String> {
    xgame_core::normalize_project_relative(raw).ok()
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

// 尚不存在的输出也要解析已有祖先，避免 Build 目录链接绕过检查。
fn resolved_path(path: &Path) -> Result<PathBuf, BuildError> {
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // 悬空链接不能当作尚未创建的普通目录。
            if std::fs::symlink_metadata(path).is_ok() {
                return Err(io_err(path)(e));
            }
            let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
            let parent = match parent {
                Some(parent) => resolved_path(parent)?,
                None => std::env::current_dir().map_err(io_err(path))?,
            };
            Ok(parent.join(path.file_name().ok_or_else(|| io_err(path)(e))?))
        }
        Err(e) => Err(io_err(path)(e)),
    }
}

fn paths_overlap(a: &Path, b: &Path) -> bool {
    // Windows 的路径比较还需处理尚不存在的路径段的大小写。
    #[cfg(windows)]
    let (a, b) = (
        PathBuf::from(a.to_string_lossy().to_lowercase()),
        PathBuf::from(b.to_string_lossy().to_lowercase()),
    );
    #[cfg(windows)]
    let (a, b) = (a.as_path(), b.as_path());
    a.starts_with(b) || b.starts_with(a)
}

fn validate_output(
    project: &ProjectHandle,
    dirs: &[String],
    output: &Path,
) -> Result<(), BuildError> {
    let destination = resolved_path(output)?;
    let root = resolved_path(project.root())?;
    // 项目根可以包含默认输出，但输出绝不能包含项目根。
    if root.starts_with(&destination) {
        return Err(BuildError::PathConflict {
            source: root,
            output: output.into(),
        });
    }
    for relative in dirs.iter().map(String::as_str).chain([
        crate::plugin_build::PLUGINS_DIR,
        PROJECT_FILE_NAME,
        "input.ron",
    ]) {
        let source = resolved_path(&project.path(relative))?;
        if paths_overlap(&source, &destination) {
            return Err(BuildError::PathConflict {
                source,
                output: output.into(),
            });
        }
    }
    Ok(())
}

/// 只靠随机 staging 无法恢复强杀后的状态：下一进程必须能找到上一事务留下的旧包。
fn previous_release_path(output: &Path) -> PathBuf {
    let mut name = std::ffi::OsString::from(PUBLISH_PREVIOUS_PREFIX);
    name.push(output.file_name().expect("构建输出必须有目录名"));
    output.parent().expect("构建输出必须有父目录").join(name)
}

struct PublishLock {
    _file: std::fs::File,
}

impl PublishLock {
    fn acquire(output: &Path) -> Result<Self, BuildError> {
        let lock_path = output
            .parent()
            .expect("构建输出必须有父目录")
            .join(PUBLISH_LOCK_FILE);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(io_err(&lock_path))?;
        file.lock().map_err(io_err(&lock_path))?;
        Ok(Self { _file: file })
    }
}

/// 检查内部事务路径；链接和普通文件都不能被当成引擎创建的发布目录删除。
fn release_dir_exists(path: &Path, output: &Path) -> Result<bool, BuildError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(BuildError::PathConflict {
            source: path.into(),
            output: output.into(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io_err(path)(e)),
    }
}

fn transaction_marker_exists(release: &Path, output: &Path) -> Result<bool, BuildError> {
    let marker = release.join(PUBLISH_TRANSACTION_MARKER);
    match std::fs::symlink_metadata(&marker) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
            let mut entries = std::fs::read_dir(&marker).map_err(io_err(&marker))?;
            match entries.next() {
                None => Ok(true),
                Some(Ok(_)) => Err(BuildError::PathConflict {
                    source: marker,
                    output: output.into(),
                }),
                Some(Err(e)) => Err(io_err(marker)(e)),
            }
        }
        Ok(_) => Err(BuildError::PathConflict {
            source: marker,
            output: output.into(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io_err(marker)(e)),
    }
}

fn remove_transaction_marker(release: &Path, output: &Path) -> Result<(), BuildError> {
    let marker = release.join(PUBLISH_TRANSACTION_MARKER);
    if transaction_marker_exists(release, output)? {
        // 只删空目录：即使有人手工造了同名目录并放入内容，也不能递归误删。
        std::fs::remove_dir(&marker).map_err(io_err(marker))?;
    }
    Ok(())
}

/// 新包提交后先把旧包移回唯一 staging，再递归删除。
///
/// `remove_dir_all(previous)` 若被强杀，可能先删掉事务标记、留下不可判别的半个目录；
/// staging 即使清到一半也不参与下一次发布判定，最多成为可人工清理的构建残留。
fn retire_previous_into(previous: &Path, staging: &Path) -> Result<Option<String>, BuildError> {
    let retired = staging.join("previous");
    std::fs::rename(previous, &retired).map_err(io_err(previous))?;
    Ok(std::fs::remove_dir_all(&retired).err().map(|e| {
        format!(
            "旧包已移出发布状态，但清理失败 ({}): {e}",
            retired.display()
        )
    }))
}

/// 在持有发布锁时把上一次中断归并回两种稳定状态：只有正式输出，或两者都没有。
///
/// - 只有 `previous`：死在旧包移走后，恢复旧包；
/// - `output + previous`：新包已完成原子改名，保留新包并清理旧包。
fn recover_interrupted_publish_locked(output: &Path) -> Result<Option<String>, BuildError> {
    let previous = previous_release_path(output);
    let output_exists = release_dir_exists(output, output)?;
    let previous_exists = release_dir_exists(&previous, output)?;
    match (output_exists, previous_exists) {
        (false, true) => {
            if !transaction_marker_exists(&previous, output)? {
                return Err(BuildError::PathConflict {
                    source: previous,
                    output: output.into(),
                });
            }
            std::fs::rename(&previous, output).map_err(io_err(&previous))?;
            remove_transaction_marker(output, output)?;
            Ok(Some(format!(
                "检测到上次发布在旧包移走后中断，已恢复 {}",
                output.display()
            )))
        }
        (true, true) => {
            if transaction_marker_exists(output, output)?
                || !transaction_marker_exists(&previous, output)?
            {
                return Err(BuildError::PathConflict {
                    source: previous,
                    output: output.into(),
                });
            }
            let cleanup = Staging::new(output.parent().expect("构建输出必须有父目录"))?;
            let cleanup_warning = retire_previous_into(&previous, &cleanup.path)?;
            let mut message = format!("检测到上次发布已完成，已归档旧包 {}", previous.display());
            if let Some(warning) = cleanup_warning {
                message.push_str(&format!("；{warning}"));
            }
            Ok(Some(message))
        }
        (true, false) if transaction_marker_exists(output, output)? => {
            remove_transaction_marker(output, output)?;
            Ok(Some(format!(
                "检测到上次发布在旧包移动前中断，已保留 {}",
                output.display()
            )))
        }
        (false, false) | (true, false) => Ok(None),
    }
}

/// 构建开始时先恢复，不让后续无关的预检失败把正式输出继续留在缺席状态。
fn recover_interrupted_publish(output: &Path) -> Result<Option<String>, BuildError> {
    let parent = output.parent().expect("构建输出必须有父目录");
    match std::fs::symlink_metadata(parent) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_err(parent)(e)),
        Ok(_) => {}
    }
    let _lock = PublishLock::acquire(output)?;
    recover_interrupted_publish_locked(output)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PublishBoundary {
    TransactionMarked,
    PreviousMoved,
    PackageMoved,
}

struct Staging {
    path: PathBuf,
}

impl Staging {
    fn new(parent: &Path) -> Result<Self, BuildError> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::fs::create_dir_all(parent).map_err(io_err(parent))?;
        loop {
            let path = parent.join(format!(
                ".xgame-staging-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(io_err(path)(e)),
            }
        }
    }

    fn publish(
        self,
        package: &Path,
        output: &Path,
        warnings: &mut Vec<String>,
    ) -> Result<(), BuildError> {
        self.publish_inner(package, output, warnings, |_| {})
    }

    fn publish_inner(
        self,
        package: &Path,
        output: &Path,
        warnings: &mut Vec<String>,
        mut boundary: impl FnMut(PublishBoundary),
    ) -> Result<(), BuildError> {
        // 每个构建有自己的 staging，所以复制与自检可以并行；但下面三步是一个整体：
        // 旧包移走 → 新包就位 → 旧包清理。两个进程交错执行时，其中一个会把另一个
        // 的新包当成 previous，失败回滚还可能撞上已经存在的输出目录。
        //
        // 锁文件不能在解锁后删除：等待者锁住的是旧句柄，此时第三个进程若创建同名
        // 新文件，就能拿到另一把锁并闯进临界区。长期保留文件、只让句柄代表所有权，
        // 进程崩溃后操作系统也会自动释放，不会留下“死锁文件”。
        let _lock = PublishLock::acquire(output)?;
        if let Some(message) = recover_interrupted_publish_locked(output)? {
            warnings.push(message);
        }

        let backup = previous_release_path(output);
        let had_previous = release_dir_exists(output, output)?;
        if had_previous {
            let marker = output.join(PUBLISH_TRANSACTION_MARKER);
            std::fs::create_dir(&marker).map_err(io_err(&marker))?;
            boundary(PublishBoundary::TransactionMarked);
            if let Err(e) = std::fs::rename(output, &backup) {
                if let Err(cleanup) = remove_transaction_marker(output, output) {
                    return Err(io_err(output)(std::io::Error::other(format!(
                        "旧包移动失败：{e}；事务标记清理失败：{cleanup}"
                    ))));
                }
                return Err(io_err(output)(e));
            }
            boundary(PublishBoundary::PreviousMoved);
        }
        if let Err(e) = std::fs::rename(package, output) {
            if had_previous {
                if let Err(rollback) = std::fs::rename(&backup, output) {
                    return Err(io_err(&backup)(std::io::Error::other(format!(
                        "发布失败：{e}；恢复失败：{rollback}；旧发布保留在 {}",
                        backup.display()
                    ))));
                }
                if let Err(cleanup) = remove_transaction_marker(output, output) {
                    return Err(io_err(output)(std::io::Error::other(format!(
                        "发布失败：{e}；旧包已恢复，但事务标记清理失败：{cleanup}"
                    ))));
                }
            }
            return Err(io_err(output)(e));
        }
        boundary(PublishBoundary::PackageMoved);
        if had_previous {
            match retire_previous_into(&backup, &self.path) {
                Ok(Some(warning)) => warnings.push(format!("发布成功，但{warning}")),
                Ok(None) => {}
                Err(e) => warnings.push(format!(
                    "发布成功，但旧包未能移出事务路径 ({}): {e}",
                    backup.display()
                )),
            }
        }
        Ok(())
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// 递归复制目录，返回复制的文件数。
fn copy_dir(src: &Path, dst: &Path) -> Result<usize, BuildError> {
    let metadata = std::fs::symlink_metadata(src).map_err(io_err(src))?;
    if metadata.file_type().is_symlink()
        || paths_overlap(&resolved_path(src)?, &resolved_path(dst)?)
    {
        return Err(BuildError::PathConflict {
            source: src.into(),
            output: dst.into(),
        });
    }
    std::fs::create_dir_all(dst).map_err(io_err(dst))?;

    let mut count = 0;
    for entry in std::fs::read_dir(src).map_err(io_err(src))? {
        let entry = entry.map_err(io_err(src))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());

        let kind = entry.file_type().map_err(io_err(&from))?;
        if kind.is_symlink() {
            return Err(BuildError::PathConflict {
                source: from,
                output: to,
            });
        }
        if kind.is_dir() {
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

    struct Fixture {
        base: PathBuf,
        project: ProjectHandle,
        player: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let base = std::env::temp_dir().join(format!(
                "xgame_builder_{}_{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&base).unwrap();
            let project = ProjectHandle::create(&base, "demo").unwrap();
            let player = base.join(PLAYER_EXE);
            std::fs::write(&player, b"player").unwrap();
            Self {
                base,
                project,
                player,
            }
        }

        fn output(&self) -> PathBuf {
            self.project.root().join("Build/demo")
        }

        fn build(&self) -> Result<BuildReport, BuildError> {
            // 测试不经过 `locate_player`：那要求磁盘上真有同一份源码编出来的
            // xgame-game。指纹判据由 `pick_player` 的定向测试单独钉住。
            build_with_player(&self.project, || Ok(PlayerPick::plain(self.player.clone())))
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn package_includes_scripts_and_input() {
        for scripts in ["Scripts", "Logic/Lua"] {
            let mut fixture = Fixture::new();
            fixture.project.manifest_mut().content.scripts_dir = scripts.into();
            fixture.project.save().unwrap();
            let src = fixture.project.root().join(scripts);
            std::fs::create_dir_all(src.join("nested")).unwrap();
            std::fs::write(src.join("nested/main.lua"), b"return 42").unwrap();
            let input = b"(actions: {\"Jump\": [Key(Space)]})";
            std::fs::write(fixture.project.root().join("input.ron"), input).unwrap();
            let report = fixture.build().unwrap();
            assert_eq!(
                std::fs::read(report.output_dir.join(scripts).join("nested/main.lua")).unwrap(),
                b"return 42"
            );
            assert_eq!(
                std::fs::read(report.output_dir.join("input.ron")).unwrap(),
                input
            );
            assert!(report.packed_dirs.contains(&scripts.to_string()));
            assert!(report.executable.is_file());
        }
    }

    /// `search_paths` 是运行时可读根；只把传统内容目录复制进包，会让源码工程
    /// 能加载的模型、脚本和音频在发布后一起消失。
    #[test]
    fn package_includes_search_path_roots() {
        let mut fixture = Fixture::new();
        fixture.project.manifest_mut().content.search_paths = vec!["Mods".into()];
        fixture.project.save().unwrap();

        let shared = fixture.project.path("Mods/Scripts/Shared.lua");
        std::fs::create_dir_all(shared.parent().unwrap()).unwrap();
        std::fs::write(&shared, b"return 42").unwrap();

        let report = fixture.build().expect("合法搜索根必须随发布包复制");
        assert_eq!(
            std::fs::read(report.output_dir.join("Mods/Scripts/Shared.lua")).unwrap(),
            b"return 42"
        );
        assert!(
            report.packed_dirs.contains(&"Mods".to_string()),
            "目录清单要如实报告: {:?}",
            report.packed_dirs
        );
    }

    #[test]
    fn package_preserves_custom_input_bytes() {
        let fixture = Fixture::new();
        let bytes = b"(actions: {\"jump\": [Key(Space)]})";
        std::fs::write(fixture.project.root().join("input.ron"), bytes).unwrap();
        let report = fixture.build().unwrap();
        assert_eq!(
            std::fs::read(report.output_dir.join("input.ron")).unwrap(),
            bytes
        );
    }

    /// 玩家端加载坏映射会退回默认键位；发布必须在覆盖旧成品之前发现它。
    #[test]
    fn malformed_input_does_not_replace_previous_release() {
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.output()).unwrap();
        std::fs::write(fixture.output().join("old.bin"), b"old").unwrap();
        std::fs::write(fixture.project.root().join("input.ron"), b"(actions: {").unwrap();

        let error = fixture.build().expect_err("坏输入映射不能发布");
        assert!(error.to_string().contains("input.ron"), "{error}");
        assert_eq!(
            std::fs::read(fixture.output().join("old.bin")).unwrap(),
            b"old"
        );
        assert!(!fixture.output().join("demo.exe").exists());
    }

    /// 复制独立 input.ron 时也要遵守内容目录已有的链接拒绝规则。
    #[cfg(any(windows, unix))]
    #[test]
    fn linked_input_is_not_copied_into_release() {
        let fixture = Fixture::new();
        let outside = fixture.base.join("external-input.ron");
        std::fs::write(&outside, b"(actions: {})").unwrap();
        let input = fixture.project.root().join("input.ron");
        #[cfg(windows)]
        let result = std::os::windows::fs::symlink_file(&outside, &input);
        #[cfg(unix)]
        let result = std::os::unix::fs::symlink(&outside, &input);
        if let Err(error) = result {
            // 没有开发者模式 / 管理员权限时 Windows 回 `ERROR_PRIVILEGE_NOT_HELD`
            // （1314）。它在新版 std 里**不是** `PermissionDenied` 而是
            // `Uncategorized`，只判 kind 会让这台机器上整条测试红掉 ——
            // 而"没权限建链接"本来就该跳过，不该失败。
            let no_privilege = error.kind() == std::io::ErrorKind::PermissionDenied
                || cfg!(windows) && error.raw_os_error() == Some(1314);
            if no_privilege {
                eprintln!("当前环境无创建符号链接权限，跳过链接夹具: {error}");
                return;
            }
            panic!("创建链接夹具失败: {error}");
        }
        assert!(matches!(
            fixture.build(),
            Err(BuildError::PathConflict { .. })
        ));
        assert!(!fixture.output().exists());
    }

    // -- 清单外依赖：包里必须自带场景引用的东西 --------------------------------

    /// `build.scenes` 里某一关放在内容目录之外（`Extra/boss.ron`）时也必须进包。
    ///
    /// 否则玩家切到第二关那一刻黑屏 —— 而编辑器里一切正常，`--check` 也只查
    /// 磁盘上的源码工程，看不出"发布包少了一个文件"。
    #[test]
    fn levels_outside_content_dirs_are_packaged() {
        let mut fixture = Fixture::new();
        let scene = fixture.project.load_entry_scene().unwrap();
        fixture
            .project
            .write_scene("Extra/boss.ron", &scene)
            .unwrap();
        fixture.project.manifest_mut().build.scenes = vec![
            xgame_core::DEFAULT_ENTRY_SCENE.to_string(),
            "Extra/boss.ron".into(),
        ];
        fixture.project.save().unwrap();

        let report = fixture.build().expect("清单里的关卡也该能打包");
        assert!(
            report.output_dir.join("Extra/boss.ron").is_file(),
            "清单里的关卡必须进包，实际打了: {:?}",
            report.packed_dirs
        );
        assert!(
            report.packed_dirs.contains(&"Extra".to_string()),
            "目录清单要如实报告: {:?}",
            report.packed_dirs
        );
    }

    /// 场景引用了内容目录之外的预制体：源码里有、包里没有 —— 构建必须失败。
    ///
    /// 这是"清单外依赖"最隐蔽的形态：复制目录不会报错，编辑器里一切正常，
    /// 打包出来的游戏却少一整棵子树。宁可明确失败，也不要发一个残缺的包。
    #[test]
    fn build_rejects_a_package_that_lost_references() {
        let fixture = Fixture::new();
        let mut prefab = xgame_core::prefab::Prefab::new("Loose");
        prefab.entities.push(xgame_core::Entity::new("Root"));
        fixture
            .project
            .write_prefab("Assets/Loose.prefab.ron", &prefab)
            .unwrap();
        let mut scene = fixture.project.load_entry_scene().unwrap();
        scene.instantiate_prefab(&prefab, "Assets/Loose.prefab.ron");
        fixture
            .project
            .write_scene(xgame_core::DEFAULT_ENTRY_SCENE, &scene)
            .unwrap();

        let error = fixture.build().expect_err("包里缺引用不能静默通过");
        let text = error.to_string();
        assert!(
            text.contains("Assets/Loose.prefab.ron"),
            "要点名缺了什么: {text}"
        );
        assert!(
            !fixture.output().join("project.ron").exists(),
            "失败不该留下半成品"
        );
    }

    /// 资产只由预制体持有时仍会被实例化读取，不能让清单外资源从成品中消失。
    #[test]
    fn build_rejects_prefab_only_dependency_missing_from_package() {
        let fixture = Fixture::new();
        let resource = fixture.project.path("Loose/Only.glb");
        std::fs::create_dir_all(resource.parent().unwrap()).unwrap();
        std::fs::write(&resource, b"model bytes").unwrap();
        let mut prefab = xgame_core::prefab::Prefab::new("Only");
        prefab
            .entities
            .push(xgame_core::Entity::new("OnlyChild").with_model("Loose/Only.glb"));
        fixture
            .project
            .write_prefab("Prefabs/Only.prefab.ron", &prefab)
            .unwrap();

        let error = fixture.build().expect_err("预制体独有依赖丢失应阻断发布");
        assert!(
            error.to_string().contains("Loose/Only.glb"),
            "要指向丢失的文件：{error}"
        );
        assert!(
            !fixture.output().join("project.ron").exists(),
            "失败后不得留下残缺成品"
        );
    }

    /// 原工程已有的坏文件在包里字节相同，不应因 staging 绝对路径不同而拒绝发布。
    #[test]
    fn build_accepts_existing_invalid_scene_and_material() {
        let fixture = Fixture::new();
        let mut scene = fixture.project.load_entry_scene().unwrap();
        let mut entity = xgame_core::Entity::new("BadMaterial");
        entity.material_asset = Some("Resources/Materials/Bad.mat".into());
        scene.entities.push(entity);
        fixture
            .project
            .write_scene(xgame_core::DEFAULT_ENTRY_SCENE, &scene)
            .unwrap();
        std::fs::write(fixture.project.path("Scenes/Bad.ron"), "(not valid RON").unwrap();
        let material = fixture.project.path("Resources/Materials/Bad.mat");
        std::fs::create_dir_all(material.parent().unwrap()).unwrap();
        std::fs::write(material, "(not valid RON").unwrap();

        let report = fixture
            .build()
            .expect("源码与成品的同一坏文件不能被当成新问题");
        let source = xgame_core::check_project(fixture.project.root()).unwrap();
        let packaged = xgame_core::check_project(&report.output_dir).unwrap();
        assert_eq!(source.invalid_scenes.len(), 1);
        assert_eq!(source.invalid_materials.len(), 1);
        assert_eq!(source.reference_issues(), packaged.reference_issues());
    }

    /// 源码工程**本来就缺**的资产只提醒、不挡构建。
    ///
    /// 判据是"构建有没有引入新问题"：打包前后一样缺的东西归 `--check` 管，
    /// 一条早就存在的坏引用不该让作者连包都打不出来。
    #[test]
    fn already_missing_assets_do_not_block_the_build() {
        let fixture = Fixture::new();
        let mut scene = fixture.project.load_entry_scene().unwrap();
        scene.entities[0].model = "Resources/Nope.glb".to_string();
        fixture
            .project
            .write_scene(xgame_core::DEFAULT_ENTRY_SCENE, &scene)
            .unwrap();

        let report = fixture.build().expect("源码本来就缺的资产不该挡构建");
        assert!(report.output_dir.join("project.ron").is_file());
    }

    #[test]
    fn copy_failure_preserves_previous_release() {
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.output()).unwrap();
        std::fs::write(fixture.output().join("old.bin"), b"old release").unwrap();
        // 播放器复制完成后，清单复制失败，不能先删除旧发布目录。
        std::fs::remove_file(fixture.project.root().join(PROJECT_FILE_NAME)).unwrap();
        assert!(fixture.build().is_err());
        assert_eq!(
            std::fs::read(fixture.output().join("old.bin")).unwrap(),
            b"old release"
        );
        assert_eq!(
            std::fs::read_dir(fixture.output().parent().unwrap())
                .unwrap()
                .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_dir())
                .count(),
            1
        );
    }

    #[test]
    fn rebuild_replaces_release_and_optional_input_can_be_absent() {
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.output()).unwrap();
        std::fs::write(fixture.output().join("stale.bin"), b"old").unwrap();
        fixture.build().unwrap();
        assert!(!fixture.output().join("stale.bin").exists());
        assert!(!fixture.output().join("input.ron").exists());
        assert!(
            fixture
                .output()
                .parent()
                .unwrap()
                .join(PUBLISH_LOCK_FILE)
                .is_file(),
            "成功发布后应留下稳定锁节点"
        );
        assert_eq!(
            std::fs::read_dir(fixture.output().parent().unwrap())
                .unwrap()
                .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_dir())
                .count(),
            1
        );
    }

    #[test]
    fn output_inside_source_or_source_inside_output_is_rejected() {
        for source in ["Build", "Build/demo", "Build/demo/assets"] {
            let mut fixture = Fixture::new();
            fixture.project.manifest_mut().content.models_dir = source.into();
            let src = fixture.project.root().join(source);
            std::fs::create_dir_all(&src).unwrap();
            std::fs::write(src.join("keep.bin"), b"source").unwrap();
            assert!(fixture.build().is_err(), "必须拒绝源目录 {source}");
            assert_eq!(std::fs::read(src.join("keep.bin")).unwrap(), b"source");
        }
    }

    #[test]
    fn failed_publish_restores_previous_release() {
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.output()).unwrap();
        std::fs::write(fixture.output().join("old.bin"), b"old").unwrap();
        let staging = Staging::new(fixture.output().parent().unwrap()).unwrap();
        let missing_package = staging.path.join("missing");
        assert!(staging
            .publish(&missing_package, &fixture.output(), &mut Vec::new())
            .is_err());
        assert_eq!(
            std::fs::read(fixture.output().join("old.bin")).unwrap(),
            b"old"
        );
    }

    #[test]
    fn interrupted_after_previous_move_is_recovered_before_publish_retry() {
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.output()).unwrap();
        std::fs::write(fixture.output().join("old.bin"), b"old").unwrap();

        // 模拟进程死在“旧包移走”之后：下一次发布即使自己的 package 也有问题，
        // 仍必须先把上一版恢复到正式输出，不能让 Build/demo 永久消失。
        let recovery = fixture
            .output()
            .parent()
            .unwrap()
            .join(".xgame-previous-demo");
        std::fs::rename(fixture.output(), &recovery).unwrap();
        std::fs::create_dir(recovery.join(PUBLISH_TRANSACTION_MARKER)).unwrap();
        let staging = Staging::new(fixture.output().parent().unwrap()).unwrap();
        let missing_package = staging.path.join("missing");
        assert!(staging
            .publish(&missing_package, &fixture.output(), &mut Vec::new())
            .is_err());

        assert_eq!(
            std::fs::read(fixture.output().join("old.bin")).unwrap(),
            b"old"
        );
        assert!(!recovery.exists(), "恢复完成后不该留下事务旧包");
        assert!(!fixture.output().join(PUBLISH_TRANSACTION_MARKER).exists());
    }

    #[test]
    fn interrupted_after_package_move_keeps_new_release_and_cleans_previous() {
        let fixture = Fixture::new();
        let recovery = fixture
            .output()
            .parent()
            .unwrap()
            .join(".xgame-previous-demo");
        std::fs::create_dir_all(&recovery).unwrap();
        std::fs::write(recovery.join("old.bin"), b"old").unwrap();
        std::fs::create_dir(recovery.join(PUBLISH_TRANSACTION_MARKER)).unwrap();
        std::fs::create_dir_all(fixture.output()).unwrap();
        std::fs::write(fixture.output().join("new.bin"), b"new").unwrap();

        // 新包已经占住正式路径就代表 commit 已完成；下一次发布先清掉旧包。即使
        // 这一次自己的 package 随后失败，也不能退回上一个事务的旧版本。
        let staging = Staging::new(fixture.output().parent().unwrap()).unwrap();
        let missing_package = staging.path.join("missing");
        assert!(staging
            .publish(&missing_package, &fixture.output(), &mut Vec::new())
            .is_err());

        assert_eq!(
            std::fs::read(fixture.output().join("new.bin")).unwrap(),
            b"new"
        );
        assert!(!recovery.exists(), "已提交的新包不该伴随孤立旧包");
    }

    #[test]
    fn recovery_runs_before_unrelated_build_validation_failure() {
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.output()).unwrap();
        std::fs::write(fixture.output().join("old.bin"), b"old").unwrap();
        let recovery = previous_release_path(&fixture.output());
        std::fs::rename(fixture.output(), &recovery).unwrap();
        std::fs::create_dir(recovery.join(PUBLISH_TRANSACTION_MARKER)).unwrap();
        std::fs::remove_file(fixture.project.entry_scene_path()).unwrap();

        assert!(matches!(
            fixture.build(),
            Err(BuildError::EntrySceneMissing(_))
        ));
        assert_eq!(
            std::fs::read(fixture.output().join("old.bin")).unwrap(),
            b"old",
            "场景预检失败前也必须先恢复正式输出"
        );
        assert!(!recovery.exists());
    }

    #[test]
    fn unmarked_previous_directory_is_rejected_without_deleting_anything() {
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.output()).unwrap();
        std::fs::write(fixture.output().join("old.bin"), b"old").unwrap();
        let recovery = previous_release_path(&fixture.output());
        std::fs::create_dir(&recovery).unwrap();
        std::fs::write(recovery.join("user.bin"), b"not an engine directory").unwrap();

        let staging = Staging::new(fixture.output().parent().unwrap()).unwrap();
        let package = staging.path.join("package");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(package.join("new.bin"), b"new").unwrap();
        assert!(matches!(
            staging.publish(&package, &fixture.output(), &mut Vec::new()),
            Err(BuildError::PathConflict { .. })
        ));

        assert_eq!(
            std::fs::read(fixture.output().join("old.bin")).unwrap(),
            b"old"
        );
        assert_eq!(
            std::fs::read(recovery.join("user.bin")).unwrap(),
            b"not an engine directory"
        );
    }

    #[test]
    fn non_empty_transaction_marker_is_rejected_without_deleting_anything() {
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.output()).unwrap();
        std::fs::write(fixture.output().join("new.bin"), b"new").unwrap();
        let recovery = previous_release_path(&fixture.output());
        let marker = recovery.join(PUBLISH_TRANSACTION_MARKER);
        std::fs::create_dir_all(&marker).unwrap();
        std::fs::write(recovery.join("old.bin"), b"old").unwrap();
        std::fs::write(marker.join("user.bin"), b"keep").unwrap();

        let staging = Staging::new(fixture.output().parent().unwrap()).unwrap();
        let package = staging.path.join("package");
        std::fs::create_dir(&package).unwrap();
        assert!(matches!(
            staging.publish(&package, &fixture.output(), &mut Vec::new()),
            Err(BuildError::PathConflict { .. })
        ));

        assert_eq!(
            std::fs::read(fixture.output().join("new.bin")).unwrap(),
            b"new"
        );
        assert_eq!(std::fs::read(recovery.join("old.bin")).unwrap(), b"old");
        assert_eq!(std::fs::read(marker.join("user.bin")).unwrap(), b"keep");
    }

    const CRASH_BOUNDARY_ENV: &str = "XGAME_TEST_PUBLISH_CRASH_BOUNDARY";
    const CRASH_PARENT_ENV: &str = "XGAME_TEST_PUBLISH_CRASH_PARENT";
    const CRASH_OUTPUT_ENV: &str = "XGAME_TEST_PUBLISH_CRASH_OUTPUT";
    const CRASH_MARKER_ENV: &str = "XGAME_TEST_PUBLISH_CRASH_MARKER";

    /// 只由 `killed_publish_recovers_at_every_rename_boundary` 启动的子进程入口。
    /// 没有环境变量时立即通过，因此正常测试枚举不需要特殊过滤规则。
    #[test]
    fn publish_crash_child_process() {
        let Ok(raw_boundary) = std::env::var(CRASH_BOUNDARY_ENV) else {
            return;
        };
        let boundary = match raw_boundary.as_str() {
            "marked" => PublishBoundary::TransactionMarked,
            "previous" => PublishBoundary::PreviousMoved,
            "package" => PublishBoundary::PackageMoved,
            other => panic!("未知发布故障点: {other}"),
        };
        let parent = PathBuf::from(std::env::var_os(CRASH_PARENT_ENV).unwrap());
        let output = PathBuf::from(std::env::var_os(CRASH_OUTPUT_ENV).unwrap());
        let marker = PathBuf::from(std::env::var_os(CRASH_MARKER_ENV).unwrap());
        let staging = Staging::new(&parent).unwrap();
        let package = staging.path.join("package");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(package.join("new.bin"), b"new").unwrap();

        let result = staging.publish_inner(&package, &output, &mut Vec::new(), |reached| {
            if reached == boundary {
                std::fs::write(&marker, b"ready").unwrap();
                // 父进程会在这里用 Child::kill 强杀；park 保证不会执行析构或后续步骤。
                loop {
                    std::thread::park();
                }
            }
        });
        panic!("故障点没有截住发布: {result:?}");
    }

    #[test]
    fn killed_publish_recovers_at_every_rename_boundary() {
        for (raw_boundary, expected_file, rejected_file) in [
            ("marked", "old.bin", "new.bin"),
            ("previous", "old.bin", "new.bin"),
            ("package", "new.bin", "old.bin"),
        ] {
            let fixture = Fixture::new();
            std::fs::create_dir_all(fixture.output()).unwrap();
            std::fs::write(fixture.output().join("old.bin"), b"old").unwrap();
            let marker = fixture.base.join(format!("crash-{raw_boundary}.ready"));

            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "builder::tests::publish_crash_child_process",
                    "--quiet",
                ])
                .env(CRASH_BOUNDARY_ENV, raw_boundary)
                .env(CRASH_PARENT_ENV, fixture.output().parent().unwrap())
                .env(CRASH_OUTPUT_ENV, fixture.output())
                .env(CRASH_MARKER_ENV, &marker)
                .spawn()
                .unwrap();

            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !marker.is_file() {
                if let Some(status) = child.try_wait().unwrap() {
                    panic!("发布子进程在故障点前退出: {status}");
                }
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("发布子进程没有到达 {raw_boundary} 故障点");
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }

            child.kill().unwrap();
            let status = child.wait().unwrap();
            assert!(!status.success(), "强杀的子进程不应正常退出");

            // 能在这里重新取得同一把锁，也同时证明强杀已由操作系统释放文件句柄。
            let message = recover_interrupted_publish(&fixture.output())
                .unwrap()
                .expect("两个 rename 边界都应留下可识别的恢复状态");
            assert!(message.contains("上次发布"), "{message}");
            assert!(fixture.output().join(expected_file).is_file());
            assert!(!fixture.output().join(rejected_file).exists());
            assert!(!previous_release_path(&fixture.output()).exists());
            assert!(!fixture.output().join(PUBLISH_TRANSACTION_MARKER).exists());
        }
    }

    #[test]
    fn publish_waits_for_another_publisher() {
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.output()).unwrap();
        std::fs::write(fixture.output().join("old.bin"), b"old").unwrap();

        let staging = Staging::new(fixture.output().parent().unwrap()).unwrap();
        let package = staging.path.join("package");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(package.join("new.bin"), b"new").unwrap();

        // 模拟另一个编辑器进程正处在“旧包移走 → 新包发布”的临界区。
        // 锁文件本身会长期留在 Build/；真正的所有权由文件句柄决定，进程退出即释放。
        let lock_path = fixture.output().parent().unwrap().join(PUBLISH_LOCK_FILE);
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        lock.lock().unwrap();

        let output = fixture.output();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = staging.publish(&package, &output, &mut Vec::new());
            done_tx.send(result).unwrap();
        });
        started_rx.recv().unwrap();

        assert!(
            matches!(
                done_rx.recv_timeout(std::time::Duration::from_millis(200)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "另一个发布者持锁时不能改动同一个输出目录"
        );
        assert_eq!(
            std::fs::read(fixture.output().join("old.bin")).unwrap(),
            b"old",
            "等待期间上一版发布必须保持可用"
        );

        lock.unlock().unwrap();
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("释放锁后发布必须继续")
            .unwrap();
        worker.join().unwrap();
        assert_eq!(
            std::fs::read(fixture.output().join("new.bin")).unwrap(),
            b"new"
        );
        assert!(!fixture.output().join("old.bin").exists());
    }

    #[test]
    fn invalid_input_preserves_previous_release() {
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.output()).unwrap();
        std::fs::write(fixture.output().join("old.bin"), b"old").unwrap();
        std::fs::create_dir(fixture.project.root().join("input.ron")).unwrap();
        assert!(fixture.build().is_err());
        assert_eq!(
            std::fs::read(fixture.output().join("old.bin")).unwrap(),
            b"old"
        );
    }

    // -----------------------------------------------------------------
    // 播放器新鲜度：只有"文件存在"是不够的
    // -----------------------------------------------------------------

    /// 造一个假播放器：`fresh` 决定它带不带当前源码指纹。
    fn fake_player(dir: &Path, name: &str, fresh: bool) -> PathBuf {
        let path = dir.join(name);
        let mut bytes = b"MZ fake player payload".to_vec();
        if fresh {
            bytes.extend_from_slice(xgame::plugin::SOURCE_FINGERPRINT);
            bytes.extend_from_slice(b" tail");
        }
        std::fs::write(&path, &bytes).unwrap();
        path
    }

    #[test]
    fn pick_player_prefers_a_matching_fingerprint_over_an_earlier_stale_one() {
        let dir = std::env::temp_dir().join(format!(
            "xgame_pick_player_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let stale = fake_player(&dir, "stale.exe", false);
        let fresh = fake_player(&dir, "fresh.exe", true);

        // 旧的那个排在前面，但必须被跳过 —— 否则打出去的就是旧引擎。
        let pick = pick_player(&[stale.clone(), fresh.clone()]).unwrap();
        assert_eq!(pick.path, fresh);
        assert_eq!(
            pick.warnings.len(),
            1,
            "跳过旧播放器要有一条提醒: {:?}",
            pick.warnings
        );
        assert!(
            pick.warnings[0].contains("stale.exe"),
            "{:?}",
            pick.warnings
        );

        // 只剩旧的：明确拒绝，并把找过的路径都列出来。
        let error = pick_player(std::slice::from_ref(&stale)).unwrap_err();
        match &error {
            BuildError::PlayerStale { searched } => assert_eq!(searched, &vec![stale.clone()]),
            other => panic!("应报 PlayerStale，实际 {other}"),
        }
        let text = error.to_string();
        assert!(text.contains("cargo build -p xgame-game"), "{text}");
        assert!(text.contains("stale.exe"), "{text}");

        // 一个都不存在：还是原来的 PlayerNotFound。
        let missing = dir.join("missing.exe");
        assert!(matches!(
            pick_player(&[missing]),
            Err(BuildError::PlayerNotFound { .. })
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pick_player_without_a_earlier_stale_candidate_stays_quiet() {
        let dir = std::env::temp_dir().join(format!(
            "xgame_pick_player_quiet_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let fresh = fake_player(&dir, "fresh.exe", true);
        let pick = pick_player(std::slice::from_ref(&fresh)).unwrap();
        assert_eq!(pick.path, fresh);
        assert!(pick.warnings.is_empty(), "第一个就是对的，不该有噪音");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn project_root_and_escaping_content_are_rejected() {
        for source in [".", "../demo", "/Resources", "Build/../Scenes"] {
            let mut fixture = Fixture::new();
            fixture.project.manifest_mut().content.scripts_dir = source.into();
            assert!(fixture.build().is_err(), "必须拒绝 {source}");
            assert!(!fixture.output().exists());
        }
    }

    #[test]
    fn transaction_marker_package_root_is_reserved() {
        let mut fixture = Fixture::new();
        fixture.project.manifest_mut().content.scripts_dir = PUBLISH_TRANSACTION_MARKER.into();
        fixture.project.save().unwrap();
        let source = fixture.project.path(PUBLISH_TRANSACTION_MARKER);
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("user.lua"), b"return 42").unwrap();

        assert!(matches!(
            fixture.build(),
            Err(BuildError::PathConflict { .. })
        ));
        assert_eq!(
            std::fs::read(source.join("user.lua")).unwrap(),
            b"return 42"
        );
        assert!(!fixture.output().exists());
    }

    /// 搜索根与普通内容目录一样参与磁盘读取和复制，越界写法不能只在
    /// `--check` 里提示后仍继续打包。
    #[test]
    fn escaping_search_path_is_rejected() {
        let mut fixture = Fixture::new();
        fixture.project.manifest_mut().content.search_paths = vec!["../Shared".into()];
        fixture.project.save().unwrap();

        fixture.build().expect_err("越界搜索根必须阻断构建");
        assert!(!fixture.output().exists(), "失败不该留下半成品");
    }

    #[cfg(unix)]
    #[test]
    fn build_directory_link_to_source_is_rejected() {
        let fixture = Fixture::new();
        std::os::unix::fs::symlink(
            fixture.project.root().join("Resources"),
            fixture.project.root().join("Build"),
        )
        .unwrap();
        assert!(fixture.build().is_err());
        assert!(!fixture.project.root().join("Resources/demo").exists());
    }

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
        assert_eq!(normalize_relative("Resources/"), Some("Resources".into()));
        assert_eq!(normalize_relative("/Resources/"), None);
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
