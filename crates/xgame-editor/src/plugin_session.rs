//! 插件会话：已加载 DLL 句柄的保活 + 每次 Play 的注册表重建。
//!
//! # 为什么旧 DLL 永不卸载
//!
//! Windows 上 `FreeLibrary` 之后，DLL 里的静态析构、TLS 析构仍可能在进程退出
//! 时跑（或已经跑过），而 Rust 的 `libloading::Library::drop` 就是 `FreeLibrary`。
//! 更要命的是：只要还有一个 `Box<dyn Lifecycle>` 组件实例活着，它的 vtable 就
//! 指进已卸载的代码区 —— 下一次方法调用是未定义行为。
//!
//! 编辑器**无法证明**"没有实例活着"（用户可能正 Play 到一半、可能有后台线程
//! 在跑），所以采取 UE Hot Reload 同款策略：旧代句柄只增不减，泄漏到进程结束。
//! 代价是每换一代多几 MB 的镜像映射，换来"永不悬垂"的硬保证 —— 这是计划文档
//! 风险表里已经接受的 Rust/Windows 限制。
//!
//! # 每次 Play 重建注册表
//!
//! 组件实例可能持有上次 Play 的状态（比如"已旋转的角度"），复用旧注册表会让
//! "停止再播放"回不到初始状态。所以每次 Play 都要一份**全新的**
//! [`BehaviorRegistry`]：在已加载的 DLL 上重跑一次入口（`register_into`，
//! 微秒级），不重新 dlopen。
//!
//! # Drop 顺序是硬约束
//!
//! `EditorState` 里本类型的字段必须声明在 `game_runtime` **之后**：struct 字段
//! 按声明顺序 drop，`World` 里的组件实例与注册表先销毁、DLL 句柄后销毁，函数
//! 指针才不会在 drop 过程中悬垂。即便如此，[`Drop`] 实现仍刻意 `mem::forget`
//! 所有句柄 —— 进程退出时 FreeLibrary 跑 DLL 静态析构没有安全保证。

use std::path::{Path, PathBuf};

use xgame::plugin::{load_plugin, LoadedPlugin};
use xgame::prelude::BehaviorRegistry;

use super::plugin_build::{collect_dylibs, temp_copy, PLUGINS_DIR};

/// 编辑器会话内的插件宿主状态。
pub struct PluginSession {
    /// 当前生效的一代插件句柄（按文件名排序加载，顺序稳定）。
    current: Vec<LoadedPlugin>,
    /// 当前代注册过的组件名。缓存下来是因为 Inspector 在**编辑态**就要渲染
    /// 下拉菜单，而那时没有注册表在手。
    names: Vec<String>,
    /// 与 [`names`](Self::names) 同一批装配出来的注册表（`load_from_dir` 里
    /// 本来就在手，原先读完名字就丢掉了）。
    ///
    /// # 与 [`Self::build_registry`] 的分工
    ///
    /// - **本字段**：常驻缓存，给**每帧都要走**的路径（Inspector 查字段表、
    ///   惰性构造编辑态组件副本）—— 一次 `HashMap` 查询，不重跑插件入口。
    /// - **`build_registry()`**：每次 Play 现造一份，语义不变（见模块文档：
    ///   入口函数可能带副作用，"停止再播放"必须从干净状态开始）。
    ///
    /// 为什么不能让每帧的读也走 `build_registry()`：入口重跑失败会
    /// `log::error!`，而"组件名在 `names` 里、重跑却没注册上"的错位会让
    /// 构造每次失败 → 每帧重试 → **每帧一条错误日志**（日志洪水）。
    registry: BehaviorRegistry,
    /// 旧代句柄：永不卸载（见模块文档）。
    _retired: Vec<LoadedPlugin>,
    /// 临时副本计数器：保证 `temp_copy` 的目标文件名进程内唯一。
    copy_counter: u64,
    /// 当前代是从哪几个文件加载的（路径, mtime）——用来判断"要不要重载"。
    sources: Vec<(PathBuf, Option<std::time::SystemTime>)>,
}

impl PluginSession {
    pub fn new() -> Self {
        Self {
            current: Vec::new(),
            names: Vec::new(),
            registry: BehaviorRegistry::new(),
            _retired: Vec::new(),
            copy_counter: 0,
            sources: Vec::new(),
        }
    }

    /// 已注册的组件名（Inspector 的「Behaviors」下拉菜单数据源）。
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// 当前是否有插件句柄在手。
    pub fn is_empty(&self) -> bool {
        self.current.is_empty()
    }

    /// 确保 `<项目>/Plugins/` 里的 DLL 已加载且是当前内容；有变化才重载。
    ///
    /// 幂等：内容没变（路径 + mtime 全同）时直接返回，不碰文件系统以外的开销。
    /// 返回 `(组件名, 是否发生了重载)`。加载失败的 DLL 记日志跳过 —— 一个坏
    /// 插件不该让整个编辑器起不来。
    pub fn ensure_loaded(&mut self, project_root: &Path) -> (Vec<String>, bool) {
        let sources = super::plugin_build::plugin_dll_mtimes(project_root);
        if sources == self.sources {
            return (self.names.clone(), false);
        }
        if sources.is_empty() && self.current.is_empty() {
            // 从"没有插件"到"没有插件"，没什么可做的（也别清空 sources，
            // 否则每帧都会走一遍这个分支 —— 记住空扫描的结果即可）。
            self.sources = sources;
            return (Vec::new(), false);
        }
        let names = self.load_from_dir(project_root.join(PLUGINS_DIR));
        self.sources = super::plugin_build::plugin_dll_mtimes(project_root);
        (names, true)
    }

    /// 扫描 `dir`，把其中所有 DLL 复制为临时副本后加载；旧代整体退役。
    ///
    /// 返回注册成功的组件名（加载失败的已记日志跳过）。
    fn load_from_dir(&mut self, dir: PathBuf) -> Vec<String> {
        let dlls = collect_dylibs(&dir);
        // 旧代只增不减：挪进 _retired 保活，绝不 drop（见模块文档）。
        let old = std::mem::take(&mut self.current);
        self._retired.extend(old);

        let mut registry = BehaviorRegistry::new();
        let mut loaded = Vec::new();
        for dll in &dlls {
            match temp_copy(dll, &mut self.copy_counter) {
                Ok(temp) => match load_plugin(&temp, &mut registry) {
                    Ok(handle) => {
                        log::info!(
                            "已加载插件 {}（临时副本 {}）",
                            dll.display(),
                            temp.display()
                        );
                        loaded.push(handle);
                    }
                    Err(e) => {
                        log::error!("插件 {} 加载失败，已跳过：{e}", dll.display());
                        // 握手失败的库句柄已随 Err 返回被 drop（不会泄漏）；
                        // 副本文件留着没用，删掉（失败也无所谓）。
                        let _ = std::fs::remove_file(&temp);
                    }
                },
                Err(e) => log::error!("插件 {} 复制失败，已跳过：{e}", dll.display()),
            }
        }

        let names: Vec<String> = registry.names().into_iter().map(str::to_owned).collect();
        self.current = loaded;
        self.names = names.clone();
        // 名字和注册表必须**同批**：分两处装配早晚会出现"名字在、表里没有"。
        self.registry = registry;
        names
    }

    /// 按注册名取字段表；未注册返回 `None`。
    ///
    /// 走常驻缓存，可以每帧调（见 [`Self::registry`] 的说明）。
    pub fn field_table(&self, name: &str) -> Option<&'static [xgame::FieldDesc]> {
        self.registry.field_table(name)
    }

    /// 按注册名构造一个组件实例（编辑态调参用）；未注册返回 `None`。
    ///
    /// 同样走常驻缓存 —— 不重跑插件入口，避免入口失败时**每帧**一条错误日志。
    pub fn construct(&self, name: &str) -> Option<Box<dyn xgame::Behavior>> {
        self.registry.construct(name)
    }

    /// 测试注入：绕过 DLL 直接往常驻注册表里塞一个组件。
    ///
    /// 生产路径的注册只来自插件 DLL（编辑器不链接用户代码），而单测没有 DLL
    /// 可加载 —— 这条是给 `state.rs` 测试里"编辑副本要吃到存盘参数"这类断言
    /// 用的后门，`#[cfg(test)]` 保证它进不了发行版。
    #[cfg(test)]
    pub(crate) fn register_for_test<T>(&mut self, name: &'static str)
    where
        T: xgame::Behavior + Default + 'static,
    {
        self.registry.register::<T>(name);
        let mut names: Vec<String> = self
            .registry
            .names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        names.sort();
        self.names = names;
    }

    /// 用当前代插件装配一份**全新的**行为注册表（每次 Play 调用）。
    ///
    /// 重跑入口只是一次 dlsym + 握手 + 注册闭包调用，微秒级 —— 换来的是
    /// "停止再播放"永远从干净状态开始。某个插件恰好在这次重跑中失败时记日志
    /// 继续：那一部分组件本次不生效，但不该拖垮整个 Play（崩溃隔离）。
    pub fn build_registry(&self) -> BehaviorRegistry {
        let mut registry = BehaviorRegistry::new();
        for plugin in &self.current {
            if let Err(e) = plugin.register_into(&mut registry) {
                log::error!(
                    "插件 {} 重注册失败，本次 Play 其组件不生效：{e}",
                    plugin.path().display()
                );
            }
        }
        registry
    }
}

impl Default for PluginSession {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PluginSession {
    fn drop(&mut self) {
        // 刻意泄漏全部句柄：此刻无法保证没有活实例 / 后台线程指进这些 DLL，
        // FreeLibrary 会让它们变悬垂。进程退出时由 OS 统一回收。
        let current = std::mem::take(&mut self.current);
        let retired = std::mem::take(&mut self._retired);
        std::mem::forget(current);
        std::mem::forget(retired);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_build::{sync_plugins_dir, temp_copy};

    /// `ensure_loaded` 必须是幂等的：内容没变不重载，变了才重载。
    /// （旧实现若每次 Play 都重载，世代泄漏会无限累积 —— 这条测试守着它。）
    #[test]
    fn ensure_loaded_is_idempotent() {
        let root = std::env::temp_dir().join(format!("xgame-sess-{}", std::process::id()));
        let plugins = root.join(PLUGINS_DIR);
        std::fs::create_dir_all(&plugins).unwrap();

        let mut session = PluginSession::new();

        // 空目录：第一次扫描记录"空"，第二次不重载。
        let (names, reloaded) = session.ensure_loaded(&root);
        assert!(names.is_empty() && !reloaded);
        let (_, reloaded) = session.ensure_loaded(&root);
        assert!(!reloaded, "空目录扫描结果没变，不该重载");

        // 放一个**故意坏掉**的 DLL（不是合法动态库）：加载失败记日志跳过，
        // 但仍然算"内容变了"，触发一次重载尝试。
        let fake = plugins.join("broken.dll");
        std::fs::write(&fake, b"definitely not a dll").unwrap();
        let (names, reloaded) = session.ensure_loaded(&root);
        assert!(reloaded, "DLL 集合变了必须重载");
        assert!(names.is_empty(), "坏 DLL 不该注册出组件");

        // 内容没变 ⇒ 不再重载。
        let (_, reloaded) = session.ensure_loaded(&root);
        assert!(!reloaded, "内容没变不该重载");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 临时副本必须是**唯一**文件，且内容与源一致。
    #[test]
    fn temp_copy_is_unique_and_faithful() {
        let tmp = std::env::temp_dir().join("xgame-plugin-cache");
        std::fs::create_dir_all(&tmp).unwrap();
        let src = std::env::temp_dir().join(format!("xg-tc-src-{}.dll", std::process::id()));
        std::fs::write(&src, b"payload").unwrap();

        let mut counter = 0u64;
        let a = temp_copy(&src, &mut counter).unwrap();
        let b = temp_copy(&src, &mut counter).unwrap();
        assert_ne!(a, b, "两次复制必须得到不同路径（否则撞上被锁的旧副本）");
        assert_eq!(std::fs::read(&a).unwrap(), b"payload", "副本内容必须一致");

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    /// `sync_plugins_dir` 把产物落进规范目录 `Plugins/`，文件名原样保留。
    #[test]
    fn sync_places_dlls_in_plugins_dir() {
        let root = std::env::temp_dir().join(format!("xgame-sync-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let built = std::env::temp_dir().join(format!("xgame-built-{}.dll", std::process::id()));
        std::fs::write(&built, b"built").unwrap();

        let synced = sync_plugins_dir(&root, std::slice::from_ref(&built));
        assert_eq!(synced.len(), 1, "应同步一个产物");
        let dst = synced[0].clone();
        assert!(
            dst.starts_with(root.join(PLUGINS_DIR)),
            "必须落在 Plugins/ 下"
        );
        assert_eq!(std::fs::read(&dst).unwrap(), b"built");

        let _ = std::fs::remove_file(&built);
        let _ = std::fs::remove_dir_all(&root);
    }
}
