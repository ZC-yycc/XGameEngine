//! 运行时从哪装载场景。
//!
//! # 为什么是注入的 trait，而不是让 `World` 直接持有 `ProjectHandle`
//!
//! `World` 的既有契约是"一份场景 + 一个根目录"——测试、无头、独立游戏都按这个
//! 契约构造它，没有人需要项目清单。把 `ProjectHandle` 焊进 `World` 会顺手把
//! "项目布局"也焊进去，而 `World` 只该知道"给我一个相对路径，还我一份场景"。
//!
//! 更实际的理由：**只有宿主知道场景该从哪来**。编辑器 / 独立游戏 / 无头运行
//! 手里都有 `ProjectHandle`（`Scenes/` 下的 RON，资产引用是 `guid://`，
//! 读盘时要解码成项目相对路径）；而绝大多数单元测试只想给一份内存里的
//! `Scene`。注入让两边都成立，且"没有装载器"是一个**可诊断的状态**：
//! 脚本请求切场景时记一条错误日志，而不是静默无事发生。
//!
//! # 为什么 `load_scene` 返回的是**内存态**场景
//!
//! 磁盘上的资产引用是 GUID，内存里是项目相对路径（决策记录 §十四）。
//! 解码只发生在 `ProjectHandle` 的读盘边界 —— 所以装载器交出来的场景必须
//! 已经是解码过的形态，`World` 拿到就能直接跑，不再碰资产索引。

use xgame_core::{ProjectHandle, Scene};

/// 场景装载来源（项目相对路径 → 内存态场景）。
///
/// 实现者必须保证**只读**：`--check` / 无头运行都可能在只读挂载上跑，
/// 装载场景不该顺手生成 `.meta`（那是 `--migrate-assets` 的事）。
pub trait SceneLoader {
    /// 按项目相对路径装载（如 `"Scenes/Level2.ron"`）。
    fn load_scene(&self, relative: &str) -> Result<Scene, String>;
}

/// 真实路径：走 `ProjectHandle`（解码 GUID 引用、路径解析、错误文案都复用它）。
impl SceneLoader for ProjectHandle {
    fn load_scene(&self, relative: &str) -> Result<Scene, String> {
        ProjectHandle::load_scene(self, relative)
    }
}

/// 让宿主可以直接把 `Arc<ProjectHandle>` 交给 `World`（共享同一份资产索引）。
impl<T: SceneLoader + ?Sized> SceneLoader for std::sync::Arc<T> {
    fn load_scene(&self, relative: &str) -> Result<Scene, String> {
        (**self).load_scene(relative)
    }
}
