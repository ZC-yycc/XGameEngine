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
cargo run -q -p xgc-cli --offline -- plan
cargo run -q -p xgc-editor --offline -- [世界.json]
cargo run -q --release -p xgc-cli --offline -- bench
```

本机 cargo 在 `C:\Users\Administrator\.cargo\bin\cargo.exe`，不在 PATH 时用全路径。
本机 GPU 集成测试须 `RUST_TEST_THREADS=1`（并行建两个 headless GPU 设备会死锁）。
网络不可用时一律加 `--offline`（serde / serde_json 已缓存）。

## 入口文件

| 文件 | 职责 |
|---|---|
| `crates/xgc-base/src/lib.rs` | `EntityId` / `EntityKey` / `Tick` / `Diagnostic` |
| `crates/xgc-base/src/diagnostics.rs` | 诊断码常量 + `engine.diagnostics` 目录 |
| `crates/xgc-reflect/src/lib.rs` | `Component`、`Registry`、`SchemaDoc`、递归 `Value`（结构体、列表、字符串键映射、带载荷枚举）；`path.rs` 解析字段路径，JSON 引号键和当前枚举分支只寻址已有值；`schema.rs` 导出和校验类型结构；`state_fields()` 只用于 `state_hash` |
| `crates/xgc-world/src/lib.rs` | `World` / `Bundle` / `Archetype` / `Column` / 单组件查询 / `reserve_entity_ids` |
| `crates/xgc-world/src/query.rs` | 双 / 三组件不可变 / 可变 / archetype 分片并行查询 |
| `crates/xgc-world/src/parts.rs` | 跨系统并行写视图 `WriteSet` / `extract_writes` / `restore_writes` |
| `crates/xgc-world/src/components.rs` | 内建 `Transform` / `Velocity` / `Name` / `Parent` / `GlobalTransform` |
| `crates/xgc-world/src/render_components.rs` | 渲染侧组件：`render.mesh` / `render.material` / `render.camera` |
| `crates/xgc-world/src/lod_components.rs` | `render.lod`：按原顺序存放 `LodLevel`，模型与最大距离为反射字段，零或负值表示无限距离 |
| `crates/xgc-world/src/environment.rs` | 场景级环境 `SceneEnvironment` / `SceneLight` / `LightKind`：光源与环境光（不是实体，随 `WorldData.environment` 存取，缺省不落盘） |
| `crates/xgc-world/src/registry.rs` | `builtin_registry`：CLI / Agent / xtask 唯一组件清单 |
| `crates/xgc-world/src/skeleton_components.rs` | 骨架组件：`animation.skeleton` / `animation.bone` / `animation.pose` |
| `crates/xgc-world/src/state_machine_components.rs` | 基础动画状态机组件 `animation.state`：类型化 `StateMachine` 与数值参数映射，状态和转移可按路径编辑；基础状态机和混合层共用 `StateMachine` / `StateDef` / `TransitionDef` / `TransitionCondition` |
| `crates/xgc-world/src/layer_components.rs` | 动画混合层组件 `animation.layers` 的唯一内存类型 `Vec<AnimLayer>`，供模拟和渲染直接读取；mask 是沿 parent 链匹配的节点名子树根名单 |
| `crates/xgc-world/src/layer_reflection.rs` | 混合层的结构化反射，含模式、遮罩、参数和淡入状态；Enabled 分支复用基础状态机的字段定义与校验 |
| `crates/xgc-world/src/gameplay_components.rs` | 玩法侧组件：`script.lua` / `audio.source`（含 bus）/ `audio.listener` / `audio.gain` / `audio.mixer` |
| `crates/xgc-world/src/animation_components.rs` | 动画组件：`animation.animator`（clip / speed / time / blend_from / blend_elapsed / blend_duration / additive_clip / additive_weight / additive_speed / additive_looping / additive_time / root_motion / events）；`events` 是结构化列表，作者配置由 `state_fields()` 排除，不进 `state_hash` |
| `crates/xgc-world/src/root_motion.rs` | 根运动纯函数：`RootMotion`（显式节点名 + 轴开关，RON 配置）、`RootMotionTrack` 采样与循环展开轨迹、`root_motion_delta`（fade 混合两段增量）、`to_world_delta` / `to_local_position`（TRS 近似，无剪切项） |
| `crates/xgc-world/src/character_components.rs` | 角色组件：`physics.character`；缺省值（1.8 / 0.3 / 0.35 / 45 / 0.02 / 0.3）与旧端 `xgame_core` 逐项对齐，`sanitized()` / `half_height()` / `center_height()` 是唯一消毒与几何派生真相源（`character_kcc` 全部委托）；改常量记得同步 `fields()` 的 schema 缺省值（有 `schema_defaults_match_the_component_defaults` 守着），区间常量之间的咬合由编译期 `const _: ()` 断言守着 |
| `crates/xgc-world/src/ui_components.rs` | UI 组件：`ui.element` / `ui.layout` / `ui.interaction` / `ui.input`；`background` / `border_color` / `text_color` 存**线性** RGB（旧脚本 UI 颜色 setter 的 sRGB 在入口换算；通用字段写入按线性原值落库；渲染端写 sRGB 目标由硬件编码） |
| `crates/xgc-world/src/particle_components.rs` | 粒子组件：`particle.emitter` |
| `crates/xgc-world/src/plugin_components.rs` | Rust 插件组件：`plugin.rust` |
| `crates/xgc-world/src/prefab_components.rs` | 预制体实例：`prefab.instance`（source / source_id / root） |
| `crates/xgc-world/src/physics_components.rs` | `physics.body`：类型 / 形状 / 摩擦 / 弹性 / 质量 / 传感器；`BodyKind` / `PhysicsBody::kind()` / `is_dynamic()` 是「什么算动态体」的唯一判据（`rapier_world` 建体与回写、`GravitySystem`、`GroundCollisionSystem`、`AabbCollisionSystem` 共用，未知字符串按旧端兜底一律当 Dynamic） |
| `crates/xgc-world/src/slope_components.rs` | `physics.slope`：沿 +X 线性抬升的坡面；`rapier_world/slopes.rs` 按同一参数给坡面建静态楔形薄壳碰撞体（`SLOPE_THICKNESS = 0.1`、上表面与 `height_at` 重合，ECS 与 rapier 看同一个坡面）；`degrees()` / `base_x_at()` 是角色坡限判定的唯一口径（`base_x_at` 结果夹进坡面 X 跨度，`rise <= 0` 的下坡一律当地面、没有立面） |
| `crates/xgc-world/src/collision.rs` | 瞬态碰撞事件 `CollisionEvent { entity, other, enter }`：一对接触给双方各一条有向事件；World 用 `collision_events` 存本步事件，`Simulation::step_with` 每步开头清空。**不进 `WorldData` / 快照 / `state_hash`**（只服务脚本钩子，回放对表不应看到它） |
| `crates/xgc-world/src/animation_events.rs` | 瞬态动画事件 `FiredAnimEvent { entity, name }`：`Animator.events` 由 `AnimatorSystem` 跨帧穿越判定后写入，脚本宿主下一步消费；**不进 `WorldData` / 快照 / `state_hash`**。`AnimEvent { time, name }` 与反射结构相互投影，编辑时检查时间和名称 |
| `crates/xgc-world/src/command.rs` | `Command` / `Plan` / `Applied`：apply / revert / Replace；`command/plan.rs` 对有顺序依赖的命令在实体快照上推演，`field_edit.rs` 共用动态路径读写 |
| `crates/xgc-world/src/command/replace.rs` | 回滚与实体整份替换内部实现 |
| `crates/xgc-world/src/data.rs` | `EntityData` / `ComponentData` / 组件工厂 |
| `crates/xgc-world/src/journal.rs` | `Journal` / `TxRecord`：事务日志与重放 |
| `crates/xgc-world/src/snapshot.rs` | `EntitySnapshot` / `WorldSnapshot` / 恢复 |
| `crates/xgc-world/src/validate.rs` | `World::validate_data`：组件 / 字段 / 类型 / 重复项体检 |
| `crates/xgc-world/src/hash.rs` | `World::state_hash`：实体按 ID、组件按 ID、字段按声明序混入；字段名取 `Column::state_field_names()`（`Component::state_fields()`，缺省 = `fields()`），作者数据字段（如 `Animator.events`）在这里排除 ⇒ 加作者字段不改老场景哈希；实体行号走 `thread_local` scratch、组件 / 字段布局缓存在 `Archetype::hash_layout`（数值型组件世界 60 次调用零分配） |
| `crates/xgc-world/tests/hash.rs` | 作者字段与状态哈希的分离回归：`Animator.events` 变化不改 `state_hash`，但 `WorldData` 仍落盘、恢复后事件仍在 |
| `crates/xgc-cli/tests/image_diff.rs` | `image-diff … report` 集成回归：真实 PPM 的分桶与通道均值 / 不写 report 时保持旧输出 / 未知第四参数退出码 2 |
| `crates/xgc-cli/tests/trace.rs` | 逐帧 trace 集成回归：与进程内重放逐步对表 / 两次运行逐字节相同 / 不同世界第 0 帧即不同 / 参数错误退出码 2 / 汇总行与 `run-world` 末行逐字一致 / 带资产根时第 1 帧起与无脚本路径分叉且汇总行与 `run-scripts` 末行逐字一致 / 坏脚本与 `update` 期抛错都退出码 2 且点名出错帧 |
| `crates/xgc-cli/tests/gate.rs` | 割接门禁集成回归：参考匹配通过 / 哈希分歧退出码 1 并点名帧 / 空参考目录退出码 2 / 参考帧号不从 0 开始退出码 2 |
| `crates/xgc-sim/tests/steady_state.rs` | 热路径分配门禁：线程本地计数分配器（每个测试线程只看自己的分配）+ 稳态帧预算（60 × (实体数 + 64)），数值型组件世界 `state_hash` 60 次零分配；另有一条 release 吞吐用例默认 `#[ignore]` |
| `crates/xgc-store/src/lib.rs` | JSON 包络 v5、原子写、WorldData / Prefab / Journal 存取 |
| `crates/xgc-store/src/binary.rs` | `XGCB` v6 二进制世界：encode / decode / save / load；固定小端、递归值带标签；映射和枚举分别用 11 和 12，沿用原有递归深度限额 |
| `crates/xgc-sim/src/schedule.rs` | 执行计划与调度器：连续分批、宿主阶段、`run_plan_threaded` / 分区执行（`supports_parted` 的系统并行、其余串行；默认 1 线程）；回归 `crates/xgc-sim/tests/parallel_batch.rs` |
| `crates/xgc-sim/src/lib.rs` | `Simulation` / `Trace` / `play` / `verify` / `set_ui_input` / `step_with`（调度器 / 场景 / 错误已拆到 `schedule.rs` / `scenario.rs` / `error.rs`） |
| `crates/xgc-sim/tests/schedule_plan.rs` | 执行计划回归：连续批次、同批两两无冲突、摊平 == 注册顺序、宿主阶段位置（插在首个 `runs_after_host` 系统之前）、构造时固化计划 |
| `crates/xgc-sim/tests/host_order.rs` | 宿主阶段位置回归：默认计划宿主在动画之前 / 宿主闭包在固定步与动画之间执行 / 脚本本帧写的状态机参数当帧生效 |
| `crates/xgc-sim/tests/parent_body.rs` | 刚体建体世界变换回归：父节点在 `(-5,0.5,0)`、子节点局部 `(0,1.05,0)`，箱子必须停在世界顶面而不是父刚体顶面（局部建体会停在 1.4988） |
| `crates/xgc-sim/tests/rapier_first_frame.rs` | 物理首帧口径回归：默认调度首帧建体必须先于速度积分（第 1–6 帧 y 按旧端 4 子步等效式 `y = 7 − g·dt²·f(4f+1)/8`、速度 `−g·dt·f` 逐帧断言） |
| `crates/xgc-sim/src/particles.rs` | `ParticleSystem`：SplitMix64 确定性发射 / 生命周期 / 形状采样 / 重力阻力，产出当帧 `ParticleQuad` |
| `crates/xgc-sim/src/animation.rs` | 根变换剪辑采样 + `AnimationPlaybackSystem`；关节轨道在 `animation/joints.rs` |
| `crates/xgc-sim/src/animation/joints.rs` | `JointTrack`：按 joint 索引采样骨骼本地平移 / 旋转 / 缩放；播放系统按 Parent 链找骨骼并写入 |
| `crates/xgc-sim/src/animation_state.rs` | `StateMachine` / `AnimationStateSystem`：参数阈值转换、转场时长写 blend 字段、状态写回 Animator.clip |
| `crates/xgc-sim/src/animation_layers.rs` | `AnimationLayerSystem`：每帧推进层自己的机器 / 参数 / 时钟 / 淡入（转移时长缺省用 `fade_time`、旧状态时钟定格），只写 `animation.layers`；姿势混合在渲染侧 `glb/layers.rs`，按名写权重由 Lua 宿主处理 |
| `crates/xgc-sim/tests/animation_layers.rs` | 混合层状态回归：层机器转移并定格旧时钟 / 淡入结束清空 / 非 autoplay 不推进 |
| `crates/xgc-sim/src/input.rs` | `InputMap` / `InputState` / `KeyCode` / `Binding`：解析 `input.ron` 并折叠动作与轴 |
| `crates/xgc-sim/src/audio.rs` | `AudioMixerSystem`（支持 `supports_parted`）+ WAV 解码 / `AudioLibrary`：监听者距离 + 总线音量写 `audio.gain` |
| `crates/xgc-sim/src/skeleton.rs` | `SkeletonPoseSystem`（支持 `supports_parted`）/ `parse_pose`：关节父链合成世界矩阵 |
| `crates/xgc-sim/src/mixer.rs` | `SoftwareMixer` / `write_wav_mono`：总线增益混音与 WAV 写出 |
| `crates/xgc-sim/src/ui.rs` | `UiLayoutSystem` / `UiInteractionSystem` / `UiFocusSystem` / `hit_test`：布局、裁剪、可见性、命中、`ui.input` 驱动的交互 / Tab / 方向键焦点 |
| `crates/xgc-sim/src/character_kcc.rs` | 角色胶囊 / rapier KCC / 竖直阶段的唯一真相源：`radius` / `half_height` / `center_height` / `rest_gap = skin_width × 1.1` / `character_capsule` / `build_character_controller`（`offset = Absolute(skin)`、`autostep: None`、两个坡角、`snap_to_ground: None`、nudge 1e-4）/ `move_character`（水平 collide-and-slide + 竖直位移，events 收集接触法线）/ `probe_ground`（向下射线，间隙是法向距离 `(toi - half_height) * n.y - radius`，y=0 解析兜底）/ `try_step_up`（旧端三段扫掠：升 / 前进 / 落，落点不是坡面才采纳）/ `GRAVITY_Y` / `SLIDE_BAND_DEGREES` + `slip_limit`（`(slope_limit + 10).min(89)`，旧端 `min_slope_slide_angle` 同式；超过迟滞带的坡不清竖直速度、沿坡面投影下滑）；消毒委托 `CharacterController::sanitized()`（本文件不再自带区间常量）；`owned_by_kcc` 是「有 `CharacterController`、无 `PhysicsBody`、有 `Transform`」的唯一判据 |
| `crates/xgc-sim/src/systems.rs` | `PhysicsSystem` / `CharacterJumpSystem` / `VelocitySystem`（对带 `CharacterController` 的实体**整份跳过** —— 角色三轴位移都在 `RapierPhysicsSystem` 的角色阶段，一帧只能有一处积分）/ `GroundCollisionSystem` / `AnimatorSystem` / `GravitySystem` / `HierarchySystem`；`AnimatorSystem` 推进时钟前记 `from`，推进后按 `crossed_events` 写瞬态 `FiredAnimEvent`（`fired` / `indices` 缓冲复用），并在**本系统开头**清空上一步事件；`CharacterSystem` / `CharacterStepSystem` 已删除，竖直 / 地面 / 台阶搬进 `character_kcc` 的 rapier 角色阶段（`rapier_world/characters.rs`） |
| `crates/xgc-sim/src/animation_events.rs` | `crossed_events`：旧端穿越算法的唯一移植（非有限 / `duration <= 0` / 倒走不触发；空名 / 负时间 / 非有限跳过；循环 `time >= duration` 不可达；`started` 额外派发零时刻；一帧多事件按时间排序）；`EVENT_EPS = 1e-6` |
| `crates/xgc-sim/src/aabb.rs` | `AabbCollisionSystem`：动态刚体两两分离 / 动态 vs 静态推出，接触时清掉沿分离轴的接近速度（落地 / 堆叠稳定）；接触对按 `(min id, max id)` 排序去重、跨帧 diff，进 / 出各推一条**有向** `CollisionEvent`（双方各一条）；`is_sensor` 只记接触、不做分离（否则触发器会把动态体推出去）；`previous` / `current` 两块缓冲互换复用，稳态零分配 |
| `crates/xgc-sim/src/rapier_world.rs` | `RapierWorld` + `RapierPhysicsSystem`：与旧引擎同版本同参数的 rapier3d 0.22 刚体后端（`physics.body` 实体），建体规则按旧端 `body_from_scene`（Auto 形状取缩放绝对值 ×0.5、mass 换密度、角阻尼 0.05）；动态体回写 Transform / Velocity，运动学体每步按**世界**变换推送位姿（旧端 `push_kinematic_bodies` 同义）；建体签名 = `BodySignature`（`EntityId` + `EntityKey` + `kind`/`shape` 的 FNV-1a + 尺寸 / 半径 / 半高 / 摩擦 / 弹性 / 质量 / 重力缩放 / 传感器 的位模式；位置 / 旋转 / 缩放不进签名）；改任一构建参数都会重建那一个刚体、稳态帧不动；刚体表 / 角色 / 坡面子模块分别在 `rapier_world/{bodies,characters,slopes}.rs`（诊断口径见 bodies 行）；`ground_under` 是只读向下射线探地；`RapierPhysicsSystem::prepare` 在帧首（命令结算后、计划前，只读世界）先 `ensure_built` 建刚体表——旧端 `sync_bodies()` 同序；别把建体挪回 `run`，新刚体会吃到本帧 `VelocitySystem` 积分，整条物理轨迹多出 g·dt² |
| `crates/xgc-sim/src/rapier_world/bodies.rs` | `RapierWorld` 的刚体表子模块（从 `rapier_world.rs` 拆出守 600 行）：`BodySignature` / `BodyLink`、第一次按 ID 排序整份建表（`rebuild`）、之后按签名 ID 序列增量修补（`patch_bodies`：只重建新增 / 换代 / 构建参数变了的条目、删掉消失的条目，未变化刚体保留 handle；动过表才 `query.update`）；`rebuild_count()`（整表重建，稳态 1）与 `body_patch_count()`（增量修补，稳态 0）是诊断口径；建体位置 / 旋转 / Auto 尺寸取 `world_transform`（父链 `local_matrix` 合成 → `decompose`，非单位四元数归一化，与旧端 `Transform::from_matrix` 同口径） |
| `crates/xgc-sim/src/rapier_world/characters.rs` | `RapierWorld` 的角色子模块：按 `EntityId.raw()` 建 / 删 kinematic-position-based 胶囊，签名 = ID + `EntityKey`（index + generation）+ radius / half_height / skin_width / slope_limit 位模式（只见 ID 会漏掉 `Replace` / 快照恢复的换代，角色水平位移会静默冻结且旧胶囊变幽灵碰撞体），每帧 `sync_character_poses` + `move_characters`（`move_shape` 后回写 x / z 与水平 `Velocity`，竖直在下滑 / 重力时不清零；陡坡接触按竖直墙处理，`slipping`（超过 `slip_limit` 的坡）例外）；签名探针与收集表跨帧复用（热路径零分配），没有 `Transform` 的角色不进签名（否则补上 `Transform` 后签名不变、永远不建胶囊）；`collect_sensor_contacts` 用 `intersections_with_shape` 让角色胶囊与静态 sensor 求交，接触对跨帧 diff 推 `CollisionEvent`（传感器不参与 KCC 分离；静态 sensor 改 `Transform` 不推 rapier，只有增删 / 换体会变更） |
| `crates/xgc-sim/src/rapier_world/slopes.rs` | `RapierWorld` 的坡面子模块：按 `Slope` 的 origin / size / rise 建 **静态楔形薄壳**碰撞体（cuboid 半长 `hypot(size.x, rise) / 2`、厚 `SLOPE_THICKNESS = 0.1`、半深 `size.y / 2`，绕 Z 转 `atan2(rise, size.x)`，中心沿法线从坡面中点下移半厚 ⇒ 上表面与 `Slope::height_at` 重合）；另挂 `physics.body` 的实体让位、参数非有限的坡不建体；签名 = ID + `EntityKey` + 全部几何位模式（变了才重建，先删旧刚体连带碰撞体再 `query.update`），探针 / 收集表跨帧复用；`ground_under` 是只读向下射线探地（`QueryFilter::default()`，任何碰撞体都会命中，做成 `probe_ground` 时要换过滤器）；`RapierPhysicsSystem::access().reads` 已声明 `Slope::ID`（读取契约与实际一致） |
| `crates/xgc-agent/src/lib.rs` | JSON-RPC 会话：describe（组件 / sim / render 能力清单）/ engine.diagnostics / engine.history / engine.replay / query / get / digest / stats / validate / load / save / plan / apply / revert / undo |
| `crates/xgc-agent/src/mcp.rs` | MCP stdio 适配器：`initialize` / `tools/list` / `tools/call`；工具名 = agent 方法名把 `.` 换成 `_`，参数与结果原样透传，不实现第二套世界逻辑 |
| `crates/xgc-agent/src/snapshot.rs` | `world.snapshot` / `world.restore`：WorldData 快照与恢复 |
| `crates/xgc-agent/src/query.rs` | 查询 IR：字段路径、过滤算子、标量比较 |
| `crates/xgc-audio/src/lib.rs` | cpal 音频设备输出：设备枚举、PCM 重采样、F32 / I16 输出队列、`play_clip` |
| `crates/xgc-script/src/lib.rs` | Lua 宿主：安全标准库、执行 / 调用、`xgc` 表（字段 / 实体 / 组件增删）、预算、内存上限、热重载 |
| `crates/xgc-script/src/legacy.rs` | 旧脚本钩子与 API：`update(ctx)` / `fixed_update(ctx)`（每个固定步一次，dt 仍走 `delta(ctx)`）/ `on_collision_enter(ctx, other)` / `on_collision_exit(ctx, other)` / `entity_name(ctx, id)` / `rot` / `set_rot` / `delta` / `entity_id` / `entity_pos` / `set_pos` / `look_at`（俯仰必须放欧拉角 X 分量，放 Z 会变滚转）/ `velocity` / `set_anim_param` / `ui_*` / `action_pressed` / `axis` / `character_*` / `state_*` / `pos` / `log`；`character_simple_move` 遇到只有 `physics.character` 的旧实体先补 `core.velocity`（`AddComponent` 走 replace，旧 `EntityKey` 会失效，按 `EntityId` 重新取）；UI 颜色 setter 的 sRGB 参数经唯一的 `srgb_to_linear()`（同旧端 0.04045 分段，alpha 不转）换算后写入组件，少这一步会在 sRGB 目标上二次编码；层栈 setter（`set_anim_layer_weight` / `set_anim_layer_param`）按名称选择帧内最新层，连续整栈写入合并，动态路径命令保持调用顺序；UI 布局 / 文本 / 样式绑定已拆到 `ui_api.rs`（见下）；音频等暂无实现为 no-op |
| `crates/xgc-script/src/ui_api.rs` | UI 脚本布局 / 文本 / 样式 / 图片 API：29 个函数（布局 5 / 文本 4 / 样式 2 / 状态 2 / 读取 4 / 交互 2 / 图片 3 / 运行时创建 4）；焦点在 `ui_focus_api.rs`、创建在 `ui_spawn_api.rs`、默认值在 `component_data.rs`；语义照旧端 `ui_ctx.rs`：`ui_set_size` = `offset_min + size`、`ui_set_pos` 保持尺寸、文本尺寸 clamp 1..512、半径 max 0、对齐小写后只认 left/center|centre|middle/right、颜色 sRGB→线性（`srgb_to_linear`，alpha 不转）、缺 `ui.element` 一律 no-op；同帧多次写先看命令队列未结算 `SetField` 再退回快照 |
| `crates/xgc-script/tests/ui_layout.rs` | UI setter 回归：size 保持左上角 / 同帧 size+pos 两种顺序都成立 / 文本样式与 getter 同帧可见 / 缺元素与不存在 id no-op |
| `crates/xgc-script/src/ui_focus_api.rs` | UI 焦点三件套：`ui_focused` / `ui_set_focus` / `ui_clear_focus`；只写 `ui.interaction.focused`（0x0603，`SetField` 帧末结算），目标必须是 Button + visible + interactable 否则彻底 no-op，排他 = 同帧给其它 UI 实体写 false |
| `crates/xgc-script/tests/ui_focus.rs` | 焦点回归：同帧读回与排他 / 非 Button·隐藏·禁用·无交互组件目标 no-op / 下一帧帧首快照保留 / 清焦点 |
| `crates/xgc-script/tests/ui_image.rs` | 图片三件套回归：setter 写 `image_path` / 线性 tint / fill 钳制 / 缺元素 no-op |
| `crates/xgc-render/tests/ui_image_fill.rs` | 图片渲染回归：tint 乘色 / fill 从左侧裁 / fill=0 不画（GPU lock） |
| `crates/xgc-render/tests/ui_border.rs` | UI 覆盖层 SDF 回归：圆角描边用边框色 / 透明背景 + 描边仍绘制 / 圆角边缘有抗锯齿过渡（像素介于空与实心之间） |
| `crates/xgc-script/tests/anim_layers.rs` | 脚本混合层回归：同帧权重和参数累积 / 按名 setter 与动态路径按序执行 / 权重钳制、缺层与非有限值静默 |
| `crates/xgc-script/tests/anim_event_hook.rs` | 动画事件钩子回归：`on_anim_event` 收到事件名并写速度 / 不匹配名字不写 |
| `crates/xgc-sim/tests/animator_events.rs` | 动画事件运行时回归：跨帧只触发一次 / 循环跨圈命中 / 非循环不重复 |
| `crates/xgc-script/src/lifecycle.rs` | `start(ctx)` / `update(ctx)` / `fixed_update(ctx)` / `on_collision_enter(ctx, other)` / `on_collision_exit(ctx, other)` / `on_anim_event(ctx, name)` 生命周期分派与回退；每次调用前 `reset_budget()`，`fixed_update` 每个固定步一次且不传 dt（dt 走 `delta(ctx)`），碰撞钩子多传对方 `EntityId`，动画钩子多传事件名 |
| `crates/xgc-script/src/character_api.rs` | 角色脚本 API：`axis`（读 `ScriptView.axes`，编辑器按 `input.ron` 的轴桥）、`character_is_grounded` / `character_velocity`（读快照）、`character_simple_move`（写 `Velocity.linear` 水平两轴，缺组件先 `AddComponent`，同帧查队列去重）、`character_jump`（仅 grounded 时写 `jump_velocity` + `jump`）；写操作全部走帧末命令队列 |
| `crates/xgc-script/src/runtime.rs` | `ScriptRuntime`：按 `script.lua` 驱动实体、每帧 `ScriptView` 快照、动作输入桥 `set_action`、命令结算、GUID 脚本经 `AssetCatalog` 解析、实体移除清理、磁盘热重载；`run_frame` 在 update/fixed_update 之后消费 `World::collision_events()` 派发 `on_collision_enter/exit`，再消费 `World::animation_events()` 派发 `on_anim_event`，命令并进同一帧结算；`take_error()` 把本帧第一处脚本错误交给宿主（只打 stderr 时宿主看不到失败） |
| `crates/xgc-editor/src/lib.rs` | 编辑器无头状态：打开 / 保存 / 实体列表 / Inspector / Play / Undo/Redo / demo / 轨道相机 / 项目场景切换（脏场景与 Play 期间拒绝） |
| `crates/xgc-editor/src/scene_ops.rs` | 项目场景切换：清单登记名 → 场景世界；脏场景与 Play 期间拒绝，切换后清空选择 / 撤销 / 重做 |
| `crates/xgc-editor/src/assets_ops.rs` | `EditorState::assets()`：只读扫描清单内容目录，规范化与去重、确定性截断、隐藏 / 链接 / `.meta` 排除；不生成元数据 |
| `crates/xgc-editor/src/app/assets_panel.rs` | 资产折叠面板：首次展开缓存，Refresh 手动刷新，虚拟列表与长路径省略；点选预制体只回填工具栏路径 |
| `crates/xgc-editor/src/app/assets_panel/tests.rs` | 资产回归：2000 条上限与排序、只读、Windows 隐藏属性、折叠 / 缓存 / 刷新、有效预制体点选不实例化 |
| `crates/xgc-editor/src/play.rs` | Play / Play Scripts / 清单输入映射 / step / stop，项目脚本与根运动共用 GUID 目录 |
| `crates/xgc-editor/src/transform_ops.rs` | 位置 / 缩放 / 旋转 setter 与 Gizmo 旋转；无父节点同步 GlobalTransform；`snap` |
| `crates/xgc-editor/src/prefab_ops.rs` | 从 `.prefab.json` 实例化并选中根，按所选成员的 GUID 或项目相对路径加载来源并 Revert |
| `crates/xgc-editor/src/gizmo.rs` | 视口 MVP 投影与线段命中距离 |
| `crates/xgc-editor/src/app.rs` | eframe 窗口：层级 / 场景列表 / 资产浏览 / wgpu 视口 / 选中高亮 / 轨道相机 / Move·Rotate·Scale Gizmo / 吸附 / Ctrl+Z、Ctrl+Y / 底部状态栏；`app/inspector.rs` 按 schema 递归生成控件并提交编辑事务，`app/inspector/containers.rs` 处理映射增删和枚举切换 |
| `crates/xgc-editor/src/app/toolbar.rs` | 顶部工具栏：保存 / 预制体 / Revert / Play / Play Scripts / 吸附 |
| `crates/xgc-editor/src/main.rs` | `xgc-editor [世界.json]` 入口 |
| `crates/xgc-asset/src/lib.rs` | 扫描 `.meta`、`guid://` → 路径的只读资产目录 |
| `crates/xgc-asset/src/cook.rs` | `cook_project` / `cook_project_filtered`：按 GUID 裁剪、`.meta` / `.bin` 伴生、哈希未变跳过写入、清陈旧文件、manifest 聚合签名与校验 |
| `crates/xgc-migrate/src/lib.rs` | 旧 `SceneV2` RON → `WorldData`（全部旧组件类型：身份 / 名称 / 父子 / Transform / Mesh / Lod / Material / Camera / Lua / Audio / Physics / Animator / Character / UI / Particle / Rust 插件，支持目录批量） |
| `crates/xgc-migrate/src/old_character.rs` | 旧 `Character` → `physics.character` 的输入投影：六个字段都带旧端 serde 缺省（1.8 / 0.3 / 0.35 / 45 / 0.02 / 0.3），手写旧 RON 省略字段也能迁出 |
| `crates/xgc-migrate/src/old_scene.rs` | 旧 `SceneV2` 顶层结构 |
| `crates/xgc-migrate/src/environment.rs` | 场景级光 / 环境光 → `SceneEnvironment` 映射 |
| `crates/xgc-migrate/src/old_animator.rs` | 旧 `Animator` → `animation.animator`（含基础姿态）；旧 `machine`（entry / parameters / states / transitions / duration）一次性投影成 `animation.state` 的机器结构与参数映射；旧 `layers` 一次性投影成结构化 `animation.layers`；旧 `additive` 一次性搬进 Animator 的 additive_* 字段；旧 `events` 一次性转为反射列表（空表使用组件默认值） |
| `crates/xgc-migrate/tests/animation.rs` | 机器迁移回归：旧 Animator `machine` → `animation.state`（machine 可回读、状态 / 转移 / 参数齐全），旧 `events` → `animation.animator.events` 可回读 |
| `crates/xgc-migrate/src/old_lod.rs` | 旧 `LodGroup` → `render.lod` 的结构化层级；非有限距离转为表示无限距离的零值 |
| `crates/xgc-migrate/src/old_rust.rs` | 旧 Rust 插件组件 → `plugin.rust` |
| `crates/xgc-migrate/src/old_particle.rs` | 旧 `ParticleEmitter` → `particle.emitter` |
| `crates/xgc-migrate/src/old_ui.rs` | 旧 `UiElement` → `ui.element`（kind 由内容推断） |
| `crates/xgc-world/src/prefab_ops.rs` | 预制体实例化与 Revert 走同一世界事务；按所选成员定位最近实例根，保留已有 ID、变换与根的外部父节点 |
| `crates/xgc-migrate/src/project_migrate/documents.rs` | 场景与预制体共享一次性投影，重写父节点与预制体来源 ID，伴生 `.meta` 随资产改名 |
| `crates/xgc-migrate/src/project_migrate/files.rs` | 干跑与事务发布共用拷贝清单，预检生成文件和项目内容冲突 |
| `crates/xgc-migrate/src/transform_migrate.rs` | 旧 Transform → XGC Transform + GlobalTransform（同值，层级系统随后重算） |
| `crates/xgc-migrate/src/compare.rs` | `compare_headless`：旧引擎 `--headless` JSON 与 XGC 世界的 entities / bodies / colliders / emitters / audio 对照 |
| `crates/xgc-migrate/src/verify.rs` | `verify_migration`：重新迁移旧场景与目标 `WorldData` 逐实体 / 字段对比，浮点按 f32 容差 |
| `crates/xgc-migrate/src/report.rs` | 迁移错误 / 报告 / 批量结果 |
| `crates/xgc-render/src/lib.rs` | 渲染图：资源表 / pass 读写、确定性拓扑排序、环 / 多写者 / 未声明资源 / 读写用途检测、`PassExecutor` |
| `crates/xgc-render/src/gpu.rs` | wgpu 设备、资源创建、clear pass、draw list 执行、RGBA8 回读；`from_arc` 复用于编辑器视口；场景采样数在设备创建时由 `msaa::preferred_sample_count` 定下（`scene_sample_count`），多采样附件上的清屏走 `clear_scene_view`、收尾统一 `resolve_scene_view` |
| `crates/xgc-render/src/msaa.rs` | 场景 pass 采样数：`PREFERRED_SAMPLE_COUNT = 4`，`preferred_sample_count(adapter)` 同时要求 `Rgba16Float` 与 `Depth32Float` 支持 4×，否则退回 1×（1× 与切片前逐字节一致） |
| `crates/xgc-cli/src/parity_cmd.rs` / `crates/xgc-cli/tests/parity.rs` | `xgc parity`：结构性差异拆成「轮廓 1 px 带」与「内部」两部分（轮廓判据 = 与 8 邻域任一分量差 > 40，与结构性 ≥8 是两个独立口径）；缺省门槛内部 ≤ 0.01%、总 ≤ 0.1%；退出码 0/1/2 |
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
| `crates/xgc-render/src/pipeline.rs` | `TrianglePipeline` / `RenderCache`：单 draw 路径、颜色 / 深度管线、scissor / 圆角 SDF 裁剪（WGSL 在 `triangle.wgsl`；`Material` uniform 128 字节 = color / mvp / rect / border_color / radius / border_width / pad，圆角与边框都为 0 时片元直接返回实例色）、实例化缓存 |
| `crates/xgc-render/src/draw.rs` | 从 `render.mesh` + `render.material` + `GlobalTransform` 抽 `DrawList`（含模型矩阵、材质资产引用与祖先骨架姿态）；`DrawItem.has_material` 记录「实体真的带 `render.material`」，没带时用模型材质；`DrawItem.layers` 带模型动画层栈（无基础 pose 时也参与） |
| `crates/xgc-render/src/frame_color.rs` | 逐 draw 颜色合成：`模型基色因子 + 模型材质因子 → .mat 资产 → 实体覆盖（仅 has_material）→ 选择高亮`（分流前一次做完；模型材质在 mesh 上，别拿 `Material::default()` 顶） |
| `crates/xgc-render/src/material.rs` | `.mat` 材质资产解析（基色乘子 + metallic / roughness，加载边界清洗） |
| `crates/xgc-render/src/tone.rs` | ACES 曲线 CPU 镜像与 8 位对表辅助；另含着色器平光口径的 CPU 镜像（`ambient_light` / `fresnel_schlick` / `sky_kd_env`），平光测试期望值由它现算；`env_brdf_approx` / `env_specular` 已随预过滤环境 IBL 删除——镜像必须镜像着色器，留着就是死代码 |
| `crates/xgc-render/src/shadow_settings.rs` | 阴影画质设置：`softness`（PCSS 灯光尺寸）/ `cascade_blend`（级间混合带宽），`sanitized()` 把非有限 / 非正一律落 0（不夹上界，旧端 `ShadowConfig` 同口径），全 0 = 关（走 3×3 PCF + 硬级间切换，逐位不变） |
| `crates/xgc-render/src/shadow_csm.rs` | 方向光阴影 CPU 侧：单级拟合 `fit_directional_shadow`（关 CSM 的等价退路）+ **4 级 CSM** `cascade_splits`（λ=0.7）/ `cascade_shadow_matrices`（逐级紧致正交拟合、纹素吸附防抖；带 `overlap` 把非 0 级拟合框近端多铺 `overlap × 本级跨度`，并回填 `view_range` 的相机深度范围），与旧端逐行同式 |
| `crates/xgc-render/src/shadow.rs` | 方向光阴影 GPU 侧：2048² 深度**数组**（每层一级）+ 逐级深度 pass；静态与蒙皮投射体共用同一支 pass 与同一套 depth bias；接收端按视线深度选层 + 3×3 PCF（`render.shadow` 开时可切 PCSS + 级间混合）、偏置随 N·L 放大；另持 `PointShadowTarget`，点光六面复用同一支深度管线与实例缓冲（`ShadowStats::point_draw_calls` 是诊断口径）；`render` 同时接受方向光级联与点光计划，两者可以只有一个 |
| `crates/xgc-render/src/sky_draw.rs` | 天空盒 pass：全屏三角形 + 解析天空逐像素求值（远平面 / `LessEqual` / 不写深度），`inv_view_proj` 反推视线方向；着色器 = `sky_common.wgsl`（公式唯一一份、不声明绑定）+ 本地 `SkyFrame`（`inv_view_proj` / `eye` / 嵌套 `SkyUniform`，160B 布局不变） |
| `crates/xgc-render/src/exposure.rs` | 画面曝光的唯一消毒口径：`EXPOSURE_DEFAULT = 1.0` / `MIN_EXPOSURE = 0.05` / `MAX_EXPOSURE = 20.0` / `sanitize_exposure`（旧端 `RenderConfig::sanitized` 同式：非有限 / 非正落回 1.0、合法值钳制；曝光 0 是"全黑画面"不是关闭态）；乘子在 tone-map 与光晕亮部提取两处共用 |
| `crates/xgc-render/src/renderer/hdr.rs` | HDR 中间目标与呈现：按尺寸缓存 `Rgba16Float` 场景纹理（采样数 > 1 时另有一张多采样纹理，`SceneViews` 每帧给场景 pass 与收尾 resolve 的视图），按输出格式缓存全屏 tone-map pipeline；tone pass 的 6 个绑定里带三级光晕输入（关闭时绑场景纹理占位）；tone-map 输入由调用方给的视图决定（HDR 场景或 TAA 解析结果），`present(..., scene, format, Look { bloom, exposure }, glow)` 每帧建绑定组；tone-map 之后可选跑一趟 FXAA（`crates/xgc-render/src/fxaa.rs`，落点由调用方给的视图决定），TAA 开着时 FXAA 让位 |
| `crates/xgc-render/src/bloom.rs` | 三级 HDR 光晕链（旧端 `postfx.rs` 同口径）：`BloomSettings`（intensity / threshold / knee，`intensity <= 0` 即关闭）+ 亮部提取 / 逐级降采样 / 横纵二项核共 9 个 pass，`Rgba16Float` 乒乓中间目标按尺寸缓存；`BloomStage::render` 收当前曝光（亮部提取按曝光后的亮度判阈值），链缓存键带曝光位模式 |
| `crates/xgc-render/src/bloom.wgsl` | 光晕三个片元入口：`fs_bright`（软膝式取亮部）/ `fs_down`（一次双线性降采样）/ `fs_blur`（9 抽头二项核，方向由参数给横或纵） |
| `crates/xgc-render/src/taa.rs` | 时间抗锯齿（旧端 `taa.rs` 同口径）：`TaaSettings`（`enabled` / `history_weight`，默认关、上界 0.98）+ 8 相 Halton(2,3) 抖动（帧号纯函数）、像素→裁剪 / 纹理两套换算、`jitter_projection`；`TaaStage` 管两张 `Rgba16Float` 历史乒乓与上一帧未抖动视图投影（`set_sample_count` 换档重建管线并清历史），深度反投影重投影 + 3×3 邻域钳制后 `mix`，换尺寸重建、关掉即释放；`reset_history()` 清重投影状态（相机瞬移 / 切场景帧调用，历史纹理保留） |
| `crates/xgc-render/src/taa.wgsl` | TAA 解析着色器：当前色 + 深度 + 历史，`blend <= 0` / 相机背后 / 越界直通当前帧，天空按视线推 1e4 只保留旋转视差 |
| `crates/xgc-render/src/renderer/settings.rs` | 画面设置入口：`set_sky` / `set_exposure` / `set_bloom` / `set_fxaa` / `set_shadow_settings` / `set_taa` / `set_ssao`（都经各自的 `sanitized()`），从 `renderer.rs` 按行数预算拆出 |
| `crates/xgc-render/src/renderer/taa_pass.rs` | TAA 的每帧编排：`taa_begin` 返回抖动后的场景矩阵、`taa_resolve` 解析成可采样纹理并计 pass、`taa_history_size` 是诊断口径；关着两步都原样返回 |
| `crates/xgc-render/src/ssao.rs` | 环境光遮蔽（旧端 `ssao.rs` 同口径）：`SsaoSettings`（`enabled` / `strength` / `radius` / `bias`，默认关；`strength <= 0` 与关等价）+ 固定种子 LCG 的 16 抽头半球核 + 半分辨率 AO → [1,4,6,4,1]/16 可分离模糊 → 全分辨率乘算四趟 pass；`SsaoParams` 448 字节，退化矩阵把强度写 0（画面逐位不变），换尺寸重建、关掉释放；深度绑定与着色器声明随场景采样数切换（`set_sample_count`） |
| `crates/xgc-render/src/ssao.wgsl` | SSAO 四个片元入口：`fs_ao`（深度反投影 + 深度梯度法线 + 像素哈希旋转的 16 抽样）/ `fs_blur_h` / `fs_blur_v` / `fs_apply`（半分辨率 AO 线性上采样乘源色） |
| `crates/xgc-render/src/renderer/ssao_pass.rs` | SSAO 的每帧编排：`ssao_apply` 关着原样返回、开着跑四趟并计 pass；`ssao_target_size` 是诊断口径（关着为 None） |
| `crates/xgc-render/src/renderer/sky_pass.rs` | 天空 pass 的每帧绘制（`draw_sky`：几何为空时清屏 + 全屏三角形；从 `renderer.rs` 按行数预算拆出） |
| `crates/xgc-render/src/fxaa.rs` | 显示空间 FXAA（旧端 `postfx.rs` 同口径）：`FxaaSettings`（`enabled`，默认关）+ 与输出同格式的 LDR 暂存（按尺寸 / 格式缓存，关闭时释放）+ tone-map 之后、UI 之前一趟全屏 pass；`present` 返回本帧跑的 pass 数 |
| `crates/xgc-render/src/fxaa.wgsl` | FXAA 3.11 质量路径（5 抽头、亚像素 0.75、方向收缩 1/8、跨度上限 8 texel、阈值 1/12 与 1/6、Rec.601 luma、sRGB 手动 encode / decode）；平坦区直接返回中心采样，保证关掉边缘时逐字节不变 |
| `crates/xgc-render/src/renderer/shadow_prep.rs` | 阴影的每帧准备（主阴影光取自 `lights::primary_shadow_light`，与光源表同判据）：点光挑灯 + 六面计划、方向光按场景包围盒 + 相机矩阵拟合级联，把 draw 画进两种深度图；返回 `PreparedShadows`（含阴影蒙皮竞技场本帧新建的缓冲数与点光绘制次数），并按需建深度 / 天空 / 阴影 / 贴图目标（`ensure_frame_targets`）——从 `renderer.rs` 按行数预算拆出 |
| `crates/xgc-render/src/renderer/ui.rs` | UI 覆盖层：背景 / 焦点环 / 真字体文本 / 图片在 tone-map 后画到显示目标；次序 = 背景 → 焦点环 → 图片/文字，边框走 `border_color` / `border_width`（透明背景 + 描边也算可绘制），内容区 inset = `padding + border_width`（从 `renderer.rs` 按行数预算拆出；字形先排版后上传图集） |
| `crates/xgc-render/src/sky.rs` | 解析天空（天顶/地平线/地面 + 太阳盘 + 光晕）与漫反射 IBL：1024 点斐波那契球投影成 L2 球谐（E/π，无随机数）；参数来自 `project.ron` 的 `render.sky`；环境**镜面**已改由 `env_map.rs` 预过滤，这里只剩漫反射 SH 与 CPU 侧公式 |
| `crates/xgc-render/src/sky_common.wgsl` | 天空公式唯一真相源：`SkyUniform` / `SkySH` / `SKY_PI` / `SKY_SUN_DISK_RADIANCE` / `SKY_SUN_GLOW` / `ENV_MAX_LOD`、`sky_radiance` / `sky_sh`；**不声明任何绑定**（天空盒与预过滤布局各异），三处 include 共用 |
| `crates/xgc-render/src/env_map.rs` | 预过滤环境图：天空按 GGX 重要性采样烘成 5 级 `Rgba16Float` 立方体（基级 128²、每级 96/48/24/16/12 样本、Hammersley 固定序列）+ 256 样本 BRDF 积分表（Karis A/B）；缓存键 = 80B 天空 uniform 的 FNV-1a 哈希，首帧 31 趟 pass、同设置 0 趟（`RenderStats::env_map_passes`）；关天空不跑 pass 也不采样 |
| `crates/xgc-render/src/env_prefilter.wgsl` | 环境图两个片元入口：`fs_prefilter`（立方体面方向 hammersley + GGX 重要性采样累积 `sky_radiance`）/ `fs_brdf`（256 样本 `A/B` 积分，无绑定故管线用空 layout） |
| `crates/xgc-render/src/lights.rs` | 多光源表：把 `is_active()` 的方向光 / 点光按原顺序打包成旧端同款的 8 槽 `GpuLight`（方向光存传播方向 `w = 0`、点光存世界坐标 `w = 1`、`shadow_params.x` = 用方向光阴影图、`.y` = 点光半径），`intensity <= 0` 与第 9 盏起丢弃，只有第一盏投射阴影的方向光打 CSM 标记；`primary_shadow_light` 与打包表同判据（主阴影光排在第 9 槽之后就不拟合 CSM） |
| `crates/xgc-render/src/point_shadow.rs` | 点光立方体阴影 CPU 侧：挑第一盏 `casts_shadow` 且 `range > 0` 的点光（前 8 槽），六面投影 = `flip_u · perspective(90°) · look_at`（up 照 WebGPU 映射表、u 轴镜像），主轴分量参考深度；`PointShadowTarget` 持 1024² 六层 `Depth32Float`（采样视图是 Cube）与 1×1 假图兜底 |
| `crates/xgc-render/src/frame_uniform.rs` | 帧 uniform 布局与绑定：1008B（相机 / 环境光 / 天空球谐 / 4 级光视图投影 / 阴影参数 / 阴影画质 / 8 槽光源表 / 点光阴影段 32B；六面矩阵只进每面 64B 阴影帧），group 0 七绑定（frame、方向光阴影数组、比较采样器、点光立方体、环境立方体、BRDF 表、环境采样器；后三者随预过滤环境 IBL 加入，参数收口到 `FrameTextures`） |
| `crates/xgc-render/src/instanced.rs` | 按模型分组实例化；帧 uniform 已搬到 `frame_uniform.rs`（相机位置 + 环境光 + 天空球谐 + 4 级光视图投影/切分/视线/阴影参数 + 阴影画质 + 8 槽光源表（方向光 / 点光）），逐实例 96B（模型 + 颜色 + metallic/roughness）；片元按 `环境光 + 天空漫反射 SH + 预过滤环境镜面（split-sum，立方体 + BRDF 表）+ 逐盏光色 × 强度 × 衰减 × N·L × 阴影`（方向光无衰减、点光 `fade/(1 + 0.09d + 0.032d²)`）输出线性 HDR |
| `crates/xgc-render/src/particles.rs` | 实例化 billboard 粒子：相机朝向软圆、Alpha / Additive、后向前排序、在线性 HDR 中预乘混合、深测开 / 深写关 |
| `crates/xgc-render/src/texture_data.rs` | 基色贴图 RGBA8：PNG 字节解码（8-bit）+ 1×1 白纹理兜底 |
| `crates/xgc-render/src/texture_bind.rs` | 贴图 GPU 绑定：group(2) 布局 + 线性 Repeat 采样器 + 白纹理兜底绑定组 |
| `crates/xgc-render/src/gpu_mesh.rs` | 模型 → 位置 / 法线 / UV / 索引缓冲 + 贴图绑定组，跨帧复用（没法线时法线流灌全零哨兵；CPU 蒙皮回退每帧只重算位置、法线流清零退回导数） |
| `crates/xgc-render/src/scene_shading.wgsl` | 静态 / GPU 蒙皮唯一主场景片元真相源：Frame / 贴图 binding、环境光菲涅尔、Cook-Torrance、天空漫反射 SH + 预过滤环境镜面（`textureSampleLevel(env_cube, …, roughness × ENV_MAX_LOD)` × `(f0 × A + B)`）、CSM PCF / 可选 PCSS 软阴影 + 级间混合（`shadow_soft`），多光源循环（方向光 / 点光 + 点光距离衰减），点光立方体阴影（主轴分量参考深度 + 3×3 比较，`params.z = 0` 整段跳过），片元法线优先用顶点法线（逆转置、不按视线翻号）、缺省退回屏幕导数叉积，输出线性 HDR |
| `crates/xgc-render/src/tone_map.wgsl` | GPU 唯一 ACES 真相源：从 HDR 逐像素 `textureLoad`（关闭光晕时逐位不变），先乘曝光（`ToneParams.exposure`，整块 32 字节），`intensity > 0` 时再按 `uv` 双线性采样三级光晕叠回线性场景**再**过 ACES，全屏三角形写调用方给的视图并固定 alpha=1（关闭 FXAA 时就是显示目标） |
| `crates/xgc-render/src/instanced.wgsl` | 静态实例顶点入口：读取 group(1) 实例 model / color / material + 法线（逆转置变换），输出共享 `SceneVsOut` |
| `crates/xgc-render/src/skinned.wgsl` | GPU 蒙皮主场景顶点入口：group(3) model / material + share 的变形函数（法线先按 palette 变形再逆转置），输出共享 `SceneVsOut` |
| `crates/xgc-render/src/shot.rs` | 截图取景：相机优先级 = 显式机位 > 场景相机实体（按 `EntityId` 升序第一台 active）> 顺光自动取景；**近 / 远裁剪面一律由 `clip_planes` 按场景包围球重算**（旧端 `screenshot.rs` 同口径），`shot_frame` 是唯一入口 |
| `crates/xgc-render/src/mesh.rs` | `Mesh` 位置 + 索引 + 顶点法线（可为空：空 = 模型没导 `NORMAL`）+ `SkinData` + `base_color_factor` + `uvs` + 基色贴图（`Arc<TextureData>`）+ 模型材质因子 `metallic` / `roughness`（glTF spec 缺省 1.0，内建图元用旧端 `MaterialParams::default` 的 0.0 / 0.45）；builtin quad / cube / sphere / plane / cylinder 为真实几何；与旧端 `crates/xgame-render/src/mesh.rs` 逐顶点一致（cube 24 顶点带面法线 / UV、`sphere(0.5, 24, 32)` 平滑法线、cylinder 32 段、plane +Y 法线） |
| `crates/xgc-render/src/glb/attributes.rs` | 首图元属性装配：`NORMAL`（调 `glb/normals.rs` 并校验条数）+ JOINTS_0 / WEIGHTS_0 + 逆绑定矩阵，从 `glb.rs` 按行数预算拆出；缺 `NORMAL` 时调 `face_normals` 按几何生成面积加权顶点法线（旧端 `compute_face_normals` 同式） |
| `crates/xgc-render/src/glb/normals.rs` | `NORMAL` 访问器读取：只接受 float VEC3、逐顶点三份、不提前归一（归一在片元）；读之前按 `checked_add` / `saturating_mul` 校验偏移与 BIN 长度（JSON 的 `count` 不可信，坏资产必须是 `MeshError` 而不是 panic）；另含 `face_normals`（缺 `NORMAL` 时的生成算法：逐三角形叉积 · 长度即两倍面积 · 累加到三顶点再归一，非索引图元按 `POSITION` 顺序展开）与 4 条纯函数单测 |
| `crates/xgc-render/src/glb.rs` | GLB / GLTF 解析：POSITION / 索引 / `NORMAL`（装配在 `glb/attributes.rs`，条数必须与 POSITION 一致）/ JOINTS_0 / WEIGHTS_0 / 首图元材质 / `load_glb_skeleton`；`.gltf` + 外部 bin 经内存 GLB 合并；缺 `NORMAL` 的模型加载期会补齐顶点法线（不再是空），片元导数回退只剩内建图元 / 手工构造 |
| `crates/xgc-render/src/glb/animation.rs` | glTF 动画解析：node 层级 / 默认 TRS、channel 时间与值、LINEAR / STEP 采样（CUBICSPLINE 明确 Unsupported）、`AnimationSet::joint_pose` 沿 parent 链算关节全局矩阵；`AnimationNode.name` 供遮罩匹配、`bind_locals` 给空基础剪辑当基底；叠加层混合在 `glb/additive.rs`（见下），`root_motion_set` 在 `glb/root_motion.rs` |
| `crates/xgc-render/src/glb/additive.rs` | additive 叠加层混合：局部空间平移 / 缩放差 + `ref⁻¹·current` 旋转增量向单位四元数 slerp（weight clamp [0,1]，权重 0 逐位等于基础姿势），参考 = 叠加剪辑第 0 帧；层栈的 Additive 层复用同一份 `blend_additive` |
| `crates/xgc-render/src/glb/crossfade.rs` | 交叉淡入：`CrossfadeLayer`（旧剪辑 + 按旧剪辑自己时钟走的旧时间 + alpha）；`joint_pose_played` 在局部 TRS 上按 alpha 把旧剪辑姿势混进新剪辑（T/S lerp、R slerp），再叠 additive 后转关节矩阵；旧剪辑缺失退回新剪辑；根运动剥离在 `glb/root_motion.rs`（crossfade 后、additive 前把开关轴平移换回绑定值）；层栈参数在 additive 之后叠加（`layers`） |
| `crates/xgc-render/src/glb/root_motion.rs` | 根运动渲染剥离：按节点名把 `root_motion` 开关轴的平移换回绑定值；未知节点 / 空名字 no-op |
| `crates/xgc-sim/src/root_motion.rs` | `apply_root_motion`：宿主在 `step_with` 之后按 Animator 时钟推进实体位置（有 Parent 转回局部），不进默认调度（轨道表来自模型资产） |
| `crates/xgc-sim/tests/root_motion.rs` | 根运动回归：剪辑增量 / 循环跨边界展开 / 轴过滤与缺失配置 / 缩放旋转作用于世界增量 / 淡入按 alpha 分摊 |
| `crates/xgc-sim/tests/animator_crossfade.rs` | 交叉淡入状态回归：切换时刻定格旧剪辑时钟 / 淡入时长结束清空旧半边 / 硬切清掉遗留淡入字段 |
| `crates/xgc-render/tests/crossfade.rs` | 交叉淡入渲染回归：alpha=0 逐位等于旧剪辑、alpha=1 等于新剪辑、缺失旧剪辑退回新剪辑（红证据：短路淡入分支后 alpha=0 与旧剪辑差 22.7） |
| `crates/xgc-render/src/glb/layers.rs` | 模型动画层栈合成：`LayerSample`（mode / clip / time / looping / weight / fade / mask）；Override 按遮罩在局部 TRS 上覆盖、Additive 复用 `blend_additive`（参考 = 剪辑第 0 帧），mask 沿 parent 链匹配节点名；叠在 crossfade + additive 之后 |
| `crates/xgc-render/tests/layers.rs` | 混合层渲染回归：Override 只动遮罩内关节 / 权重 0 等于基础姿势 / 空基础剪辑从绑定姿态起 / Additive 只动遮罩内 / 缺剪辑退回基础姿势 |
| `crates/xgc-render/src/animated_pose.rs` | 渲染侧动画姿态：按 `DrawItem.animation`（`Animator.clip` / `time` / `looping`）采样 `Mesh.animations`，把 `node_global × inverse_bind` 写成 pose 喂现有 GPU 蒙皮；有叠加层时走 `joint_pose_with_additive`，权重 0 / 空剪辑直接跳过；有根运动配置时走 `joint_pose_played` 并在 crossfade 后剥离根轴；有动画层栈时在 additive 之后叠 `DrawItem.layers`（基础剪辑为空也从绑定姿态起）；已有场景骨架 pose 时不插手 |
| `crates/xgc-render/src/glb/texture.rs` | `TEXCOORD_0` 与 `baseColorTexture`：外部 uri（相对模型目录）/ GLB 内嵌 bufferView 解码 PNG；data URI 直接报不支持 |
| `crates/xgc-render/src/glb/material.rs` | glTF 材质 JSON 最小投影：`baseColorFactor`（缺省白）+ `baseColorTexture` 索引 |
| `crates/xgc-render/src/assets.rs` | `MeshCache`：builtin / 相对路径 GLB / AssetCatalog `guid://` 解析 / 越界拒绝；`.mat` 材质按引用缓存，读不到退回模型材质；`root_motion_library` 建根运动轨道表（CLI / editor / player 三宿主共用） |
| `crates/xgc-render/src/resources.rs` | `ResourceDesc` / `ResourceKind` / `ResourceUsage` / `ResourceTable` |
| `crates/xgc-world/tests/` | world / commands / journal / query / snapshot 回归 |
| `crates/xgc-player/src/lib.rs` | 窗口与无头共用场景加载和 GUID 目录；原生项目可按登记名或唯一文件名主干选场景，cooked 校验清单并优先读 XGCB |
| `crates/xgc-player/src/window.rs` | eframe + wgpu 视口：原生项目读清单的输入路径，cooked 读根目录默认映射；模型与根运动共用网格缓存 |
| `crates/xgc-player/src/input.rs` | 播放器输入桥：`egui::Key` / `PointerButton` → `InputState`，每固定步重写全部动作 / 轴 |
| `crates/xgc-player/src/main.rs` | `xgc-player <cooked 目录 | 项目目录 | project.xgc> [帧数] [--scene <场景名>]`：无帧数打开窗口，有帧数 headless |
| `crates/xgc-cli/src/main.rs` | CLI 参数分派；文件预算拆分后的轻量入口 |
| `crates/xgc-cli/src/check_cmd.rs` | `xgc check` 分派：schema / 单世界 / 旧迁移源 / 原生项目；原生清单存在就按原生格式检查 |
| `crates/xgc-cli/src/check_cmd/project.rs` | 原生项目只读体检：清单与世界 schema、跨文件 ID、父链、预制体来源、资产及输入映射 |
| `crates/xgc-cli/src/plan_cmd.rs` | `xgc plan`：默认调度的确定性 JSON 执行计划（系统访问声明 + 连续批次 + 宿主阶段） |
| `crates/xgc-cli/src/clips_cmd.rs` | `xgc clips <剪辑目录|模型.glb|模型.gltf>`：目录走 `.ron` 剪辑库，模型走 glTF 剪辑列表（按名字排序、确定性输出） |
| `crates/xgc-cli/tests/clips.rs` | clips 两层输入回归：Fox.gltf 列出 Survey/Walk/Run + 逐字节确定；Box.glb 报「没有 glTF 动画」 |
| `crates/xgc-render/tests/animations.rs` | glTF 剪辑回归：Survey/Walk/Run 各 21 channel、Walk 两个时间点姿态不同、缺失剪辑 / 坏时间返回 None、Box.glb 无动画 |
| `crates/xgc-cli/tests/plan.rs` | 执行计划 CLI 回归：两次输出逐字节相同、系统名与 `default_system_names` 同源、组件名全部来自内建注册表、多余参数忽略 |
| `crates/xgc-cli/src/runtime_cmd.rs` | `run-scripts` / 截图 / `xgc trace <世界.json> [帧数] [资产根]` 共用的 `ScriptedRunner`（系统 → 脚本 → 粒子，逐帧可读世界）：`Simulation::step_with` + `ScriptRuntime::run_frame` 固定帧桥；截图路径同步产出粒子四边形；`run-scripts` 第 4 参数（输出.json）把终态世界写成同形 JSON 包络，供跨端状态对表 |
| `crates/xgc-cli/src/trace_cmd.rs` | `xgc trace <世界.json> [帧数] [资产根]`：逐帧打印 `frame / entities / revision / hash`（无资产根时与 `run-world` 同一条无脚本步进路径，带资产根时走 `ScriptedRunner`、与 `run-scripts` 同一路径；第 0 帧是装载后状态），末行汇总与 `run-world` / `run-scripts` 同口径 |
| `crates/xgc-cli/src/gate_cmd.rs` | `xgc gate <世界.json> <资产根> <参考目录> [总差异%] [结构性%] [--strict-total]`：参考目录里 `trace.txt` 逐帧对表 + `frame-<N>.png|ppm` 截图分桶判定；结构性（缺省 0.5%）始终硬判，总差异（缺省 2%）只在参考带同环境 `env.json` / 显式阈值 / `--strict-total` 时硬判，否则仅诊断；`xgc gpu-env [参考目录]` 写 `env.json`；参考集在 `xgc/acceptance/`，换机器必须重生成截图；退出码 0/1/2 |
| `crates/xgc-cli/src/world_cmd.rs` | `xgc run-world <世界.json> [帧数]` / `verify-migration <旧.ron> <世界.json>` |
| `crates/xgc-cli/src/assets_cmd.rs` | `xgc assets <项目根>`：GUID 扫描 |
| `crates/xgc-cli/src/cook_cmd.rs` | `xgc cook <项目根> <输出目录>` / `pack <世界.json> <项目根> <输出目录>` / `run-cooked <目录> [帧数]` |
| `crates/xgc-cli/src/compare_cmd.rs` | `xgc compare-headless` / `xgc image-diff <旧图> <新图> [阈值] [report]`；PNG / PPM 逐像素差异，`report` 追加余差归因 |
| `crates/xgc-cli/src/audio.rs` | `xgc play <wav>`：解码后推默认音频设备 |
| `crates/xgc-cli/src/input_cmd.rs` | `xgc input <input.ron>`：校验并列出动作 / 轴 |
| `crates/xgc-cli/src/render.rs` | `render-frame <世界> <PPM> [资产根] [宽] [高] [帧数]`；缺省 0 帧，最终重算层级 / UI；曝光、天空、光晕、阴影画质与 FXAA 设置由 `project_render.rs` 从项目级 `project.ron` 读出后灌进 `Renderer` |
| `crates/xgc-cli/src/project_render.rs` | `project.ron` 画面设置投影：`render.exposure` / `render.sky` / `render.bloom` / `render.fxaa` / `render.shadow`；缺文件 / 缺节一律落回旧端缺省（曝光 1.0、其余关闭态），缺省值必须逐项等于 `RenderConfig` 的曝光 1.0 与 `SkyConfig` / `BloomConfig` / `FxaaConfig` / `ShadowConfig` 的 `default` |
| `crates/xgc-xtask/src/main.rs` | `xtask check` / `size` / `deps` / `schema` |
| `.github/workflows/xgc.yml` | XGC CI：`check`（fmt / clippy / xtask / CLI 冒烟）+ `test`（12 个不碰 GPU 的包 + 14 个 CPU-only render 目标 + release 基线）；toolchain 钉 1.95.0 |

## 不变量（改代码前先看）

- **编辑器资产浏览**：`EditorState::assets()` 只读清单内容目录，路径先走 `normalize_project_relative`，重叠根按规范目录去重；每层先排序再递归截到 2000 条，隐藏文件 / Windows Hidden / 符号链接与 reparse point / `.meta` 不进入清单。UI 只在首次展开或点击 Refresh 时扫描（每帧只读缓存），长路径截断并以悬停显示全文；点选预制体只回填路径，实例化仍由工具栏 Prefab 按钮发起。不得在浏览时生成 `.meta` 或改世界。

- 一个组件一个稳定 `ComponentId`；注册表按 ID 排序输出，schema 必须确定性。
- **新增组件要同时改两处**：`registry.rs::builtin_registry`（schema/Agent）与 `data.rs::register_builtin_components`（World 工厂），缺一不可。
- `EntityId` 是持久身份，`EntityKey` 是运行时句柄；句柄复用必须换 generation。
- `Replace` 会销毁并重建实体行，**旧 EntityKey 失效**，按 ID 重取句柄。
- archetype 的每一行列长度必须相等；`despawn` 用 `swap_remove` 并修正被搬实体行号。
- `World::spawn` 只接受元组 `Bundle`；重复组件在 debug 断言里拒绝。
- **模型材质因子**：实体不带 `render.material` 时用模型自带的 `metallic` / `roughness`（glTF spec 缺省 1.0；内建图元 = 旧端 `MaterialParams::default` 的 metallic 0.0 / roughness 0.45）；只有 `DrawItem.has_material` 为真才用组件覆盖 roughness。`Material::default().roughness = 0.5` 不是模型缺省。
- **内建图元**：生成器必须与旧端逐顶点一致（cube 24 顶点带面法线 / UV、`sphere(0.5, 24, 32)` 平滑法线、cylinder 32 段、plane +Y 法线）；内建图元没有法线时会退回屏幕导数（分面），球面高光与条带会重新出现。
- **并行写视图**：同批系统并行前先 `World::extract_writes` 取成 `WriteSet`（不 bump revision、不动实体表）；提取期间世界读不到该列、窗口内不许增删实体（`restore_writes` 断言兜底）。批内并行由 `System::supports_parted` / `run_parted` 声明，`Schedule::set_parallel_threads(n)` 显式打开（默认串行）：分区执行——支持 `supports_parted` 的系统并行、其余按注册顺序串行（批内两两无冲突 ⇒ 先串后并等价），CLI `run-world` / `trace` 可传线程数；panic 先还原再 unwind。
- **帧首准备**：`System::prepare(&World)` 是命令结算之后、执行计划之前的**只读**阶段（按注册顺序调用，只重建派生缓存，不能改语义）；刚体表等派生状态必须在这里建，别挪进 `run`——会吃到本帧 `VelocitySystem` 的积分，整条物理轨迹多出 g·dt²。
- **宿主阶段位置**：`Schedule::plan()` 把 `PlanStage::Host` 插在第一个 `System::runs_after_host()` 系统之前（没有这种系统才落最后）；动画系统（`animation_state` / `animation` / `animation_apply` / `animation_layers` / `skeleton_pose`）都声明 `runs_after_host`，UI 系统排在宿主之前（旧端 `update_ui()` 先于 `Update` 钩子）。不要再假设宿主阶段恒在最后。
- **碰撞事件**：`World::collision_events` 是瞬态、`Simulation::step_with` 每步开头清空，**不进 `WorldData` / 快照 / `state_hash`**；AABB 接触表与角色-传感器接触表各自跨帧 diff，一对接触给双方各一条有向 `CollisionEvent`；sensor 只记接触、不做分离（否则触发器会把动态体推出去）；脚本在 `update` / `fixed_update` 之后按 `entity` 派发 `on_collision_enter/exit(ctx, other)`，命令与 update 的命令同帧结算。静态 sensor 改 `Transform` 不会推给 rapier（只有 kinematic 体每帧 `set_position`），只有增删 / 换体才变更接触。
- **项目宿主**：编辑器与播放器以 `ProjectSession` 确定项目根、场景与输入路径，视口、脚本和根运动以项目根加载 GUID 资产。窗口的原生项目输入以清单登记为准，未登记则无映射，登记文件读失败或解析失败则报错。cooked 窗口与单世界编辑器使用各自根目录下的可选 `input.ron`。播放器每次把所有动作 / 轴（包括松开后的 false / 0）送入脚本。回归见 `crates/xgc-player/src/window.rs` 和 `crates/xgc-editor/tests/editor.rs`。
- **播放器场景选择**：项目目录和 `project.xgc` 均支持 `--scene`。先按清单登记名精确匹配，再按唯一文件名主干匹配（`character` → `scenes/character.json`）。多个同名主干必须写完整路径。cooked 目录传 `--scene` 返回错误。回归见 `crates/xgc-player/tests/player.rs`。
- **MCP 适配器**：`xgc mcp` 只做 `initialize` / `tools/list` / `tools/call` 封装，工具名 = `xgc.agent/1` 方法名把 `.` 换成 `_`，参数与结果原样透传；**不要在这里复制查询 / 编辑逻辑**。工具执行失败放 `isError`，未知工具 -32602，未知方法 / 坏 JSON 走 JSON-RPC 错误码；通知（无 id）不回应。回归 `crates/xgc-agent/tests/mcp.rs`。
- **动画事件**：`Animator.events` 是作者数据（`time` 秒数和 `name` 组成结构，Inspector 和 Agent 用同一条 `events[0].time` 路径读写）；`AnimatorSystem` 推进时钟后按 `crossed_events` 跨帧判定并写瞬态 `World::animation_events`（**不进 `WorldData` / 快照 / `state_hash`**），本系统开头清空上一步；脚本宿主在动画系统**之前**，所以 `on_anim_event(ctx, name)` 比旧端晚一个固定步收到（别把动画挪到宿主前，会打乱状态机时序）。循环剪辑只在真的跨圈时触发；`started`（首帧）额外派发零时刻事件。新增作者数据字段一律走 `Component::state_fields()` 排除，别只加进 `fields()` 让老场景哈希漂移（`Animator.events` 就是这么做的，回归 `crates/xgc-world/tests/hash.rs`）。
- **glTF 动画**：剪辑挂 `Mesh.animations`（模型内部节点层级），`animated_pose::resolve` 按 `Animator.clip` / `time` 采样成 pose 喂蒙皮路径，不为迁移世界伪造骨骼实体；旧状态机迁移成 `animation.state`（sim / migrate 共用 `StateMachine`；`AnimationStateSystem` 支持 `AtLeast` / `Below` + 迟滞、时长缺省 `fade_time`）。`additive`（参考 = 叠加剪辑第 0 帧、权重 0 逐位等于基础）、`layers`（`animation.layers` 结构化层栈；mask 沿 parent 链匹配节点名；基础 + crossfade + additive 之后叠 Override / Additive）、渲染侧交叉淡入（`Animator.blend_from*` 定格旧剪辑时钟 + `glb/crossfade.rs` 局部 TRS 按 alpha 混合）、`root_motion` 均已迁移；宿主阶段夹在固定步与动画之间（脚本当帧参数当帧生效）；渲染侧在 crossfade 后 additive 前剥离根轴；CLI / editor / player 三宿主均已接根运动。细节 / 逐刀数字见 STATUS / CUTOVER。
- **根运动**：`Animator.root_motion` 是 RON 快照（`(node:"b_Hip_01", axes:(true,false,true))`，空 = 关；节点名必须显式写，不能猜根节点）；world 侧 `root_motion.rs` 是纯函数（循环剪辑用展开轨迹算跨度增量、fade 按 alpha 混两段增量、轴过滤）；实体位移由宿主在 `step_with` 之后调 `apply_root_motion`（不进默认调度：轨道表来自模型资产，默认调度保持零资产依赖）；轨道表统一由 `MeshCache::root_motion_library` 建一次、CLI / editor / player 同源（去掉任一宿主的应用行都会让该宿主的哈希 / 位移回归变红）；渲染侧在基础 + crossfade 之后、additive/layers 之前把开关轴平移换回绑定值（否则蒙皮与实体双倍前进）；local→world 用 TRS 线性部分（无剪切项）近似，父级非均匀缩放 + 子级旋转与旧端完整矩阵有细微差。
- 热路径先保证正确；性能数字必须写进 README / 基准，不许凭感觉写"更快"。
- **CI**：`.github/workflows/xgc.yml` 只跑不碰 wgpu 的测试，GPU 集成测试留在带显卡机器；release 基线只记数字不设阈值；toolchain 钉 1.95.0（stable 漂移会让新 lint 变红）。
- 核心 crate 默认 `#![deny(unsafe_code)]`；加 unsafe 必须单独说明调用窗口。

- **UI 脚本 API**：29 个函数分在 `crates/xgc-script/src/{ui_api,ui_focus_api,ui_spawn_api}.rs`（`legacy.rs` 只留注册）；语义以旧端 `ui_ctx.rs` 为准：`ui_set_size` 写 `offset_max = offset_min + size`（丢 offset_min 会让血条宽高各小 2 px）、`ui_set_pos` 保持尺寸、文本尺寸 clamp 1..512、半径 max 0、对齐小写匹配、颜色 sRGB→线性（`srgb_to_linear`，alpha 不转）、缺 `ui.element` 的 setter no-op；同帧多次写先读命令队列未结算 `SetField` 再退回 view 快照。焦点三件套只写组件帧末结算，目标非 Button / 隐藏 / 禁用 no-op。图像三件套：`image_tint` 线性、`image_fill` 0..=1 与九宫格互斥（fill<1 矩形与 UV 一起左裁）、`keep_aspect` 复用 `image_mode="preserve"`；旧 `slice` 四值仍单值简化。`ui_spawn_*` 走 `Command::Spawn`（六件套、id 用 `WorldData::next_id`、同帧靠 `ScriptView::pending_spawns`、失败返 0；焦点排他要算 pending）。回归 `crates/xgc-script/tests/{ui_layout,ui_focus,ui_image,ui_spawn}.rs` 与 `crates/xgc-render/tests/ui_image_fill.rs`。

- **UI 覆盖层 SDF**：圆角公式照旧端（`fill = 1 - smoothstep(-0.5, 0.5, d)`、边框环带按覆盖率混色），`radius == 0 && border_width == 0` 直接返回实例色（全屏三角形 / 清屏路径依赖）；焦点环画在背景之后；内容区 inset = `padding + border_width`。WGSL 在 `triangle.wgsl`，改 uniform 字段必须同步 128 字节布局。回归 `crates/xgc-render/tests/ui_border.rs`。

- **刚体建体取世界变换**：位置 / 旋转 / `Auto` 尺寸一律走 `rapier_world/bodies.rs::world_transform`（父链 `local_matrix` 相乘 → `decompose`），与旧端 `body_from_scene(def, scene.world_transform())` 同口径；非单位四元数在这里被矩阵分解归一化 —— 直接把它塞给 `UnitQuaternion::from_quaternion` 会得到非正交旋转矩阵（落地接触分叉，2026-11 修过一次）。子实体刚体因此建在世界位置；运动学体每帧推送也用世界变换。分解来源是 glam 语义，XGC 用 nalgebra 实现（残差 ~1e-5 级）。
- **截图裁剪面**：相机优先级 = 显式机位 > 场景相机实体 > 顺光自动取景；**近 / 远裁剪面一律由 `shot::clip_planes` 按场景包围球重算**（旧端 `screenshot.rs` 同口径），不要读相机组件 `near` / `far`——差 0.08 就能把高处投射体裁出 CSM 第 0 / 1 级深度图。回归 `crates/xgc-render/tests/shot.rs`。
- **割接门禁**：`xgc gate <世界.json> <资产根> <参考目录>` 逐帧比对 `trace.txt` 的 `frame / entities / hash`，忽略内部计数器 `revision`。截图结构性差异（单通道 ≥8）始终按默认 0.5% 判定。总差异默认 2%，在参考指纹完整一致、显式给总差异阈值或 `--strict-total` 时判定，否则只报告比例。`tools/regenerate-xgc-acceptance.ps1` 从旧端实际截图报告提取 GPU、后端、驱动与版本，通过仓库轨迹和严格门禁后发布参考集。门禁对所有帧复用同一次 GPU 选择。退出码 0/1/2。回归 `crates/xgc-cli/tests/gate.rs`。

- **视觉对等判定**：`xgc parity <旧图> <新图>` 用 `diff::parity_report` 把结构性差异（单通道 ≥8）拆成「轮廓 1 px 带内」与「内部」：轮廓像素 = 中心像素与 8 邻域任一分量差 > `PARITY_EDGE_THRESHOLD = 40`（与 ≥8 是两个独立口径），两张图各判一次取并集；门槛 = 内部结构性 ≤ 0.01% 且总结构性 ≤ 0.1%（`mean_abs` / `max` 只记录）。状态对齐帧（第 1 / 150 帧）内部仅 9 / 30 像素；第 200 / 300 帧的内部差异来自动态体状态漂移（软阴影边缘 / 面着色），由 `xgc gate` 端到端口径守。旧端参考必须在验收机重生成。回归 `crates/xgc-cli/tests/parity.rs`。

## 当前下一步（按顺序）

1. 割接视觉：已接项见 STATUS / CUTOVER；逐帧 trace（`xgc trace`）与割接门禁（`xgc gate`）已建。视觉第 150 帧 **1.01% / mean_abs 0.02 / 结构性 0.08%**（第 1 帧 0.48%、第 200 / 300 帧结构性 0.13% / 0.26%）；动态体状态差 1e-5 级、Fox 位置差 4e-7。下一刀收狐狸位姿残差（第 150 帧 244 / 第 300 帧 664）；图像 tint·fill 与运行时创建已接（第 206 / 207 轮，UI 脚本 API 29 个与旧端对齐）；视觉对等判定已工具化（第 211 轮：`xgc parity` 拆轮廓 / 内部，门槛内部 ≤ 0.01% 且总 ≤ 0.1%，状态对齐帧逐帧通过）；跨系统并行已收口（第 210 轮）；关节 / CCD 排后面。逐刀数字见 STATUS.md / CUTOVER.md。
2. 跨系统并行与脚本生命周期：分区执行已收口（第 210 轮：`supports_parted` 子系统并行、其余串行，`skeleton_pose` / `audio_mixer` 入批，1/2/8 线程 parity 与 CLI 线程数进门禁；默认串行守零分配门禁）；剩余为常驻线程池、`HierarchySystem` 的 `set_field` revision 语义与脚本生命周期（`ScriptRuntime` 是 `!Send`，依赖门禁也不许 `xgc-script → xgc-sim`）。
3. 割接：旧新逐帧 trace 的 XGC 侧（`xgc trace`）已建、脚本帧也已纳入；旧端用 `--headless --dump-scene` + `xgc run-scripts … <输出.json>` 做逐 checkpoint 状态对表（Spinner 全等、物理差 g·dt²≈2.7 mm 落地收敛、相机 4e-6），截图阈值门禁已建（`xgc gate`：逐帧 trace 151 个 checkpoint + 4 张截图全过，参考在 `xgc/acceptance/`；退出码 0/1/2）；视觉余差归因工具 `image-diff … report` 已建（0.72% 只差 1 LSB、结构性 0.08%、总差异 1.01%），收窄待旧端逐帧截图；性能门禁的 XGC 侧热路径分配门禁已建；CI release 基线已建（`.github/workflows/xgc.yml` 的 `test` job 跑 `steady_state --ignored`，只记数字）。
4. 资产：密码学签名 / 跨机器增量。
5. 推送与 CI 已建（`.github/workflows/xgc.yml`）；最大 Rust 文件 597 行。
   角色回归分三个文件：`crates/xgc-sim/tests/character_kcc.rs`（水平 KCC + 台阶 + 陡坡挤入 + 顶着墙起跳 / 贴墙下落：撞墙 / 斜墙滑 / 球体阻挡 / 刚体优先 / `Replace` 换代后仍移动 / 默认调度陡坡不挤入 / 小半径胶囊上台阶 / 低台阶抬升 / 带刚体不被角色系统写 / 确定性）、
   `crates/xgc-sim/tests/character_ground.rs`（竖直 / 地面 / 坡：自由落体 / 跳跃弧线 / 站地不沉 / 站坡不滑 / 走上 30° 坡 / 走下 30° 坡贴地吸附 / 走出坡沿落地 / 落到箱顶 / 60° 坡上会滑下去 / 50° 迟滞带内站得住 / 确定性）、
   `crates/xgc-sim/tests/rapier_schedule.rs`（刚体表：默认调度动态体落地 / `Replace` 换代后仍移动 / 改 `PhysicsBody` 参数会重建 / 稳态帧不重建 / 缩放动画不重建 / 逐字段表驱动（`half_extents` 三个分量各一条）/ `kind` 拼错仍按动态体回写 / 运行期增删只增量修补 / 增量修补不扰动已有刚体）、
   `crates/xgc-sim/tests/slope_collider.rs`（坡面碰撞体：陡坡挡住角色 / 上表面与解析坡面逐点对齐 / 改坡面参数后重建 / 带 `physics.body` 的坡让位 / 尺寸退化与非有限参数被跳过 / 下坡同样对齐）。
   各文件世界构造不同，别把语义混着塞回一个文件。

## Token 纪律

- 先读本文件 + README；不要读 `target/` 或整份主文档。
- 每轮只动一个可验证的垂直切片；先跑测试再写总结。
- 长任务交给子智能体时，给它**文件路径 + 不变量 + 验收命令**，不要复制整份设计。
