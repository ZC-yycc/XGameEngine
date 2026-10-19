//! XGameEngine 核心类型与 trait。
//!
//! 这是整个引擎的基础层，提供所有上层 crate 依赖的核心数据结构。
//!
//! # 模块
//!
//! | 模块 | 说明 |
//! |---|---|
//! | `color` | RGBA 颜色类型（线性浮点存储） |
//! | `config` | 引擎/窗口/内容路径配置 |
//! | `transform` | 层级空间变换（位置/旋转/缩放）|
//! | `scene` | 场景图：`Scene`, `Entity` |
//! | `prefab` | 预制体：`Prefab`, `PrefabInstanceRef`（可复用的实体子树模板）|
//! | `id` | 稳定实体标识符 `EntityId` |
//! | `animation` | 动画播放配置 `Animator` + 状态机 `AnimMachine` + 叠加层 `AnimAdditive` + 混合层栈 `AnimLayer`（声明式，落盘进场景文件） |
//! | `audio` | 音频纯数据层：`AudioSource` 组件 + `AudioPort` 命令束 + `ListenerPose`（引擎封装在 `xgame-audio`） |
//! | `material` | 逐实体材质覆盖 `MaterialOverride`（PBR 参数，字段级打补丁） |
//! | `component` | `Component` trait + `ComponentMap` + 内置组件 |
//! | `inspect` | 组件字段检视契约 `Inspect` + `FieldDesc`/`FieldValue`（Inspector 与 Lua 参数的同一份真相源）|
//! | `time` | `GameClock`, `Timer`, `Stopwatch` |
//! | `asset` | `AssetId` 轻量资产句柄 |
//! | `project` | 项目清单 `Project` + 项目句柄 `ProjectHandle` |
//!
//! # 依赖方向
//!
//! ```text
//! core (本 crate)
//!   ↑
//! math (glam 重导出)
//!   ↑
//! {render, physics}
//!   ↑
//! editor / game
//! ```

mod color;
mod config;
mod scene;
mod transform;
pub mod prefab;
pub mod project;

// 新增模块
mod animation;
mod asset;
mod audio;
mod check;
mod component;
pub mod inspect;
mod id;
mod light;
pub mod material;
pub mod physics;
pub mod ray;
mod time;

// -- 导出：旧有类型（保持向后兼容）
pub use check::{check_project, CheckReport};
pub use color::Color;
pub use config::{ContentConfig, EngineConfig, RuntimeConfig, WindowConfig};
pub use prefab::{Prefab, PrefabInstanceRef, PREFAB_EXTENSION};
pub use scene::{Entity, Scene};
pub use transform::Transform;
pub use project::{
    Project, ProjectError, ProjectHandle, DEFAULT_ENTRY_SCENE, PROJECT_FILE_NAME,
    PROJECT_FORMAT_VERSION,
};

// -- 导出：新增基础设施
pub use animation::{
    AnimAdditive, AnimCondition, AnimLayer, AnimLayerMode, AnimMachine, AnimState,
    AnimTransition, Animator,
};
pub use asset::AssetId;
pub use audio::{
    AudioCommand, AudioPort, AudioStats, AudioSource, ListenerPose, PlaySound,
};
pub use component::{Component, ComponentMap, ModelPath, Name};
pub use inspect::{FieldDesc, FieldKind, FieldValue, Inspect};
pub use id::{EntityId, EntityMap, EntitySet};
pub use light::{Light, LightKind};
pub use material::MaterialOverride;
pub use physics::{BodyKind, CollisionShape, PhysicsBody, PhysicsHandle};
pub use ray::{Ray, RayHit};
pub use time::{GameClock, Stopwatch, Timer};
