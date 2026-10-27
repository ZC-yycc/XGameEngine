//! 原子写盘与遗留暂存回收。
//!
//! # 为什么要有这个模块
//!
//! 半截文件比写失败更糟：读的一方分不清"文件坏了"和"文件本来就这样"。
//! 所以所有落盘都走**同目录独占暂存 → 落盘同步 → 替换目标**：替换是原子的，
//! 目标要么是旧内容、要么是完整的新内容。
//!
//! 这条路径原来在 `core::project`、`editor::prefs`、`input::action` 各抄了一份，
//! 而且三份的差错点各不相同（一份补建父目录、一份支持"只发布到空位"、
//! 一份两者都没有）。写盘可靠性是 G0 的头等项，抄三遍等于三处漏，
//! 所以收敛到这一个实现，调用方只做错误类型映射。
//!
//! # 命名约定
//!
//! 暂存文件叫 `<主名>.<扩展名>.tmp.<进程号>.<序号>`（没有扩展名时是
//! `<主名>.tmp.<进程号>.<序号>`）。两段数字是**回收判据**：只要有崩溃残留，
//! 后面的保存会顺手把超龄的那些删掉（见 [`sweep_staging`]）。
//! 因此改命名必须同步 `is_staging_name`，否则垃圾会重新开始堆积。

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// 暂存文件超过这个年龄才算"遗留"。
///
/// 阈值的存在是为了不误删**并发写入者**正在用的那份：本引擎的落盘都是小文本，
/// 一次写完远不到 10 分钟；真超过这个时间还没替换的，基本只能是崩溃残留。
pub const STAGING_MAX_AGE: Duration = Duration::from_secs(600);

/// 原子写：写临时文件、同步、再替换目标。父目录不存在时补建。
///
/// 任何一步失败都返回错误并清掉暂存，**绝不退回直接覆写** ——
/// 那条路在替换失败时会把旧文件变成半截。
pub fn write_atomic(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    publish(path, contents.as_ref(), false)
}

/// 原子写，但只发布到**尚未存在**的名字。
///
/// 用于首次分配资产 GUID：`rename` 在 Windows 上会覆盖目标，
/// 一旦并发进程抢先登记，覆盖就等于把对方刚写下的 GUID 抹掉。
/// 这里用 `hard_link` 创建，目标已存在时由文件系统直接拒绝。
pub fn write_atomic_new(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    publish(path, contents.as_ref(), true)
}

/// 回收 `dir` 里超龄的暂存文件，返回删除数量。失败一律忽略（尽力而为）。
pub fn sweep_staging(dir: &Path) -> usize {
    sweep_staging_older_than(dir, STAGING_MAX_AGE)
}

/// 同上，但自定义年龄阈值（测试用：不想靠改文件时间就能覆盖两条分支）。
pub fn sweep_staging_older_than(dir: &Path, max_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_staging_name(name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            // 同名目录不是我们建的；删不掉也不该尝试。
            continue;
        }
        // 时间戳拿不到或早于当前时钟（时钟被回拨）都当作"不确定"：宁可留着。
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age < max_age {
            continue;
        }
        if std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 文件名是否是本模块写下的暂存文件。
///
/// 判据是**收尾必须恰好是 `.tmp.<数字>.<数字>`**。单段数字（`notes.tmp.5`）
/// 和旧版本的固定暂存名（`scene.ron.tmp`）都不算 —— 它们与用户自己的文件
/// 无法区分，宁可留着让人手删，也不能猜。
pub fn is_staging_name(name: &str) -> bool {
    let mut parts = name.rsplit('.');
    let (Some(sequence), Some(pid), Some(marker)) = (parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    marker == "tmp" && is_digits(sequence) && is_digits(pid)
}

fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

fn publish(path: &Path, bytes: &[u8], create_only: bool) -> io::Result<()> {
    if let Some(parent) = parent_dir(path) {
        std::fs::create_dir_all(parent)?;
    }
    let (staged, mut file) = create_staging(path)?;
    // 先落盘再替换：替换之后内容必须已经在盘上，否则掉电会得到一个空目标。
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);

    let published = written.and_then(|()| {
        if create_only {
            std::fs::hard_link(&staged, path)?;
            // 发布已经成功，清暂存失败只留下残留，不能把成功误报为写入失败
            // （那条残留会在下次保存时被 `sweep_staging` 收走）。
            let _ = std::fs::remove_file(&staged);
            Ok(())
        } else {
            std::fs::rename(&staged, path)
        }
    });
    if let Err(error) = published {
        let _ = std::fs::remove_file(&staged);
        return Err(error);
    }

    if let Some(parent) = parent_dir(path) {
        sync_dir(parent);
        // 刚保存成功是最自然的清理时机：崩在中途的那份不会有人再来删它。
        sweep_staging(parent);
    }
    Ok(())
}

/// 建一个同目录独占的暂存文件。
fn create_staging(path: &Path) -> io::Result<(PathBuf, std::fs::File)> {
    static NEXT_STAGING: AtomicU64 = AtomicU64::new(0);
    loop {
        let id = NEXT_STAGING.fetch_add(1, Ordering::Relaxed);
        let staged = staged_path(path, id);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)
        {
            Ok(file) => return Ok((staged, file)),
            // 上一次运行用掉了同一个进程号：换个序号就是了。
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(source),
        }
    }
}

fn staged_path(path: &Path, sequence: u64) -> PathBuf {
    let suffix = format!("tmp.{}.{sequence}", std::process::id());
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => path.with_extension(format!("{ext}.{suffix}")),
        None => path.with_extension(suffix),
    }
}

fn parent_dir(path: &Path) -> Option<&Path> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
}

/// 让"替换"这个动作本身在断电后仍然可见。
///
/// 数据文件自己 `sync_all` 只保证内容落盘，目录项还是可能丢，所以 POSIX 上
/// 要再同步一次父目录。Windows 上标准库打不开目录句柄（需要
/// `FILE_FLAG_BACKUP_SEMANTICS`，std 没有暴露），这里退化成空操作 ——
/// NTFS 的元数据日志覆盖了绝大多数掉电场景，但没有 fsync 目录这条硬保证。
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    {
        if let Ok(handle) = std::fs::File::open(dir) {
            let _ = handle.sync_all();
        }
        let _ = dir;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("xgame-fsx-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn age(path: &Path, ago: Duration) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        let when = SystemTime::now() - ago;
        file.set_times(std::fs::FileTimes::new().set_modified(when))
            .unwrap();
    }

    #[test]
    fn staging_names_are_matched_exactly() {
        for name in [
            "scene.ron.tmp.1234.0",
            "scene.ron.tmp.1.99",
            "project.tmp.42.7",
            "foo.prefab.ron.tmp.7.3",
        ] {
            assert!(is_staging_name(name), "应识别为暂存: {name}");
        }
        for name in [
            "scene.ron",         // 正常文件
            "scene.ron.tmp",     // 旧版本的固定暂存名
            "notes.tmp.5",       // 只有一段数字
            "scene.ron.tmp.7",   // 只有一段数字
            "scene.ron.tmp..7",  // 进程号为空
            "scene.ron.tmp.7.x", // 序号不是数字
            "scene.tmp.7.3.bak", // 数字不在收尾
            "tmp",
            "",
        ] {
            assert!(!is_staging_name(name), "不能当成暂存: {name}");
        }
    }

    #[test]
    fn sweep_removes_only_aged_staging_files() {
        let dir = temp_dir("sweep");
        let fresh = dir.join("scene.ron.tmp.1.0");
        let old = dir.join("scene.ron.tmp.1.1");
        let user = dir.join("notes.tmp");
        let legacy = dir.join("scene.ron.tmp");
        let keep = dir.join("scene.ron");
        for path in [&fresh, &old, &user, &legacy, &keep] {
            std::fs::write(path, b"x").unwrap();
        }
        age(&old, Duration::from_secs(3600));
        age(&user, Duration::from_secs(3600));
        age(&legacy, Duration::from_secs(3600));

        assert_eq!(sweep_staging(&dir), 1, "只该回收那一个超龄暂存");
        assert!(!old.exists());
        for path in [&fresh, &user, &legacy, &keep] {
            assert!(path.exists(), "不该动 {}", path.display());
        }
        // 什么都不剩时再扫一遍是 0，不是错误。
        assert_eq!(sweep_staging(&dir), 0);
        // 不存在的目录也只是 0。
        assert_eq!(sweep_staging(&dir.join("missing")), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn zero_age_sweeps_every_staging_file() {
        let dir = temp_dir("sweep_all");
        let staged = dir.join("a.ron.tmp.9.9");
        std::fs::write(&staged, b"x").unwrap();
        assert_eq!(sweep_staging_older_than(&dir, Duration::ZERO), 1);
        assert!(!staged.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn write_leaves_no_staging_behind() {
        let dir = temp_dir("clean");
        let path = dir.join("sub").join("scene.ron");
        write_atomic(&path, "内容").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "内容");
        let names: Vec<String> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["scene.ron"], "父目录应被补建且不残留暂存");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn write_new_refuses_existing_target() {
        let dir = temp_dir("new_only");
        let path = dir.join("asset.meta");
        write_atomic_new(&path, "第一个").unwrap();
        let error = write_atomic_new(&path, "第二个").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "第一个");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn successful_write_sweeps_aged_staging_in_the_same_dir() {
        let dir = temp_dir("gc_on_save");
        let path = dir.join("scene.ron");
        write_atomic(&path, "旧").unwrap();
        let abandoned = dir.join("scene.ron.tmp.1.0");
        std::fs::write(&abandoned, "崩在中途".as_bytes()).unwrap();
        age(&abandoned, Duration::from_secs(3600));
        write_atomic(&path, "新").unwrap();
        assert!(!abandoned.exists(), "保存成功应顺手回收超龄暂存");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "新");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn overwrite_replaces_content_atomically() {
        let dir = temp_dir("overwrite");
        let path = dir.join("file.ron");
        write_atomic(&path, "一二三四五").unwrap();
        write_atomic(&path, "短").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "短");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn locked_target_keeps_old_bytes_and_cleans_staging() {        use std::os::windows::fs::OpenOptionsExt;
        let dir = temp_dir("locked");
        let path = dir.join("scene.ron");
        write_atomic(&path, "旧内容").unwrap();
        // 允许读写但拒绝删除/替换：直接覆写的实现会在这里悄悄成功。
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&path)
            .unwrap();
        assert!(write_atomic(&path, "新内容").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "旧内容");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "scene.ron")
            .collect();
        assert!(leftovers.is_empty(), "失败路径不该留下暂存: {leftovers:?}");
        drop(lock);
        // 释放后可以重试成功。
        write_atomic(&path, "新内容").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "新内容");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 并发写同一个目标：读者要么读到旧内容、要么读到某一次完整的新内容，
    /// 不会读到半截；最后也没留下暂存。
    ///
    /// 这条钉的是"独占暂存 + 替换"在**多写者**下的行为，以及回收扫描不会误删
    /// 别人还在用的那份（阈值 600 秒，刚建出来的暂存一律不动）。
    ///
    /// 读失败（Windows 上替换瞬间的共享冲突）**不算**问题：那是"文件正在被换掉"，
    /// 不是"内容坏了"。所以只对读成功的结果做断言。
    #[test]
    fn concurrent_writers_never_expose_a_half_written_file() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = temp_dir("concurrent");
        let path = dir.join("scene.ron");
        write_atomic(&path, "seed").unwrap();
        let writers = 4;
        let remaining = std::sync::Arc::new(AtomicUsize::new(writers));

        std::thread::scope(|scope| {
            for id in 0..writers {
                let path = path.clone();
                let remaining = remaining.clone();
                scope.spawn(move || {
                    for round in 0..50u32 {
                        write_atomic(&path, format!("writer {id} round {round}")).unwrap();
                    }
                    remaining.fetch_sub(1, Ordering::Relaxed);
                });
            }
            let path = path.clone();
            let remaining = remaining.clone();
            scope.spawn(move || {
                // 写者全部收工就退出，不靠外部信号（否则自己等自己，死锁）。
                while remaining.load(Ordering::Relaxed) > 0 {
                    let Ok(text) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    assert!(
                        text == "seed"
                            || (text.starts_with("writer ") && text.contains(" round ")),
                        "读到了半截内容: {text:?}"
                    );
                }
            });
        });

        let leftovers: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "scene.ron")
            .collect();
        assert!(leftovers.is_empty(), "并发写入不该留下暂存: {leftovers:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 成本测量（默认 ignore，不进 CI 门禁）。
    ///
    /// 为什么要有它：回收扫描挂在每次保存成功之后，必须能说清它值多少时间。
    /// 实测见决策记录 §四十九；默认不跑是因为绝对数字依赖机型。
    #[test]
    #[ignore = "手动成本测量：cargo test -p xgame-core --release --lib fsx::tests::sweep_cost -- --ignored --nocapture"]
    fn sweep_cost() {
        let dir = temp_dir("measure");
        for i in 0..100 {
            std::fs::write(dir.join(format!("asset{i}.ron")), b"x").unwrap();
        }
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            let _ = sweep_staging(&dir);
        }
        println!("SWEEP: 1000x 扫描 100 个条目 = {:?}", start.elapsed());
        let path = dir.join("scene.ron");
        let start = std::time::Instant::now();
        for i in 0..200 {
            write_atomic(&path, format!("content {i}")).unwrap();
        }
        println!("SAVES: 200 次写盘 = {:?}", start.elapsed());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
