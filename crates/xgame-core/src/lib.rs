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
//! | `layers` | 层与碰撞矩阵（物理过滤的唯一真相源，落盘在 `project.ron` 的 `layers` 节） |
//! | `animation` | 动画播放配置 `Animator` + 状态机 `AnimMachine` + 叠加层 `AnimAdditive` + 混合层栈 `AnimLayer`（声明式，落盘进场景文件） |
//! | `audio` | 音频纯数据层：`AudioSource` 组件 + `AudioPort` 命令束 + `ListenerPose`（引擎封装在 `xgame-audio`） |
//! | `camera` | 相机组件 `Camera`（投影参数 + 主相机标记）+ 朝向反解 `look_rotation` |
//! | `character` | 角色控制器参数 `CharacterController`（可序列化）+ 运行时状态 `CharacterState` |
//! | `material` | 材质：共享资产 `MaterialAsset`（`.mat`）+ 逐实体覆盖 `MaterialOverride` |
//! | `component` | `Component` trait + `ComponentMap` + 内置组件 |
//! | `inspect` | 组件字段检视契约 `Inspect` + `FieldDesc`/`FieldValue`（Inspector 与 Lua 参数的同一份真相源）|
//! | `time` | `GameClock`, `Timer`, `Stopwatch` |
//! | `asset` | 资产身份 `AssetGuid` + `.meta` + `AssetDatabase`（GUID 引用、导入设置、诊断） |
//! | `project` | 项目清单 `Project` + 项目句柄 `ProjectHandle` |
//! | `loader` | 资产装载注入点 `SceneLoader`（场景 / 预制体，按项目相对路径装载）|
//! | `ui` | 屏幕空间 UI 组件 `UiElement`（锚点布局的纯数据层，布局与绘制在 `xgame-ui`）|
//! | `particle` | 粒子发射器 `ParticleEmitter`（纯声明；模拟在 `xgame-particle`，绘制在 `xgame-render`）|
//! | `schema` | 场景格式的机器可读说明书（`--export-schema` / MCP `schema_read` 的真相源）|
//! | `script_api` | 脚本 API 的机器可读清单（`--export-script-api` / MCP `script_api_read` 的真相源）|
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
pub mod camera;
mod character;
mod check;
mod component;
pub mod inspect;
mod id;
pub mod layers;
mod light;
mod loader;
pub mod material;
pub mod particle;
pub mod physics;
pub mod ray;
mod time;
pub mod ui;

// 场景格式的机器可读说明书（Agent-Native ③）：--export-schema / MCP schema_read 的真相源。
pub mod schema;

// 脚本 API 的机器可读清单（Agent-Native ③）：--export-script-api / MCP script_api_read 的真相源。
pub mod script_api;

// -- 导出：旧有类型（保持向后兼容）
pub use check::{check_project, CheckReport};
pub use color::Color;
pub use config::{
    BloomConfig, BuildConfig, ContentConfig, EngineConfig, FxaaConfig, RenderConfig, RuntimeConfig,
    SkyConfig, TaaConfig, ToneMapMode, WindowConfig,
};
pub use prefab::{Prefab, PrefabInstanceRef, PREFAB_EXTENSION};
pub use scene::{Entity, Scene};
pub use transform::Transform;
pub use project::{
    AssetMigrationReport, Project, ProjectError, ProjectHandle, DEFAULT_ENTRY_SCENE,
    PROJECT_FILE_NAME, PROJECT_FORMAT_VERSION, normalize_scene_relative, same_scene_path,
};

// -- 导出：新增基础设施
pub use animation::{
    AnimAdditive, AnimCondition, AnimLayer, AnimLayerMode, AnimMachine, AnimState,
    AnimTransition, Animator,
};
pub use asset::{
    decode_prefab_references, decode_scene_references, encode_prefab_references,
    encode_scene_references, is_dangling_reference, is_guid_reference, normalize_asset_relative,
    read_import_settings, render_asset_list, AssetDatabase, AssetDiagnostics, AssetGuid, AssetId,
    AssetKind, AssetMeta, AssetMoveReport, AssetRecord, GuidConflict, ImportSettings, InvalidMeta,
    GUID_SCHEME, META_EXTENSION,
};
pub use audio::{
    AudioCommand, AudioPort, AudioStats, AudioSource, ListenerPose, PlaySound,
};
pub use camera::{look_rotation, Camera};
pub use character::{
    CharacterContacts, CharacterController, CharacterState, MAX_CHARACTER_CONTACTS,
};
pub use component::{Component, ComponentMap, ModelPath, Name};
pub use inspect::{FieldDesc, FieldKind, FieldValue, Inspect};
pub use id::{EntityId, EntityMap, EntitySet};
pub use layers::{sanitize_layer, Layers, DEFAULT_LAYER, LAYER_COUNT};
pub use light::{Light, LightKind};
pub use loader::SceneLoader;
pub use material::{MaterialAsset, MaterialOverride, MATERIAL_EXTENSION};
pub use particle::{
    EmissionShape, ParticleBlend, ParticleEmitter, MAX_PARTICLES_LIMIT,
};
pub use physics::{BodyKind, CollisionShape, PhysicsBody, PhysicsHandle};
pub use ray::{Ray, RayHit};
pub use schema::{scene_schema, SchemaDoc, SCHEMA_FORMAT_VERSION};
pub use script_api::{script_api, ScriptApiDoc, SCRIPT_API_FORMAT_VERSION};
pub use time::{GameClock, Stopwatch, Timer, MAX_TIME_SCALE};
pub use ui::{UiAlign, UiElement, UiImageStyle, UiKind, UiState, UiStyle, UiTextStyle, UiVAlign};
