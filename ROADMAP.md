# XGameEngine 开发计划

> Rust 实现的类 Unity 游戏引擎。**本文只记阶段与状态** —— 为什么这样做看 `PLAN.md`，
> 能力差距与优先级看 [`Docs/差距分析.md`](Docs/差距分析.md)，不可推翻的约束看
> [`Docs/决策记录.md`](Docs/决策记录.md)。项目结构、运行命令、各功能怎么用统一在
> [`README.md`](README.md)，本文不再复制。

---

## 整体路线图

| 阶段 | 状态 | 内容 |
|---|---|---|
| **一** | ✅ 完成 | Workspace + 窗口 + 三角形 |
| **二** | ✅ 完成 | 基础渲染（glTF、纹理、Blinn-Phong 光照） |
| **三** | ✅ 完成 | Model 矩阵 + 局部变换 |
| **四** | ✅ 完成 | 场景系统（RON 序列化、多实体） |
| **五** | ✅ 完成 | 物理集成（rapier3d） |
| **六** | ✅ 完成 | 编辑器（egui 面板） |
| **六·五** | ✅ 完成 | 玩法层：输入系统 + 脚本系统（Lua）+ 统一运行时 |
| **六·六** | ✅ 完成 | 稳定性 / 流畅度 / 性能硬化（无新功能：运行时热路径、渲染批上传、崩溃面与无界增长治理，附分档微基准） |
| **六·七** | ✅ 完成 | 编辑器产品化第一波：项目设置、未保存保护、无头检查 `--check`、差距分析入库 |
| **P0 资产管线** | ✅ 完成 | 资产 GUID + `.meta` + 资产库诊断 + `--migrate-assets` + 资源浏览器（浏览 / 拖入场景）+ MCP `asset_list`；见 PLAN.md 三十二、决策记录 §十四 |
| **双轨 P0–P3** | ✅ 完成 | Rust 组件（UE 插件模式）+ Lua 脚本（Unity 模式 + 热更）双轨；P2.5 基准判定后 P3 删 Rhai，脚本层收敛单一 Lua 轨。决策 D1–D11 见[计划文档](Docs/双轨语言架构计划.md)，实施记录见 PLAN.md 十五–二十一 |
| **七** | 🟨 进行中 | 动画系统（前五片 ✅：骨骼蒙皮 + 单剪辑 + 交叉淡入 + 阈值状态机 + 叠加 + 多层混合栈与骨骼遮罩；morph / Phase B 待做） |
| **八** | ✅ 完成 | 音频系统（kira：`AudioSource` + 3D 空间音频 + 监听者跟随 + 静音降级 + 脚本 API，见 PLAN.md 二十二） |
| **Agent-Native ①②** | ✅ 完成 | MCP 工具面（8 工具，含 `asset_list`）+ 无头运行 `--headless` / `--screenshot`；见 [MCP.md](Docs/MCP.md) / [无头运行.md](Docs/无头运行.md) |
| **渲染 PBR** | ✅ 完成 | Cook-Torrance + 逐实体材质覆盖 + 无头截图（PLAN.md 二十四） |
| **渲染 HDR** | ✅ 完成 | HDR 中间标 + ACES + 曝光（二十五）、Bloom（二十六） |
| **渲染天空** | ✅ 完成 | 程序化天空盒 + 球谐漫反射 + 预过滤镜面 + BRDF LUT（二十七） |
| **渲染批处理** | ✅ 完成 | GPU 实例化合批（二十八）+ 视锥剔除与阴影补投（三十）+ FXAA（三十一） |
| **九** | 🟨 进行中 | 后处理 + 阴影：上述渲染片已落地；**TAA、CSM、点光阴影、SSAO、LOD 待做** |
| **游戏 UI** | ✅ 完成（最小集） | 屏幕空间 UI：锚点布局 + 文本（中文 / 粗体 / 换行 / 对齐）+ 圆角面板 / 图片 / 按钮 + 指针交互（悬停 / 按下 / 点击）+ 23 个 `ui_*` 脚本 API + Inspector 区块；见 PLAN.md 三十四、决策记录 §十六 |
| ~~**十**~~ | ✅ 完成 | 脚本系统 + 热重载（已并入六·五与双轨 P0–P3） |

---

## 第一阶段：骨架 ✅

- [x] Cargo workspace 搭建
- [x] xgame-core（Color, WindowConfig）
- [x] xgame-math（glam re-export, Transform）
- [x] xgame-render（RenderContext, 深度缓冲, triangle example）

## 第二阶段：基础渲染 ✅

- [x] glTF 模型加载（.gltf / .glb）
- [x] 纹理上传 + GPU 采样
- [x] 方向光 + Blinn-Phong 着色（已由 PBR 取代，见第九阶段）
- [x] 轨道相机
- [x] 深度测试 + 背面剔除

## 第三阶段：Model 矩阵 ✅

- [x] ModelUniform + ModelTransform
- [x] 顶点着色器 world_pos 变换
- [x] 法线矩阵（mat3x3）
- [x] WASD 键盘操控 + 旋转/重置

## 第四阶段：场景系统 ✅

- [x] Entity / Scene 类型（RON 序列化）
- [x] 场景文件归类到 Scenes/

## 第五阶段：物理集成 ✅

- [x] rapier3d 依赖引入
- [x] RigidBody 组件（static / dynamic / kinematic）
- [x] Collider 组件（cuboid, ball, capsule, cylinder, trimesh）
- [x] 物理步骤与渲染同步
- [x] 碰撞事件回调（引擎内；脚本侧尚未暴露，见差距分析 P1）
- [x] 物理可视化（debug draw）
- [x] 射线查询 + 过滤器（`PhysicsWorld::raycast`，脚本 `raycast`）

## 第六阶段：编辑器 ✅

- [x] egui 集成到 wgpu/winit
- [x] 面板布局（Hierarchy / Inspector / Scene View / Console / Prefabs）
- [x] 场景视图（3D 渲染到纹理 → egui 显示）
- [x] Hierarchy 树形面板（选择/可见性/拖拽重挂/防环）
- [x] Inspector 属性编辑（名称 / Transform / 可见性 / 物理 / 脚本 / 动画 / 音频 / 材质 / Behaviors）
- [x] Play / Pause / Stop 模式
- [x] 渲染路径统一（ScenePipeline 共享渲染）
- [x] Gizmo 变换操作柄（平移 / 旋转 / 缩放，吸附字段已在但未接线）
- [x] 资源浏览器（Assets 面板：`Resources/` 等目录树、类型图标、搜索、详情、GUID、
      导入设置；双击 / 拖入视口放入场景 —— P0 资产管线）

## 额外里程碑

| 里程碑 | 状态 | 内容 |
|---|---|---|
| 共享渲染管线 | ✅ 完成 | 从 SceneRenderer 提取 ScenePipeline 到 xgame-render |
| 独立游戏二进制 | ✅ 完成 | xgame-game crate，原生命令行运行 |
| 引擎 / 项目分离 | ✅ 完成 | `project.ron` + `ProjectHandle`；编辑器项目枢纽（新建 / 打开 / 最近项目） |
| 一键构建 | ✅ 完成 | 发布构建脚本（双击 `一键构建.cmd` → `dist/`）；编辑器 Build 按钮把项目打包成独立游戏 |

---

## 商业引擎对齐（状态速览）

对照 Unity / Godot / Unreal 的必备能力。**完整矩阵（39 项缺口 + 已对齐项）与排序理由见
[`Docs/差距分析.md`](Docs/差距分析.md)**，这里只留状态。

| 能力 | 状态 | 说明 |
|---|---|---|
| PBR 渲染 | ✅ 完成 | Cook-Torrance + 逐实体材质覆盖（此前 Blinn-Phong） |
| 纹理 / 多光源 / Alpha | ✅ 完成 | 贴图采样 + 单帧 8 盏 + glTF `alphaMode` 半透明排序 |
| 内置基础图元 | ✅ 完成 | Cube / Sphere / Plane / Cylinder（`builtin://`） |
| 场景层级 | ✅ 完成 | `Entity::parent` + `Scene::hierarchy()`；拖拽保世界变换、防环 |
| Undo / 复制粘贴 | ✅ 完成 | 快照式撤销（拖拽一步）；整棵子树复制粘贴 |
| 预制体（Prefab） | ✅ 完成 | 资产化 + 实例来源标记 + Apply / Revert |
| 资产引用稳定性（GUID） | ✅ 完成 | 磁盘存 `guid://` + `.meta`（GUID 永不变）、内存存路径；改名 / 挪目录不断链；`--check` 报悬空 GUID / 缺 meta / 冲突 |
| 资源浏览器 | ✅ 完成 | Assets 面板：目录树 / 类型图标 / 搜索 / 详情 / GUID / 导入设置；双击与拖拽放入场景 |
| 导入设置消费 | 🟨 待接线 | `.meta` 的 `model_scale` / `texture_*` / `audio_streaming` 已落盘且可编辑，渲染侧尚未读取（见决策记录 §十四.6） |
| 多场景管理 | ⬜ 待做 | 单场景；Save As 可存新文件并切过去 |
| 角色控制器 | ✅ 完成 | 胶囊控制器 + 自动上台阶 + 坡面 / 陡坡 + 碰撞事件进脚本（见阶段六·六） |
| 游戏 UI | ✅ 完成（最小集） | Canvas 等价物 = 实体 + 锚点；文本 / 图片 / 按钮 / 面板；`ui_*` 23 API；缺裁剪、九宫格、输入框、手柄导航 |
| 输入系统 | ✅ 完成 | `InputMap`（`input.ron`）+ 编辑器 / 游戏共用；无手柄 / 触摸 |
| 项目设置 | ✅ 完成 | 项目名 / 入口场景 / 窗口 / 重力 / 步长 / 画面（曝光 / ACES / Bloom / FXAA），草稿 + Apply；单入口 `from_project_runtime`。内容目录只在 RON 里手改 |
| 未保存保护 / Save As | ✅ 完成 | 脏标记 + 双路径确认 + 标题星号 + 快捷键 |
| 无头检查 `--check` | ✅ 完成 | 退出码 0/1/2，CI 与智能体的可机读入口 |
| 无头运行 `--headless` | ✅ 完成 | 固定 dt 逐位可复现；`--screenshot` 离屏出图 |
| 阴影 / 天空 / 后处理 | 🟨 部分 | 方向光阴影 + MSAA + HDR/ACES + Bloom + 天空 IBL + FXAA 已落地；CSM / TAA / SSAO 待做 |
| 实例化 / 剔除 | 🟨 部分 | GPU 实例化 + 视锥剔除 + 阴影补投已落地；LOD / 遮挡剔除待做 |
| Gizmo 吸附 / Rect 工具 | ⬜ 待做 | `gizmo_snap` 字段已在但未接线；Rect 工具无实现 |
| 编辑器交互稳定性 | ✅ 完成 | 射线 × 世界 AABB 拾取；轴拖拽参考线锚定；选中索引每帧收敛 |
| 每帧开销 / 可扩展性 | ✅ 完成 | 2000 实体纯视觉 13 µs/帧；渲染 2225 → 344 µs（实例化）、3295 → 1146 µs（剔除）。剩余瓶颈是 rapier 求解器 |
| 崩溃面 / 无界增长 | ✅ 完成 | 未知 body 只警告；控制台上限 1000 条；撤销栈 100 步；缓存有界 |
| 窗口 / 交互延迟 | ✅ 完成 | 空闲不重绘（CPU 0.00%）；拖边框节流重建（43 → 1 次） |

---

## Agent-Native：AI 智能体深度操控引擎（①② 已落地）

**目标**：智能体能**深度、精确、可验证地**操作引擎 —— 读写项目、运行检查、拿到可机读
结果，而不是"截图点按钮"。**架构判决式：后续每个架构决策都要过一问 ——「智能体没有
鼠标时，这条路走得通吗？」**

已具备的地基（详见[差距分析](Docs/差距分析.md)第九节）：

- **数据即文本** —— 场景 / 预制体 / 清单 / 输入映射全 RON，可直接读写与 diff；
- **程序化验证** —— `--check`（退出码 0/1/2）；
- **程序化实跑** —— `--headless`（固定 dt 逐位可复现）；
- **程序化观察** —— `--screenshot` 离屏 PNG（同机逐字节一致），MCP 以图片块回传；
- **程序化配置** —— 重力 / 步长 / 渲染参数都在 `project.ron`；
- **单一运行时** —— 无头验证与玩家实跑完全一致；
- **MCP 工具面** —— 8 个工具（读 / 改 / 查 / 看效果 / 看资产）；
- **安全网** —— 491 条回归测试、每条先证红。

进度：① MCP server ✅ → ② 无头运行 + 截图 ✅ → ③ 场景 schema 导出（`--export-schema`）
⬜ → ④ 资产 GUID 落地 ✅，`asset_list` ✅（`asset_import` 仍待，导入参数目前只能在编辑器里改）。

---

## 阶段六·五：玩法层 ✅

在"能摆场景"和"能做游戏"之间差着三样东西：**输入**、**脚本**、**一个统一的运行时**。
这一轮把它们补齐，并刻意按"编辑器 Play 与打包游戏共用同一段实现"来做。

| 能力 | 状态 | 说明 |
|---|---|---|
| 输入系统 | ✅ | `xgame-input`：与 winit 解耦的 `KeyCode` / `MouseButton`、三级按键语义、`InputMap` 动作映射（多绑定 / 修饰键 / 双向轴） |
| 脚本系统 | ✅ | `xgame-script`：Lua（mlua，沙箱 + 指令预算）、四个可选钩子、`state_*` 每实体状态、编译缓存 + 热重载、崩溃 / 死循环不拖垮引擎 |
| 统一运行时 | ✅ | `xgame-runtime::World`：场景 / 物理 / 输入 / 脚本 / 音频一条帧管线，固定步长与渲染帧解耦 |
| 声明式物理 | ✅ | `Entity::physics`（kind / shape / mass / friction / restitution / gravity_scale / sensor）；不声明 = 纯视觉实体 |
| 物理射线 | ✅ | `raycast` / `raycast_with_filter`（忽略传感器 / 只打动态体 / 排除自身） |
| 编辑器接入 | ✅ | Inspector 的 Scripts / Physics 区块；工具栏热重载；Console 收脚本日志；Play 时 `input_bridge` 灌键鼠 |
| 角色控制器 | ✅ | 胶囊控制器 + 碰撞事件进脚本，见下方「阶段六·六」 |

> **已知欠账**：`crates/xgame-editor/src/editor_input.rs` 是一套"输入绑定表 → 动作"的
> 抽象，**从未接入**（`EditorState::input_manager` 只构造、没人读），整体
> `#![allow(dead_code)]` 保留；接入前要先修它内部三处（拖拽增量滞后一帧、修饰键全等
> 比较会让"不带修饰键"的绑定在按住 Ctrl 时整批失效、两个字段写了没人读）。
>
> **独立播放器**已迁到 winit 0.30 的 `run_app` + `ApplicationHandler`：窗口与 `GameApp`
> 推迟到 `resumed` 里创建；`about_to_wait` 主动 `request_redraw()` 的连续帧机制不变。

## 阶段六·六：角色控制器 + 碰撞事件 ✅

玩法闭环的第一块：角色能走、能跳、能上台阶、能踩触发器。

| 能力 | 状态 | 说明 |
|---|---|---|
| 胶囊角色控制器 | ✅ | `Entity::controller`（`xgame_core::CharacterController`）：半径 / 总高 / 台阶 / 坡度 / 皮肤 / 吸附，可 RON 序列化，`sanitized()` 钳制坏数据 |
| 移动解析 | ✅ | `PhysicsWorld::move_character`：rapier `KinematicCharacterController` + **四处上游缺陷的绕过**（见决策记录 §十五）：自实现 grounded / snap / 陡坡滑落 / 自动上台阶 |
| 角色刚体托管 | ✅ | `World::sync_bodies` 为 controller 实体建运动学位置胶囊；`push_kinematic_bodies` / `pull_dynamic_bodies` 跳过它（位姿归控制器） |
| 脚本 API | ✅ | `character_simple_move` / `character_move` / `character_jump` / `character_velocity` / `character_set_velocity` / `character_is_grounded` / `character_ground_normal` / `character_is_sliding`；Lua 与 Rust 组件共用 |
| 碰撞 / 触发事件 | ✅ | `on_collision_enter` / `on_collision_exit`（第二参 = 对方实体 id）：动态体走 rapier 事件，角色走"接触面差分"（运动学胶囊永远停在 skin 距离外，物理层没有真实接触）；双向派发、双方各收一次 |
| 编辑器 | ✅ | Inspector 的 Character 区块（勾选式，与 Physics 正交） |
| 测试 | ✅ | 物理 21 条行为测试 + 运行时 3 条（行走 / 跳跃 / 碰撞）+ Lua 端到端 1 条 |

> **已知缺失**：移动平台不带动角色（上游该特性走同一条坏掉的 contact manifold）；
> 无蹲伏 / 根运动 / 多人同步（属网络层）。

## 双轨语言架构：P0 ✅ P0.5 ✅ P1 ✅ P1.5 ✅ P2 ✅ P2.5 ✅ P3 ✅

Rust（UE 插件模式）+ Lua（Unity 模式 + 热更）双轨；**P3 起脚本层收敛为单一 Lua 轨**
（Rhai 已删）。计划与决策记录：[`Docs/双轨语言架构计划.md`](Docs/双轨语言架构计划.md)；
实施记录：PLAN.md 十五–二十一。

| 期 | 交付（一句话） |
|---|---|
| **P0** | `xgame` 门面 crate + `GameCtx` 方法层 + Rust 组件（`Lifecycle` / 注册表）+ cdylib 插件边界（`export_plugin!` + 版本握手）+ 示例插件全链路 |
| **P0.5** | 编辑器 PIE：有 `Cargo.toml` 即插件项目 → ▶ Play 后台构建 → `Plugins/` → 临时副本加载；Inspector Behaviors 区；演示项目 `projects/BehaviorsDemo` |
| **P1** | `#[derive(Inspect)]` + Inspector 按字段表自动渲染 + Edit/Play 双态调参 |
| **P1.5** | 组件参数落盘（`Entity::behavior_params`，注册名 → 字段 → 值）+ 撤销语义修正 |
| **P2** | Lua 宿主（环境表沙箱 + 指令预算）+ 路径驻留表（`intern.rs`，id 永不清） |
| **P2.5** | 三负载基准闸门：Lua 全占优（1.31× / 1.26× / 9.16×）→ 判定 P3 删 Rhai |
| **P3** | 删 Rhai、单轨 Lua；样例 / 文档迁移；闸门工具退役 |

**有意延后的遗留**：P0.5 的 Play 中途热换插件、组件徽章、release profile 插件出货、
cargo 构建超时；P1 的 `Quat` / `Color` 进 `FieldKind`、`trybuild`；P1.5 的 prefab 组件
字段、Play 中热调参写回；P2 的 `dispatch_destroy` / `forget_entity` 接线、编辑器"新建
脚本"。这些都是**明确决定不做**，不是遗漏（理由见计划文档与 PLAN.md 各期）。

## 第七阶段：动画系统 🟨

前五片已落地：

- [x] 骨骼蒙皮（GPU skinning；阴影 pass 同步蒙皮；每实体一个 256 骨槽位）
- [x] 动画片段加载（glTF → `AnimClip` / `Track`；LINEAR / STEP 运行时插值，CUBICSPLINE 降采样）
- [x] 关键帧插值（位置 / 缩放线性、旋转 slerp；循环取模、非循环钳位）
- [x] `Animator` 组件（clip / speed / looping / autoplay / fade_time，随场景落盘）
- [x] 播放时钟 `AnimationDriver`（编辑器墙钟、游戏按帧 dt；换剪辑归零）
- [x] 编辑器 Animation 区块（剪辑下拉 / Speed / Fade / Loop / Autoplay / 摘除；指纹与帧策略）
- [x] 换剪辑交叉淡入（`fade_time` 默认 0.25s；淡入中连切两刀重起淡入）
- [x] 阈值状态机（参数表 + 转移表（AtLeast ≥ / Below <）+ 滞回；坏配置安全退回单剪辑；脚本 `set_anim_param`）
- [x] 叠加（additive）混合 v1（增量 = 叠加剪辑当前帧 − 第 0 帧；独立时钟；权重 0 逐位一致）
- [x] 多层混合栈与骨骼遮罩 v1（每层 name / mode / clip / weight / speed / looping / mask；按数组顺序合成；遮罩 = 骨骼子树根名单；`compute_bones_composed` 唯一路径）
- [ ] morph target（表情等）；逐条转移时长、分层状态机、根运动（Phase B）

> 明确不含：真倒放（负速 = 停在第 0 帧）、逐条转移时长（统一 `fade_time`）、遮罩羽化
> （二值权重）。每个模型取第一张蒙皮，超过 256 骨的骨架解析时告警截断。

## 第八阶段：音频系统 ✅

- [x] kira 0.12.5 集成（`xgame-audio`：设备 + 解码缓存 + 设备失败无声降级）
- [x] `AudioSource` 组件（clip / volume / pitch / looping / autoplay / spatial / min-max distance，Inspector 可配、随场景落盘）
- [x] 3D 空间音频（距离衰减 + 每帧推发射位置；实体绑定 `follow`，摘除 / 删除自动停音）
- [x] 听者跟随（编辑器 Play 每帧推相机；脚本 `set_listener` 可覆盖）
- [x] 脚本 API（6 个：`play_sound` / `play_sound_at` / `stop_sound` / `stop_all_sounds` / `set_listener` / `set_master_volume`）
- [ ] 混音总线分组、流式长音频、音频遮挡 / 低通 —— 有意延后（差距分析 P2）

## 第九阶段：后处理 + 阴影 🟨

- [x] 方向光阴影贴图（单盏、正交视锥拟合、3×3 PCF）
- [ ] 级联阴影贴图（CSM，覆盖远处）
- [ ] 点光阴影（6 面立方体贴图）
- [x] ToneMapping（ACES）+ HDR 渲染路径 + 曝光（场景 pass → `Rgba16Float` → 曝光 + ACES → LDR；`project.ron` 的 `render` 节三宿主同源；`Off` = 旧管线出口）
- [x] Bloom 泛光（软膝 + 三级 mip + 可分离高斯；阈值在曝光后；**默认关**，整段跳过）
- [x] 程序化天空盒 + 环境光照（IBL）（解析天空一个真相源 + 球谐漫反射 + GGX 预过滤 + BRDF LUT；**默认关**）
- [x] GPU 实例化（同图元合批 + 逐实例 storage buffer；2000 实体 2001 → 2 批、CPU -84%）
- [x] 视锥剔除 + 阴影补投（实体级 AABB × 六平面；相机外投影物仅跑阴影 pass；蒙皮 50% 余量；开/关逐字节一致）
- [x] FXAA 抗锯齿（3.11 质量路径、显示空间 luma、合成后第二趟 pass、惰性 LDR scratch、**默认关**；1× MSAA 硬台阶 -44%）
- [ ] TAA（历史缓冲 / 运动向量 / 重投影，见 PLAN.md 三十一·五）
- [ ] SSAO（HDR 链已就位）
- [ ] LOD（距离分级 / 网格切替）
- [ ] RenderGraph（等出现带依赖分叉的 pass 再抽，见 PLAN.md 二十五）

---

## 项目结构与运行命令

已合并到 [`README.md`](README.md)：crate 清单 / 目录树 / 依赖方向见「引擎与游戏项目是分开的」，
全部运行命令见「快速开始」，每个功能怎么用见 README 对应小节。文档导航也统一放在
README 顶部。