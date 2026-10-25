//! 项目内容的外部改动检测 —— 资产 / 脚本热重载的"眼睛"。
//!
//! # 为什么是轮询，而不是文件系统通知
//!
//! 文件系统通知（`notify` / `ReadDirectoryChangesW`）要么引一个新依赖，要么自己
//! 写一套平台分支；而且不同编辑器保存文件的方式不一样（直接覆盖 / 写临时文件再
//! 改名 / 先删后建），事件序列并不统一，还得配一层去抖。本项目的内容目录是
//! 清单里显式配置的几棵小树（`Scenes` / `Scripts` / `Prefabs` / `Resources`），
//! 一轮扫描只是一串 `metadata` 调用。**1 Hz 轮询**的代价远小于"多一个平台相关
//! 依赖 + 一套事件去抖逻辑"，而且扫描不读文件内容，测试可以把时间拨快。
//!
//! # 判据是 `(路径, mtime, 长度)`
//!
//! 不读内容、不做哈希：这里要回答的是"值不值得重新加载"，不是"内容是否逐字节
//! 相同"。真正的加载器（glTF / 材质 / Lua）自己有解析与负缓存；这里多报一次
//! 只是多一次重载，漏报才是 bug。长度是给 mtime 打补丁：某些文件系统 / 编辑器的
//! 时间戳粒度粗，快速改一版长度不同的内容时 mtime 可能原地不动。
//!
//! # 为什么先扫描、动作后再 rebase
//!
//! 应用改动时会顺手补 `.meta`（资产库的身份证）。如果只把"这一轮扫描结果"当成
//! 下一轮基线，这些**自己写下的** `.meta` 会在下一秒被当成"外部新增"，形成每
//! 秒一次的自激循环。所以 [`ProjectWatcher::poll`] 只报告、同步推进到本轮扫描
//! 结果，动作跑完后由 [`ProjectWatcher::rebase`] 再扫一遍 —— 本轮由我们造成的
//! 变化一起吞掉。
//!
//! # 什么不在这里管
//!
//! - **场景文件不自动重载**：磁盘上的 `.ron` 变了就覆盖内存，会把未保存的编辑
//!   直接冲掉。Scenes 菜单本来就是现扫的，新场景会自动出现。
//! - **已实例化的预制体不换**：实例是场景里的普通实体，热替换得先定义"哪些
//!   字段该保留"（Unity 也要 Prefab Mode）。这里只刷新预制体面板的清单。
//! - **项目清单 `project.ron`**：窗口 / 渲染 / 关卡顺序是项目级配置，改动生效
//!   需要重建运行时；重启编辑器是最可预期的语义。
//! - **已解码的音频片段**：kira 的解码缓存没有公开失效口，正在播的声音不会换。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use xgame_core::project::ProjectHandle;

/// 两轮扫描的间隔。
///
/// 1 秒是"手一离开保存快捷键就能看到"与"空闲时每秒醒一次"之间的折中。
/// 每次醒来的成本是几棵内容树的 `metadata`，不读文件内容。
pub const SCAN_INTERVAL: Duration = Duration::from_secs(1);

/// 递归深度上限。内容目录按约定是浅树；这个上限只防异常目录结构把扫描拖死。
const MAX_DEPTH: usize = 32;

/// 本引擎自己产生的目录：不参与内容扫描。
///
/// `.autosave/` 必须在这里：它每 60 秒被写一次，扫进来会让热重载永远停不下来。
/// `target` / `Build` / `Plugins` 是构建产物，改了也不该触发资产重载。
const SKIP_DIRS: &[&str] = &[".autosave", "target", "Build", "Plugins", ".git"];

/// 变化文件的类别。决定编辑器该走哪条重载路径。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// `.lua`：运行时重新编译。
    Script,
    /// 场景 `.ron`：只记日志，不覆盖内存。
    Scene,
    /// 预制体 `.prefab.ron`：刷新面板清单。
    Prefab,
    /// 模型 / 贴图 / 材质 / 音频 / `.meta`：刷新资产库并丢渲染缓存。
    Asset,
}

impl Kind {
    /// 同一文件被多个内容根扫到时，取"更具体"的类别。
    fn rank(self) -> u8 {
        match self {
            Self::Script => 3,
            Self::Prefab => 2,
            Self::Scene => 1,
            Self::Asset => 0,
        }
    }
}

/// 一条变化是新增、修改还是删除。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Removed,
}

/// 一条文件变化。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// 项目相对路径（正斜杠）。
    pub path: String,
    pub kind: Kind,
    pub what: ChangeKind,
}

/// 一轮扫描的差异。空集表示"什么都没变"。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    changes: Vec<Change>,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// 逐条遍历（测试断言用）。
    #[cfg(test)]
    pub fn iter(&self) -> impl Iterator<Item = &Change> {
        self.changes.iter()
    }

    /// 指定类别的变化条数（日志用）。
    pub fn count(&self, kind: Kind) -> usize {
        self.changes.iter().filter(|c| c.kind == kind).count()
    }

    pub fn has(&self, kind: Kind) -> bool {
        self.changes.iter().any(|c| c.kind == kind)
    }

    pub fn scripts_changed(&self) -> bool {
        self.has(Kind::Script)
    }

    pub fn assets_changed(&self) -> bool {
        self.has(Kind::Asset)
    }

    pub fn prefabs_changed(&self) -> bool {
        self.has(Kind::Prefab)
    }

    /// 这个项目相对路径是否在本轮变化里（材质草稿重载用）。
    pub fn touched(&self, path: &str) -> bool {
        self.changes.iter().any(|c| c.path == path)
    }
}

/// 一个文件的身份快照。
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    relative: String,
    modified: Option<SystemTime>,
    len: u64,
    kind: Kind,
}

/// 项目内容的外部改动检测器。
///
/// 时间全部由参数注入（不在内部读时钟），于是单测可以"把时间拨到两秒后"
/// 而不必真的等 —— 与 `autosave` 的计时器同一套路。
pub struct ProjectWatcher {
    last: Vec<FileStamp>,
    next_due: Instant,
}

impl ProjectWatcher {
    /// 以当前磁盘状态为基线起步：打开项目时不该把已有文件当成"刚发生变化"。
    pub fn new(project: &ProjectHandle, now: Instant) -> Self {
        Self {
            last: scan(project),
            next_due: now + SCAN_INTERVAL,
        }
    }

    /// 现在是否到点。
    pub fn due_at(&self, now: Instant) -> bool {
        now >= self.next_due
    }

    /// 主循环拿它去 `request_repaint_after`：界面静止时帧调度会停帧，
    /// 没人来调 [`poll`](Self::poll) 就永远发现不了外部改动。
    pub fn wait(&self, now: Instant) -> Duration {
        self.next_due.saturating_duration_since(now)
    }

    /// 到点则扫描并返回与上一轮的差异；没到点返回空集。
    ///
    /// 无论有没有差异，基线都会推进到本轮扫描结果 —— 调用方之后若写了文件
    /// （补 `.meta`），还要再调 [`rebase`](Self::rebase) 把它吞掉。
    pub fn poll(&mut self, project: &ProjectHandle, now: Instant) -> ChangeSet {
        if !self.due_at(now) {
            return ChangeSet::default();
        }
        self.next_due = now + SCAN_INTERVAL;
        let current = scan(project);
        let changes = diff(&self.last, &current);
        self.last = current;
        changes
    }

    /// 动作跑完后重扫：把本轮自己写下的文件（`ensure_metas` 生成的 `.meta`）
    /// 收进基线，避免下一秒被当成外部改动再触发一次。
    pub fn rebase(&mut self, project: &ProjectHandle) {
        self.last = scan(project);
    }
}

/// 扫描项目内容目录，返回按项目相对路径排序的快照。
///
/// 只扫清单里声明的目录：项目根下的其它东西（`.autosave/`、`Build/`、用户
/// 自己放的工具脚本）不进入判据。重叠的目录（默认 `Resources` 同时是模型 /
/// 贴图 / 音频根）只扫一遍。
fn scan(project: &ProjectHandle) -> Vec<FileStamp> {
    let content = &project.manifest().content;
    let mut roots: BTreeMap<PathBuf, Kind> = BTreeMap::new();
    let mut push = |dir: PathBuf, kind: Kind| {
        roots
            .entry(dir)
            .and_modify(|existing| {
                if kind.rank() > existing.rank() {
                    *existing = kind;
                }
            })
            .or_insert(kind);
    };
    push(project.scripts_dir(), Kind::Script);
    push(project.scenes_dir(), Kind::Scene);
    push(project.prefabs_dir(), Kind::Prefab);
    for dir in [
        &content.models_dir,
        &content.textures_dir,
        &content.audio_dir,
        &content.shaders_dir,
    ] {
        push(project.path(dir), Kind::Asset);
    }

    let root = project.root();
    let mut out = BTreeMap::<String, FileStamp>::new();
    for (dir, kind) in roots {
        walk(root, &dir, kind, 0, &mut out);
    }
    out.into_values().collect()
}

/// 递归收集一个内容根下的文件。
///
/// 不跟随符号链接：内容目录里链到父目录（或项目根）会形成环，`read_dir` 递归
/// 就再也回不来了。跳过链接的代价是"链接进来的资产不会被热检测"，可以接受。
fn walk(
    root: &Path,
    dir: &Path,
    root_kind: Kind,
    depth: usize,
    out: &mut BTreeMap<String, FileStamp>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if SKIP_DIRS.iter().any(|skip| name.eq_ignore_ascii_case(skip)) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            if !meta.file_type().is_symlink() {
                walk(root, &path, root_kind, depth + 1, out);
            }
            continue;
        }
        if !meta.is_file() {
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));
        let kind = classify(&path, root_kind);
        let stamp = FileStamp {
            relative: relative.clone(),
            modified: meta.modified().ok(),
            len: meta.len(),
            kind,
        };
        // 同一文件被两个根扫到（例如把 Scripts 放进 Resources）时取更具体的类别。
        out.entry(relative)
            .and_modify(|old| {
                if kind.rank() > old.kind.rank() {
                    old.kind = kind;
                }
            })
            .or_insert(stamp);
    }
}

/// 按扩展名分类；`root_kind` 只用来判断"这个 `.ron` / `.lua` 是不是内容根里的"。
fn classify(path: &Path, root_kind: Kind) -> Kind {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name.ends_with(".prefab.ron") {
        Kind::Prefab
    } else if name.ends_with(".ron") {
        if root_kind == Kind::Scene {
            Kind::Scene
        } else {
            Kind::Asset
        }
    } else if name.ends_with(".lua") {
        if root_kind == Kind::Script {
            Kind::Script
        } else {
            Kind::Asset
        }
    } else {
        Kind::Asset
    }
}

/// 比较两份快照，产出增 / 改 / 删。
fn diff(old: &[FileStamp], new: &[FileStamp]) -> ChangeSet {
    use std::cmp::Ordering;

    let mut changes = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < old.len() && j < new.len() {
        match old[i].relative.cmp(&new[j].relative) {
            Ordering::Equal => {
                if old[i].modified != new[j].modified || old[i].len != new[j].len {
                    changes.push(Change {
                        path: new[j].relative.clone(),
                        kind: new[j].kind,
                        what: ChangeKind::Modified,
                    });
                }
                i += 1;
                j += 1;
            }
            Ordering::Less => {
                changes.push(Change {
                    path: old[i].relative.clone(),
                    kind: old[i].kind,
                    what: ChangeKind::Removed,
                });
                i += 1;
            }
            Ordering::Greater => {
                changes.push(Change {
                    path: new[j].relative.clone(),
                    kind: new[j].kind,
                    what: ChangeKind::Added,
                });
                j += 1;
            }
        }
    }
    for stamp in &old[i..] {
        changes.push(Change {
            path: stamp.relative.clone(),
            kind: stamp.kind,
            what: ChangeKind::Removed,
        });
    }
    for stamp in &new[j..] {
        changes.push(Change {
            path: stamp.relative.clone(),
            kind: stamp.kind,
            what: ChangeKind::Added,
        });
    }
    ChangeSet { changes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 临时项目（用完删目录）。
    fn temp_project(tag: &str) -> (ProjectHandle, PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "xgame_hot_reload_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let project = ProjectHandle::create(&base, "HotReloadTest").expect("创建临时项目");
        (project, base)
    }

    fn write(project: &ProjectHandle, relative: &str, contents: &str) {
        let path = project.path(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    /// 时间拨到第 `secs` 秒（基线在构造时建立，间隔 1 秒）。
    fn at(t0: Instant, secs: u64) -> Instant {
        t0 + Duration::from_secs(secs)
    }

    /// 新增 / 修改 / 删除都在一轮里报出来；脚本类别的判定正确。
    #[test]
    fn reports_added_modified_and_removed_files() {
        let (project, base) = temp_project("diff");
        write(&project, "Scripts/a.lua", "function start(ctx) end");
        let t0 = Instant::now();
        let mut watcher = ProjectWatcher::new(&project, t0);

        write(
            &project,
            "Scripts/a.lua",
            "function start(ctx) -- 改长了\nend",
        );
        write(&project, "Scripts/sub/b.lua", "function update(ctx) end");
        let changes = watcher.poll(&project, at(t0, 2));

        assert_eq!(changes.count(Kind::Script), 2, "{changes:?}");
        assert!(changes.touched("Scripts/a.lua"));
        assert!(changes.touched("Scripts/sub/b.lua"));
        assert!(changes.scripts_changed());
        assert!(
            changes
                .iter()
                .any(|c| c.what == ChangeKind::Modified && c.path == "Scripts/a.lua"),
            "改内容应报 Modified: {changes:?}"
        );
        assert!(
            changes
                .iter()
                .any(|c| c.what == ChangeKind::Added && c.path == "Scripts/sub/b.lua"),
            "新文件（含子目录）应报 Added: {changes:?}"
        );

        std::fs::remove_file(project.path("Scripts/a.lua")).unwrap();
        let changes = watcher.poll(&project, at(t0, 4));
        assert!(
            changes
                .iter()
                .any(|c| c.what == ChangeKind::Removed && c.path == "Scripts/a.lua"),
            "删除应报 Removed: {changes:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 没到点不扫；到点但没变化返回空集；`wait` 永远落在 [0, 1s]。
    #[test]
    fn unchanged_project_reports_nothing() {
        let (project, base) = temp_project("quiet");
        let t0 = Instant::now();
        let mut watcher = ProjectWatcher::new(&project, t0);

        assert!(!watcher.due_at(at(t0, 0)), "刚构造时不该立刻到点");
        assert!(watcher.poll(&project, at(t0, 0)).is_empty());
        assert!(
            watcher.wait(at(t0, 0)) <= SCAN_INTERVAL,
            "唤醒间隔不能超过扫描间隔"
        );

        assert!(
            watcher.poll(&project, at(t0, 2)).is_empty(),
            "没变化就该是空集"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 模型 / 贴图 / `.meta` 都算资产变化；`.meta` 是导入设置的载体，必须算。
    #[test]
    fn assets_and_meta_files_are_asset_changes() {
        let (project, base) = temp_project("assets");
        let t0 = Instant::now();
        let mut watcher = ProjectWatcher::new(&project, t0);

        write(&project, "Resources/Box.glb", "glb");
        write(&project, "Resources/Box.glb.meta", "guid");
        write(&project, "Resources/Wood.mat", "mat");
        let changes = watcher.poll(&project, at(t0, 2));

        assert_eq!(changes.count(Kind::Asset), 3, "{changes:?}");
        assert!(changes.assets_changed());
        assert!(!changes.scripts_changed());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 预制体是双后缀，不能被当成场景；场景也不能被当成普通资产。
    #[test]
    fn prefab_and_scene_are_classified_apart() {
        let (project, base) = temp_project("kinds");
        let t0 = Instant::now();
        let mut watcher = ProjectWatcher::new(&project, t0);

        write(&project, "Prefabs/Tower.prefab.ron", "prefab");
        write(&project, "Scenes/Level2.ron", "scene");
        let changes = watcher.poll(&project, at(t0, 2));

        assert_eq!(changes.count(Kind::Prefab), 1, "{changes:?}");
        assert_eq!(changes.count(Kind::Scene), 1, "{changes:?}");
        assert_eq!(
            changes.count(Kind::Asset),
            0,
            ".ron 不该混进资产: {changes:?}"
        );
        assert!(changes.prefabs_changed());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 自动保存 / 构建产物即使藏在内容目录里也不参与扫描。
    #[test]
    fn autosave_and_build_products_are_ignored() {
        let (project, base) = temp_project("ignored");
        let t0 = Instant::now();
        let mut watcher = ProjectWatcher::new(&project, t0);

        write(&project, "Resources/.autosave/Scenes/x.ron", "copy");
        write(&project, "Resources/target/foo.txt", "build");
        write(&project, "Resources/Build/foo.txt", "build");
        write(&project, "Resources/Plugins/foo.dll", "plugin");
        assert!(
            watcher.poll(&project, at(t0, 2)).is_empty(),
            "本引擎自己的产物目录不该触发重载"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// **防自激**：动作写下的 `.meta` 经 `rebase` 后不再被报成外部变化。
    ///
    /// 旧实现（基线与动作不重扫）会在下一秒把 `ensure_metas` 生成的
    /// `.meta` 当新增，于是每秒刷新一次资产库 —— 这条测试钉住 rebase 的必要性。
    #[test]
    fn rebase_absorbs_metas_written_by_the_reload_action() {
        let (project, base) = temp_project("rebase");
        let t0 = Instant::now();
        let mut watcher = ProjectWatcher::new(&project, t0);

        write(&project, "Resources/Box.glb", "glb");
        let changes = watcher.poll(&project, at(t0, 2));
        assert!(changes.assets_changed(), "首次新增应被报出来");

        // 模拟 `refresh_assets` 补写的身份证。
        write(&project, "Resources/Box.glb.meta", "guid");
        watcher.rebase(&project);

        assert!(
            watcher.poll(&project, at(t0, 4)).is_empty(),
            "自己写的 .meta 不能触发第二轮"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
