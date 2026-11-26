//! 同目录组包与发布。稳定的 previous 只由随旧包移动的空目录标记认领。

use std::ffi::OsString;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use xgc_base::path::normalize_project_relative;

use crate::is_link;

use super::{check_package_entries, has_reserved_name, parse_manifest, verify_files, MANIFEST};

const LOCK: &str = ".xgc-pack.lock";
const STAGING_PREFIX: &str = ".xgc-pack-staging-";
const PREVIOUS_PREFIX: &str = ".xgc-pack-previous-";
pub(super) const MARKER: &str = ".xgc-pack-transaction";

pub struct PackTarget {
    pub(super) source: PathBuf,
    pub(super) output: PathBuf,
    previous: PathBuf,
}

impl PackTarget {
    /// 先恢复上一事务，即使之后读取场景或组包出错，正式包也应可用。
    pub fn prepare(source: &Path, world: &Path, output: &Path) -> Result<Self, String> {
        if source.as_os_str().is_empty() {
            return Err("项目根不是目录".into());
        }
        let source = resolve_path(source)?;
        let world = resolve_path(world)?;
        let name = output_name(output)?;
        let parent = resolve_path(output.parent().unwrap_or(Path::new(".")))?;
        let output = parent.join(name);
        protect_source(&source, &world, &output)?;
        let lock = directory_exists(&parent)?
            .then(|| lock(&parent))
            .transpose()?;
        // 持锁查看现有输出，避免另一发布进程恰好把它移走。
        let output = if directory_exists(&output)? {
            fs::canonicalize(&output).map_err(at(&output))?
        } else {
            output
        };
        let mut previous_name = OsString::from(PREVIOUS_PREFIX);
        previous_name.push(output_name(&output)?);
        let previous = parent.join(previous_name);
        protect_source(&source, &world, &output)?;
        protect_source(&source, &world, &previous)?;
        let target = Self {
            source,
            output,
            previous,
        };
        if lock.is_some() {
            target.recover_locked()?;
        }
        Ok(target)
    }

    pub(super) fn staging(&self) -> Result<Staging, String> {
        let parent = self.parent();
        fs::create_dir_all(parent).map_err(at(parent))?;
        Staging::new(parent)
    }

    pub(super) fn release(&self, staging: &Staging) -> Result<Option<String>, String> {
        let _lock = lock(self.parent())?;
        self.recover_locked()?;
        let replace = directory_exists(&self.output)?;
        if replace {
            self.check_owned_output(true)?;
            let marker = self.output.join(MARKER);
            fs::create_dir(&marker).map_err(at(&marker))?;
            if let Err(err) = fs::rename(&self.output, &self.previous) {
                return Err(clear_after_error(&self.output, at(&self.output)(err)));
            }
        }
        if let Err(err) = fs::rename(staging.package(), &self.output) {
            let publish_error = at(&self.output)(err);
            if replace {
                if let Err(err) = fs::rename(&self.previous, &self.output) {
                    return Err(format!(
                        "{publish_error}；回退失败：{err}；旧包保留在 {}",
                        self.previous.display()
                    ));
                }
                return Err(clear_after_error(&self.output, publish_error));
            }
            return Err(publish_error);
        }
        // 新包已经就位。旧包先移出稳定事务路径，递归清理才可中断。
        Ok(replace
            .then(|| staging.retire(&self.previous).err())
            .flatten()
            .map(|err| format!("发布成功，但旧包清理失败：{err}")))
    }

    fn parent(&self) -> &Path {
        self.output.parent().expect("输出路径已校验")
    }

    fn check_owned_output(&self, allow_empty: bool) -> Result<(), String> {
        if allow_empty
            && fs::read_dir(&self.output)
                .map_err(at(&self.output))?
                .next()
                .transpose()
                .map_err(at(&self.output))?
                .is_none()
        {
            return Ok(());
        }
        let manifest = self.output.join(MANIFEST);
        if !metadata(&manifest)?.is_some_and(|meta| meta.is_file() && !is_link(&meta)) {
            return Err(conflict(&self.output));
        }
        let text = fs::read_to_string(&manifest).map_err(at(&manifest))?;
        let manifest = parse_manifest(&text)?;
        check_package_entries(&self.output, &manifest)?;
        if !allow_empty {
            verify_files(&self.output, &manifest).map_err(|errors| errors.join("\n"))?;
        }
        Ok(())
    }

    fn recover_locked(&self) -> Result<(), String> {
        let has_output = directory_exists(&self.output)?;
        let marked = has_output && marker_exists(&self.output)?;
        if !directory_exists(&self.previous)? {
            if marked {
                clear_marker(&self.output)?;
            }
            return Ok(());
        }
        if marked || !marker_exists(&self.previous)? {
            return Err(conflict(&self.previous));
        }
        if has_output {
            // 没有可信的正式包作证，不能认定上次的新包已就位。
            self.check_owned_output(false)?;
            Staging::new(self.parent())?.retire(&self.previous)
        } else {
            fs::rename(&self.previous, &self.output).map_err(at(&self.previous))?;
            clear_marker(&self.output)
        }
    }
}

pub(super) struct Staging(PathBuf);

impl Staging {
    fn new(parent: &Path) -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = parent.join(format!(
                "{STAGING_PREFIX}{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    let marker = path.join(MARKER);
                    if let Err(err) = fs::create_dir(&marker) {
                        let _ = fs::remove_dir(&path);
                        return Err(at(&marker)(err));
                    }
                    return Ok(Self(path));
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(at(&path)(err)),
            }
        }
    }

    pub(super) fn package(&self) -> PathBuf {
        self.0.join("package")
    }

    fn retire(&self, previous: &Path) -> Result<(), String> {
        fs::rename(previous, self.0.join("previous")).map_err(at(previous))
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if directory_exists(&self.0) == Ok(true) && marker_exists(&self.0) == Ok(true) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn lock(parent: &Path) -> Result<File, String> {
    let path = parent.join(LOCK);
    if metadata(&path)?.is_some_and(|meta| !meta.is_file() || is_link(&meta)) {
        return Err(conflict(&path));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(at(&path))?;
    // 只关闭句柄，不能删除锁文件，否则等待者与新发布者可能持有不同的锁。
    file.lock().map_err(at(&path))?;
    Ok(file)
}

fn marker_exists(release: &Path) -> Result<bool, String> {
    let marker = release.join(MARKER);
    if !directory_exists(&marker)? {
        return Ok(false);
    }
    if fs::read_dir(&marker)
        .map_err(at(&marker))?
        .next()
        .transpose()
        .map_err(at(&marker))?
        .is_some()
    {
        return Err(conflict(&marker));
    }
    Ok(true)
}

fn output_name(path: &Path) -> Result<&str, String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|&name| normalize_project_relative(name).as_deref() == Ok(name))
        .filter(|&name| !internal_name(name) && !has_reserved_name(name, &[LOCK, MARKER]))
        .ok_or_else(|| conflict(path))
}

fn protect_source(source: &Path, world: &Path, output: &Path) -> Result<(), String> {
    if within(source, output) || within(world, output) {
        return Err(format!("发布路径覆盖源项目或世界：{}", output.display()));
    }
    Ok(())
}

fn clear_marker(path: &Path) -> Result<(), String> {
    let marker = path.join(MARKER);
    fs::remove_dir(&marker).map_err(at(&marker))
}

fn clear_after_error(path: &Path, error: String) -> String {
    match clear_marker(path) {
        Ok(()) => error,
        Err(err) => format!("{error}；清理事务标记失败：{err}"),
    }
}

fn directory_exists(path: &Path) -> Result<bool, String> {
    match metadata(path)? {
        None => Ok(false),
        Some(meta) if meta.is_dir() && !is_link(&meta) => Ok(true),
        Some(_) => Err(conflict(path)),
    }
}

fn metadata(path: &Path) -> Result<Option<Metadata>, String> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(meta)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(at(path)(err)),
    }
}

/// 解析不存在的输出的最近祖先，源路径比较在创建目录前完成。
fn resolve_path(path: &Path) -> Result<PathBuf, String> {
    let path = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    match fs::canonicalize(path) {
        Ok(resolved) => Ok(resolved),
        Err(err) if err.kind() == io::ErrorKind::NotFound && metadata(path)?.is_none() => {
            let name = path.file_name().ok_or_else(|| at(path)(err))?;
            Ok(resolve_path(path.parent().unwrap_or(Path::new(".")))?.join(name))
        }
        Err(err) => Err(at(path)(err)),
    }
}

pub(super) fn within(path: &Path, dir: &Path) -> bool {
    #[cfg(windows)]
    {
        let path = path.to_string_lossy().to_lowercase();
        let dir = dir.to_string_lossy().to_lowercase();
        Path::new(&path).starts_with(Path::new(&dir))
    }
    #[cfg(not(windows))]
    path.starts_with(dir)
}

pub(super) fn internal_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(internal_name)
}

fn internal_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with(STAGING_PREFIX) || name.starts_with(PREVIOUS_PREFIX)
}

pub(super) fn conflict(path: &Path) -> String {
    format!("发布路径冲突，保留现有内容：{}", path.display())
}

fn at(path: &Path) -> impl FnOnce(io::Error) -> String + '_ {
    move |err| format!("{}：{err}", path.display())
}
