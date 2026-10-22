//! 运行时"从哪装载资产"的注入点（场景 / 预制体）。
//!
//! # 为什么是注入的 trait，而不是让 `World` 直接持有 `ProjectHandle`
//!
//! `World` 的既有契约是"一份场景 + 一个根目录"—— 测试、无头、独立游戏都按这个
//! 契约构造它，没有人需要项目清单。把 `ProjectHandle` 焊进 `World` 会顺手把
//! "项目布局"也焊进去，而 `World` 只该知道"给我一个相对路径，还我一份数据"。
//!
//! 更实际的理由：**只有宿主知道资产该从哪来**。编辑器 / 独立游戏 / 无头运行
//! 手里都有 `ProjectHandle`（`Scenes/*.ron` 里的资产引用是 `guid://`，读盘时要
//! 解码成项目相对路径）；而绝大多数单元测试只想给一份内存里的 `Scene`。注入让
//! 两边都成立，且"没有装载器"是一个**可诊断的状态**：脚本请求切场景 / 生成
//! 预制体时记一条错误日志，而不是静默无事发生。
//!
//! # 为什么这个 trait 住在 core（而不是 runtime）
//!
//! `load_prefab` 的服务对象是脚本层（`xgame-script::GameCtx` 要能在钩子里
//! 按路径实例化预制体），而依赖方向是 `script ← runtime ← editor/game`，
//! 脚本层**看不到** runtime 的类型。trait 与它唯一的真实实现（`ProjectHandle`）
//! 一起待在 core，两边都能命名它 —— runtime 只做 `pub use` 转出，
//! 宿主侧的 `use xgame_runtime::SceneLoader` 不因此改变。
//!
//! # 为什么 `load_prefab` 有默认实现
//!
//! 场景是**必需**能力（没有它连游戏都起不来），预制体是**增量**能力。
//! 默认实现返回 `Err`，让既有实现者（如果有第三方/测试自己接了装载器）
//! 不必为了拿新能力而改代码；同时"没接"仍然是可诊断的：脚本侧拿到的是
//! 一条带原因的错误日志，而不是一个静默的 `nil`。
//!
//! # 为什么返回的是**内存态**数据
//!
//! 磁盘上的资产引用是 GUID，内存里是项目相对路径（决策记录 §十四）。
//! 解码只发生在 `ProjectHandle` 的读盘边界 —— 所以装载器交出的场景 / 预制体
//! 必须已经是解码过的形态，调用方拿到就能直接跑，不再碰资产索引。

use std::sync::Arc;

use crate::prefab::Prefab;
use crate::project::ProjectHandle;
use crate::scene::Scene;

/// 资产装载来源（项目相对路径 → 内存态场景 / 预制体）。
///
/// 实现者必须保证**只读**：`--check` / 无头运行都可能在只读挂载上跑，
/// 装载资产不该顺手生成 `.meta`（那是 `--migrate-assets` 的事）。
pub trait SceneLoader {
    /// 按项目相对路径装载场景（如 `"Scenes/Level2.ron"`）。
    fn load_scene(&self, relative: &str) -> Result<Scene, String>;

    /// 按项目相对路径装载预制体（如 `"Prefabs/Enemy.prefab.ron"`）。
    ///
    /// 默认实现 = "本装载器不支持预制体"。**不要**在这里返回一个空预制体：
    /// 空预制体在实例化侧等价于"什么都没生成"，会把"没接线"伪装成
    /// "预制体是空的"，两种故障的处理方式完全不同。
    fn load_prefab(&self, relative: &str) -> Result<Prefab, String> {
        let _ = relative;
        Err("当前宿主没有接入预制体装载器（预制体生成需要 with_scene_loader + 项目句柄）"
            .to_string())
    }
}

/// 真实路径：走 `ProjectHandle`（解码 GUID 引用、路径解析、错误文案都复用它）。
impl SceneLoader for ProjectHandle {
    fn load_scene(&self, relative: &str) -> Result<Scene, String> {
        ProjectHandle::load_scene(self, relative)
    }

    fn load_prefab(&self, relative: &str) -> Result<Prefab, String> {
        ProjectHandle::load_prefab(self, relative)
    }
}

/// 让宿主可以直接把 `Arc<ProjectHandle>` 交给 `World`（共享同一份资产索引）。
impl<T: SceneLoader + ?Sized> SceneLoader for Arc<T> {
    fn load_scene(&self, relative: &str) -> Result<Scene, String> {
        (**self).load_scene(relative)
    }

    fn load_prefab(&self, relative: &str) -> Result<Prefab, String> {
        (**self).load_prefab(relative)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只实现 `load_scene` 的装载器：预制体走默认实现，必须是**可诊断的错**。
    struct SceneOnly;

    impl SceneLoader for SceneOnly {
        fn load_scene(&self, _relative: &str) -> Result<Scene, String> {
            Ok(Scene::new("S"))
        }
    }

    #[test]
    fn default_load_prefab_is_a_diagnosable_error() {
        let err = SceneOnly.load_prefab("Prefabs/X.prefab.ron").unwrap_err();
        assert!(
            err.contains("预制体装载器"),
            "错误文案要让作者知道是「没接线」而不是「文件坏了」：{err}"
        );
        // 场景能力不受影响
        assert!(SceneOnly.load_scene("Scenes/A.ron").is_ok());
    }

    #[test]
    fn arc_forwards_both_capabilities() {
        let loader = Arc::new(SceneOnly);
        assert!(loader.load_scene("Scenes/A.ron").is_ok());
        assert!(loader.load_prefab("Prefabs/X.prefab.ron").is_err());
    }
}
