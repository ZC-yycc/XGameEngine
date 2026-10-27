//! 最近打开的项目列表。
//!
//! 持久化到用户配置目录（Windows 下是 `%APPDATA%\XGameEngine\recent_projects.ron`），
//! 让项目枢纽每次启动都能列出上次用过的那几个项目。
//!
//! 这里只存**绝对路径**，不存项目内容——项目本身仍然完全由磁盘上的目录决定。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 最近项目列表上限。
const MAX_RECENTS: usize = 12;

/// 配置文件名。
const RECENTS_FILE_NAME: &str = "recent_projects.ron";

/// 最近打开过的项目路径列表（最近的在最前）。
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RecentProjects {
    pub paths: Vec<PathBuf>,
}

impl RecentProjects {
    /// 从磁盘读取；文件缺失或损坏时返回空列表（不报错——最近列表丢了不是致命问题）。
    ///
    /// 读出来的列表会**裁剪到上限**：磁盘上的文件是外部输入，可能被手改过、也可能是
    /// 旧版本写的（上限曾经不同）。不在入口处夹一次，"列表长度 ≤ MAX_RECENTS" 这条
    /// 不变量就只在 `promote` 里成立，别的路径都靠运气。
    pub fn load() -> Self {
        let Some(path) = store_path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        let mut recents: Self = ron::from_str(&text).unwrap_or_default();
        recents.paths.truncate(MAX_RECENTS);
        recents
    }

    /// 写回磁盘；失败只记警告，不影响编辑器运行。
    pub fn save(&self) {
        let Some(path) = store_path() else {
            return;
        };
        if let Err(e) = self.save_to(&path) {
            log::warn!("无法写入最近项目列表 {}: {e}", path.display());
        }
    }

    /// 指定路径写盘，供失败注入与配置路径隔离测试使用。
    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::prefs::write_staged(path, &text)
    }

    /// 把项目路径提到最前（去重 + 裁剪到上限），然后落盘。
    pub fn add(&mut self, path: &Path) {
        self.promote(path.to_path_buf());
        self.save();
    }

    /// 从列表中移除某个项目。
    ///
    /// 不需要再截断：它只会让列表变短，而"长度 ≤ `MAX_RECENTS`"由 [`load`](Self::load)
    /// 和 [`promote`](Self::promote) 两个入口保证。
    pub fn remove(&mut self, path: &Path) {
        self.paths.retain(|p| p != path);
        self.save();
    }

    /// 只返回磁盘上仍然存在的项目（目录里还有 `project.ron`）。
    pub fn existing(&self) -> impl Iterator<Item = &PathBuf> {
        self.paths
            .iter()
            .filter(|p| p.join(xgame_core::PROJECT_FILE_NAME).is_file())
    }

    /// 纯列表操作，不碰磁盘（便于单测）。
    fn promote(&mut self, path: PathBuf) {
        self.paths.retain(|p| p != &path);
        self.paths.insert(0, path);
        self.paths.truncate(MAX_RECENTS);
    }
}

/// 配置文件路径。取不到用户配置目录时返回 `None`（此时最近列表功能静默关闭）。
fn store_path() -> Option<PathBuf> {
    crate::config::file(RECENTS_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "xgame-recents-{tag}-{}-{}.ron",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[cfg(windows)]
    #[test]
    fn locked_recents_preserve_previous_bytes_then_retry() {
        use std::os::windows::fs::OpenOptionsExt;
        let path = temp_path("locked");
        let old = RecentProjects {
            paths: vec![PathBuf::from("/old")],
        };
        old.save_to(&path).unwrap();
        let before = std::fs::read(&path).unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&path)
            .unwrap();
        let new = RecentProjects {
            paths: vec![PathBuf::from("/new")],
        };
        assert!(new.save_to(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(lock);
        new.save_to(&path).unwrap();
        let persisted: RecentProjects =
            ron::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(persisted.paths, new.paths);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn stale_recents_staging_does_not_block_save() {
        let path = temp_path("stale");
        let legacy = path.with_extension("ron.tmp");
        std::fs::write(&legacy, b"old staging").unwrap();
        let recents = RecentProjects {
            paths: vec![PathBuf::from("/new")],
        };
        recents.save_to(&path).unwrap();
        let persisted: RecentProjects =
            ron::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(persisted.paths, recents.paths);
        assert_eq!(std::fs::read(&legacy).unwrap(), b"old staging");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(legacy).unwrap();
    }

    #[test]
    fn add_dedupes_and_moves_to_front() {
        let mut recents = RecentProjects::default();
        recents.promote(PathBuf::from("/a"));
        recents.promote(PathBuf::from("/b"));
        recents.promote(PathBuf::from("/a"));

        assert_eq!(
            recents.paths,
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }

    #[test]
    fn add_caps_length() {
        let mut recents = RecentProjects::default();
        for i in 0..(MAX_RECENTS + 5) {
            recents.promote(PathBuf::from(format!("/p{i}")));
        }
        assert_eq!(recents.paths.len(), MAX_RECENTS);
    }
}
