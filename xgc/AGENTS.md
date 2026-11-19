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
| `crates/xgc-world/src/character_components.rs` | 角色组件：`physics.character`；缺省值（1.8 / 0.3 / 0.35 / 45 / 0.02 / 0.3）与旧端 `xgame_core` 逐项对齐，`sanitized()` / `half_height()` / `center_height()` 是唯一消毒与几何派生真相源（`character_kcc` 全部委托）；改常量记得同步 `fields()` 的 schema 缺省值（有 `schema_defaults_match_the_component_defaults` 守着），区间常量之间的咬合由编译期 `const _: ()` 断言守着 |
| `crates/xgc-world/src/ui_components.rs` | UI 组件：`ui.element` / `ui.layout` / `ui.interaction` / `ui.input`；`background` / `border_color` / `text_color` 存**线性** RGB（旧脚本 UI 颜色 setter 的 sRGB 在入口换算；通用字段写入按线性原值落库；渲染端写 sRGB 目标由硬件编码） |
| `crates/xgc-world/src/particle_components.rs` | 粒子组件：`particle.emitter` |
| `crates/xgc-world/src/plugin_components.rs` | Rust 插件组件：`plugin.rust` |
| `crates/xgc-world/src/prefab_components.rs` | 预制体实例：`prefab.instance`（source / source_id / root） |
| `crates/xgc-world/src/physics_components.rs` | `physics.body`：类型 / 形状 / 摩擦 / 弹性 / 质量 / 传感器；`BodyKind` / `PhysicsBody::kind()` / `is_dynamic()` 是「什么算动态体」的唯一判据（`rapier_world` 建体与回写、`GravitySystem`、`GroundCollisionSystem`、`AabbCollisionSystem` 共用，未知字符串按旧端兜底一律当 Dynamic） |
| `crates/xgc-world/src/slope_components.rs` | `physics.slope`：沿 +X 线性抬升的坡面；`rapier_world/slopes.rs` 按同一参数给坡面建静态楔形薄壳碰撞体（`SLOPE_THICKNESS = 0.1`、上表面与 `height_at` 重合，ECS 与 rapier 看同一个坡面）；`degrees()` / `base_x_at()` 是角色坡限判定的唯一口径（`base_x_at` 结果夹进坡面 X 跨度，`rise <= 0` 的下坡一律当地面、没有立面） |
| `crates/xgc-world/src/command.rs` | `Command` / `Plan` / `Applied`：plan / apply / revert / Replace（含组件增删与计划内预测集合） |
| `crates/xgc-world/src/command/replace.rs` | 回滚与实体整份替换内部实现 |
| `crates/xgc-world/src/data.rs` | `EntityData` / `ComponentData` / 组件工厂 |
| `crates/xgc-world/src/journal.rs` | `Journal` / `TxRecord`：事务日志与重放 |
| `crates/xgc-world/src/snapshot.rs` | `EntitySnapshot` / `WorldSnapshot` / 恢复 |
| `crates/xgc-world/src/validate.rs` | `World::validate_data`：组件 / 字段 / 类型 / 重复项体检 |
| `crates/xgc-world/src/hash.rs` | `World::state_hash`：实体按 ID、组件按 ID、字段按声明序混入；实体行号走 `thread_local` scratch、组件 / 字段布局缓存在 `Archetype::hash_layout`（数值型组件世界 60 次调用零分配） |
| `crates/xgc-cli/tests/image_diff.rs` | `image-diff … report` 集成回归：真实 PPM 的分桶与通道均值 / 不写 report 时保持旧输出 / 未知第四参数退出码 2 |
| `crates/xgc-cli/tests/trace.rs` | 逐帧 trace 集成回归：与进程内重放逐步对表 / 两次运行逐字节相同 / 不同世界第 0 帧即不同 / 参数错误退出码 2 / 汇总行与 `run-world` 末行逐字一致 / 带资产根时第 1 帧起与无脚本路径分叉且汇总行与 `run-scripts` 末行逐字一致 / 坏脚本与 `update` 期抛错都退出码 2 且点名出错帧 |
| `crates/xgc-sim/tests/steady_state.rs` | 热路径分配门禁：线程本地计数分配器（每个测试线程只看自己的分配）+ 稳态帧预算（60 × (实体数 + 64)），数值型组件世界 `state_hash` 60 次零分配；另有一条 release 吞吐用例默认 `#[ignore]` |
| `crates/xgc-store/src/lib.rs` | 版本化 JSON 包络、原子写、WorldData / Journal 存取 |
| `crates/xgc-store/src/binary.rs` | `XGCB` 二进制世界：encode / decode / save / load；固定小端、值带标签 |
| `crates/xgc-sim/src/lib.rs` | `Simulation` / `Schedule` / `SystemAccess` / `Scenario` / `Trace` / `play` / `verify` / `set_ui_input` / `step_with` |
| `crates/xgc-sim/src/particles.rs` | `ParticleSystem`：SplitMix64 确定性发射 / 生命周期 / 形状采样 / 重力阻力，产出当帧 `ParticleQuad` |
| `crates/xgc-sim/src/animation.rs` | 根变换剪辑采样 + `AnimationPlaybackSystem`；关节轨道在 `animation/joints.rs` |
| `crates/xgc-sim/src/animation/joints.rs` | `JointTrack`：按 joint 索引采样骨骼本地平移 / 旋转 / 缩放；播放系统按 Parent 链找骨骼并写入 |
| `crates/xgc-sim/src/animation_state.rs` | `StateMachine` / `AnimationStateSystem`：参数阈值转换、转场时长写 blend 字段、状态写回 Animator.clip |
| `crates/xgc-sim/src/animation_layers.rs` | `AnimationLayerSystem` / `LayerDef`：根变换按权重叠加；`mask` 实体名白名单、`joints` 关节白名单（走关节轨道，不动根） |
| `crates/xgc-sim/src/input.rs` | `InputMap` / `InputState` / `KeyCode` / `Binding`：解析 `input.ron` 并折叠动作与轴 |
| `crates/xgc-sim/src/audio.rs` | `AudioMixerSystem` + WAV 解码 / `AudioLibrary`：监听者距离 + 总线音量写 `audio.gain` |
| `crates/xgc-sim/src/skeleton.rs` | `SkeletonPoseSystem` / `parse_pose`：关节父链合成世界矩阵 |
| `crates/xgc-sim/src/mixer.rs` | `SoftwareMixer` / `write_wav_mono`：总线增益混音与 WAV 写出 |
| `crates/xgc-sim/src/ui.rs` | `UiLayoutSystem` / `UiInteractionSystem` / `UiFocusSystem` / `hit_test`：布局、裁剪、可见性、命中、`ui.input` 驱动的交互 / Tab / 方向键焦点 |
| `crates/xgc-sim/src/character_kcc.rs` | 角色胶囊 / rapier KCC / 竖直阶段的唯一真相源：`radius` / `half_height` / `center_height` / `rest_gap = skin_width × 1.1` / `character_capsule` / `build_character_controller`（`offset = Absolute(skin)`、`autostep: None`、两个坡角、`snap_to_ground: None`、nudge 1e-4）/ `move_character`（水平 collide-and-slide + 竖直位移，events 收集接触法线）/ `probe_ground`（向下射线，间隙是法向距离 `(toi - half_height) * n.y - radius`，y=0 解析兜底）/ `try_step_up`（旧端三段扫掠：升 / 前进 / 落，落点不是坡面才采纳）/ `GRAVITY_Y` / `SLIDE_BAND_DEGREES` + `slip_limit`（`(slope_limit + 10).min(89)`，旧端 `min_slope_slide_angle` 同式；超过迟滞带的坡不清竖直速度、沿坡面投影下滑）；消毒委托 `CharacterController::sanitized()`（本文件不再自带区间常量）；`owned_by_kcc` 是「有 `CharacterController`、无 `PhysicsBody`、有 `Transform`」的唯一判据 |
| `crates/xgc-sim/src/systems.rs` | `PhysicsSystem` / `CharacterJumpSystem` / `VelocitySystem`（对带 `CharacterController` 的实体**整份跳过** —— 角色三轴位移都在 `RapierPhysicsSystem` 的角色阶段，一帧只能有一处积分）/ `GroundCollisionSystem` / `AnimatorSystem` / `GravitySystem` / `HierarchySystem`；`CharacterSystem` / `CharacterStepSystem` 已删除，竖直 / 地面 / 台阶搬进 `character_kcc` 的 rapier 角色阶段（`rapier_world/characters.rs`） |
| `crates/xgc-sim/src/aabb.rs` | `AabbCollisionSystem`：动态刚体两两分离 / 动态 vs 静态推出，接触时清掉沿分离轴的接近速度（落地 / 堆叠稳定） |
| `crates/xgc-sim/src/rapier_world.rs` | `RapierWorld` + `RapierPhysicsSystem`：与旧引擎同版本同参数的 rapier3d 0.22 刚体后端（`physics.body` 实体），建体规则按旧端 `body_from_scene`（Auto 形状取缩放绝对值 ×0.5、mass 换密度、角阻尼 0.05）；动态体回写 Transform / Velocity，运动学体每步推送位姿；建体签名 = `BodySignature`（`EntityId` + `EntityKey` + `kind`/`shape` 的 FNV-1a + 尺寸 / 半径 / 半高 / 摩擦 / 弹性 / 质量 / 重力缩放 / 传感器 的位模式；位置 / 旋转 / 缩放不进签名）；改任一构建参数都会重建那一个刚体、稳态帧不动，刚体表整块在 `rapier_world/bodies.rs`（整表重建只在第一次建表，之后按 ID 差量修补；`rebuild_count()` / `body_patch_count()` 是两个诊断口径）（只比数量会漏换代、只比身份会让改字段静默不生效，探针缓冲跨帧复用）；`physics.character` 实体额外托管 kinematic-position-based 胶囊（比脚底高 `rest_gap`，避开 0.22 空实现的 penetration 修复），`physics.slope` 的静态楔形碰撞体由 `rapier_world/slopes.rs` 托管（`step` 里先于角色推进）；角色托管见 `rapier_world/characters.rs`；`ground_under` 是只读向下射线探地（回归对表 + 下一刀 `probe_ground` 的底子）；本文件只留刚体表与 `step` 编排 |
| `crates/xgc-sim/src/rapier_world/bodies.rs` | `RapierWorld` 的刚体表子模块（从 `rapier_world.rs` 拆出守 600 行）：`BodySignature` / `BodyLink`、第一次按 ID 排序整份建表（`rebuild`）、之后按签名 ID 序列增量修补（`patch_bodies`：只重建新增 / 换代 / 构建参数变了的条目、删掉消失的条目，未变化刚体保留 handle；动过表才 `query.update`）；`rebuild_count()`（整表重建，稳态 1）与 `body_patch_count()`（增量修补，稳态 0）是诊断口径 |
| `crates/xgc-sim/src/rapier_world/characters.rs` | `RapierWorld` 的角色子模块：按 `EntityId.raw()` 建 / 删 kinematic-position-based 胶囊，签名 = ID + `EntityKey`（index + generation）+ radius / half_height / skin_width / slope_limit 位模式（只见 ID 会漏掉 `Replace` / 快照恢复的换代，角色水平位移会静默冻结且旧胶囊变幽灵碰撞体），每帧 `sync_character_poses` + `move_characters`（`move_shape` 后回写 x / z 与水平 `Velocity`，竖直在下滑 / 重力时不清零；陡坡接触按竖直墙处理，`slipping`（超过 `slip_limit` 的坡）例外）；签名探针与收集表跨帧复用（热路径零分配），没有 `Transform` 的角色不进签名（否则补上 `Transform` 后签名不变、永远不建胶囊） |
| `crates/xgc-sim/src/rapier_world/slopes.rs` | `RapierWorld` 的坡面子模块：按 `Slope` 的 origin / size / rise 建 **静态楔形薄壳**碰撞体（cuboid 半长 `hypot(size.x, rise) / 2`、厚 `SLOPE_THICKNESS = 0.1`、半深 `size.y / 2`，绕 Z 转 `atan2(rise, size.x)`，中心沿法线从坡面中点下移半厚 ⇒ 上表面与 `Slope::height_at` 重合）；另挂 `physics.body` 的实体让位、参数非有限的坡不建体；签名 = ID + `EntityKey` + 全部几何位模式（变了才重建，先删旧刚体连带碰撞体再 `query.update`），探针 / 收集表跨帧复用；`ground_under` 是只读向下射线探地（`QueryFilter::default()`，任何碰撞体都会命中，做成 `probe_ground` 时要换过滤器）；`RapierPhysicsSystem::access().reads` 已声明 `Slope::ID`（读取契约与实际一致） |
| `crates/xgc-agent/src/lib.rs` | JSON-RPC 会话：describe（组件 / sim / render 能力清单）/ engine.diagnostics / engine.history / engine.replay / query / get / digest / stats / validate / load / save / plan / apply / revert / undo |
| `crates/xgc-agent/src/snapshot.rs` | `world.snapshot` / `world.restore`：WorldData 快照与恢复 |
| `crates/xgc-agent/src/query.rs` | 查询 IR：字段路径、过滤算子、标量比较 |
| `crates/xgc-audio/src/lib.rs` | cpal 音频设备输出：设备枚举、PCM 重采样、F32 / I16 输出队列、`play_clip` |
| `crates/xgc-script/src/lib.rs` | Lua 宿主：安全标准库、执行 / 调用、`xgc` 表（字段 / 实体 / 组件增删）、预算、内存上限、热重载 |
| `crates/xgc-script/src/legacy.rs` | 旧脚本 API：`update(ctx)` / `rot` / `set_rot` / `delta` / `entity_id` / `entity_pos` / `set_pos` / `look_at`（俯仰必须放欧拉角 X 分量，放 Z 会变滚转）/ `velocity` / `set_anim_param` / `ui_*` / `action_pressed`；UI 颜色 setter 的 sRGB 参数经唯一的 `srgb_to_linear()`（同旧端 0.04045 分段，alpha 不转）换算后写入组件，少这一步会在 sRGB 目标上二次编码；音频等暂无实现为 no-op |
| `crates/xgc-script/src/lifecycle.rs` | `start(ctx)` / `update(ctx)` 生命周期分派与回退 |
| `crates/xgc-script/src/runtime.rs` | `ScriptRuntime`：按 `script.lua` 驱动实体、每帧 `ScriptView` 快照、动作输入桥 `set_action`、命令结算、GUID 脚本经 `AssetCatalog` 解析、实体移除清理、磁盘热重载；`take_error()` 把本帧第一处脚本错误交给宿主（只打 stderr 时宿主看不到失败） |
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
| `crates/xgc-migrate/src/old_character.rs` | 旧 `Character` → `physics.character` 的输入投影：六个字段都带旧端 serde 缺省（1.8 / 0.3 / 0.35 / 45 / 0.02 / 0.3），手写旧 RON 省略字段也能迁出 |
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
| `crates/xgc-render/src/diff.rs` | `diff_images`：尺寸 / 差异像素比例 / 平均绝对误差 / 最大通道差；`diff_breakdown`：余差归因（相同 / ±1 / 2-3 / 4-7 / ≥8 五桶、三通道均值、16×9 块里最差的 5 块） |
| `crates/xgc-render/src/diff/tests.rs` | 余差归因纯函数回归（7 条）：分桶边界（含差值 3 / 7 / 8）/ 平手按行列升序 / 最多 5 块上限 / 块网格与最差块排序 / 窄轴切段不丢像素 / 尺寸不同报错 / 只改 alpha 不算差异 |
| `crates/xgc-render/src/ui_draw.rs` | UI 像素矩形 → NDC 四边形矩阵 |
| `crates/xgc-render/src/image.rs` | PNG / PPM（P6）解码 / 平均色块 |
| `crates/xgc-render/src/skinning.rs` | CPU 蒙皮：`skin_positions` / `identity` / `translation` |
| `crates/xgc-render/src/skinned.rs` | GPU 蒙皮顶点入口：256 关节 uniform（16 KiB，对齐旧端 `BONES_PER_SLOT`）、权重归一、palette 可表示性路由 + 设备 binding 上限判定；UV / 基色贴图绑定后与静态实例共享主场景 PBR；>256 或不完整 palette 回退 CPU；`palette_bytes` 是 `pose × inverse_bind` 的唯一 CPU 计算处，`posed_bounds` 给取景 / 级联提供当前姿态包围盒（单元回归在 `skinned/tests.rs`）；`draw_batch` 的 palette / model 走竞技场（`&mut self`，返回 `SkinnedBatchOutcome`） |
| `crates/xgc-render/src/skinned/arena.rs` | 蒙皮 uniform 竞技场：palette / model 并进常驻缓冲按 dynamic offset 分槽，槽距按设备 `min_uniform_buffer_offset_alignment` 向上取整、容量按 2 的幂扩容，同尺寸跨帧不建缓冲（`RenderStats::skin_uniform_buffers` / `shadow_skin_uniform_buffers` 是口径：各首批 2 张、之后 0） |
| `crates/xgc-render/src/skin_vertex.wgsl` | GPU 蒙皮顶点变形唯一真相源：palette 绑定 + `Σwₖ·palette·pos` 与权重归一；主场景与阴影入口都 include 这一份 |
| `crates/xgc-render/src/shadow_skin.rs` | 阴影 pass 的蒙皮投射体：把 draw 解析成 caster（GPU palette / CPU 回退已变形顶点 / 静态），每帧把 palette+model 传进阴影自己的两张 `UniformArena`、`SkinnedBinding` 只记槽号，4 级级联只换 dynamic offset（`SkinnedShadow::bindings` 返回本帧新建缓冲数） |
| `crates/xgc-render/src/shadow_skin.wgsl` | 蒙皮阴影顶点入口：只复用 `skin_vertex.wgsl` 的变形公式与光视图投影，纯深度 pass 不着色 |
| `crates/xgc-render/src/texture.rs` | RGBA8 纹理四边形管线（UI 图片） |
| `crates/xgc-render/src/text.rs` | 5x7 位图字体 → NDC 文本网格（保留给纯位图调试文本；UI 文本已走 `font.rs`） |
| `crates/xgc-render/src/font.rs` | 真字体文本：`ab_glyph` + 与旧端同一套系统字体候选（msyh / **msyhbd 粗体** / Deng / simhei…）、shelf 字形图集（RGBA8，rgb=255 / alpha=覆盖率）、换行 / 对齐 / 字体原生 kerning；粗体缺失时静默退回常规字重，字体缺失时静默跳过 |
| `crates/xgc-render/src/text_draw.rs` | 文本绘制管线：一次 draw 画一批字形四边形（alpha 混合、不写深度），图集变动才重传 |
| `crates/xgc-render/src/camera.rs` | `render.camera` + `GlobalTransform` → view-projection / 模型矩阵 / MVP；列主序 4x4 求逆与正交投影（天空盒 / 阴影视锥） |
| `crates/xgc-render/src/pipeline.rs` | `TrianglePipeline` / `RenderCache`：单 draw 路径、颜色 / 深度管线、scissor / 圆角 SDF 裁剪、实例化缓存 |
| `crates/xgc-render/src/draw.rs` | 从 `render.mesh` + `render.material` + `GlobalTransform` 抽 `DrawList`（含模型矩阵、材质资产引用与祖先骨架姿态） |
| `crates/xgc-render/src/frame_color.rs` | 逐 draw 颜色合成：`模型基色因子 → .mat 资产 → 实体覆盖 → 选择高亮`（分流前一次做完） |
| `crates/xgc-render/src/material.rs` | `.mat` 材质资产解析（基色乘子 + metallic / roughness，加载边界清洗） |
| `crates/xgc-render/src/tone.rs` | ACES 曲线 CPU 镜像与 8 位对表辅助；另含着色器平光口径的 CPU 镜像（`ambient_light` / `fresnel_schlick` / `sky_kd_env`），平光测试期望值由它现算 |
| `crates/xgc-render/src/shadow_settings.rs` | 阴影画质设置：`softness`（PCSS 灯光尺寸）/ `cascade_blend`（级间混合带宽），`sanitized()` 把非有限 / 非正一律落 0（不夹上界，旧端 `ShadowConfig` 同口径），全 0 = 关（走 3×3 PCF + 硬级间切换，逐位不变） |
| `crates/xgc-render/src/shadow_csm.rs` | 方向光阴影 CPU 侧：单级拟合 `fit_directional_shadow`（关 CSM 的等价退路）+ **4 级 CSM** `cascade_splits`（λ=0.7）/ `cascade_shadow_matrices`（逐级紧致正交拟合、纹素吸附防抖；带 `overlap` 把非 0 级拟合框近端多铺 `overlap × 本级跨度`，并回填 `view_range` 的相机深度范围），与旧端逐行同式 |
| `crates/xgc-render/src/shadow.rs` | 方向光阴影 GPU 侧：2048² 深度**数组**（每层一级）+ 逐级深度 pass；静态与蒙皮投射体共用同一支 pass 与同一套 depth bias；接收端按视线深度选层 + 3×3 PCF（`render.shadow` 开时可切 PCSS + 级间混合）、偏置随 N·L 放大；另持 `PointShadowTarget`，点光六面复用同一支深度管线与实例缓冲（`ShadowStats::point_draw_calls` 是诊断口径）；`render` 同时接受方向光级联与点光计划，两者可以只有一个 |
| `crates/xgc-render/src/sky_draw.rs` | 天空盒 pass：全屏三角形 + 解析天空逐像素求值（远平面 / `LessEqual` / 不写深度），`inv_view_proj` 反推视线方向 |
| `crates/xgc-render/src/exposure.rs` | 画面曝光的唯一消毒口径：`EXPOSURE_DEFAULT = 1.0` / `MIN_EXPOSURE = 0.05` / `MAX_EXPOSURE = 20.0` / `sanitize_exposure`（旧端 `RenderConfig::sanitized` 同式：非有限 / 非正落回 1.0、合法值钳制；曝光 0 是"全黑画面"不是关闭态）；乘子在 tone-map 与光晕亮部提取两处共用 |
| `crates/xgc-render/src/renderer/hdr.rs` | HDR 中间目标与呈现：按尺寸缓存 `Rgba16Float` 场景纹理，按输出格式缓存全屏 tone-map pipeline；tone pass 的 6 个绑定里带三级光晕输入（关闭时绑场景纹理占位）；tone-map 输入由调用方给的视图决定（HDR 场景或 TAA 解析结果），`present(..., scene, format, Look { bloom, exposure }, glow)` 每帧建绑定组；tone-map 之后可选跑一趟 FXAA（`crates/xgc-render/src/fxaa.rs`，落点由调用方给的视图决定），TAA 开着时 FXAA 让位 |
| `crates/xgc-render/src/bloom.rs` | 三级 HDR 光晕链（旧端 `postfx.rs` 同口径）：`BloomSettings`（intensity / threshold / knee，`intensity <= 0` 即关闭）+ 亮部提取 / 逐级降采样 / 横纵二项核共 9 个 pass，`Rgba16Float` 乒乓中间目标按尺寸缓存；`BloomStage::render` 收当前曝光（亮部提取按曝光后的亮度判阈值），链缓存键带曝光位模式 |
| `crates/xgc-render/src/bloom.wgsl` | 光晕三个片元入口：`fs_bright`（软膝式取亮部）/ `fs_down`（一次双线性降采样）/ `fs_blur`（9 抽头二项核，方向由参数给横或纵） |
| `crates/xgc-render/src/taa.rs` | 时间抗锯齿（旧端 `taa.rs` 同口径）：`TaaSettings`（`enabled` / `history_weight`，默认关、上界 0.98）+ 8 相 Halton(2,3) 抖动（帧号纯函数）、像素→裁剪 / 纹理两套换算、`jitter_projection`；`TaaStage` 管两张 `Rgba16Float` 历史乒乓与上一帧未抖动视图投影，深度反投影重投影 + 3×3 邻域钳制后 `mix`，换尺寸重建、关掉即释放；`reset_history()` 清重投影状态（相机瞬移 / 切场景帧调用，历史纹理保留） |
| `crates/xgc-render/src/taa.wgsl` | TAA 解析着色器：当前色 + 深度 + 历史，`blend <= 0` / 相机背后 / 越界直通当前帧，天空按视线推 1e4 只保留旋转视差 |
| `crates/xgc-render/src/renderer/settings.rs` | 画面设置入口：`set_sky` / `set_exposure` / `set_bloom` / `set_fxaa` / `set_shadow_settings` / `set_taa` / `set_ssao`（都经各自的 `sanitized()`），从 `renderer.rs` 按行数预算拆出 |
| `crates/xgc-render/src/renderer/taa_pass.rs` | TAA 的每帧编排：`taa_begin` 返回抖动后的场景矩阵、`taa_resolve` 解析成可采样纹理并计 pass、`taa_history_size` 是诊断口径；关着两步都原样返回 |
| `crates/xgc-render/src/ssao.rs` | 环境光遮蔽（旧端 `ssao.rs` 同口径）：`SsaoSettings`（`enabled` / `strength` / `radius` / `bias`，默认关；`strength <= 0` 与关等价）+ 固定种子 LCG 的 16 抽头半球核 + 半分辨率 AO → [1,4,6,4,1]/16 可分离模糊 → 全分辨率乘算四趟 pass；`SsaoParams` 448 字节，退化矩阵把强度写 0（画面逐位不变），换尺寸重建、关掉释放 |
| `crates/xgc-render/src/ssao.wgsl` | SSAO 四个片元入口：`fs_ao`（深度反投影 + 深度梯度法线 + 像素哈希旋转的 16 抽样）/ `fs_blur_h` / `fs_blur_v` / `fs_apply`（半分辨率 AO 线性上采样乘源色） |
| `crates/xgc-render/src/renderer/ssao_pass.rs` | SSAO 的每帧编排：`ssao_apply` 关着原样返回、开着跑四趟并计 pass；`ssao_target_size` 是诊断口径（关着为 None） |
| `crates/xgc-render/src/renderer/sky_pass.rs` | 天空 pass 的每帧绘制（`draw_sky`：几何为空时清屏 + 全屏三角形；从 `renderer.rs` 按行数预算拆出） |
| `crates/xgc-render/src/fxaa.rs` | 显示空间 FXAA（旧端 `postfx.rs` 同口径）：`FxaaSettings`（`enabled`，默认关）+ 与输出同格式的 LDR 暂存（按尺寸 / 格式缓存，关闭时释放）+ tone-map 之后、UI 之前一趟全屏 pass；`present` 返回本帧跑的 pass 数 |
| `crates/xgc-render/src/fxaa.wgsl` | FXAA 3.11 质量路径（5 抽头、亚像素 0.75、方向收缩 1/8、跨度上限 8 texel、阈值 1/12 与 1/6、Rec.601 luma、sRGB 手动 encode / decode）；平坦区直接返回中心采样，保证关掉边缘时逐字节不变 |
| `crates/xgc-render/src/renderer/shadow_prep.rs` | 阴影的每帧准备（主阴影光取自 `lights::primary_shadow_light`，与光源表同判据）：点光挑灯 + 六面计划、方向光按场景包围盒 + 相机矩阵拟合级联，把 draw 画进两种深度图；返回 `PreparedShadows`（含阴影蒙皮竞技场本帧新建的缓冲数与点光绘制次数），并按需建深度 / 天空 / 阴影 / 贴图目标（`ensure_frame_targets`）——从 `renderer.rs` 按行数预算拆出 |
| `crates/xgc-render/src/renderer/ui.rs` | UI 覆盖层：背景 / 焦点环 / 真字体文本 / 图片在 tone-map 后画到显示目标（从 `renderer.rs` 按行数预算拆出；字形先排版后上传图集） |
| `crates/xgc-render/src/sky.rs` | 解析天空（天顶/地平线/地面 + 太阳盘 + 光晕）与漫反射 IBL：1024 点斐波那契球投影成 L2 球谐（E/π，无随机数）；参数来自 `project.ron` 的 `render.sky` |
| `crates/xgc-render/src/lights.rs` | 多光源表：把 `is_active()` 的方向光 / 点光按原顺序打包成旧端同款的 8 槽 `GpuLight`（方向光存传播方向 `w = 0`、点光存世界坐标 `w = 1`、`shadow_params.x` = 用方向光阴影图、`.y` = 点光半径），`intensity <= 0` 与第 9 盏起丢弃，只有第一盏投射阴影的方向光打 CSM 标记；`primary_shadow_light` 与打包表同判据（主阴影光排在第 9 槽之后就不拟合 CSM） |
| `crates/xgc-render/src/point_shadow.rs` | 点光立方体阴影 CPU 侧：挑第一盏 `casts_shadow` 且 `range > 0` 的点光（前 8 槽），六面投影 = `flip_u · perspective(90°) · look_at`（up 照 WebGPU 映射表、u 轴镜像），主轴分量参考深度；`PointShadowTarget` 持 1024² 六层 `Depth32Float`（采样视图是 Cube）与 1×1 假图兜底 |
| `crates/xgc-render/src/frame_uniform.rs` | 帧 uniform 布局与绑定：1008B（相机 / 环境光 / 天空球谐 / 4 级光视图投影 / 阴影参数 / 阴影画质 / 8 槽光源表 / 点光阴影段 32B；六面矩阵只进每面 64B 阴影帧），group 0 四绑定（frame、方向光阴影数组、比较采样器、点光立方体） |
| `crates/xgc-render/src/instanced.rs` | 按模型分组实例化；帧 uniform 已搬到 `frame_uniform.rs`（相机位置 + 环境光 + 天空球谐 + 4 级光视图投影/切分/视线/阴影参数 + 阴影画质 + 8 槽光源表（方向光 / 点光）），逐实例 96B（模型 + 颜色 + metallic/roughness）；片元按 `环境光 + 天空 IBL + 环境反射 + 逐盏光色 × 强度 × 衰减 × N·L × 阴影`（方向光无衰减、点光 `fade/(1 + 0.09d + 0.032d²)`）输出线性 HDR |
| `crates/xgc-render/src/particles.rs` | 实例化 billboard 粒子：相机朝向软圆、Alpha / Additive、后向前排序、在线性 HDR 中预乘混合、深测开 / 深写关 |
| `crates/xgc-render/src/texture_data.rs` | 基色贴图 RGBA8：PNG 字节解码（8-bit）+ 1×1 白纹理兜底 |
| `crates/xgc-render/src/texture_bind.rs` | 贴图 GPU 绑定：group(2) 布局 + 线性 Repeat 采样器 + 白纹理兜底绑定组 |
| `crates/xgc-render/src/gpu_mesh.rs` | 模型 → 位置 / 法线 / UV / 索引缓冲 + 贴图绑定组，跨帧复用（没法线时法线流灌全零哨兵；CPU 蒙皮回退每帧只重算位置、法线流清零退回导数） |
| `crates/xgc-render/src/scene_shading.wgsl` | 静态 / GPU 蒙皮唯一主场景片元真相源：Frame / 贴图 binding、环境光菲涅尔、Cook-Torrance、天空 SH / 环境反射、CSM PCF / 可选 PCSS 软阴影 + 级间混合（`shadow_soft`），多光源循环（方向光 / 点光 + 点光距离衰减），点光立方体阴影（主轴分量参考深度 + 3×3 比较，`params.z = 0` 整段跳过），片元法线优先用顶点法线（逆转置、不按视线翻号）、缺省退回屏幕导数叉积，输出线性 HDR |
| `crates/xgc-render/src/tone_map.wgsl` | GPU 唯一 ACES 真相源：从 HDR 逐像素 `textureLoad`（关闭光晕时逐位不变），先乘曝光（`ToneParams.exposure`，整块 32 字节），`intensity > 0` 时再按 `uv` 双线性采样三级光晕叠回线性场景**再**过 ACES，全屏三角形写调用方给的视图并固定 alpha=1（关闭 FXAA 时就是显示目标） |
| `crates/xgc-render/src/instanced.wgsl` | 静态实例顶点入口：读取 group(1) 实例 model / color / material + 法线（逆转置变换），输出共享 `SceneVsOut` |
| `crates/xgc-render/src/skinned.wgsl` | GPU 蒙皮主场景顶点入口：group(3) model / material + share 的变形函数（法线先按 palette 变形再逆转置），输出共享 `SceneVsOut` |
| `crates/xgc-render/src/shot.rs` | 截图取景：`Bounds` 拟合、顺光视轴（由场景主方向光推）、近远裁剪；`render_shot` 优先场景相机 |
| `crates/xgc-render/src/mesh.rs` | `Mesh` 位置 + 索引 + 顶点法线（可为空：空 = 模型没导 `NORMAL`）+ `SkinData` + `base_color_factor` + `uvs` + 基色贴图（`Arc<TextureData>`）；builtin quad / cube / sphere / plane / cylinder 为真实几何 |
| `crates/xgc-render/src/glb/attributes.rs` | 首图元属性装配：`NORMAL`（调 `glb/normals.rs` 并校验条数）+ JOINTS_0 / WEIGHTS_0 + 逆绑定矩阵，从 `glb.rs` 按行数预算拆出 |
| `crates/xgc-render/src/glb/normals.rs` | `NORMAL` 访问器读取：只接受 float VEC3、逐顶点三份、不提前归一（归一在片元）；读之前按 `checked_add` / `saturating_mul` 校验偏移与 BIN 长度（JSON 的 `count` 不可信，坏资产必须是 `MeshError` 而不是 panic） |
| `crates/xgc-render/src/glb.rs` | GLB / GLTF 解析：POSITION / 索引 / `NORMAL`（装配在 `glb/attributes.rs`，条数必须与 POSITION 一致）/ JOINTS_0 / WEIGHTS_0 / 首图元材质 / `load_glb_skeleton`；`.gltf` + 外部 bin 经内存 GLB 合并 |
| `crates/xgc-render/src/glb/texture.rs` | `TEXCOORD_0` 与 `baseColorTexture`：外部 uri（相对模型目录）/ GLB 内嵌 bufferView 解码 PNG；data URI 直接报不支持 |
| `crates/xgc-render/src/glb/material.rs` | glTF 材质 JSON 最小投影：`baseColorFactor`（缺省白）+ `baseColorTexture` 索引 |
| `crates/xgc-render/src/assets.rs` | `MeshCache`：builtin / 相对路径 GLB / AssetCatalog `guid://` 解析 / 越界拒绝；`.mat` 材质按引用缓存，读不到退回模型材质 |
| `crates/xgc-render/src/resources.rs` | `ResourceDesc` / `ResourceKind` / `ResourceUsage` / `ResourceTable` |
| `crates/xgc-world/tests/` | world / commands / journal / query / snapshot 回归 |
| `crates/xgc-player/src/lib.rs` | `load_world_dir` / `runtime_for` / `run_cooked`：manifest 校验、优先 XGCB、脚本驱动 |
| `crates/xgc-player/src/window.rs` | eframe + wgpu 视口：每帧仿真 / 脚本 / 粒子并贴图到窗口 |
| `crates/xgc-player/src/main.rs` | `xgc-player <cooked 目录> [帧数]`：无帧数打开窗口，有帧数 headless |
| `crates/xgc-cli/src/main.rs` | CLI 参数分派；文件预算拆分后的轻量入口 |
| `crates/xgc-cli/src/runtime_cmd.rs` | `run-scripts` / 截图 / `xgc trace <世界.json> [帧数] [资产根]` 共用的 `ScriptedRunner`（系统 → 脚本 → 粒子，逐帧可读世界）：`Simulation::step_with` + `ScriptRuntime::run_frame` 固定帧桥；截图路径同步产出粒子四边形 |
| `crates/xgc-cli/src/trace_cmd.rs` | `xgc trace <世界.json> [帧数] [资产根]`：逐帧打印 `frame / entities / revision / hash`（无资产根时与 `run-world` 同一条无脚本步进路径，带资产根时走 `ScriptedRunner`、与 `run-scripts` 同一路径；第 0 帧是装载后状态），末行汇总与 `run-world` / `run-scripts` 同口径 |
| `crates/xgc-cli/src/world_cmd.rs` | `xgc run-world <世界.json> [帧数]` / `verify-migration <旧.ron> <世界.json>` |
| `crates/xgc-cli/src/assets_cmd.rs` | `xgc assets <项目根>`：GUID 扫描 |
| `crates/xgc-cli/src/cook_cmd.rs` | `xgc cook <项目根> <输出目录>` / `pack <世界.json> <项目根> <输出目录>` / `run-cooked <目录> [帧数]` |
| `crates/xgc-cli/src/compare_cmd.rs` | `xgc compare-headless` / `xgc image-diff <旧图> <新图> [阈值] [report]`；PNG / PPM 逐像素差异，`report` 追加余差归因 |
| `crates/xgc-cli/src/audio.rs` | `xgc play <wav>`：解码后推默认音频设备 |
| `crates/xgc-cli/src/input_cmd.rs` | `xgc input <input.ron>`：校验并列出动作 / 轴 |
| `crates/xgc-cli/src/render.rs` | `render-frame <世界> <PPM> [资产根] [宽] [高] [帧数]`；缺省 0 帧，最终重算层级 / UI；曝光、天空、光晕、阴影画质与 FXAA 设置由 `project_render.rs` 从项目级 `project.ron` 读出后灌进 `Renderer` |
| `crates/xgc-cli/src/project_render.rs` | `project.ron` 画面设置投影：`render.exposure` / `render.sky` / `render.bloom` / `render.fxaa` / `render.shadow`；缺文件 / 缺节一律落回旧端缺省（曝光 1.0、其余关闭态），缺省值必须逐项等于 `RenderConfig` 的曝光 1.0 与 `SkyConfig` / `BloomConfig` / `FxaaConfig` / `ShadowConfig` 的 `default` |
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

1. 割接视觉：glTF `NORMAL` 顶点法线着色已接入（缺 NORMAL 退回屏幕导数）；RGBA16F 场景、线性粒子合成、唯一全局 ACES、曝光后曲线前三级光晕（`render.bloom`，默认关）、tone 后 UI、蒙皮网格投射 CSM 阴影、256 槽 GPU palette、姿态感知包围盒（`scene_bounds` 走 `posed_bounds`）、蒙皮 draw 单次提交（`SkinnedPipeline::draw_batch`）、蒙皮 palette / model 竞技场（`skinned/arena.rs` 的 dynamic offset 合并）已贯通（frame-150 对旧端 mean_abs 0.61）。阴影 pass 的蒙皮 palette / model 也已并进同一套竞技场（`renderer/shadow_prep.rs`），角色坡面 `slope_limit` / 贴地 `snap_to_ground` 也已生效（陡坡当墙 + 下行贴地）、tone-map 后可选的显示空间 FXAA（`render.fxaa`，默认关；平坦区逐字节不变）也已贯通，角色的**水平**位移已走 rapier `KinematicCharacterController`（`character_kcc.rs`，签名带实体代际；`CharacterBlockSystem` 已删、默认调度 16 个系统），纯角色重力（`character_kcc::GRAVITY_Y` 在角色阶段积分，腾空自由下落、落地清速度）也已生效，`physics.slope` 的静态楔形碰撞体也已补上（`rapier_world/slopes.rs`，上表面与解析坡面重合、`ground_under` 提供向下射线探地），角色控制器缺省值也与旧端逐项对齐（`0.35 / 0.02 / 0.3`、消毒收口到 `CharacterController::sanitized()`），角色的竖直 / 地面 / 台阶也已搬进同一个 KCC（`probe_ground` / `try_step_up` / 竖直积分在 `rapier_world/characters.rs`，`CharacterSystem` / `CharacterStepSystem` 已删除、默认调度 16 个系统）；陡坡沿坡面下滑与 `(slope_limit + 10°)` 迟滞带也已生效（`character_kcc::slip_limit`，下滑时不清竖直速度），刚体建体签名也已覆盖全部构建参数（改 `PhysicsBody` 字段会重建、稳态帧不重建），刚体表也已改为按 ID 差量修补（增删 / 改参数只动受影响的刚体），画面曝光也已接上（`render.exposure` 同时进 tone-map 与光晕亮部提取，缺省 1.0 时逐字节不变），阴影画质也已接上（`render.shadow` 的 `softness` / `cascade_blend`，可选 PCSS 软阴影 + 级间混合、默认全 0 = 关、关着走 3×3 PCF + 硬切换且逐位不变），多光源照明也已接上（方向光 / 点光最多 8 盏 + 点光距离衰减，帧 uniform 扩到 976 字节，单盏方向光时逐字节不变）；点光立方体阴影也已接上（`point_shadow.rs` + `frame_uniform.rs`，六面 1024² 复用同一支深度管线、帧 uniform 扩到 1008 字节，证据世界没有点光所以逐字节不变）；TAA 也已接上（`render.taa` 的 `enabled` / `history_weight`，默认关、关着不建历史且相机不抖动、逐字节不变，开着时抖动只叠相机、历史重投影 + 邻域钳制、FXAA 让位）；SSAO 也已接上（`render.ssao` 的 `enabled` / `strength` / `radius` / `bias`，默认关、关着不建 AO 纹理且 `passes` 不增、逐字节不变，开着时半分辨率 16 抽样 + 可分离模糊 + 线性上采样乘算，排在 TAA 之后、光晕之前）；热路径分配门禁也已建立（`crates/xgc-sim/tests/steady_state.rs`：稳态帧从 1697 次/帧降到 230 次/帧、预算门禁；数值型组件 `state_hash` 零分配；release 下 0.057 ms/帧）；逐帧 trace 的 XGC 侧工具也已建立（`xgc trace`，逐帧打印实体数 / revision / `state_hash`；带资产根时走 `ScriptedRunner`、与 `run-scripts` 同一条路径，脚本帧也纳入），视觉余差也已能按「舍入 vs 结构性」归因（`image-diff … report`：26.5% 像素只差 1 LSB、结构性 1.82%），下一刀做关节 / 连续碰撞（CCD）/ 正式 System 调度。级联正交盒与旧端同为“按相机视锥切片拟合 + 逐级剔除”，视锥外投射体不需要单独扩展覆盖。
2. 脚本：正式 System 调度、并行执行与脚本组件生命周期完整化。
3. 割接：旧新逐帧 trace（XGC 侧 `xgc trace` 已建、脚本帧也已纳入，缺旧端输出）/ 截图；视觉余差归因工具 `image-diff … report` 已建（26.5% 像素只差 1 LSB、结构性 1.82%），收窄仍待旧端逐帧截图；性能门禁的 XGC 侧热路径分配门禁已建（还缺 CI release 基线）。
4. 资产：密码学签名 / 跨机器增量。
5. 推送与 CI；当前最大 Rust 文件 599 行（`crates/xgc-render/src/ssao.rs`），继续守住 600 行软上限。
   角色回归按语义分三个文件：`crates/xgc-sim/tests/character_kcc.rs`（水平 KCC + 台阶 + 陡坡挤入 + 顶着墙起跳 / 贴墙下落：撞墙 / 斜墙滑 / 球体阻挡 / 刚体优先 / `Replace` 换代后仍移动 / 默认调度陡坡不挤入 / 小半径胶囊上台阶 / 低台阶抬升 / 带刚体不被角色系统写 / 确定性）、
   `crates/xgc-sim/tests/character_ground.rs`（竖直 / 地面 / 坡：自由落体 / 跳跃弧线 / 站地不沉 / 站坡不滑 / 走上 30° 坡 / 走下 30° 坡贴地吸附 / 走出坡沿落地 / 落到箱顶 / 60° 坡上会滑下去 / 50° 迟滞带内站得住 / 确定性）、
   `crates/xgc-sim/tests/rapier_schedule.rs`（刚体表：默认调度动态体落地 / `Replace` 换代后仍移动 / 改 `PhysicsBody` 参数会重建 / 稳态帧不重建 / 缩放动画不重建 / 逐字段表驱动（`half_extents` 三个分量各一条）/ `kind` 拼错仍按动态体回写 / 运行期增删只增量修补 / 增量修补不扰动已有刚体）、
   `crates/xgc-sim/tests/slope_collider.rs`（坡面碰撞体：陡坡挡住角色 / 上表面与解析坡面逐点对齐 / 改坡面参数后重建 / 带 `physics.body` 的坡让位 / 尺寸退化与非有限参数被跳过 / 下坡同样对齐）。
   各文件世界构造不同（坡 + 无刚体 / 台阶 / 静态盒 + 刚体 / 只有坡没有角色），别把语义混着塞回一个文件。

## Token 纪律

- 先读本文件 + README；不要读 `target/` 或整份主文档。
- 每轮只动一个可验证的垂直切片；先跑测试再写总结。
- 长任务交给子智能体时，给它**文件路径 + 不变量 + 验收命令**，不要复制整份设计。
