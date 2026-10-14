//! Rust 插件项目（P0.5）：探测、后台构建、产物同步 —— UE 模式的心脏。
//!
//! 用户在项目根放一个 `Cargo.toml`（cdylib crate，依赖 `xgame`）。编辑器按下
//! ▶ Play 时**后台**跑 `cargo build`，把产物 DLL 同步进 `<项目>/Plugins/`，
//! 再由 [`PluginSession`]（`plugin_session` 模块）加载进当前进程。全程 UI 不阻塞。
//!
//! # 三个刻意的取舍
//!
//! - **新鲜度是纯文件时间比较，不问 cargo**：`Plugins/` 里的 DLL 比"会改变它们
//!   的东西"都新 ⇒ 直接用，连 cargo 进程都不启动（省掉几百毫秒）。所谓"会改变
//!   它们的东西"有两样：项目源文件（`Cargo.toml` / `Cargo.lock` / `src/**/*.rs`）
//!   与**编辑器自身的 exe** —— 后者是"引擎变了"的代理：插件静态链接引擎，引擎
//!   改了插件必须重编，否则版本握手可能照样通过、ABI 实际却漂了。
//! - **后台线程跑 cargo，mpsc 传结果**：cargo 可能编译几分钟（首次拉依赖），
//!   阻塞 UI 线程等于编辑器假死。线程只做"跑进程 + 解析 JSON"，不碰 egui / wgpu。
//! - **产物同步进 `<项目>/Plugins/`**：`target/` 是 cargo 的地盘（可能被并发构建
//!   清空、改名），而玩家与打包器只认 `Plugins/`。编辑器是唯一写入方、玩家只读 ——
//!   单向数据流，没有锁。
//!
//! cargo 可执行文件的定位链（双击启动的编辑器 PATH 里往往没有 cargo）：
//! `CARGO` 环境变量 → `~/.cargo/bin/cargo.exe` → PATH 兜底。
//!
//! # 加载为什么必须走临时副本
//!
//! Windows 上被 `dlopen` 的 DLL 文件会被锁住，下一次 `cargo build` 拷贝新产物
//! 覆盖它就会失败（sharing violation）。所以宿主**永远**先把 DLL 复制成带
//! pid + 计数器的唯一临时文件、再 dlopen 那份副本 —— 源文件保持可写。副本的
//! 生命周期与句柄一致（见 `plugin_session` 的 Drop 约定），历史残留由
//! [`sweep_temp_copies`] 在启动时尽力清理。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Instant, SystemTime};

/// 项目里的插件目录名。**规范位置**：编辑器写入、玩家读取、打包器复制，
/// 三者只认 `<项目>/Plugins/`。
pub const PLUGINS_DIR: &str = "Plugins";

/// 一次后台构建的结果。
pub enum BuildOutcome {
    /// 构建成功；参数是 cdylib 产物在 `target/` 里的绝对路径。
    Success(Vec<PathBuf>),
    /// 构建失败；参数是已截尾的人类可读诊断（`cargo build` 的 stderr）。
    Failed(String),
}

/// 正在飞行的后台构建：结果经 mpsc 异步取回。
pub struct BuildTask {
    rx: mpsc::Receiver<BuildOutcome>,
    /// 启动时刻（工具栏显示"已编译 Xs"用）。
    pub started: Instant,
}

impl BuildTask {
    /// 非阻塞取结果；`None` = 还在编译。
    ///
    /// 线程 panic / 意外退出会让通道断开 —— 按"失败"处理，否则 UI 会永远
    /// 卡在"构建中"。
    pub fn poll(&self) -> Option<BuildOutcome> {
        match self.rx.try_recv() {
            Ok(o) => Some(o),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(BuildOutcome::Failed("构建线程意外退出（可能 panic）".into()))
            }
        }
    }
}

/// 项目是否是 Rust 插件项目。
///
/// 探测刻意只看**一个文件的存在**：`cargo metadata` 要起进程（首次几百毫秒），
/// 而"项目根有没有 Cargo.toml"是插件项目定义本身（计划文档 P0.5）。真正需要
/// cargo 的只有构建那一步，那时再起进程顺理成章。
pub fn is_plugin_project(root: &Path) -> bool {
    root.join("Cargo.toml").is_file()
}

/// 定位 cargo 可执行文件。
///
/// `cargo run` 启动的编辑器有 `CARGO` 环境变量，双击启动的没有 —— 所以按
/// 环境变量 → rustup 默认安装位置 → PATH 的顺序退化，三种启动方式都覆盖到。
pub fn cargo_exe() -> PathBuf {
    if let Some(c) = std::env::var_os("CARGO") {
        let p = PathBuf::from(c);
        if p.is_file() {
            return p;
        }
    }
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let p = PathBuf::from(home)
            .join(".cargo")
            .join("bin")
            .join(format!("cargo{}", std::env::consts::EXE_SUFFIX));
        if p.is_file() {
            return p;
        }
    }
    PathBuf::from("cargo")
}

// ---------------------------------------------------------------------------
// 新鲜度（纯函数族，可测）
// ---------------------------------------------------------------------------

/// 插件是否需要重新构建（**纯函数**：只比较时间戳，不做任何 IO）。
///
/// 三个输入：
/// - `dll_mtimes`：`Plugins/` 里每个 DLL 的修改时间（空 = 从没构建过 ⇒ 必须构建）；
/// - `source_mtime`：项目里最新"会影响编译产物"的源文件时间；
/// - `editor_mtime`：编辑器 exe 时间（引擎变更代理）。
///
/// 新鲜的条件是**每个** DLL 都同时 ≥ 两样东西 —— 用 `>=` 而非 `>`，是为了绕开
/// 文件系统时间戳粒度（部分 FS 是秒级）：源文件与 DLL 在同一秒内先后落盘时，
/// 严格大于会让"刚构建完又进 Play"永远判为过期，陷入重建循环。宁可偶尔多跑
/// 一次 cargo —— 它自己有增量指纹，没改动时几乎是瞬间返回。
pub fn is_fresh(
    dll_mtimes: &[Option<SystemTime>],
    source_mtime: Option<SystemTime>,
    editor_mtime: Option<SystemTime>,
) -> bool {
    if dll_mtimes.is_empty() {
        return false; // 一个 DLL 都没有 ⇒ 无产物 ⇒ 必须构建
    }
    // 任一 DLL 缺失时间戳（被删了一半 / 读不出）按过期处理：状态不确定时
    // 走重建这条路永远是对的。
    let Some(oldest_dll) = dll_mtimes.iter().flatten().min().copied() else {
        return false;
    };
    if source_mtime.is_some_and(|s| s > oldest_dll) {
        return false;
    }
    if editor_mtime.is_some_and(|e| e > oldest_dll) {
        return false;
    }
    true
}

/// 项目里最新"会影响插件编译产物"的源文件时间：`Cargo.toml`、`Cargo.lock`、
/// `src/**/*.rs`。
///
/// 只看这三类是刻意的：场景、贴图、脚本等资产不参与 cdylib 编译，把它们算进
/// 来会让"改个场景就重建插件"。引擎侧的变更由编辑器 exe 的时间代理（见
/// [`is_fresh`]），不需要在这里递归到 workspace 根。
pub fn newest_source_mtime(root: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let mut bump = |p: &Path| {
        if let Ok(m) = std::fs::metadata(p).and_then(|m| m.modified()) {
            if newest.is_none_or(|n| m > n) {
                newest = Some(m);
            }
        }
    };
    bump(&root.join("Cargo.toml"));
    bump(&root.join("Cargo.lock"));
    bump_dir_rs(&root.join("src"), &mut bump);
    newest
}

/// 递归收集 `dir` 下所有 `.rs` 文件的时间戳（`bump` 取最大）。
fn bump_dir_rs(dir: &Path, bump: &mut impl FnMut(&Path)) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            bump_dir_rs(&p, bump);
        } else if p
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| x.eq_ignore_ascii_case("rs"))
        {
            bump(&p);
        }
    }
}

/// 收集 `Plugins/` 里所有本平台动态库的修改时间（按路径排序，可复现）。
pub fn plugin_dll_mtimes(project_root: &Path) -> Vec<(PathBuf, Option<SystemTime>)> {
    let mut dlls = collect_dylibs(&project_root.join(PLUGINS_DIR));
    dlls.sort();
    dlls.iter()
        .map(|p| (p.clone(), std::fs::metadata(p).and_then(|m| m.modified()).ok()))
        .collect()
}

/// 判断项目插件是否需要重建（拼上面两个纯函数 + 编辑器自身时间）。
///
/// 语义是 `!is_fresh`：**需要**构建 = 不新鲜。这一层薄薄的取反正是这个函数
/// 存在的理由 —— 调用方只问"要不要编"，不该再各自去翻转一次（第一版写反过，
/// 被 `freshness_against_real_files` 抓住了）。
pub fn needs_build(project_root: &Path) -> bool {
    let entries = plugin_dll_mtimes(project_root);
    let dll_mtimes: Vec<Option<SystemTime>> = entries.iter().map(|(_, m)| *m).collect();
    let src = newest_source_mtime(project_root);
    let editor = std::env::current_exe().ok().and_then(|p| {
        std::fs::metadata(p).and_then(|m| m.modified()).ok()
    });
    !is_fresh(&dll_mtimes, src, editor)
}

// ---------------------------------------------------------------------------
// 构建
// ---------------------------------------------------------------------------

/// 在后台线程跑 `cargo build`，UI 线程经 mpsc 取结果。
///
/// `--message-format=json-render-diagnostics`：诊断仍走 stderr（人类可读，直接
/// 进编辑器 Console），产物路径走 stdout 的 JSON 行 —— 比自己扫 `target/`
/// 目录可靠（workspace 可能有多个 crate、多个 profile 子目录）。
///
/// profile 跟随编辑器自身：debug 编辑器构建 debug 插件、release 编辑器构建
/// release 插件。混用会让"编辑器里跑得好好的、打包出去就崩"更难查（debug /
/// release 是不同的 build flags，属于版本握手防不住的 ABI 差异）。
pub fn spawn_build(project_root: &Path) -> Result<BuildTask, String> {
    let cargo = cargo_exe();
    let root = project_root.to_owned();
    let release = !cfg!(debug_assertions);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let outcome = run_cargo_build(&cargo, &root, release);
        // 接收端可能已随编辑器关闭被丢弃 —— 失败无所谓，线程即将结束。
        let _ = tx.send(outcome);
    });
    Ok(BuildTask { rx, started: Instant::now() })
}

/// 同步跑一次 `cargo build` 并解析产物（`spawn_build` 的线程体，拆出来便于测试）。
fn run_cargo_build(cargo: &Path, root: &Path, release: bool) -> BuildOutcome {
    let mut cmd = Command::new(cargo);
    // `json-render-diagnostics`：产物与诊断走 JSON（stdout），同时把诊断按人类
    // 可读形式渲染到 stderr。注意是**复数** diagnostics —— 少个 s cargo 直接拒收，
    // 而这个错误只会出现在真正按下 Play 的那一刻。
    cmd.arg("build")
        .arg("--message-format=json-render-diagnostics");
    if release {
        cmd.arg("--release");
    }
    cmd.current_dir(root);
    // 接管三根管道：stdout 要逐行解析 JSON，stderr 要当人类可读诊断。
    // `output()` 内部用 std 的 read2 同时排空两根管道，不会互相堵死。
    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => return BuildOutcome::Failed(format!("无法启动 {}: {e}", cargo.display())),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // 编译错误在末尾、头部通常是依赖的 warning —— 取尾部保住最有用的 40 行。
        return BuildOutcome::Failed(tail_lines(&stderr, 40));
    }
    let dlls = parse_artifact_dlls(&stdout);
    if dlls.is_empty() {
        return BuildOutcome::Failed(
            "cargo build 成功但没有找到 cdylib 产物 —— [lib] 的 crate-type 是否包含 \"cdylib\"？"
                .into(),
        );
    }
    BuildOutcome::Success(dlls)
}

/// 从 `cargo build --message-format=json` 的 stdout 里抽出 cdylib 产物路径。
///
/// **纯函数**，便于用固定 JSON 回归。非 JSON 行（理论上不该有）容错跳过。
fn parse_artifact_dlls(stdout: &str) -> Vec<PathBuf> {
    let dll_ext = std::env::consts::DLL_SUFFIX.trim_start_matches('.');
    let mut dlls = Vec::new();
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("reason").and_then(|r| r.as_str()) != Some("compiler-artifact") {
            continue;
        }
        // 只认 cdylib：rlib / bin 产物即使在同一次构建里也不该被当成插件加载。
        let is_cdylib = v
            .get("target")
            .and_then(|t| t.get("kind"))
            .and_then(|k| k.as_array())
            .is_some_and(|k| k.iter().any(|x| x.as_str() == Some("cdylib")));
        if !is_cdylib {
            continue;
        }
        if let Some(filenames) = v.get("filenames").and_then(|f| f.as_array()) {
            for f in filenames {
                if let Some(p) = f.as_str() {
                    let path = PathBuf::from(p);
                    if path
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case(dll_ext))
                    {
                        dlls.push(path);
                    }
                }
            }
        }
    }
    dlls.sort();
    dlls.dedup();
    dlls
}

/// 取文本的最后 `n` 行（cargo 的错误在末尾）。
fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

// ---------------------------------------------------------------------------
// 产物同步
// ---------------------------------------------------------------------------

/// 把构建产物同步进 `<项目>/Plugins/`（玩家与打包器的规范位置）。
///
/// 单个文件拷贝失败只记日志不中断：一个 DLL 被锁住不该让整个构建流程失败
/// （用户可能正跑着上一次的 Play，锁着的是**旧** DLL，新产物照样能拷进去）。
/// 返回同步成功的路径。
///
/// # 为什么同步完还要盖一个"现在"时间戳
///
/// 新鲜度里有一条"编辑器 exe 比 DLL 新 ⇒ 要重建"，而 cargo 在**没有源码改动**
/// 时不会重写产物（时间戳停在旧值）。两件事凑一起会死循环：引擎改了 → 编辑器
/// 重编 → 按 Play → 我们的检查说"DLL 比编辑器旧" → 起 cargo → cargo 说"没改动，
/// 什么都不做" → DLL 还是旧的 → 再按 Play 又来一遍。同步完成后统一盖"现在"戳，
/// 这一轮就收敛了（代价是把"编译期间恰好又存了文件"这种窄窗口交给下一次存盘，
/// cargo 自己的指纹本来也有同款窗口）。
pub fn sync_plugins_dir(project_root: &Path, built: &[PathBuf]) -> Vec<PathBuf> {
    let dir = project_root.join(PLUGINS_DIR);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::error!("无法创建插件目录 {}: {e}", dir.display());
        return Vec::new();
    }
    let mut synced = Vec::new();
    for dll in built {
        let Some(name) = dll.file_name() else { continue; };
        let dst = dir.join(name);
        match std::fs::copy(dll, &dst) {
            Ok(_) => {
                touch(&dst);
                synced.push(dst);
            }
            Err(e) => log::error!(
                "同步插件 {} → {} 失败: {e}（目标可能被上一次加载锁住）",
                dll.display(),
                dst.display()
            ),
        }
    }
    synced
}

/// 把文件的修改时间戳设成"现在"（`fs::copy` 是否保留源时间戳因平台而异，
/// 统一盖掉才可预期）。失败只记 debug 日志 —— 时间戳只是新鲜度的输入，
/// 盖不上的后果是多跑一次 cargo，不是错误。
fn touch(path: &Path) {
    let now = SystemTime::now();
    let times = std::fs::FileTimes::new().set_modified(now);
    if let Err(e) = std::fs::File::options()
        .write(true)
        .truncate(false)
        .open(path)
        .and_then(|f| f.set_times(times))
    {
        log::debug!("无法更新 {} 的时间戳: {e}", path.display());
    }
}

/// 把 DLL 复制成唯一文件名的临时副本再加载（见模块文档「加载为什么必须走
/// 临时副本」）。副本名：`xgame-plugin-<pid>-<counter>-<原文件名>`。
///
/// 计数器由调用方持有 —— 同一进程内每次复制都必须得到不同路径，否则新副本会
/// 撞上还被旧句柄锁着的文件。
pub fn temp_copy(dll: &Path, counter: &mut u64) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("xgame-plugin-cache");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("无法创建插件缓存目录 {}: {e}", dir.display()))?;
    let file_name = dll
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("plugin.dll");
    *counter += 1;
    let dst = dir.join(format!(
        "xgame-plugin-{}-{}-{file_name}",
        std::process::id(),
        *counter
    ));
    std::fs::copy(dll, &dst)
        .map_err(|e| format!("复制 {} → {}: {e}", dll.display(), dst.display()))?;
    Ok(dst)
}

/// 启动时清扫历史会话残留的临时副本（**尽力而为**，失败静默）。
///
/// 别的编辑器进程还开着的 DLL 删不掉（Windows 文件锁）—— 那正好是该留下的；
/// 已退出进程的残留则能删干净。跳过本进程自己的目录（正在用）。
pub fn sweep_temp_copies() {
    let me = std::process::id();
    let tmp = std::env::temp_dir().join("xgame-plugin-cache");
    let Ok(rd) = std::fs::read_dir(&tmp) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue; };
        let Some(pid_str) = name.strip_prefix("xgame-plugin-") else {
            continue;
        };
        let pid: u32 = match pid_str.split('-').next().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        if pid == me {
            continue;
        }
        let _ = std::fs::remove_file(e.path());
    }
}

/// 收集目录里所有本平台的动态库文件（`.dll` / `.so` / `.dylib`）。
///
/// 目录不存在或读不开 ⇒ 空表：纯数据项目本就没有插件目录，不是错误。
pub fn collect_dylibs(dir: &Path) -> Vec<PathBuf> {
    let suffix = std::env::consts::DLL_SUFFIX.trim_start_matches('.');
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dlls: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case(suffix))
        })
        .collect();
    dlls.sort(); // 排序保证注册顺序稳定（同名组件后注册者覆盖）
    dlls
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs)
    }

    /// 证明 `is_fresh` 的三条过期规则各自生效（旧实现若漏一条，这里会红）。
    #[test]
    fn freshness_rules() {
        // 全新鲜
        assert!(is_fresh(&[Some(t(100))], Some(t(50)), Some(t(50))));
        // 空产物 ⇒ 过期
        assert!(!is_fresh(&[], Some(t(50)), Some(t(50))));
        // 源文件比 DLL 新 ⇒ 过期
        assert!(!is_fresh(&[Some(t(100))], Some(t(200)), Some(t(50))));
        // 编辑器比 DLL 新（引擎变更代理）⇒ 过期
        assert!(!is_fresh(&[Some(t(100))], Some(t(50)), Some(t(200))));
        // 边界：相等算新鲜（时间戳粒度陷阱）
        assert!(is_fresh(&[Some(t(100))], Some(t(100)), Some(t(100))));
        // 多个 DLL：最旧的那个也要过关
        assert!(!is_fresh(
            &[Some(t(100)), Some(t(50))],
            Some(t(60)),
            Some(t(40))
        ));
        // 读不出时间戳的 DLL ⇒ 不确定 ⇒ 过期
        assert!(!is_fresh(&[None], Some(t(50)), Some(t(50))));
    }

    /// `cargo build` JSON 解析：只挑 cdylib 且扩展名匹配的文件名。
    ///
    /// 固定 JSON 是**实测形状**（`cargo build --message-format=json-render-diagnostics`
    /// 在 BehaviorsDemo 上的真实输出）：`crate-type = ["cdylib", "rlib"]` 的目标
    /// 一次给五个文件名，其中 `behaviors_demo.dll.lib` 的扩展名是 `lib` 而不是
    /// `dll` —— 靠 `Path::extension` 取最后一段来区分，不能用 `contains("dll")`。
    #[test]
    fn parse_cdylib_artifacts() {
        let json = r#"
{"reason":"compiler-artifact","target":{"kind":["cdylib","rlib"],"name":"behaviors_demo"},"filenames":["D:/t/debug/behaviors_demo.dll","D:/t/debug/behaviors_demo.dll.lib","D:/t/debug/behaviors_demo.dll.exp","D:/t/debug/behaviors_demo.pdb","D:/t/debug/libbehaviors_demo.rlib"]}
{"reason":"compiler-artifact","target":{"kind":["lib"],"name":"dep"},"filenames":["D:/t/debug/libdep.rlib"]}
{"reason":"build-finished","success":true}
not json at all
"#;
        let dlls = parse_artifact_dlls(json);
        assert_eq!(
            dlls.len(),
            1,
            "应只挑出 cdylib 的 DLL，忽略 rlib / pdb / .dll.lib，实际 {dlls:?}"
        );
        let name = dlls[0].file_name().unwrap().to_str().unwrap();
        assert!(name.eq_ignore_ascii_case("behaviors_demo.dll"), "实际拿到 {name}");
    }

    /// 构建失败时保留 stderr 尾部（编译错误在末尾）。
    #[test]
    fn tail_keeps_end() {
        let text = "a\nb\nc\nd\ne";
        assert_eq!(tail_lines(text, 2), "d\ne");
        assert_eq!(tail_lines(text, 100), text);
    }

    /// 临时副本名必须唯一 —— Windows 文件锁下重名会撞上还被锁着的旧副本。
    #[test]
    fn temp_copy_names_are_unique() {
        let tmp = std::env::temp_dir().join("xgame-plugin-cache");
        std::fs::create_dir_all(&tmp).unwrap();
        let src = std::env::temp_dir().join(format!("xgame-pc-src-{}.dll", std::process::id()));
        std::fs::write(&src, b"x").unwrap();

        let mut counter = 0u64;
        let a = temp_copy(&src, &mut counter).unwrap();
        let b = temp_copy(&src, &mut counter).unwrap();
        assert_ne!(a, b, "两次复制必须得到不同的目标路径");
        assert_eq!(counter, 2);

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    /// `collect_dylibs` 只认本平台后缀，且结果有序。
    #[test]
    fn collect_only_platform_dylibs() {
        let dir = std::env::temp_dir().join(format!("xgame-collect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let suffix = std::env::consts::DLL_SUFFIX;
        std::fs::write(dir.join(format!("b{suffix}")), b"x").unwrap();
        std::fs::write(dir.join(format!("a{suffix}")), b"x").unwrap();
        std::fs::write(dir.join("readme.txt"), b"x").unwrap();

        let got = collect_dylibs(&dir);
        assert_eq!(got.len(), 2, "txt 不该被当成插件: {got:?}");
        let first = got[0].file_name().unwrap().to_str().unwrap();
        assert!(first.starts_with('a'), "应按文件名排序，实际 {first}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 同步产物之后，新鲜度必须立刻转为"新鲜" —— 这是防死循环的关键。
    ///
    /// # 这条测试在防什么
    ///
    /// 新鲜度里有一条"编辑器 exe 比 DLL 新 ⇒ 要重建"（引擎变更的代理），而
    /// cargo 在**源码没变**时不会重写产物，时间戳就停在旧值。两件事凑一起会
    /// 死循环：引擎改了 → 编辑器重编 → 按 Play → 检查说"DLL 比编辑器旧" →
    /// 起 cargo → cargo 说"没改动" → DLL 还是旧的 → 再按 Play 又来一遍。
    ///
    /// `sync_plugins_dir` 里的 `touch` 就是解药。这里把待同步的产物时间戳
    /// **倒退回一小时前**来模拟"cargo 认为没改动、不重写产物"的情形 ——
    /// 去掉 `touch` 这条测试立刻变红。
    #[test]
    fn sync_makes_plugins_fresh_even_when_cargo_did_nothing() {
        let root = std::env::temp_dir().join(format!("xgame-syncfresh-{}", std::process::id()));
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(root.join(PLUGINS_DIR)).unwrap();

        // 源文件（比下面那份"产物"新 ⇒ 必须重建）。
        std::fs::write(root.join("Cargo.toml"), b"[package]\nname=\"x\"\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(src.join("lib.rs"), b"pub fn a() {}").unwrap();

        // 待同步的产物带上**过去**的时间戳 —— cargo 增量判定为"无需重编"时，
        // target/ 里的文件就是这个样子（mtime 停在上一次真正编译的时刻）。
        let built =
            std::env::temp_dir().join(format!("xgame-stale-built-{}.dll", std::process::id()));
        std::fs::write(&built, b"v2").unwrap();
        let past = SystemTime::now() - std::time::Duration::from_secs(3600);
        let handle = std::fs::File::options().write(true).open(&built).unwrap();
        handle
            .set_times(std::fs::FileTimes::new().set_modified(past))
            .unwrap();
        drop(handle);

        let synced = sync_plugins_dir(&root, std::slice::from_ref(&built));
        assert_eq!(synced.len(), 1);
        assert!(
            !needs_build(&root),
            "同步之后必须判定为新鲜 —— 否则每次按 Play 都会空跑一轮 cargo（死循环）"
        );

        let _ = std::fs::remove_file(&built);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 新鲜度判定对**真实文件**成立（把纯函数和文件系统接起来）。
    #[test]
    fn freshness_against_real_files() {
        let root = std::env::temp_dir().join(format!("xgame-fresh-{}", std::process::id()));
        let src = root.join("src");
        let plugins = root.join(PLUGINS_DIR);
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&plugins).unwrap();
        let dll = plugins.join("x.dll");

        // 1) 先有产物，随后源文件被改（顺序很重要：源必须比产物**新**才叫过期）。
        std::fs::write(&dll, b"x").unwrap();
        std::fs::write(root.join("Cargo.toml"), b"[package]\nname=\"x\"\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(src.join("lib.rs"), b"pub fn a() {}").unwrap();

        assert!(newest_source_mtime(&root).is_some(), "应扫到 Cargo.toml / src");
        assert!(needs_build(&root), "源文件比 DLL 新时必须重建");

        // 2) 重写产物（模拟"编译完成 + 同步"）⇒ 新鲜，不再触发构建。
        //    这一步同时守着"编辑器 exe 时间"那条规则：本测试二进制在跑之前
        //    就编好了，晚于它写的 DLL 必然更新 —— 否则会永远判过期。
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(&dll, b"yy").unwrap();
        assert!(!needs_build(&root), "产物比源与编辑器都新时应当是新鲜的");

        let _ = std::fs::remove_dir_all(&root);
    }
}
