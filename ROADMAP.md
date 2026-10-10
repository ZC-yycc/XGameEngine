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
| **七** | ⬜ 待开始 | 动画系统 |
| **八** | ⬜ 待开始 | 音频系统 |
| **九** | 🟨 进行中 | 后处理 + 阴影（方向光阴影已落地；CSM / Bloom / ACES / 天空盒待做） |
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
| 多场景管理 | ⬜ 待做 | 新建 / 切换 / 按场景保存（当前只认清单里的入口场景） |
| 物理射线 / 角色控制器 | ⬜ 待做 | `QueryPipeline` 已创建但未暴露 API；无角色控制器 |
| 游戏侧输入系统 | ⬜ 待做 | 独立游戏只认 Close / Resize / Esc，没有键鼠抽象与动作映射 |
| 阴影 / 天空盒 / 后处理 | 🟨 部分 | **方向光阴影贴图**（单盏、正交视锥按场景包围盒拟合、3×3 PCF、深度偏置 + 硬件斜率偏置）与 **MSAA 4×**（颜色多采样 → resolve，采样数按适配器能力选）已落地；天空盒、CSM、Bloom、ACES、FXAA/TAA 仍待做 |
| Gizmo 吸附 / Rect 工具 | ⬜ 待做 | `gizmo_snap` 字段已在但未接线；Rect 工具无实现 |
| 编辑器交互稳定性 | ✅ 完成 | 拾取改「射线 × 世界 AABB」（旧的"包围球"被拉伸物体会连相机一起吞进去）；轴拖拽的参考线锚定在拖拽起点（修掉 `Δ_k = A_k − Δ_{k-1}` 的正反馈 —— "拖拽抽搐"的根因）；每帧收敛选中索引，堵住越界 panic |
| 每帧开销 / 可扩展性 | ✅ 完成 | 运行时热路径稳态零分配（`sync_bodies` 快路径、运动学/动态体早退、层级一次解算、`world_matrices` 链缓冲复用）；渲染模型矩阵批上传；编辑器指纹不再先解算世界矩阵。2000 实体下引擎自身开销 226 → 13 µs/帧（纯视觉）、1026 → 89 µs/帧（全静态）。**剩余瓶颈是 rapier 自身**（1000 个互挤动态体 ≈13 ms/帧），要再往前得动作业系统 / 减少接触岛 |
| 崩溃面 / 无界增长 | ✅ 完成 | `add_collider` 对未知 body 只警告不 panic；控制台消息表加上限（1000 条，成批丢最旧）；复核撤销栈（100 步上限）、模型/材质缓存（按模型路径有界）均无泄漏 |
| 窗口 / 交互延迟 | ✅ 完成 | 空闲时**完全不重绘**（丢掉位置未变的 `PointerMoved` —— Windows 指针静止时反复投同一坐标的 `WM_MOUSEMOVE`，egui 便一直请求立即重绘；实测空闲 CPU 5~7% → 0.00%）；拖窗口边框时**节流重建**离屏目标（重建次数 43 → 1）。这两条一起治好了"拖窗口卡顿、拖拽/缩放有延迟" |

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
| 音频系统 | ⬜ 待做 | 尚未开始（`ContentConfig::audio_dir` 字段已在） |
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

## 第七阶段：动画系统 ⬜

- [ ] 骨骼蒙皮（GPU skinning）
- [ ] 动画片段加载（glTF animation）
- [ ] 关键帧插值
- [ ] 动画混合（过渡、叠加）
- [ ] 动画状态机（Animator）

## 第八阶段：音频系统 ⬜

- [ ] kira / rodio 集成
- [ ] AudioSource 组件
- [ ] 3D 空间音频
- [ ] 混音器

## 第九阶段：后处理 + 阴影 🟨

- [x] 方向光阴影贴图（单盏、正交视锥拟合、PCF 3×3）
- [ ] 级联阴影贴图（CSM，覆盖远处）
- [ ] 点光阴影（6 面立方体贴图）
- [ ] RenderGraph 构建
- [ ] Bloom 后处理
- [ ] ToneMapping（ACES）
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
│           └── Texture.png
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
│   │       └── project.rs         # Project / ProjectHandle（新）
│   ├── xgame-math/                # 数学库
│   │   └── src/
│   │       ├── projection.rs
│   │       └── transform.rs
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
│   │       ├── state.rs           # 中央状态（持有 ProjectHandle）
│   │       ├── scene_renderer.rs  # 离屏渲染（按项目根解析模型）
│   │       ├── game_runtime.rs    # Play 模式运行时
│   │       ├── editor_input.rs    # 输入管理
│   │       ├── gizmo/             # Gizmo 变换操作柄
│   │       └── panels/
│   │           ├── mod.rs
│   │           ├── dock.rs        # 面板布局
│   │           ├── toolbar.rs     # 顶部工具条（含 ⌂ Projects）
│   │           ├── hierarchy.rs   # 实体层级树
│   │           ├── prefabs.rs     # 预制体资源面板（新）
│   │           ├── inspector.rs   # 属性编辑
│   │           ├── scene_view.rs  # 3D 视口
│   │           ├── console.rs     # 控制台
│   │           └── theme.rs       # 暗色主题
│   └── xgame-game/                # 独立游戏
│       └── src/
│           └── main.rs            # 游戏入口（参数 > exe 同目录项目 > 示例项目）
└── README.md
```

## 运行命令

```powershell
# 编辑器 —— 打开项目枢纽
cargo run -p xgame-editor

# 编辑器 —— 直接打开指定项目
cargo run -p xgame-editor -- projects/SampleScene

# 独立游戏 —— 运行指定项目（省略参数时默认 projects/SampleScene）
cargo run -p xgame-game -- projects/SampleScene

# 一键构建发布包到 dist\（等价于双击根目录的「一键构建.cmd」）
powershell -NoProfile -ExecutionPolicy Bypass -File tools\build-release.ps1
```

编辑器内的「🔨 Build」按钮则是把**当前项目**打包到 `<项目目录>/Build/<项目名>/`，
产物里的 exe 双击即玩。
