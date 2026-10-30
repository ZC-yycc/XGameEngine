//! 引擎日志的三个去处：stderr（照旧）、控制台面板、磁盘 `Editor.log`。
//!
//! # 为什么要有这个模块
//!
//! 在此之前控制台只显示**编辑器自己**显式 `state.log(...)` 的消息，`log::warn!`
//! 一律只进 stderr。后果是：用户双击 `dist` 里的 exe 跑起来（没有终端）、场景加载
//! 失败、模型缺骨骼、脚本报错 —— 这些**最需要看的**行全落进了虚空。而排查问题时
//! 我们手上既没有控制台内容，也没有磁盘上的现场。
//!
//! 装一个 [`log::Log`] 实现把三件事一次做齐：
//!
//! ```text
//! log::warn!("…") ──┬─▶ stderr          （env_logger 原样转发，RUST_LOG 照旧生效）
//!                   ├─▶ 控制台队列        （有界环形，帧末被 EditorState 取走）
//!                   └─▶ Editor.log        （追加写，超过上限轮转一次）
//! ```
//!
//! # 收谁不收谁（[`captures`]）
//!
//! | 等级 | 自家 target（`xgame*`） | 第三方 |
//! |---|---|---|
//! | `error` / `warn` | ✅ | ✅ |
//! | `info` | ✅ | ❌ |
//! | `debug` / `trace` | ❌ | ❌ |
//!
//! 第三方 `info` 不收：`wgpu` / `naga` 开局就吐几十条，会把真正有信息量的行挤走。
//! `debug` / `trace` 一律不收：`RUST_LOG=trace` 一次运行就能把日志写成 GB 级 ——
//! 想看细粒度日志请照旧看 stderr，那是**当次**的现场，不需要留档。
//!
//! 注意磁盘/控制台的网比 stderr **宽**：`RUST_LOG=error` 时 warn 不进终端，但仍然
//! 落在控制台和文件里。这正是想要的 —— 终端的过滤是"现在想看什么"，落档是"以后
//! 要查什么"。
//!
//! # 时间戳为什么是"启动以来第几秒"
//!
//! 不用本地时钟：跨时区 / 夏令时都不变，不需要额外依赖（`std` 只能拿到 UTC 的
//! `SystemTime`，硬转本地时间是错的），而且与 `frame_probe` 的 `t_us=` 是同一族
//! 时间轴 —— 排查"加载完之后那 3 帧到底发生了什么"可以直接对齐。文件的创建时间
//! 给出墙上时间。
//!
//! # 轮转
//!
//! 单文件上限 [`MAX_FILE_BYTES`]，超了把当前文件改名成 `Editor.log.1`（只留**一份**
//! 备份，旧的直接删）。留一份而不是 N 份：日志的用途是"复现最后一次问题"，留十份
//! 只会让人不知道该看哪个，还占磁盘。

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 日志文件名（位于用户配置目录下的 `logs/`，见 [`crate::config`]）。
pub const FILE_NAME: &str = "Editor.log";
/// 轮转备份名。只留这一份。
pub const ROTATED_NAME: &str = "Editor.log.1";
/// 子目录名（配置目录下）。
pub const DIR_NAME: &str = "logs";
/// 单文件字节上限。
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// 控制台队列上限。超了丢**最旧**的一批 —— 用户要看的是"刚才发生了什么"。
pub const MAX_PENDING: usize = 1024;

// ---------------------------------------------------------------------------
// 一条记录
// ---------------------------------------------------------------------------

/// 从 `log` 门面抓到的一条消息。
#[derive(Debug, Clone)]
pub struct Record {
    /// 进程启动到这条消息的时长。
    pub elapsed: Duration,
    pub level: log::Level,
    /// 完整 target（`xgame_render::gltf_loader` 这样的模块路径）。
    pub target: String,
    pub message: String,
    /// 打这条日志的**源码位置**（`file!()`，相对仓库根）。控制台点它可跳转。
    ///
    /// 对引擎作者有用（`warn!` 就在眼前那行）；对拿 `dist` 跑的用户，源码
    /// 通常不在本机 —— 那就只是显示一下，不假装能打开。
    pub file: Option<String>,
    pub line: u32,
}

impl Record {
    /// 落盘格式：`[+  1.234s] WARN  xgame_render::gltf_loader: 消息`。
    ///
    /// 等级左对齐补到 5 列：日志里的列对齐不是为了好看，是为了 `grep '^\[+ *[0-9.]*s\] WARN'`
    /// 这种按位置读的过滤能生效。
    pub fn line(&self) -> String {
        format!(
            "[+{}.{:03}s] {:<5} {}: {}",
            self.elapsed.as_secs(),
            self.elapsed.subsec_millis(),
            level_tag(self.level),
            self.target,
            self.message
        )
    }

    /// 控制台里显示的文字：模块路径去掉 `xgame_` 前缀、保留层级。
    ///
    /// `xgame_render::gltf_loader` → `render::gltf_loader`。crate 名对排查没有
    /// 信息量（全项目都叫 xgame_*），但**模块**名有：`render::gltf_loader` 立刻
    /// 告诉你去哪儿看代码。
    pub fn console_text(&self) -> String {
        format!("{}: {}", short_target(&self.target), self.message)
    }
}

/// 等级 → 定宽标签。
pub fn level_tag(level: log::Level) -> &'static str {
    match level {
        log::Level::Error => "ERROR",
        log::Level::Warn => "WARN",
        log::Level::Info => "INFO",
        log::Level::Debug => "DEBUG",
        log::Level::Trace => "TRACE",
    }
}

/// 把完整 target 压成"人读的模块路径"。
///
/// 只去掉**第一段**的 `xgame_` / `xgame-` 前缀；`xgame` 这种单段 target 原样返回
/// （去掉前缀会剩下空串）。
pub fn short_target(target: &str) -> String {
    let mut parts = target.split("::");
    let first = parts.next().unwrap_or(target);
    let rest: Vec<&str> = parts.collect();
    let head = first
        .strip_prefix("xgame_")
        .or_else(|| first.strip_prefix("xgame-"))
        .unwrap_or(first);
    if rest.is_empty() {
        head.to_string()
    } else {
        format!("{head}::{}", rest.join("::"))
    }
}

/// 这条记录要不要进控制台 / 磁盘（策略表见模块头）。
pub fn captures(level: log::Level, target: &str) -> bool {
    match level {
        log::Level::Error | log::Level::Warn => true,
        log::Level::Info => target.starts_with("xgame"),
        log::Level::Debug | log::Level::Trace => false,
    }
}

// ---------------------------------------------------------------------------
// 磁盘：追加写 + 到顶轮转
// ---------------------------------------------------------------------------

/// 一个日志文件（追加写）。
///
/// 构造时就建立父目录并打开文件；打开失败则**整体降级**为不写盘（控制台照旧），
/// 不重试 —— 一个连目录都建不出来的环境，每条日志都试一次只会拖慢主循环。
struct FileOut {
    path: PathBuf,
    rotated: PathBuf,
    limit: u64,
    file: Option<File>,
    written: u64,
}

impl FileOut {
    fn open(path: PathBuf) -> Self {
        Self::with_limit(path, MAX_FILE_BYTES)
    }

    fn with_limit(path: PathBuf, limit: u64) -> Self {
        let rotated = path.with_file_name(
            path.file_name()
                .map(|name| format!("{}.1", name.to_string_lossy()))
                .unwrap_or_else(|| ROTATED_NAME.to_owned()),
        );
        let mut out = Self {
            path,
            rotated,
            limit,
            file: None,
            written: 0,
        };
        if let Some(dir) = out.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        out.file = open_append(&out.path);
        out.written = out
            .file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map(|m| m.len())
            .unwrap_or(0);
        if out.file.is_none() {
            // 不能用 `log::`：这里就是日志后端本身，会无限递归。
            eprintln!(
                "xgame: 无法写入日志文件 {}（只保留控制台输出）",
                out.path.display()
            );
        }
        out
    }

    fn rotate(&mut self) {
        self.file = None;
        // Windows 上不能 rename 到已存在的文件：先删旧的。
        let _ = std::fs::remove_file(&self.rotated);
        let _ = std::fs::rename(&self.path, &self.rotated);
        self.file = open_append(&self.path);
        self.written = self
            .file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map(|m| m.len())
            .unwrap_or(0);
    }

    /// 写一行（自动补 `\n`）。到顶先轮转。
    ///
    /// 不 fsync：`write` 到内核缓冲已经能扛住**进程**崩溃（也就是我们最关心的那种），
    /// 掉电才需要 fsync —— 为那点概率给每条日志加一次磁盘同步不值得。
    fn write_line(&mut self, line: &str) {
        if self.file.is_none() {
            return;
        }
        // 单行本身就超上限（一条巨型报错）：照写，否则会陷入"永远写不进去"的死循环。
        if self.written > 0 && self.written + line.len() as u64 + 1 > self.limit {
            self.rotate();
        }
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let ok = file.write_all(line.as_bytes()).is_ok() && file.write_all(b"\n").is_ok();
        if ok {
            self.written += line.len() as u64 + 1;
        } else {
            // 写坏了（磁盘满 / 文件被删）：这一轮剩下的日志就别再试了。
            self.file = None;
        }
    }
}

fn open_append(path: &Path) -> Option<File> {
    OpenOptions::new().create(true).append(true).open(path).ok()
}

// ---------------------------------------------------------------------------
// 队列 + 磁盘（与 stderr 无关的那一半，可单测）
// ---------------------------------------------------------------------------

/// 收件箱：有界队列 + 磁盘。刻意不依赖 `env_logger`，这样测试不需要装全局 logger。
struct Capture {
    pending: Mutex<VecDeque<Record>>,
    dropped: AtomicU64,
    file: Mutex<Option<FileOut>>,
    capacity: usize,
}

impl Capture {
    fn new(file: Option<FileOut>) -> Self {
        Self {
            pending: Mutex::new(VecDeque::new()),
            dropped: AtomicU64::new(0),
            file: Mutex::new(file),
            capacity: MAX_PENDING,
        }
    }

    #[cfg(test)]
    fn with_capacity(capacity: usize, file: Option<FileOut>) -> Self {
        Self {
            capacity,
            ..Self::new(file)
        }
    }

    fn push(&self, record: Record) {
        // 先落盘：即使队列被刷爆丢掉，文件里仍然有完整的一份。
        if let Ok(mut guard) = self.file.lock() {
            if let Some(file) = guard.as_mut() {
                file.write_line(&record.line());
            }
        }
        let Ok(mut pending) = self.pending.lock() else {
            return;
        };
        if pending.len() >= self.capacity {
            pending.pop_front();
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        pending.push_back(record);
    }

    fn take(&self) -> Vec<Record> {
        let Ok(mut pending) = self.pending.lock() else {
            return Vec::new();
        };
        pending.drain(..).collect()
    }

    fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// log::Log 实现
// ---------------------------------------------------------------------------

struct Sink {
    started: Instant,
    /// stderr 那一半：`env_logger` 原样转发，`RUST_LOG` 的语义完全不变。
    inner: env_logger::Logger,
    capture: Capture,
    /// 日志文件路径（配置目录不可用时为 `None`）。存一份，面板每帧都要读。
    path: Option<PathBuf>,
}

impl log::Log for Sink {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        // 比 stderr 宽：stderr 关掉的 info/warn，控制台和磁盘仍要收。
        captures(metadata.level(), metadata.target()) || self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record<'_>) {
        // stderr 自己会再判一次过滤（`matches`），这里直接转发。
        if self.inner.enabled(record.metadata()) {
            self.inner.log(record);
        }
        if captures(record.level(), record.target()) {
            self.capture.push(Record {
                elapsed: self.started.elapsed(),
                level: record.level(),
                target: record.target().to_owned(),
                message: record.args().to_string(),
                file: record.file().map(str::to_owned),
                line: record.line().unwrap_or(0),
            });
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

static SINK: OnceLock<Sink> = OnceLock::new();

/// 装上日志后端。进程内只生效一次（重复调用是 no-op）。
///
/// 返回日志文件路径（配置目录不可用时为 `None`，此时只剩 stderr 与控制台）。
pub fn init() -> Option<PathBuf> {
    let path = crate::config::dir().map(|dir| dir.join(DIR_NAME).join(FILE_NAME));
    let inner =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).build();
    // `max_level` 只是宏层面的快速路径，真正的过滤在 `enabled` / `matches`。
    // 取"任一可能出现的 target 被放行的最高等级"，免得把 RUST_LOG 打开的等级挡在
    // 宏外面；探测失败就退回 Error（最保守，宏不展开就不可能漏格式化成灾）。
    let max = [
        log::Level::Error,
        log::Level::Warn,
        log::Level::Info,
        log::Level::Debug,
        log::Level::Trace,
    ]
    .into_iter()
    .rev()
    .find(|level| {
        ["xgame", "wgpu", ""].iter().any(|target| {
            log::Log::enabled(
                &inner,
                &log::Metadata::builder()
                    .level(*level)
                    .target(target)
                    .build(),
            )
        })
    })
    .map(|level| level.to_level_filter())
    .unwrap_or(log::LevelFilter::Error);

    let sink = SINK.get_or_init(|| Sink {
        started: Instant::now(),
        inner,
        capture: Capture::new(path.clone().map(FileOut::open)),
        path: path.clone(),
    });

    if log::set_logger(sink).is_err() {
        // 已经装过（测试宿主或重复 init）：保留原有后端，别覆盖。
        return path;
    }
    log::set_max_level(max);
    if let Some(path) = path.as_ref() {
        // 一行里同时给三件事：文件在哪、上限多少、**这是哪一次运行**（pid）。
        // 追加写的日志是多会话共用一个文件，没有会话边界就没法回答"我看到的
        // 这段是刚才那次还是上周那次"。
        log::info!(
            target: "xgame_editor::logging",
            "日志写入 {}（上限 {} MiB，超了轮转为 {ROTATED_NAME}；本次 pid {}）",
            path.display(),
            MAX_FILE_BYTES / (1024 * 1024),
            std::process::id()
        );
    }
    path
}

/// 取走队列里的全部记录（每帧一次，见 `EditorApp::update`）。
pub fn take_records() -> Vec<Record> {
    SINK.get()
        .map(|sink| sink.capture.take())
        .unwrap_or_default()
}

/// 因队列满被丢掉的条数（诊断用：控制台里能看到"还有多少条没显示"）。
pub fn dropped() -> u64 {
    SINK.get().map(|sink| sink.capture.dropped()).unwrap_or(0)
}

/// 日志文件路径（未初始化或配置目录不可用时是 `None`）。
///
/// 返回 `&'static`：路径在 `init` 时定死，面板每帧读它，不该为此分配。
pub fn log_file_path() -> Option<&'static Path> {
    SINK.get().and_then(|sink| sink.path.as_deref())
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn record(level: log::Level, target: &str, message: &str) -> Record {
        Record {
            elapsed: Duration::from_millis(1234),
            level,
            target: target.to_owned(),
            message: message.to_owned(),
            file: None,
            line: 0,
        }
    }

    /// 临时目录：名字带进程号与计数器，避免并行测试互撞。
    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::AtomicU32;
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("xgame-log-{tag}-{}-{seq}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn warns_and_errors_are_always_captured_but_third_party_info_is_not() {
        assert!(captures(log::Level::Warn, "wgpu_core::device"));
        assert!(captures(log::Level::Error, "naga"));
        assert!(captures(log::Level::Info, "xgame_render::gltf_loader"));
        assert!(!captures(log::Level::Info, "wgpu_core::device"));
        // debug / trace 走 stderr，永不入档 —— 否则 RUST_LOG=trace 能把盘写满。
        assert!(!captures(log::Level::Debug, "xgame_core"));
        assert!(!captures(log::Level::Trace, "xgame_core"));
    }

    #[test]
    fn console_text_drops_the_crate_prefix_but_keeps_modules() {
        assert_eq!(
            short_target("xgame_render::gltf_loader"),
            "render::gltf_loader"
        );
        assert_eq!(short_target("xgame_core"), "core");
        // 单段 target 去掉前缀会剩下空串：必须原样返回。
        assert_eq!(short_target("xgame"), "xgame");
        assert_eq!(short_target("wgpu::hal"), "wgpu::hal");
        assert_eq!(
            record(log::Level::Warn, "xgame_render::gltf_loader", "缺骨骼").console_text(),
            "render::gltf_loader: 缺骨骼"
        );
    }

    #[test]
    fn the_file_line_carries_uptime_and_a_fixed_width_level() {
        let line = record(log::Level::Warn, "xgame_core::project", "打不开").line();
        assert_eq!(line, "[+1.234s] WARN  xgame_core::project: 打不开");
        // 等级定宽到 5 列：INFO / WARN / ERROR 三种长度的 target 必须从同一列开始。
        let start = |level| record(level, "xgame", "m").line().find("xgame").unwrap();
        assert_eq!(start(log::Level::Info), start(log::Level::Warn));
        assert_eq!(start(log::Level::Info), start(log::Level::Error));
        assert_eq!(level_tag(log::Level::Info), "INFO");
        assert_eq!(level_tag(log::Level::Error), "ERROR");
        let err = record(log::Level::Error, "xgame", "炸了").line();
        assert!(err.starts_with("[+1.234s] ERROR xgame: "), "{err}");
    }

    #[test]
    fn a_full_queue_drops_the_oldest_and_counts_it() {
        let capture = Capture::with_capacity(3, None);
        for i in 0..5 {
            capture.push(record(log::Level::Info, "xgame", &format!("m{i}")));
        }
        let taken = capture.take();
        assert_eq!(taken.len(), 3, "队列必须有界");
        // 留下的是最新的三条 —— 用户要看的是"刚才发生了什么"。
        assert_eq!(taken[0].message, "m2");
        assert_eq!(taken[2].message, "m4");
        assert_eq!(capture.dropped(), 2);
        assert!(capture.take().is_empty(), "取走后必须清空");
    }

    #[test]
    fn records_are_written_to_disk_even_when_the_queue_overflows() {
        let dir = temp_dir("overflow");
        let path = dir.join(FILE_NAME);
        let capture = Capture::with_capacity(1, Some(FileOut::open(path.clone())));
        for i in 0..5 {
            capture.push(record(log::Level::Warn, "xgame", &format!("m{i}")));
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 5, "队列丢了条也要落盘：{text}");
        assert!(text.contains("m0") && text.contains("m4"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_log_file_rotates_once_and_keeps_the_previous_half() {
        let dir = temp_dir("rotate");
        let path = dir.join(FILE_NAME);
        let rotated = dir.join(ROTATED_NAME);
        // 每行 26 字节（25 + 换行），上限 150 → 第 6 行（line-5）写入时必然轮转。
        let mut out = FileOut::with_limit(path.clone(), 150);
        for i in 0..9 {
            out.write_line(&format!("line-{i}-xxxxxxxxxxxxxxxxxxxx"));
        }
        assert!(rotated.exists(), "超过上限必须轮转");
        let current = std::fs::read_to_string(&path).unwrap();
        let old = std::fs::read_to_string(&rotated).unwrap();
        assert!(
            current.len() as u64 <= 150,
            "轮转后的新文件不能超过上限：{}",
            current.len()
        );
        // 备份里是**更早**的内容，当前文件是**更晚**的 —— 别把顺序写反。
        assert!(old.contains("line-0"), "{old}");
        assert!(!old.contains("line-5"), "备份不该含轮转后的行：{old}");
        assert!(current.contains("line-8"), "{current}");
        // 只留一份备份：再写两轮也不能出现 Editor.log.2。
        for i in 9..30 {
            out.write_line(&format!("line-{i}-xxxxxxxxxxxxxxxxxxxx"));
        }
        assert!(rotated.exists(), "轮转过的备份还得在");
        assert!(!dir.join("Editor.log.2").exists(), "只留一份备份");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_single_huge_line_is_still_written_instead_of_looping_forever() {
        let dir = temp_dir("huge");
        let path = dir.join(FILE_NAME);
        let mut out = FileOut::with_limit(path.clone(), 64);
        let huge = "x".repeat(500);
        out.write_line(&huge);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.trim_end().len(), 500, "超长单行必须照写");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unwritable_path_degrades_to_no_file_instead_of_panicking() {
        // 把目录当文件用：必然打不开。降级 = 不写盘，控制台照旧。
        let dir = temp_dir("unwritable");
        let mut out = FileOut::open(dir.clone());
        out.write_line("hello");
        assert!(out.file.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
