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
cargo run -q -p xgc-editor --offline -- [世界.json]
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
| `crates/xgc-world/src/lib.rs` | `World` / `Bundle` / `Archetype` / `Column` / 单组件查询 / `reserve_entity_ids` |
| `crates/xgc-world/src/query.rs` | 双 / 三组件不可变 / 可变 / archetype 分片并行查询 |
| `crates/xgc-world/src/components.rs` | 内建 `Transform` / `Velocity` / `Name` / `Parent` / `GlobalTransform` |
| `crates/xgc-world/src/render_components.rs` | 渲染侧组件：`render.mesh` / `render.material` / `render.camera` |
| `crates/xgc-world/src/lod_components.rs` | LOD 组件：`render.lod` |
| `crates/xgc-world/src/environment.rs` | 场景级环境 `SceneEnvironment` / `SceneLight` / `LightKind`：光源与环境光（不是实体，随 `WorldData.environment` 存取，缺省不落盘） |
| `crates/xgc-world/src/registry.rs` | `builtin_registry`：CLI / Agent / xtask 唯一组件清单 |
| `crates/xgc-world/src/skeleton_components.rs` | 骨架组件：`animation.skeleton` / `animation.bone` / `animation.pose` |
| `crates/xgc-world/src/state_machine_components.rs` | 动画状态机组件：`animation.state`（machine / state / time / parameters） |
| `crates/xgc-world/src/layer_components.rs` | 动画叠加层组件：`animation.layers`（RON `[(clip, weight)]`） |
| `crates/xgc-world/src/gameplay_components.rs` | 玩法侧组件：`script.lua` / `audio.source`（含 bus）/ `audio.listener` / `audio.gain` / `audio.mixer` |
| `crates/xgc-world/src/animation_components.rs` | 动画组件：`animation.animator`（clip / speed / time / blend_from / blend_elapsed / blend_duration） |
| `crates/xgc-world/src/character_components.rs` | 角色组件：`physics.character` |
| `crates/xgc-world/src/ui_components.rs` | UI 组件：`ui.element` / `ui.layout` / `ui.interaction` / `ui.input` |
| `crates/xgc-world/src/particle_components.rs` | 粒子组件：`particle.emitter` |
| `crates/xgc-world/src/plugin_components.rs` | Rust 插件组件：`plugin.rust` |
| `crates/xgc-world/src/prefab_components.rs` | 预制体实例：`prefab.instance`（source / source_id / root） |
| `crates/xgc-world/src/physics_components.rs` | `physics.body`：类型 / 形状 / 摩擦 / 弹性 / 质量 / 传感器 |
| `crates/xgc-world/src/slope_components.rs` | `physics.slope`：沿 +X 线性抬升的坡面 |
| `crates/xgc-world/src/command.rs` | `Command` / `Plan` / `Applied`：plan / apply / revert / Replace（含组件增删与计划内预测集合） |
| `crates/xgc-world/src/command/replace.rs` | 回滚与实体整份替换内部实现 |
| `crates/xgc-world/src/data.rs` | `EntityData` / `ComponentData` / 组件工厂 |
| `crates/xgc-world/src/journal.rs` | `Journal` / `TxRecord`：事务日志与重放 |
| `crates/xgc-world/src/snapshot.rs` | `EntitySnapshot` / `WorldSnapshot` / 恢复 |
| `crates/xgc-world/src/validate.rs` | `World::validate_data`：组件 / 字段 / 类型 / 重复项体检 |
| `crates/xgc-world/src/hash.rs` | `World::state_hash` |
| `crates/xgc-store/src/lib.rs` | 版本化 JSON 包络、原子写、WorldData / Journal 存取 |
| `crates/xgc-store/src/binary.rs` | `XGCB` 二进制世界：encode / decode / save / load；固定小端、值带标签 |
| `crates/xgc-sim/src/lib.rs` | `Simulation` / `Schedule` / `SystemAccess` / `Scenario` / `Trace` / `play` / `verify` / `set_ui_input` / `step_with` |
| `crates/xgc-sim/src/animation.rs` | 根变换剪辑采样 + `AnimationPlaybackSystem`；关节轨道在 `animation/joints.rs` |
| `crates/xgc-sim/src/animation/joints.rs` | `JointTrack`：按 joint 索引采样骨骼本地平移 / 旋转 / 缩放；播放系统按 Parent 链找骨骼并写入 |
| `crates/xgc-sim/src/animation_state.rs` | `StateMachine` / `AnimationStateSystem`：参数阈值转换、转场时长写 blend 字段、状态写回 Animator.clip |
| `crates/xgc-sim/src/animation_layers.rs` | `AnimationLayerSystem` / `LayerDef`：根变换按权重叠加；`mask` 实体名白名单、`joints` 关节白名单（走关节轨道，不动根） |
| `crates/xgc-sim/src/input.rs` | `InputMap` / `InputState` / `KeyCode` / `Binding`：解析 `input.ron` 并折叠动作与轴 |
| `crates/xgc-sim/src/audio.rs` | `AudioMixerSystem` + WAV 解码 / `AudioLibrary`：监听者距离 + 总线音量写 `audio.gain` |
| `crates/xgc-sim/src/skeleton.rs` | `SkeletonPoseSystem` / `parse_pose`：关节父链合成世界矩阵 |
| `crates/xgc-sim/src/mixer.rs` | `SoftwareMixer` / `write_wav_mono`：总线增益混音与 WAV 写出 |
| `crates/xgc-sim/src/ui.rs` | `UiLayoutSystem` / `UiInteractionSystem` / `UiFocusSystem` / `hit_test`：布局、裁剪、可见性、命中、`ui.input` 驱动的交互 / Tab / 方向键焦点 |
| `crates/xgc-sim/src/character.rs` | `CharacterStepSystem` / `CharacterBlockSystem`：低台阶抬升、高墙 / 球体推出 |
| `crates/xgc-sim/src/systems.rs` | `PhysicsSystem` / `CharacterJumpSystem` / `VelocitySystem` / `GroundCollisionSystem` / `CharacterSystem` / `AabbCollisionSystem` / `AnimatorSystem` / `GravitySystem` / `HierarchySystem` |
| `crates/xgc-agent/src/lib.rs` | JSON-RPC 会话：describe（组件 / sim / render 能力清单）/ engine.diagnostics / engine.history / engine.replay / query / get / digest / stats / validate / load / save / plan / apply / revert / undo |
| `crates/xgc-agent/src/snapshot.rs` | `world.snapshot` / `world.restore`：WorldData 快照与恢复 |
| `crates/xgc-agent/src/query.rs` | 查询 IR：字段路径、过滤算子、标量比较 |
| `crates/xgc-audio/src/lib.rs` | cpal 音频设备输出：设备枚举、PCM 重采样、F32 / I16 输出队列、`play_clip` |
| `crates/xgc-script/src/lib.rs` | Lua 宿主：安全标准库、执行 / 调用、`xgc` 表（字段 / 实体 / 组件增删）、预算、内存上限、热重载 |
| `crates/xgc-script/src/legacy.rs` | 旧脚本 API：`update(ctx)` / `rot` / `set_rot` / `delta` / `entity_id` / `entity_pos` / `set_pos` / `look_at` / `velocity` / `set_anim_param` / `ui_*` / `action_pressed`；音频等暂无实现为 no-op |
| `crates/xgc-script/src/lifecycle.rs` | `start(ctx)` / `update(ctx)` 生命周期分派与回退 |
| `crates/xgc-script/src/runtime.rs` | `ScriptRuntime`：按 `script.lua` 驱动实体、每帧 `ScriptView` 快照、动作输入桥 `set_action`、命令结算、GUID 脚本经 `AssetCatalog` 解析、实体移除清理、磁盘热重载 |
| `crates/xgc-editor/src/lib.rs` | 编辑器无头状态：打开 / 保存 / 实体列表 / Inspector / Play / Undo/Redo / demo / 轨道相机 |
| `crates/xgc-editor/src/play.rs` | Play / Play Scripts / input.ron 动作桥 / step / stop |
| `crates/xgc-editor/src/transform_ops.rs` | 位置 / 缩放 / 旋转 setter 与 Gizmo 旋转；无父节点同步 GlobalTransform；`snap` |
| `crates/xgc-editor/src/prefab_ops.rs` | `instantiate_prefab_file` / `revert_prefab_file`：旧 PrefabV2 → 新 ID 实例并选中根 |
| `crates/xgc-editor/src/gizmo.rs` | 视口 MVP 投影与线段命中距离 |
| `crates/xgc-editor/src/app.rs` | eframe 窗口：层级 / Inspector 编辑 / 组件增删 / wgpu 视口 / 选中高亮 / 轨道相机 / Move·Rotate·Scale Gizmo / 吸附 / Ctrl+Z、Ctrl+Y |
| `crates/xgc-editor/src/app/toolbar.rs` | 顶部工具栏：保存 / 预制体 / Revert / Play / Play Scripts / 吸附 |
| `crates/xgc-editor/src/main.rs` | `xgc-editor [世界.json]` 入口 |
| `crates/xgc-asset/src/lib.rs` | 扫描 `.meta`、`guid://` → 路径的只读资产目录 |
| `crates/xgc-asset/src/cook.rs` | `cook_project` / `cook_project_filtered`：按 GUID 裁剪、`.meta` / `.bin` 伴生、哈希未变跳过写入、清陈旧文件、manifest 聚合签名与校验 |
| `crates/xgc-migrate/src/lib.rs` | 旧 `SceneV2` RON → `WorldData`（全部旧组件类型：身份 / 名称 / 父子 / Transform / Mesh / Lod / Material / Camera / Lua / Audio / Physics / Animator / Character / UI / Particle / Rust 插件，支持目录批量） |
| `crates/xgc-migrate/src/old_scene.rs` | 旧 `SceneV2` 顶层结构 |
| `crates/xgc-migrate/src/environment.rs` | 场景级光 / 环境光 → `SceneEnvironment` 映射 |
| `crates/xgc-migrate/src/old_animator.rs` | 旧 `Animator` → `animation.animator`（含基础姿态） |
| `crates/xgc-migrate/src/old_lod.rs` | 旧 `LodGroup` → `render.lod`（RON 快照） |
| `crates/xgc-migrate/src/old_rust.rs` | 旧 Rust 插件组件 → `plugin.rust` |
| `crates/xgc-migrate/src/old_particle.rs` | 旧 `ParticleEmitter` → `particle.emitter` |
| `crates/xgc-migrate/src/old_ui.rs` | 旧 `UiElement` → `ui.element`（kind 由内容推断） |
| `crates/xgc-migrate/src/prefab.rs` | `instantiate_prefab` / `revert_prefab`：新 ID、Parent 重写、按 source_id 认回并保留 Transform |
| `crates/xgc-migrate/src/transform_migrate.rs` | 旧 Transform → XGC Transform + GlobalTransform（同值，层级系统随后重算） |
| `crates/xgc-migrate/src/compare.rs` | `compare_headless`：旧引擎 `--headless` JSON 与 XGC 世界的 entities / bodies / colliders / emitters / audio 对照 |
| `crates/xgc-migrate/src/verify.rs` | `verify_migration`：重新迁移旧场景与目标 `WorldData` 逐实体 / 字段对比，浮点按 f32 容差 |
| `crates/xgc-migrate/src/report.rs` | 迁移错误 / 报告 / 批量结果 |
| `crates/xgc-render/src/lib.rs` | 渲染图：资源表 / pass 读写、确定性拓扑排序、环 / 多写者 / 未声明资源 / 读写用途检测、`PassExecutor` |
| `crates/xgc-render/src/gpu.rs` | wgpu 设备、资源创建、clear pass、draw list 执行、RGBA8 回读；`from_arc` 复用于编辑器视口 |
| `crates/xgc-render/src/diff.rs` | `diff_images`：尺寸 / 差异像素比例 / 平均绝对误差 / 最大通道差 |
| `crates/xgc-render/src/ui_draw.rs` | UI 像素矩形 → NDC 四边形矩阵 |
| `crates/xgc-render/src/image.rs` | PNG / PPM（P6）解码 / 平均色块 |
| `crates/xgc-render/src/skinning.rs` | CPU 蒙皮：`skin_positions` / `identity` / `translation` |
| `crates/xgc-render/src/skinned.rs` | GPU 蒙皮：关节索引 / 权重顶点属性、64 关节 uniform、`GpuSkinMeshCache`、单网格 `SkinnedPipeline` |
| `crates/xgc-render/src/texture.rs` | RGBA8 纹理四边形管线（UI 图片） |
| `crates/xgc-render/src/text.rs` | 5x7 位图字体 → NDC 文本网格（按宽度换行） |
| `crates/xgc-render/src/gpu_mesh.rs` | `GpuMeshCache`：模型 → 顶点 / 索引缓冲，跨帧复用；`upload_deformed` 覆盖蒙皮顶点 |
| `crates/xgc-render/src/camera.rs` | `render.camera` + `GlobalTransform` → view-projection / 模型矩阵 / MVP；列主序 4x4 求逆与正交投影（天空盒 / 阴影视锥） |
| `crates/xgc-render/src/pipeline.rs` | `TrianglePipeline` / `RenderCache`：单 draw 路径、颜色 / 深度管线、scissor / 圆角 SDF 裁剪、实例化缓存 |
| `crates/xgc-render/src/draw.rs` | 从 `render.mesh` + `render.material` + `GlobalTransform` 抽 `DrawList`（含模型矩阵、材质资产引用与祖先骨架姿态） |
| `crates/xgc-render/src/frame_color.rs` | 逐 draw 颜色合成：`模型基色因子 → .mat 资产 → 实体覆盖 → 选择高亮`（分流前一次做完） |
| `crates/xgc-render/src/material.rs` | `.mat` 材质资产解析（基色乘子 + metallic / roughness，加载边界清洗） |
| `crates/xgc-render/src/tone.rs` | ACES 曲线 CPU 镜像与 8 位对表辅助；WGSL 侧逐式对应 |
| `crates/xgc-render/src/shadow.rs` | 方向光阴影：单级正交拟合（`fit_directional_shadow`，与旧端同式）+ 2048² 深度图 + 深度 pass 管线；接收端 3×3 PCF、偏置随 N·L 放大；不做 CSM / PCSS / 点光 |
| `crates/xgc-render/src/sky_draw.rs` | 天空盒 pass：全屏三角形 + 解析天空逐像素求值（远平面 / `LessEqual` / 不写深度），`inv_view_proj` 反推视线方向 |
| `crates/xgc-render/src/renderer/ui.rs` | UI 覆盖层：背景 / 焦点环 / 文本 / 图片画在场景之上（从 `renderer.rs` 按行数预算拆出） |
| `crates/xgc-render/src/sky.rs` | 解析天空（天顶/地平线/地面 + 太阳盘 + 光晕）与漫反射 IBL：1024 点斐波那契球投影成 L2 球谐（E/π，无随机数）；参数来自 `project.ron` 的 `render.sky` |
| `crates/xgc-render/src/instanced.rs` | 按模型分组实例化；帧 uniform 带相机位置 + 环境光 + 主方向光 + 天空球谐 + 光视图投影/阴影参数（368B），片元按 `环境光 + 天空 IBL + 光色 × 强度 × N·L × 阴影` 着色再过 ACES |
| `crates/xgc-render/src/shot.rs` | 截图取景：`Bounds` 拟合、顺光视轴（由场景主方向光推）、近远裁剪；`render_shot` 优先场景相机 |
| `crates/xgc-render/src/mesh.rs` | `Mesh` 位置 + 索引 + `SkinData` + `base_color_factor`（`tint` 与实体材质相乘）；builtin quad / cube / sphere / plane / cylinder 为真实几何 |
| `crates/xgc-render/src/glb.rs` | GLB / GLTF 解析：POSITION / 索引 / JOINTS_0 / WEIGHTS_0 / 首图元材质 / `load_glb_skeleton`；`.gltf` + 外部 bin 经内存 GLB 合并 |
| `crates/xgc-render/src/glb/material.rs` | glTF 材质 JSON 最小投影：`pbrMetallicRoughness.baseColorFactor`（缺省白）；纹理字段接入时在此扩展 |
| `crates/xgc-render/src/assets.rs` | `MeshCache`：builtin / 相对路径 GLB / AssetCatalog `guid://` 解析 / 越界拒绝；`.mat` 材质按引用缓存，读不到退回模型材质 |
| `crates/xgc-render/src/resources.rs` | `ResourceDesc` / `ResourceKind` / `ResourceUsage` / `ResourceTable` |
| `crates/xgc-world/tests/` | world / commands / journal / query / snapshot 回归 |
| `crates/xgc-player/src/lib.rs` | `load_world_dir` / `runtime_for` / `run_cooked`：manifest 校验、优先 XGCB、脚本驱动 |
| `crates/xgc-player/src/window.rs` | eframe + wgpu 视口：每帧仿真 / 脚本并贴图到窗口 |
| `crates/xgc-player/src/main.rs` | `xgc-player <cooked 目录> [帧数]`：无帧数打开窗口，有帧数 headless |
| `crates/xgc-cli/src/main.rs` | CLI 参数分派；文件预算拆分后的轻量入口 |
| `crates/xgc-cli/src/runtime_cmd.rs` | `run-scripts` 与截图共用的 `Simulation::step_with` + `ScriptRuntime::run_frame` 固定帧桥 |
| `crates/xgc-cli/src/world_cmd.rs` | `xgc run-world <世界.json> [帧]` / `verify-migration <旧.ron> <世界.json>` |
| `crates/xgc-cli/src/assets_cmd.rs` | `xgc assets <项目根>`：GUID 扫描 |
| `crates/xgc-cli/src/cook_cmd.rs` | `xgc cook <项目根> <输出目录>` / `pack <世界.json> <项目根> <输出目录>` / `run-cooked <目录> [帧]` |
| `crates/xgc-cli/src/compare_cmd.rs` | `xgc compare-headless` / `xgc image-diff <旧图> <新图> [阈值]`；PNG / PPM 逐像素差异 |
| `crates/xgc-cli/src/audio.rs` | `xgc play <wav>`：解码后推默认音频设备 |
| `crates/xgc-cli/src/input_cmd.rs` | `xgc input <input.ron>`：校验并列出动作 / 轴 |
| `crates/xgc-cli/src/render.rs` | `render-frame <世界> <PPM> [资产根] [宽] [高] [帧数]`；缺省 0 帧，最终重算层级 / UI；按项目级 `project.ron` 的 `render.sky` 装天空与背景色 |
| `crates/xgc-xtask/src/main.rs` | `xtask check` / `size` / `deps` / `schema` |

## 不变量（改代码前先看）

- 一个组件一个稳定 `ComponentId`；注册表按 ID 排序输出，schema 必须确定性。
- **新增组件要同时改两处**：`registry.rs::builtin_registry`（schema/Agent）与 `data.rs::register_builtin_components`（World 工厂），缺一不可。
- `EntityId` 是持久身份，`EntityKey` 是运行时句柄；句柄复用必须换 generation。
- `Replace` 会销毁并重建实体行，**旧 EntityKey 失效**，按 ID 重取句柄。
- archetype 的每一行列长度必须相等；`despawn` 用 `swap_remove` 并修正被搬实体行号。
- `World::spawn` 只接受元组 `Bundle`；重复组件在 debug 断言里拒绝。
- 热路径先保证正确；性能数字必须写进 README / 基准，不许凭感觉写"更快"。
- 核心 crate 默认 `#![deny(unsafe_code)]`；加 unsafe 必须单独说明调用窗口。

## 当前下一步（按顺序）

1. 仿真：动态刚体已能下落（迁移为动态 `physics.body` 补零速度；旧引擎速度由物理后端持有）；下一步补刚体求解器（旋转 / 静-动碰撞 / 堆叠稳定），让第 150 帧落点对齐（像素差距主因），之后回到 CSM / 软阴影 / 镜面 IBL / metallic-roughness BRDF / 网格纹理。
2. 脚本：正式 System 调度、并行执行与脚本组件生命周期完整化。
3. 割接：旧新逐帧 trace / 截图 / 性能门禁。
4. 资产：密码学签名 / 跨机器增量。
5. 推送与 CI；当前最大 Rust 文件 596 行（`instanced.rs` 已贴顶），继续守住 600 行软上限。

## Token 纪律

- 先读本文件 + README；不要读 `target/` 或整份主文档。
- 每轮只动一个可验证的垂直切片；先跑测试再写总结。
- 长任务交给子智能体时，给它**文件路径 + 不变量 + 验收命令**，不要复制整份设计。
