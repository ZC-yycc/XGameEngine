//! XGameEngine 门面 crate —— 游戏代码与插件的**唯一入口**。
//!
//! # 为什么要有这一层
//!
//! 在此之前，写一个宿主要直接面对 7 个引擎 crate 的深路径
//! （`xgame_render::scene_pipeline::ScenePipeline` 这种），引擎内部重构模块
//! 布局会把所有宿主打翻。门面 crate 把这层耦合收成**一个** crate 依赖 +
//! 一行 `use xgame::prelude::*;`：
//!
//! ```ignore
//! use xgame::prelude::*;
//! ```
//!
//! 双轨语言架构（`Docs/双轨语言架构计划.md`）里它同时是两条轨道的公共地基：
//!
//! - **Rust 轨道**：用户的 cdylib 插件 crate 依赖 `xgame`，实现 `Lifecycle`、
//!   调 `GameCtx` 方法；发布期由引擎静态链接，零边界开销。
//! - **Lua / Rhai 轨道**：脚本调的也是同一批 `GameCtx` 方法 ——
//!   "脚本层是公共 API 的第一个用户"，两轨能力边界天然一致。
//!
//! # 版本即握手
//!
//! [`ENGINE_VERSION`] 是插件边界（P0 插件加载器）的版本握手依据：插件必须用
//! **同版本**的门面 crate 编译（同 rustc、同依赖、同 build flags），版本不匹配
//! 拒载并明确报错。Rust ABI 不保证跨版本兼容，这道闸门是整个 UE 式插件模式
//! 能成立的前提。

/// 引擎版本号（= 门面 crate 版本 = 各引擎 crate 的版本）。
///
/// 插件的 `Cargo.toml` 里 `xgame` 的版本必须与此一致，否则拒载。
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 插件边界（UE 模式）：宿主加载器 + 版本握手 + `export_plugin!` 宏。
pub mod plugin;

/// 一行拿齐写游戏 / 写插件所需的全部引擎类型。
///
/// 拍平的是**深路径**（`xgame_render::scene_pipeline::ScenePipeline` 这类），
/// 引擎内部模块怎么挪都不再影响宿主代码。
pub mod prelude {
    // -- 核心数据 ---------------------------------------------------------
    pub use xgame_core::project::{ProjectHandle, PROJECT_FILE_NAME};
    pub use xgame_core::{
        BodyKind, Entity, EntityId, GameClock, PhysicsBody, PhysicsHandle, Scene,
        Transform,
    };

    // -- 输入 -------------------------------------------------------------
    pub use xgame_input::{key_from_winit, Input, InputMap, KeyCode, MouseButton};

    // -- 数学（glam 常用类型 + 整个命名空间 + 投影工具）-------------------
    pub use xgame_math::glam::{Mat4, Quat, Vec2, Vec3};
    pub use xgame_math::glam;
    pub use xgame_math::projection;

    // -- 渲染 -------------------------------------------------------------
    pub use xgame_render::animation::AnimationDriver;
    pub use xgame_render::gltf_loader::GltfMesh;
    pub use xgame_render::scene_pipeline::{
        supported_sample_count, SceneLighting, ScenePipeline,
    };

    // -- 运行时 -----------------------------------------------------------
    pub use xgame_runtime::{BehaviorRegistry, Ctx, FrameReport, Lifecycle, World, WorldConfig};

    // -- 游戏侧 API（脚本 / 组件两轨的真相源）--------------------------
    pub use xgame_script::{GameCtx, LogLevel, RayHit, ScriptLog};

    // -- 插件（用户插件 crate 写 `xgame::export_plugin!(...)` 即可，
    //    这里再导一份让 `use xgame::prelude::*` 的代码里写裸名）--------
    pub use crate::export_plugin;
}
