# XGameEngine 开发计划

> Rust 实现的类 Unity 游戏引擎

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
| **六·五** | ✅ 完成 | **玩法层：输入系统 + 脚本系统（Rhai）+ 统一运行时** |
| **六·六** | ✅ 完成 | **稳定性 / 流畅度 / 性能硬化**（无新功能：运行时热路径、渲染批上传、崩溃面与无界增长治理，附分档微基准） |
| **六·七** | ✅ 完成 | **编辑器产品化第一波**：项目设置（重力 / 步长进 project.ron + ⚙ 设置窗口）、未保存保护（脏标记 + 关窗确认 + Save As + 标题星号）、无头检查 `xgame-editor --check`（Agent-Native 第一块砖）+ 差距分析入库 `Docs/差距分析.md` |
| **双轨架构 P0** | ✅ 完成 | 双轨语言架构（Rust + Lua）第一期：`xgame` 门面 crate + `GameCtx` 方法层 + Rust 组件（`Lifecycle` / 注册表 / `Entity::behaviors`）+ cdylib 插件边界（`export_plugin!` + 版本握手 + 示例插件全链路）—— 计划与决策见 [`Docs/双轨语言架构计划.md`](Docs/双轨语言架构计划.md)，实施记录见 PLAN.md 十五 |
| **双轨架构 P0.5** | ✅ 完成 | **编辑器 PIE**：项目根有 `Cargo.toml` ⇒ 按 ▶ Play 自动后台 `cargo build`（UI 不阻塞）→ 产物同步进 `<项目>/Plugins/` → 临时副本加载进 Play 会话；Inspector 的 Behaviors 区、工具栏编译指示器、打包器带 `Plugins/`、玩家加载插件；演示项目 [`projects/BehaviorsDemo`](projects/BehaviorsDemo) 是 Rust 组件与 Rhai 脚本同场景共存 —— 决策见计划文档 **D7**，实施记录见 PLAN.md 十六 |
| **双轨架构 P1** | ✅ 完成 | **`#[derive(Inspect)]` + 编辑器调参**：组件自带静态字段表（`Inspect` trait / `FieldDesc` / 10 种 `FieldKind`），Inspector 按表自动渲染控件（有边界给滑块、无边界给拖拽值、勾选框 / 文本 / 向量行），Play 中改**活实例**（热调参）、Edit 中改副本（按 ▶ 时灌进运行时）—— 决策见计划文档 **D8**，实施记录见 PLAN.md 十七 |
| **双轨架构 P1.5** | ✅ 完成 | **组件参数落盘进场景文件**：`Entity::behavior_params`（**注册名** → 字段寻址名 → 值）、装配期套用 + 编辑副本种子、`FieldKind`/`FieldValue` serde 变体标签钉死、组件字段编辑接入场景撤销栈 —— 决策见计划文档 **D9**，实施记录见 PLAN.md 十八 |
| **双轨架构 P2** | ✅ 完成 | **Lua 宿主 + 路径驻留表**：mlua 0.10 双轨宿主（每脚本环境表沙箱 + 指令计数沙箱 + StdLib 裁剪），按扩展名分流（`.lua` → LuaHost、其余 → Rhai）；修两宿主每帧 3 处路径 `String` 克隆 → 驻留 `u32`（Rhai 轨分发 **819.6 → 713 µs/帧**，同机背靠背 -13%）；编辑器双轨接入 —— 决策见计划文档 **D10**，实施记录见 PLAN.md 十九 |
| **双轨架构 P2.5** | ✅ 完成 | **基准闸门 Rhai vs Lua**：三负载同进程背靠背（dispatch / api / compute），**Lua 三负载全占优（1.31× / 1.26× / 9.16×）→ 判定 P3 删 Rhai**；闸门暴露负载 1 Lua 慢 2.25×，探针量证并修掉两处宿主热路径实现开销（钩子句柄编译期缓存 + ctx 常驻槽轻量指针传参，Lua 轨分发 **1670 → 576 µs/帧，-65%**）—— 规则见计划文档 §六，实施记录见 PLAN.md 二十 |
| **七** | 🟨 进行中 | 动画系统（第一片 ✅：骨骼蒙皮 + 单剪辑播放；第二片 ✅：换剪辑交叉淡入；第三片 ✅：阈值状态机；第四片 ✅：叠加混合；第五片 ✅：多层混合栈 + 骨骼遮罩；morph/Phase B 待做） |
| **八** | ✅ 完成 | 音频系统（kira：`AudioSource` 组件 + 3D 空间音频 + 监听者跟随 + 静音降级 + 脚本 API，实施记录见 PLAN.md 二十二） |
| **Agent-Native ①②** | ✅ 完成 | **MCP 工具面 + 无头运行**：`xgame-mcp` server（7 个工具：项目 / 场景 / 实体读写、`project_check`、`run_headless`）；`xgame-editor --headless` 无头跑 N 帧出 JSON 统计（固定 dt 两次运行逐位一致，退出码 0=干净 / 1=加载失败 / 2=跑完有问题），MCP 工具 spawn 透传 —— 文档 [`Docs/MCP.md`](Docs/MCP.md) / [`Docs/无头运行.md`](Docs/无头运行.md)，实施记录见 PLAN.md 二十三 |
| **渲染 PBR 片** | ✅ 完成 | **PBR（Cook-Torrance）+ 逐实体材质覆盖 + 无头截图**：GGX/Smith/Schlick 替换 Blinn-Phong（metallic/roughness 原样进 GPU，π 亮度标定保持旧场景亮度）；`Entity.material: Option<MaterialOverride>`（字段级覆盖 + Inspector + 场景落盘 + `scene_signature` 刷新）；`--screenshot` 离屏截图（默认顺光取景、同机逐字节确定）+ MCP `run_headless` 图片块 —— 实施记录见 PLAN.md 二十四 |
| **九** | 🟨 进行中 | 后处理 + 阴影（方向光阴影、PBR、**HDR 渲染路径 + ACES 色调映射 + 曝光**、**Bloom 泛光**已落地；CSM / 天空盒与 IBL 待做） |
| ~~**十**~~ | ✅ 完成 | 脚本系统 + 热重载（已并入六·五） |

---

## 第一阶段：骨架 ✅

- [x] Cargo workspace 搭建
- [x] xgame-core（Color, WindowConfig）
- [x] xgame-math（glam re-export, Transform）
- [x] xgame-render（RenderContext, 深度缓冲, triangle example）

## 第二阶段：基础渲染 ✅

- [x] glTF 模型加载（.gltf / .glb）
- [x] 纹理上传 + GPU 采样
- [x] 方向光 + Blinn-Phong 着色
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
- [x] 碰撞事件回调
- [x] 物理可视化（debug draw）

## 第六阶段：编辑器 ✅

- [x] egui 集成到 wgpu/winit
- [x] 面板布局（Hierarchy / Inspector / Scene View）
- [x] 场景视图（3D 渲染到纹理 → egui 显示）
- [x] Hierarchy 树形面板（选择/可见性）
- [x] Inspector 属性编辑（名称/Transform/可见性）
- [x] Play / Pause / Stop 模式
- [x] 渲染路径统一（ScenePipeline 共享渲染）
- [ ] 资源浏览器
- [ ] Gizmo 变换操作柄

## 额外里程碑

| 里程碑 | 状态 | 内容 |
|---|---|---|
| 共享渲染管线 | ✅ 完成 | 从 SceneRenderer 提取 ScenePipeline 到 xgame-render |
| 独立游戏二进制 | ✅ 完成 | xgame-game crate，原生命令行运行 |
| 引擎 / 项目分离 | ✅ 完成 | `project.ron` + `ProjectHandle`；编辑器项目枢纽（新建 / 打开 / 最近项目）|
| 一键构建 | ✅ 完成 | 发布构建脚本（双击 `一键构建.cmd` → `dist/`）；编辑器 Build 按钮把项目打包成独立游戏 |

---

## 商业引擎对齐（进行中）

对照 Unity / Godot / Unreal 的必备能力，按「先补齐最显眼的短板」推进。
**完整差距矩阵（37 项、按优先级排序）见 [`Docs/差距分析.md`](Docs/差距分析.md)。**

| 能力 | 状态 | 说明 |
|---|---|---|
| 纹理采样 | ✅ 完成 | 此前 glTF 贴图加载了却从不采样、模型全是顶点色；现已接上 `group(1)` 材质绑定组 |
| Blinn-Phong 光照 | ✅ 完成 | 环境光 + 漫反射 + 半程向量高光（此前实为纯 Lambert，与文档描述不符） |
| 多光源 | ✅ 完成 | 单帧最多 8 盏方向光 / 点光叠加；光源是场景级数据，随场景序列化 |
| 材质参数 | ✅ 完成 | 读 glTF 的 base color factor / metallic / roughness，换算成高光强度与光泽度 |
| Alpha 混合 | ✅ 完成 | 读 glTF `alphaMode`，独立混合管线；半透明按到相机距离远 → 近排序 |
| 内置基础图元 | ✅ 完成 | Cube / Sphere / Plane / Cylinder，用 `builtin://cube` 标识，不依赖外部资产 |
| 场景层级 | ✅ 完成 | `Entity::parent`（按 ID 引用）+ `Scene::world_matrix` 世界矩阵解算；批量解父子走 `Scene::hierarchy()` / `parent_indices()` / `child_map()`（O(n)，面板与渲染不再逐节点扫表）；层级面板是**可折叠的树**，拖拽重挂父节点保留世界变换并自动防环；渲染 / 物理 / Gizmo 全部走世界变换 |
| Undo / Redo | ✅ 完成 | 快照式撤销栈，Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y；Inspector 拖拽与 **Gizmo 拖拽**都按「拖拽前快照、松手入栈」记成**一步** |
| 复制 / 粘贴 | ✅ 完成 | Ctrl+C / Ctrl+V；剪贴板保存**整棵子树**快照，粘贴时重新分配 ID 并还原内部父子关系 |
| 预制体（Prefab） | ✅ 完成 | 把一棵子树存成 `Prefabs/*.prefab.ron` 资产并可反复实例化；实例带**来源标记**（资产路径 + 资产内实体 ID），支持 **Apply**（实例改动写回资产）与 **Revert**（资产覆盖回实例，保留位置）；编辑器有 Prefabs 资源面板 |
| 资源浏览器 | 🟨 部分 | 已有 **Prefabs 面板**（列资产 / 搜索 / 点击实例化 / 删除）；`Resources/` 里的模型尚未列出、也还不能拖拽入场景 |
| 多场景管理 | ⬜ 待做 | 新建 / 切换 / 按场景保存（当前只认清单里的入口场景；**Save As 已能把场景存成新文件并切过去**，缺的是新建空场景与多场景同时打开） |
| 物理射线 / 角色控制器 | ⬜ 待做 | `QueryPipeline` 已创建但未暴露 API；无角色控制器 |
| 游戏侧输入系统 | ✅ 完成 | `xgame-game` 已接 `InputMap`（`input.ron` 动作映射）与完整键鼠状态 —— 与编辑器 Play 共用 `xgame-input`；差距仅剩手柄 / 触摸 |
| 项目设置（Project Settings） | ✅ 完成 | `⚙ Project Settings` 窗口（项目 / 窗口 / 运行时 / 内容目录，草稿 + Apply 校验）；重力与固定步长进 `project.ron` 的 `runtime` 节，编辑器与游戏共用 `WorldConfig::from_project_runtime`（非法值自动夹安全），旧清单靠 serde 默认兼容 |
| 未保存保护 / Save As | ✅ 完成 | 脏标记（落盘快照比较 + 按帧 memo）、关窗 / 返回枢纽双路径确认框（保存 / 不保存 / 取消）、Save As 写新文件并切换入口场景、工具栏 Save 按钮、标题栏与工具栏 `*` 星号、Ctrl+S / Ctrl+Shift+S |
| 无头检查 `--check` | ✅ 完成 | `xgame-editor --check <项目>`：不开窗口验证清单 / 场景 / 资产引用，退出码契约 **0=干净 / 1=加载失败 / 2=悬空引用** —— CI 与 AI 智能体的可机读验证入口（见文末 Agent-Native） |
| 无头运行 `--headless` | ✅ 完成 | `xgame-editor --headless [--frames N] [--dt 秒] [--report 路径] [--dump-scene 路径] <项目>`：不开窗口跑 N 帧输出 JSON 统计（帧耗时分布 / 实体与刚体计数 / 脚本日志含 error 样本 / 非有限变换 / 音频状态），固定 dt 两次运行**逐位一致**；退出码 **0=干净跑完 / 1=加载失败或参数错误 / 2=跑完但观察到问题**。MCP `run_headless` 的底座，角色见 [`Docs/无头运行.md`](Docs/无头运行.md) |
| 阴影 / 天空盒 / 后处理 | 🟨 部分 | **方向光阴影贴图**（单盏、正交视锥按场景包围盒拟合、3×3 PCF、深度偏置 + 硬件斜率偏置）与 **MSAA 4×**（颜色多采样 → resolve，采样数按适配器能力选）已落地；**HDR 渲染路径 + ACES 色调映射 + 曝光**与 **Bloom 泛光**已落地（2026-10-19，见 PLAN.md 二十五 / 二十六）；天空盒、CSM、FXAA/TAA 仍待做 |
| Gizmo 吸附 / Rect 工具 | ⬜ 待做 | `gizmo_snap` 字段已在但未接线；Rect 工具无实现 |
| 编辑器交互稳定性 | ✅ 完成 | 拾取改「射线 × 世界 AABB」（旧的"包围球"被拉伸物体会连相机一起吞进去）；轴拖拽的参考线锚定在拖拽起点（修掉 `Δ_k = A_k − Δ_{k-1}` 的正反馈 —— "拖拽抽搐"的根因）；每帧收敛选中索引，堵住越界 panic |
| 每帧开销 / 可扩展性 | ✅ 完成 | 运行时热路径稳态零分配（`sync_bodies` 快路径、运动学/动态体早退、层级一次解算、`world_matrices` 链缓冲复用）；渲染模型矩阵批上传；编辑器指纹不再先解算世界矩阵。2000 实体下引擎自身开销 226 → 13 µs/帧（纯视觉）、1026 → 89 µs/帧（全静态）。**剩余瓶颈是 rapier 自身**（1000 个互挤动态体 ≈13 ms/帧），要再往前得动作业系统 / 减少接触岛 |
| 崩溃面 / 无界增长 | ✅ 完成 | `add_collider` 对未知 body 只警告不 panic；控制台消息表加上限（1000 条，成批丢最旧）；复核撤销栈（100 步上限）、模型/材质缓存（按模型路径有界）均无泄漏 |
| 窗口 / 交互延迟 | ✅ 完成 | 空闲时**完全不重绘**（丢掉位置未变的 `PointerMoved` —— Windows 指针静止时反复投同一坐标的 `WM_MOUSEMOVE`，egui 便一直请求立即重绘；实测空闲 CPU 5~7% → 0.00%）；拖窗口边框时**节流重建**离屏目标（重建次数 43 → 1）。这两条一起治好了"拖窗口卡顿、拖拽/缩放有延迟" |

---

## Agent-Native：AI 智能体深度操控引擎（战略方向 🟨 ①② 已落地）

**目标**：用户的 AI 智能体能**深度、精确、可验证地**操作本引擎做游戏开发 —— 读写项目、
运行检查、拿到可机读结果，而不是"截图点按钮"。**架构判决式：后续每个架构决策都要过一问 ——
「智能体没有鼠标时，这条路走得通吗？」**

已具备的地基：

- **数据即文本** —— 场景 / 预制体 / 清单 / 输入映射全是 RON 纯文本，可直接读写与 diff；
- **程序化验证** —— `xgame-editor --check <项目>`（退出码 0/1/2，秒级，可进 CI）；
- **程序化实跑** —— `xgame-editor --headless`（跑 N 帧出 JSON 统计，固定 dt 逐位可复现）；
- **程序化观察** —— `--headless --screenshot` 离屏渲染 PNG（默认顺光取景、同机逐字节可复现），
  MCP `run_headless(screenshot: true)` 以图片块回传画面 —— 智能体**看到**自己造的场景；
- **程序化配置** —— 重力 / 步长进 `project.ron`，改一行文本即调参；
- **单一运行时** —— 无头验证的行为与玩家实跑的完全一致（编辑器 Play == 游戏二进制）；
- **安全网** —— 419 条回归测试、每条先证红，智能体改引擎也有兜底。

进度与计划：**① MCP server 最小工具集 ✅ 已落地** —— 新 crate `xgame-mcp`
（`dist\xgame-mcp.exe`，stdio 按行 JSON-RPC）→ **② 无头运行 ✅ 已落地** ——
`--headless --frames N` 输出统计 JSON（帧耗时 / 实体 / 刚体 / 脚本日志 / 非有限
变换）；`--screenshot` 额外出图，MCP 侧 `run_headless` 透传（`screenshot: true`
以 MCP 图片块回传）；现共 7 个工具（project_info / scene_read /
scene_write / entity_upsert / entity_remove / project_check / run_headless），配
28 条回归测试与工具参考 [`Docs/MCP.md`](Docs/MCP.md)、[`Docs/无头运行.md`](Docs/无头运行.md)
→ **③ 场景 schema 导出**（`--export-schema`，机器可读的字段与类型）。
详细依据与完整矩阵：[`Docs/差距分析.md`](Docs/差距分析.md) 文末战略章。

---

## 阶段六·五：玩法层（本轮完成）

在"能摆场景"和"能做游戏"之间差着三样东西：**输入**、**脚本**、**一个统一的运行时**。
这一轮把它们补齐，并且刻意按"编辑器 Play 与打包游戏共用同一段实现"来做。

| 能力 | 状态 | 说明 |
|---|---|---|
| 输入系统 | ✅ 完成 | 新增 `xgame-input`：与 winit 解耦的 `KeyCode`/`MouseButton`、`InputState`（按住 / 本帧按下 / 本帧松开三级语义、失焦自动释放、按键重复去重）、`InputMap` 动作映射（项目级 `input.ron`，支持一个动作多绑定 / 修饰键组合 / 双向轴）。窗口库的翻译只在编辑器与游戏两个二进制里发生 |
| 脚本系统 | ✅ 完成 | 新增 `xgame-script`：嵌入式 Rhai，`Entity::scripts`（项目相对路径）→ 四个可选钩子 `start` / `update` / `fixed_update` / `on_destroy`；宿主 API 覆盖变换、实体增删、物理（射线 / 冲量 / 速度）、输入、时间、日志；`state_*` 提供**按 (实体, 脚本) 分开**的持久状态；编译缓存 + 热重载；脚本崩溃 / 死循环只记日志不拖垮引擎 |
| 统一运行时 | ✅ 完成 | 新增 `xgame-runtime::World`：把场景 / 物理 / 输入 / 脚本编排成确定的帧管线（固定步长与渲染帧解耦），**编辑器 Play 与独立游戏跑的是同一段 `update`**，消掉了此前两份各自维护的物理同步实现 |
| 声明式物理 | ✅ 完成 | `Entity::physics: Option<PhysicsBody>`（kind / shape / mass / friction / restitution / gravity_scale / sensor）。取代了"凡带模型即动态盒子"的旧行为；不声明 = 纯视觉实体。`PhysicsBody` → rapier 刚体的翻译收在 `xgame-physics::scene_body` |
| 物理射线 | ✅ 完成 | `PhysicsWorld::raycast` / `raycast_with_filter`（`RaycastFilter`：忽略传感器 / 只打动态体 / 排除自身）。脚本里是 `raycast(ctx, ...)` |
| 编辑器接入 | ✅ 完成 | Inspector 新增 **Scripts**（增删 / 从项目脚本列表选 / 重扫）与 **Physics**（类型 / 形状 / 形状参数 / 摩擦 / 弹性 / 质量 / 重力缩放 / 传感器）两个区块；Hierarchy 行加 `📜`（带数量）/`⚙` 徽标；工具栏加 `⟳ Scripts` 热重载；Console 直接收脚本日志；Play 时通过 `input_bridge` 把 egui 键鼠灌进运行时 |
| 音频系统 | ✅ 完成 | 移到**第八阶段**独立落地（2026-10）：`xgame-audio`（kira）+ `AudioSource` 组件 + 3D 空间音频 + 脚本 API，见下方「第八阶段」 |
| 角色控制器 | 🟨 部分 | 射线有了；基于射线的胶囊体移动 / 台阶 / 坡度尚未实现 |
| 后处理 / 阴影 | ⬜ 待做 | `Light::casts_shadow` / `shadow_resolution` 字段已进场景，渲染侧还没接 |

> **文档修正**：`Scene` 现在带 `lights` / `ambient` 字段（旧场景文件靠
> `#[serde(default)]` 自动补一盏默认方向光和一个默认环境光）；`Entity` 现在带
> `id` / `parent` / `prefab` 三个字段，**已支持父子层级与预制体实例** ——
> `transform` 存的是**局部**变换，世界变换由 `Scene::world_matrix` 沿父链解算；
> `prefab` 为 `None` 时不会写进文件，旧场景 / 旧清单（缺 `prefabs_dir`）
> 都能靠 serde 默认值原样加载。
>
> **已知缺口**：`crates/xgame-editor/src/editor_input.rs`（672 行）是一套
> 「输入绑定表 → 动作」的抽象，**从未接入** —— `EditorState::input_manager` 只构造、
> 没人读，实际输入仍散在 `scene_view` / `main` 里。模块自带 4 个单测，属有意保留的
> 脚手架，已整体 `#![allow(dead_code)]`；接入前要先修它内部三处（拖拽增量滞后一帧、
> 修饰键全等比较会让"不带修饰键"的绑定在按住 Ctrl 时整批失效、两个字段写了没人读）。
>
> **独立播放器的循环已迁到 winit 0.30 的正式入口**：`EventLoop::run(闭包)` 与
> `EventLoop::create_window` 都已废弃，现在是 `run_app` + `ApplicationHandler`，
> 窗口与 `GameApp` 都推迟到 `resumed` 里创建（`ActiveEventLoop::create_window`
> 只在回调内可见）。`about_to_wait` 主动 `request_redraw()` 的连续帧机制不变。

## 双轨语言架构：P0 ✅ P0.5 ✅ P1 ✅ P1.5 ✅ P2 ✅ P2.5 ✅ P3 ✅

Rust（UE 插件模式）+ Lua（Unity 模式 + 热更）双轨；**P3 起脚本层收敛为单一
Lua 轨**（Rhai 已删）。计划与决策记录：
[`Docs/双轨语言架构计划.md`](Docs/双轨语言架构计划.md)；实施记录：PLAN.md 十五 / 十六 / 十七 / 十八 / 十九 / 二十 / 二十一。
**P2.5 闸门判定：Lua 三负载全占优 → P3 删 Rhai**（数据见 PLAN.md 二十），
**P3 已执行完毕**（落地注记见计划文档 D11，实施记录见 PLAN.md 二十一）。

| P0 交付 | 状态 | 说明 |
|---|---|---|
| `xgame` 门面 crate | ✅ | 游戏 / 插件代码的唯一入口：prelude 拍平深路径 + `ENGINE_VERSION`（版本握手基准） |
| `GameCtx` 方法层 | ✅ | api.rs 约 750 行函数体平移为 `impl GameCtx` 方法；脚本 API 与组件 API 是同一套方法的两个投影 |
| Rust 组件 | ✅ | `Lifecycle` 四钩子（与 Rhai 同名同语义）+ `BehaviorRegistry` + `Entity::behaviors`（serde 兼容旧场景文件） |
| 组件分发 | ✅ | `World` 帧管线的平行路径（字段级拆借不动）；钩子函数装配期拷进实例，热路径零哈希 |
| 插件边界 | ✅ | cdylib + `export_plugin!` 宏 + 版本握手（不匹配拒载、报错可读）；示例 `examples/spinner-plugin` 端到端跑通四钩子 |
| 微基准 | ✅ | 1000 no-op 组件实体分发 **37.6 µs/帧**（验收线 50）；对照 Rhai 脚本轨 493.9 µs = **13.1×**。⚠ 这是 **P0 当时机器状态**下的数字：绝对值随睿频 / 后台负载浮动 1.5× 以上，跨版本比较必须用**同机背靠背对照**（记录表见 `crates/xgame-runtime/tests/behavior_dispatch_throughput.rs` 模块文档） |

| P0.5 交付 | 状态 | 说明 |
|---|---|---|
| 插件项目探测 + 新鲜度 | ✅ | 项目根有 `Cargo.toml` 即插件项目；纯时间戳比较的纯函数，新鲜就**连 cargo 都不启动** |
| 后台构建 | ✅ | `cargo build --message-format=json-render-diagnostics` 跑在工作线程，mpsc 回传；产物路径从 JSON 解析，诊断直接进 Console；profile 跟随编辑器 |
| 产物同步 + 加载 | ✅ | 同步进规范目录 `<项目>/Plugins/`（玩家与打包器只认这里）；加载走**临时副本**绕开 Windows 文件锁；旧句柄永不卸载 |
| Play 状态机 | ✅ | 按 ▶ 过期 ⇒ 转后台编译 + 自动进 Play；失败留在编辑态、不阻塞 Play；工具栏转圈 + 秒数 + 手动 Build |
| 编辑器 UI | ✅ | Inspector 的 **Behaviors** 区（与 Scripts 同构，未注册名标红 ⚠）、工具栏编译指示器 |
| 玩家 / 打包器 | ✅ | `xgame-game` 加载 `Plugins/`；打包器复制 `Plugins/` 并在"有 Cargo.toml 却没 DLL"时把警告塞进构建报告 |
| 演示项目 | ✅ | [`projects/BehaviorsDemo`](projects/BehaviorsDemo) —— **项目本身就是 cdylib crate**，场景里 Rust 组件与 Rhai 脚本同款行为共存 |

| P1 交付 | 状态 | 说明 |
|---|---|---|
| `Inspect` 契约 | ✅ | `xgame-core::inspect`：`FieldDesc`（`'static` 描述）/ `FieldKind`（10 种）/ `FieldValue`（拥有值）/ trait `Inspect`（表 + 下标，两个对象安全方法，热路径零哈希）；类型不符的写入静默忽略、越界读返回类型零值 —— 检视是旁路，一个下标算错不该拖垮宿主 |
| `#[derive(Inspect)]` | ✅ | `xgame-derive`；表 / get / set 三处下标**由同一个变量生成**，结构上不可能错位（手写最常犯的错就是三处错位，表现是"改 A 字段动了 B 字段"且编译器不响）；字段属性 `skip / label= / min= / max= / speed= / step= / unit= / tooltip=`；`label` 只改显示名，寻址名永远是 Rust 字段名 |
| 组件契约升级 | ✅ | `pub trait Behavior: Lifecycle + Inspect {}`（blanket impl）。编辑器不链接用户代码，拿 `&mut dyn Inspect` 只能靠实例自带的字段表 —— 存 `Box<dyn Lifecycle>` 时这个降级做不到（需 downcast，跨 DLL 无共享类型信息）；`impl_no_inspect!` 是"不开放字段"的一行逃生舱 |
| Inspector 自动渲染 | ✅ | Behaviors 区按字段表画控件：有 `min+max` 给**滑块**（不强加作者没声明的范围），否则给裸拖拽值；`Bool` 勾选框、`String` 单行文本、`Vec2/3/4` 分量拖拽行；悬停先列出该组件有哪些参数 |
| Edit 模式调参 | ✅ | `EditorState.edit_components` 惰性构造的**纯数据副本**：拖滑块 → 按 ▶ → 逐字段灌进刚启动的运行时（不换实例，装配状态不作废）；换组件类型即作废重建（避免把 A 的参数写进 B 的字段）；停止 Play 丢弃 Play 中的改动（Unity 默认语义） |
| Play 热调参 | ✅ | Play 中改的是 World 里**跑着的那份** —— 拖一下下一次 `update` 就生效；`GameRuntime::behavior_at_mut` 是面板拿到活实例的唯一通道（不暴露整个 `World`） |

| P1.5 交付 | 状态 | 说明 |
|---|---|---|
| 参数落盘数据模型 | ✅ | `Entity::behavior_params: BTreeMap<String, FieldValues>`（**注册名** → 字段寻址名 → 值）。键用注册名而非数组下标 —— 下标会错位，而参数是持久化数据，错位写参表现为"改 A 组件的参数写进了 B 组件"且两边都是 `f32` 时无从发现；`BTreeMap` 有序保证场景 diff 不抖 |
| serde 格式 | ✅ | `FieldKind` / `FieldValue` 补 `Serialize + Deserialize`，**变体标签即磁盘格式**（`field_value_ron_variant_labels_are_stable` 钉死）；改标签名等于作废所有旧场景 |
| 装配期套用 | ✅ | `World::instantiate_behaviors` 按注册名查参数表并套用，注册不到的名字**静默跳过**（与"参数表只是数据"取舍一致）；行为字段级拆借未动 |
| 编辑副本种子 | ✅ | `inspect_component` 构造编辑态副本时种子场景参数 —— Edit 模式打开 Inspector 看到的就是场景真实值而非组件默认值 |
| 撤销语义推翻 | ✅ | 组件字段编辑接上场景撤销栈（拖拽挂 `undo_pending`、松手入栈、离散编辑当场 push），与 Name/Transform 同通道 —— 推翻 P1"不进撤销栈"（参数已成场景数据，不进栈会凭空丢一条场景编辑） |

| P2 交付 | 状态 | 说明 |
|---|---|---|
| Lua 宿主 | ✅ | `lua_host.rs` + `lua_api.rs`（mlua 0.10：`lua54` + `vendored` + `send`）：每脚本环境表沙箱（`__index` 必须挂**真元表**，普通字段无效）、指令计数沙箱（每 1024 条指令查预算、200_000 中断本次调用）、StdLib 裁剪（无 io/os/package/debug）、四钩子与 Rhai 同名同语义、顶层 `local` 脚本级共享；`ScriptCtx<S>` 泛型化，Rhai 侧零改动 |
| 两轨分流 | ✅ | `.lua` → LuaHost、其余 → Rhai（`is_rhai = !is_lua`，两轨恰好全覆盖）；`World` 帧管线依次派发两轨，日志/命令缓冲合并不共享指针；证红测试 `rhai_host_skips_lua_scripts` |
| 路径驻留表 | ✅ | 修两宿主每帧 3 处路径 `String` 克隆 → `(EntityId, u32)`（`xgame-script/src/intern.rs`）：探针量得上限 ≈110 µs/帧，端到端 Rhai 轨分发 **819.6 → 713 µs/帧（同机背靠背 -13%）**；驻留 id 永不回收、`invalidate*` 只清编译缓存（双宿主 `state_survives_hot_reload_*` 钉死，红对照 = "热重载清空驻留表"实测红） |
| 编辑器接入 | ✅ | `list_scripts` 双轨扫描（证红）+ 工具栏/Inspector/状态层三处文案 + 热重载链双 `invalidate_all`；MCP 项目信息同源生效；`xgame-editor --check` OK |

分期已全部完成：**P2.5** 基准闸门 ✅（判定：Lua 三负载全占优 → P3 删 Rhai）→
**P3** 按闸门结论删除 Rhai 宿主、样例 / 文档切 Lua ✅。

| P2.5 交付 | 状态 | 说明 |
|---|---|---|
| 闸门工具 | ✅ | `language_gate.rs`：三负载同进程背靠背（dispatch 1000 实体 / api 500 实体 ×20 组 / compute 200 实体 ×1000 循环），脚本两轨同源同循环数（`format!` 从同一组常量拼）；`measure()` 错误日志守卫 —— 脚本没真跑时直接 panic，不许假数据端到端通过 |
| 判定 | ✅ | **Lua 三负载全占优：dispatch 1.31×、api 1.26×、compute 9.16× → 按 §六 规则 P3 删 Rhai**。首跑曾出假数据（Rhai 无 `mut` 关键字 → 脚本编译失败 → 空调度被当"快"），守卫堵住后重跑（复盘见 PLAN.md 二十） |
| Lua 热路径修复 | ✅ | 闸门暴露负载 1 Lua 慢 2.25×，探针量证两处实现开销并修掉：钩子函数句柄**编译期缓存**（`detect_funcs` 留下 `Option<Function>`，省每钩子 `env.get` ~194 ns）+ ctx 改**宿主常驻槽轻量指针传参**（省每钩子现造 userdata ~284 ns）→ 负载 1 Lua **1670 → 576 µs/帧（-65%）**，行为回归 34 条全绿 |

| P3 交付 | 状态 | 说明 |
|---|---|---|
| 删 Rhai 宿主 | ✅ | `host.rs` 瘦身为宿主公共类型模块（`Hook` / `PreloadReport` / `CommandReport`，Lua 宿主一直共用）；`api.rs`（Rhai 转发壳）删除；workspace `rhai` 依赖移除；`ScriptCtx<S>` 泛型收成非泛型（状态表直接持 `mlua::Table`，地址束模式与 unsafe 窗口约定原样保留） |
| 单轨化 | ✅ | `World` 只持一个 `scripts: LuaHost`（原双字段合一），日志 / 命令缓冲 / 热重载 / 预载走这一份，两轨缓冲合并代码删除；`list_scripts` 只扫 `.lua`（证红测试更新为"残留 `.rhai` 不进下拉"）；编辑器三处双轨文案收敛 |
| 闸门退役 | ✅ | `language_gate.rs` 删除（一次性使命完成，判定数据已入档计划文档 §六 / PLAN.md 二十）；`behavior_dispatch_throughput` 去 Rhai 档（基线 / 组件 / Lua 三档，表中 Rhai 列留作历史） |
| 样例迁移 | ✅ | 4 个 `.rhai` 示例移植 `.lua`（Spinner / Jumper / FoxLocomotion / BehaviorsDemo Spinner），场景 ron 引用同步改；`math.sin` 走沙箱 MATH 库，`sample_project_runs` 覆盖验证 |
| 文档迁移 | ✅ | `Docs/脚本API.md` 整篇改写为 Lua 版（"两个语法坑"收敛为"顶层 `local` 脚本级共享 + 每实体状态走 `state_*`"的语义说明）；README / 差距分析 / 架构图同步；API 名（含 `spawn_entity` 后缀）一字不改 |

P0.5 遗留（有意延后）：Play 中途热换插件、组件徽章、release profile 插件出货（P3）、
cargo 构建超时。

P1 遗留（有意延后）：`Quat` / `Color` 未进 `FieldKind`（序列化变体随之受限）、
无 `trybuild`（"类型不支持又没标 `skip` 就报错"这条无法断言）。

P1.5 遗留（有意延后）：**prefab 的组件字段**（prefab 目前只覆盖 Transform 等基础
数据，待 prefab 扩展到组件层时一起）、Play 中热调参不写回（既有语义，"Play 中调参
也持久化"需另开决策）。

P2 遗留（有意延后）：`dispatch_destroy` / `forget_entity` 未接线（双轨时期的
对称遗留，P3 删 Rhai 后只剩 Lua 宿主这一份，等运行时真需要销毁钩子时一起）；
编辑器无"新建脚本"功能（放文件进 `Scripts/` 即被扫描）。~~Lua 轨每帧性能未实测~~ → P2.5 闸门已出数
（dispatch 562 µs/帧，见基准文档 P2.5 列）。

## 第七阶段：动画系统 🟨

前五片已落地：

- [x] 骨骼蒙皮（GPU skinning：顶点着色器蒙皮分支 + 骨骼存储缓冲按槽分段，
      阴影 pass 同步蒙皮；每实体一个蒙皮槽，`ModelUniform.params.x` = 槽位 + 1）
- [x] 动画片段加载（glTF animation → `AnimClip`/`Track`，LINEAR/STEP 运行时插值，
      CUBICSPLINE 读入时降采样取中段值）
- [x] 关键帧插值（位置/缩放线性、旋转 slerp；循环取模、非循环钳到 duration）
- [x] `Animator` 组件（`clip` / `speed` / `looping` / `autoplay` / `fade_time`，
      Entity 一等字段，旧场景文件缺字段读作默认值）
- [x] 播放时钟 `AnimationDriver`（编辑器墙钟推进、游戏按帧 dt 推进，换剪辑归零）
- [x] 编辑器：Inspector 的 Animation 区块（剪辑下拉 / Speed / Fade / Loop / Autoplay /
      摘除）、指纹与帧策略把动画算进去（有剪辑在播 → 持续出帧；暂停 → 回到静态帧门控）
- [x] 换剪辑交叉淡入（`fade_time` 默认 0.25s：旧剪辑连同它自己的时钟一起淡出，
      两份局部姿态按权重混合 —— 平移 lerp / 旋转 slerp；暂停中切换或 `fade_time = 0`
      硬切；淡入中连切两刀重起淡入、不叠混合栈）
- [x] 动画状态机（`Animator.machine`：浮点参数表 + 阈值转移表（AtLeast ≥ / Below <，
      进出阈值配对留滞回）+ 状态表（每状态自己的 clip/speed/looping）；每帧一步转移、
      换状态复用换剪辑交叉淡入；坏配置全部安全退回单剪辑；参数随场景落盘，
      脚本 `set_anim_param` / Inspector 均可推；示例 FoxLocomotion.lua）
- [x] 叠加（additive）混合 v1（`Animator.additive`：增量 = 叠加剪辑当前帧 − 其第 0 帧
      参考姿势，按 weight 加权；平移/缩放线性差、旋转按权重 slerp 局部增量；叠加层
      独立时钟/循环，与状态机正交；淡入中照叠；权重 0 与未配置逐位一致；脚本
      `set_anim_additive_weight` / Inspector 均可推权重；示例：Fox 把 Run 的增量叠在
      任意基础状态上，权重 sin 振荡）
- [x] 多层混合栈与骨骼遮罩 v1（`Animator.layers`：每层 `name + mode(Additive/Override)
      + clip + weight + speed + looping + mask`，按数组顺序依次合到当前已合成的姿势上、
      后层盖前层；遮罩 = **骨骼子树根名单**（`AnimNode` 补 `name`，留空 = 整副骨架），
      Override 配遮罩 = 分区域重定向（走路上半身张望、腿照走）；每层独立时钟按名对齐
      （重排不丢进度、换本层剪辑只归零自己），全部时钟进渲染指纹；管线统一成
      `compute_bones_composed` 一条路（旧 `compute_bones*` 四函数变薄封装、老测试当
      回归网）；`additive` 快捷字段保留、内部先求值；脚本 `set_anim_layer_weight(ctx,
      name, w)` 按名推权重；示例：Fox 走路段上半身盖 Survey（mask: b_Spine01_02））
- [ ] morph target（表情等）；逐条转移时长、分层状态机（Phase B）

> v1 明确不含：多层混合栈、骨骼遮罩、真倒放（负速 = 停在第 0 帧）、逐条转移时长（统一 fade_time）。
> 每个模型 v1 取第一个带蒙皮的图元、每实体一张皮肤，超过 256 骨的骨架解析时告警截断。

## 第八阶段：音频系统 ✅

- [x] kira 集成（`xgame-audio` crate：`AudioManager` 默认后端 + 解码缓存；
      设备打开失败**无声降级** —— 照常受理命令 / 计数 / GC，只是不出声）
- [x] AudioSource 组件（`Entity::audio` 一等字段：clip / volume / pitch /
      looping / autoplay / spatial / min-max distance，Inspector 可视化编辑，
      参数随场景 RON 落盘）
- [x] 3D 空间音频（kira spatial sub-track：距离衰减 + 每帧推发射位置；
      实体绑定 `follow`，组件摘除 / 实体删除自动停音）
- [x] 听者跟随（编辑器 Play 每帧 `set_listener(编辑器相机)`；脚本可
      `set_listener` 覆盖；主音量可调）
- [x] 脚本 API（`play_sound` / `play_sound_at` / `stop_sound` /
      `stop_all_sounds` / `set_listener` / `set_master_volume`）
- [ ] 混音总线分组（BGM / SFX 独立音量）、流式长音频、音频遮挡 / 低通 —— 有意延后

> 架构与取舍记录见 PLAN.md 二十二。

## 第九阶段：后处理 + 阴影 🟨

- [x] 方向光阴影贴图（单盏、正交视锥拟合、PCF 3×3）
- [ ] 级联阴影贴图（CSM，覆盖远处）
- [ ] 点光阴影（6 面立方体贴图）
- [x] ToneMapping（ACES）+ HDR 渲染路径 + 曝光（场景 pass → `Rgba16Float` 中间标
      → 曝光 + ACES → LDR；`project.ron` 的 `render` 节，三宿主同源；`Off` 模式
      = 旧管线兼容出口。2026-10-19，见 PLAN.md 二十五）
- [x] Bloom 泛光（亮部提取软膝 + 三级 mip（½/¼/⅛）+ 可分离 9 抽头二项高斯 +
      合成权重 [0.5,0.3,0.2]；阈值在曝光后判、按最大通道；**默认关**
      （intensity 0，整段跳过），开启才惰性建链；`project.ron` 的
      `render.bloom` 三参数，2026-10-19，见 PLAN.md 二十六）
- [ ] RenderGraph 构建（等出现带依赖分叉的 pass 再抽，见 PLAN.md 二十五·二）
- [ ] FXAA / TAA
- [ ] 天空盒

## 第十阶段：脚本 + 热重载 ⬜

- [ ] 脚本调度系统
- [ ] Rust 脚本热重载（动态库加载）
- [ ] 或嵌入 Lua/Rhai 解释器
- [ ] 生命周期回调（Start / Update / OnDestroy）

---

## 当前项目结构

```
XGameEngine/                       # 引擎源码仓库
├── 一键构建.cmd                   # 一键构建入口（双击即可）
├── Cargo.toml                     # workspace（含 xgame-editor / xgame-game）
├── tools/
│   └── build-release.ps1          # 发布构建：编译 + 收集产物到 dist/
├── projects/                      # 游戏项目（默认位置，可放到任意目录）
│   └── SampleScene/               # 示例项目
│       ├── project.ron            # 项目清单
│       ├── Scenes/
│       │   └── scene.ron
│       ├── Prefabs/
│       │   └── Pillar.prefab.ron  # 预制体资产（示例）
│       └── Resources/
│           ├── Box.glb
│           ├── Fox.gltf
│           ├── Fox.bin
│           ├── Texture.png
│           └── Audio/Beep.wav    # 示例音频（Spinner 空间化循环蜂鸣 / Jumper 起跳音）
├── crates/
│   ├── xgame-core/                # 核心类型
│   │   └── src/
│   │       ├── color.rs
│   │       ├── config.rs
│   │       ├── scene.rs           # Scene, Entity
│   │       ├── prefab.rs          # Prefab, PrefabInstanceRef（新）
│   │       ├── transform.rs       # Transform
│   │       ├── component.rs       # ComponentMap
│   │       ├── id.rs              # EntityId
│   │       ├── time.rs            # GameClock
│   │       ├── asset.rs           # AssetId
│   │       ├── project.rs         # Project / ProjectHandle（新）
│   │       └── check.rs           # check_project：无头验证（--check 与 MCP 共用，新）
│   ├── xgame-math/                # 数学库
│   │   └── src/
│   │       ├── projection.rs
│   │       └── transform.rs
│   ├── xgame-derive/              # proc-macro（编辑器 Panel 派生）
│   ├── xgame-script/              # Lua 脚本宿主（GameCtx 方法层在此，lua_api.rs 为转发壳）
│   ├── xgame-audio/               # 音频引擎（kira 封装：设备 / 空间轨道 / 解码缓存 / 无声降级）
│   ├── xgame-runtime/             # 统一运行时 World（帧管线 + Rust 组件分发 + 注册表）
│   ├── xgame/                     # 门面 crate（prelude + ENGINE_VERSION + 插件边界）
│   ├── xgame-render/              # 渲染器
│   │   └── src/
│   │       ├── lib.rs             # RenderContext
│   │       ├── camera.rs          # Camera + OrbitCamera
│   │       ├── mesh.rs            # Mesh 工具
│   │       ├── gltf_loader.rs     # glTF 加载器
│   │       ├── texture.rs         # GPU 纹理
│   │       ├── model.rs           # ModelUniform
│   │       ├── scene.rs           # 重新导出
│   │       └── scene_pipeline.rs  # 共享渲染管线
│   ├── xgame-physics/             # 物理引擎
│   │   └── src/
│   │       ├── lib.rs             # crate root
│   │       ├── rigid_body.rs      # RigidBody
│   │       ├── collider.rs        # Collider
│   │       ├── world.rs           # PhysicsWorld
│   │       └── debug_draw.rs      # DebugDrawBuffer
│   ├── xgame-editor/              # 编辑器（egui）
│   │   └── src/
│   │       ├── main.rs            # 入口 + 枢纽/编辑器两态（改）
│   │       ├── hub.rs             # 项目枢纽：新建 / 打开 / 最近（新）
│   │       ├── recents.rs         # 最近项目持久化（新）
│   │       ├── builder.rs         # 项目构建：打包成独立游戏（新）
│   │       ├── dialogs.rs         # 应用级弹窗：未保存确认 / 另存为（新）
│   │       ├── state.rs           # 中央状态（持有 ProjectHandle）
│   │       ├── scene_renderer.rs  # 离屏渲染（按项目根解析模型）
│   │       ├── game_runtime.rs    # Play 模式运行时
│   │       ├── editor_input.rs    # 输入管理
│   │       ├── gizmo/             # Gizmo 变换操作柄
│   │       └── panels/
│   │           ├── mod.rs
│   │           ├── dock.rs        # 面板布局
│   │           ├── toolbar.rs     # 顶部工具条（含 ⌂ Projects / 💾 Save / ⚙）
│   │           ├── hierarchy.rs   # 实体层级树
│   │           ├── prefabs.rs     # 预制体资源面板（新）
│   │           ├── inspector.rs   # 属性编辑
│   │           ├── settings.rs    # 项目设置窗口（新）
│   │           ├── scene_view.rs  # 3D 视口
│   │           ├── console.rs     # 控制台
│   │           └── theme.rs       # 暗色主题
│   ├── xgame-mcp/                 # MCP server：AI 智能体工具面（新）
│   │   └── src/
│   │       ├── main.rs            # stdin/stdout 管道（唯一碰 I/O 的一层）
│   │       ├── lib.rs             # 协议层：handle_line() 按行 JSON-RPC + 两层错误
│   │       └── tools.rs           # 6 工具：project_info / 场景读写 / 实体增删 / 检查
│   └── xgame-game/                # 独立游戏
│       └── src/
│           └── main.rs            # 游戏入口（参数 > exe 同目录项目 > 示例项目）
├── examples/
│   └── spinner-plugin/            # 示例 cdylib 插件（与 Spinner.lua 同款的 Rust 组件）
└── README.md
```

## 运行命令

```powershell
# 编辑器 —— 打开项目枢纽
cargo run -p xgame-editor

# 编辑器 —— 直接打开指定项目
cargo run -p xgame-editor -- projects/SampleScene

# 无头项目检查（不开窗口；退出码 0=干净 / 1=加载失败 / 2=悬空引用）
cargo run -p xgame-editor -- --check projects/SampleScene

# 无头运行（不开窗口跑 N 帧，stdout 出 JSON 统计；退出码 0=干净 / 1=加载失败 / 2=跑完有问题）
cargo run -p xgame-editor -- --headless --frames 300 projects/SampleScene

# MCP server —— AI 智能体接口（stdio 按行 JSON-RPC；工具参考 Docs/MCP.md）
cargo run -p xgame-mcp

# 独立游戏 —— 运行指定项目（省略参数时默认 projects/SampleScene）
cargo run -p xgame-game -- projects/SampleScene

# 一键构建发布包到 dist\（等价于双击根目录的「一键构建.cmd」）
powershell -NoProfile -ExecutionPolicy Bypass -File tools\build-release.ps1
```

编辑器内的「🔨 Build」按钮则是把**当前项目**打包到 `<项目目录>/Build/<项目名>/`，
产物里的 exe 双击即玩。
