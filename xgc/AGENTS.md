# XGC Agent 入口

## 范围

`xgc/` 是 XGame Core 的独立 workspace；**不要**把它加进根 `Cargo.toml` 的
members，也不要在 `xgc` 里依赖根仓库的 `xgame-*` crate。旧引擎与 XGC 在割接前
是两套产品路径，禁止在同一个运行时里混用。

## 命令

```sh
cd xgc
cargo test --workspace --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo run -q -p xgc-xtask --offline -- check
cargo run -q -p xgc-cli --offline -- check
cargo run -q -p xgc-cli --offline -- run 60
cargo run -q --release -p xgc-cli --offline -- bench
```

本机 cargo 在 `C:\Users\Administrator\.cargo\bin\cargo.exe`，不在 PATH 时用全路径。
网络不可用时一律加 `--offline`（serde / serde_json 已缓存）。

## 入口文件

| 文件 | 职责 |
|---|---|
| `crates/xgc-base/src/lib.rs` | `EntityId` / `EntityKey` / `Tick` / `Diagnostic` |
| `crates/xgc-base/src/diagnostics.rs` | 诊断码常量 + `engine.diagnostics` 目录 |
| `crates/xgc-reflect/src/lib.rs` | `Component`、`Registry`、`SchemaDoc`、`Value`、动态字段读写 |
| `crates/xgc-world/src/lib.rs` | `World` / `Bundle` / `Archetype` / `Column` / 单组件查询 |
| `crates/xgc-world/src/query.rs` | 双 / 三组件不可变 / 可变 / archetype 分片并行查询 |
| `crates/xgc-world/src/components.rs` | 内建 `Transform` / `Velocity` / `Name` / `Parent` / `GlobalTransform` |
| `crates/xgc-world/src/render_components.rs` | 渲染侧组件：`render.mesh` / `render.material` / `render.camera` |
| `crates/xgc-world/src/lod_components.rs` | LOD 组件：`render.lod` |
| `crates/xgc-world/src/registry.rs` | `builtin_registry`：CLI / Agent / xtask 唯一组件清单 |
| `crates/xgc-world/src/gameplay_components.rs` | 玩法侧组件：`script.lua` / `audio.source` |
| `crates/xgc-world/src/animation_components.rs` | 动画组件：`animation.animator`（clip / speed / duration / time） |
| `crates/xgc-world/src/character_components.rs` | 角色组件：`physics.character` |
| `crates/xgc-world/src/ui_components.rs` | UI 组件：`ui.element` / `ui.layout` / `ui.interaction` / `ui.input` |
| `crates/xgc-world/src/particle_components.rs` | 粒子组件：`particle.emitter` |
| `crates/xgc-world/src/plugin_components.rs` | Rust 插件组件：`plugin.rust` |
| `crates/xgc-world/src/physics_components.rs` | `physics.body`：类型 / 形状 / 摩擦 / 弹性 / 质量 / 传感器 |
| `crates/xgc-world/src/slope_components.rs` | `physics.slope`：沿 +X 线性抬升的坡面 |
| `crates/xgc-world/src/command.rs` | `Command` / `Plan` / `Applied`：plan / apply / revert（含组件增删与计划内预测集合） |
| `crates/xgc-world/src/data.rs` | `EntityData` / `ComponentData` / 组件工厂 |
| `crates/xgc-world/src/journal.rs` | `Journal` / `TxRecord`：事务日志与重放 |
| `crates/xgc-world/src/snapshot.rs` | `EntitySnapshot` / `WorldSnapshot` / 恢复 |
| `crates/xgc-world/src/validate.rs` | `World::validate_data`：组件 / 字段 / 类型 / 重复项体检 |
| `crates/xgc-world/src/hash.rs` | `World::state_hash` |
| `crates/xgc-store/src/lib.rs` | 版本化 JSON 包络、原子写、WorldData / Journal 存取 |
| `crates/xgc-sim/src/lib.rs` | `Simulation` / `Schedule` / `SystemAccess` / `Scenario` / `Trace` / `play` / `verify` / `set_ui_input` |
| `crates/xgc-sim/src/animation.rs` | `AnimationClip` / `TranslationKey` / `RotationKey` / `ScaleKey` / `load_clip_ron` / `AnimationLibrary`（含 `load_dir`）：平移 / 旋转 slerp / 缩放采样、循环、RON / 目录加载、`AnimationPlaybackSystem` 写 Transform |
| `crates/xgc-sim/src/ui.rs` | `UiLayoutSystem` / `UiInteractionSystem` / `UiFocusSystem` / `hit_test`：布局、裁剪、可见性、命中、`ui.input` 驱动的交互 / Tab / 方向键焦点 |
| `crates/xgc-sim/src/character.rs` | `CharacterStepSystem` / `CharacterBlockSystem`：低台阶抬升、高墙 / 球体推出 |
| `crates/xgc-sim/src/systems.rs` | `PhysicsSystem` / `CharacterJumpSystem` / `VelocitySystem` / `GroundCollisionSystem` / `CharacterSystem` / `AabbCollisionSystem` / `AnimatorSystem` / `GravitySystem` / `HierarchySystem` |
| `crates/xgc-agent/src/lib.rs` | JSON-RPC 会话：describe（组件 / sim / render 能力清单）/ engine.diagnostics / engine.history / engine.replay / query / get / digest / stats / validate / load / save / plan / apply / revert / undo |
| `crates/xgc-agent/src/snapshot.rs` | `world.snapshot` / `world.restore`：WorldData 快照与恢复 |
| `crates/xgc-agent/src/query.rs` | 查询 IR：字段路径、过滤算子、标量比较 |
| `crates/xgc-asset/src/lib.rs` | 扫描 `.meta`、`guid://` → 路径的只读资产目录 |
| `crates/xgc-migrate/src/lib.rs` | 旧 `SceneV2` RON → `WorldData`（全部旧组件类型：身份 / 名称 / 父子 / Transform / Mesh / Lod / Material / Camera / Lua / Audio / Physics / Animator / Character / UI / Particle / Rust 插件，支持目录批量） |
| `crates/xgc-migrate/src/old_scene.rs` | 旧 `SceneV2` 顶层结构 |
| `crates/xgc-migrate/src/old_animator.rs` | 旧 `Animator` → `animation.animator`（含基础姿态） |
| `crates/xgc-migrate/src/old_lod.rs` | 旧 `LodGroup` → `render.lod`（RON 快照） |
| `crates/xgc-migrate/src/old_rust.rs` | 旧 Rust 插件组件 → `plugin.rust` |
| `crates/xgc-migrate/src/old_particle.rs` | 旧 `ParticleEmitter` → `particle.emitter` |
| `crates/xgc-migrate/src/old_ui.rs` | 旧 `UiElement` → `ui.element`（kind 由内容推断） |
| `crates/xgc-migrate/src/report.rs` | 迁移错误 / 报告 / 批量结果 |
| `crates/xgc-render/src/lib.rs` | 渲染图：资源表 / pass 读写、确定性拓扑排序、环 / 多写者 / 未声明资源 / 读写用途检测、`PassExecutor` |
| `crates/xgc-render/src/gpu.rs` | wgpu 设备、资源创建、clear pass、draw list 执行、RGBA8 回读 |
| `crates/xgc-render/src/renderer.rs` | `Renderer` 帧入口：DrawList → MeshCache → 实例化 → GPU，active camera MVP + 深度附件；UI 背景 + 焦点环 + 文本叠加 |
| `crates/xgc-render/src/ui_draw.rs` | UI 像素矩形 → NDC 四边形矩阵 |
| `crates/xgc-render/src/text.rs` | 5x7 位图字体 → NDC 文本网格 |
| `crates/xgc-render/src/instanced.rs` | 按模型分组的实例化管线 / storage 实例缓冲 |
| `crates/xgc-render/src/gpu_mesh.rs` | `GpuMeshCache`：模型 → 顶点 / 索引缓冲，跨帧复用 |
| `crates/xgc-render/src/camera.rs` | `render.camera` + `GlobalTransform` → view-projection / 模型矩阵 / MVP |
| `crates/xgc-render/src/pipeline.rs` | `TrianglePipeline` / `RenderCache`：单 draw 路径、颜色 / 深度管线、实例化缓存 |
| `crates/xgc-render/src/draw.rs` | 从 `render.mesh` + `render.material` + `GlobalTransform` 抽 `DrawList`（含模型矩阵） |
| `crates/xgc-render/src/mesh.rs` | `Mesh` 位置 + 可选索引、builtin 网格占位 |
| `crates/xgc-render/src/glb.rs` | 最小 GLB 解析：POSITION / 索引 |
| `crates/xgc-render/src/assets.rs` | `MeshCache`：builtin / 相对路径 GLB / AssetCatalog `guid://` 解析 / 越界拒绝 |
| `crates/xgc-render/src/resources.rs` | `ResourceDesc` / `ResourceKind` / `ResourceUsage` / `ResourceTable` |
| `crates/xgc-world/tests/` | world / commands / journal / query / snapshot 回归 |
| `crates/xgc-cli/src/main.rs` | `xgc schema / check [路径] / bench / export / import / run / serve / migrate <文件|目录> / assets <项目根> / clips <目录>` |
| `crates/xgc-cli/src/render.rs` | `xgc render-graph / gpu-probe / mesh-probe / render-frame` |
| `crates/xgc-xtask/src/main.rs` | `xtask check` / `size` / `deps` / `schema` |

## 不变量（改代码前先看）

- 一个组件一个稳定 `ComponentId`；注册表按 ID 排序输出，schema 必须确定性。
- `EntityId` 是持久身份，`EntityKey` 是运行时句柄；句柄复用必须换 generation。
- archetype 的每一行列长度必须相等；`despawn` 用 `swap_remove` 并修正被搬实体行号。
- `World::spawn` 只接受元组 `Bundle`；重复组件在 debug 断言里拒绝。
- 热路径先保证正确；性能数字必须写进 README / 基准，不许凭感觉写"更快"。
- 核心 crate 默认 `#![deny(unsafe_code)]`；加 unsafe 必须单独说明调用窗口。

## 当前下一步（按顺序）

1. Agent：快照已接；下一步快照文件 / 多存档。
2. UI：图片与字体资产。
3. 物理：胶囊角色 / 坡面阻挡。
4. 动画：骨骼 / 蒙皮。
5. 现在最大文件 588 行：动它之前先拆。

## Token 纪律

- 先读本文件 + README；不要读 `target/` 或整份主文档。
- 每轮只动一个可验证的垂直切片；先跑测试再写总结。
- 长任务交给子智能体时，给它**文件路径 + 不变量 + 验收命令**，不要复制整份设计。
