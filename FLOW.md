# XGameEngine 运行流程

> 从启动到关机，每一帧发生了什么。
>
> 入口：`cargo run -p xgame-editor`（项目枢纽）| `cargo run -p xgame-editor -- <项目路径>`
> | `cargo run -p xgame-game -- <项目路径>`
>
> 编辑器与游戏都先**打开一个项目**（`project.ron`），再从中取场景与资产。

---

## 一、启动阶段

### 编辑器 (`xgame-editor`)

编辑器二进制有**两个界面**：项目枢纽（没打开项目时）与编辑器主界面。启动时：

```
main()                           crates/xgame-editor/src/main.rs
  └─ eframe::run_native(...)     egui 原生框架 (winit + wgpu)
       └─ EditorApp::new(cc)     初始化 → 停在「项目枢纽」
```

`EditorApp::new` 执行：

```
GpuHandles::from(cc.wgpu_render_state)
  └── 保留 device / queue / egui 渲染器    （打开项目后才用来构造视口渲染器）
ProjectHub::new()
  ├── RecentProjects::load()              %APPDATA%\XGameEngine\recent_projects.ron
  └── 默认新建位置 = <cwd>/projects（cwd 不像引擎根目录时退回用户文档目录）
（若命令行给了项目路径）app.hub.try_open(path) → 直接进编辑器，跳过枢纽
```

**界面切换**（`EditorApp::update`）：

```
session == None  ──▶ 项目枢纽 ProjectHub::ui(ctx)
                        └── HubAction::Open(project)
                              └── EditorApp::open_project(project)   →  进入编辑器

session == Some  ──▶ 编辑器界面 editor_ui(session, ctx)
                        └── 工具栏「⌂ Projects」→ state.return_to_hub = true
                              └── session 置 None  →  回到枢纽
```

`EditorState::new(project)` 执行：

```
project.load_entry_scene()        路径 = 项目根 / project.entry_scene
  ├── 成功 → 解析为 Scene { name, entities }
  └── 失败 → xgame_core::project::default_scene()  (Main Camera + Directional Light)
```

`EditorApp::open_project` 接着构造**视口渲染器**，并把**项目根目录**交给它：

```
scene_renderer::SceneRenderer::new(device, queue, egui_renderer, format, project_root)
  └── ScenePipeline::new()        共享渲染管线（位于 xgame-render）
```

> 3D 视口渲染在 **UI 线程上同步进行**：本帧输入处理完 → 重画离屏 3D → 同帧上屏。
> 编辑器视口必须让"输入到像素"零管线延迟（拖 gizmo、转视角时物体要钉在光标下），
> 而输入指纹门控让静态帧整帧跳过、稳态渲染实测单帧 CPU 仅 ~0.4ms —— 同步渲染
> 又快又没有第二个线程与窗口的模态移动循环抢资源。
> （2026-10-12 曾短暂改为独立渲染线程 + 有界等待，实测引入一帧滞后与 UI 阻塞，已回退。）

> 项目枢纽的「新建项目」走 `ProjectHandle::create`：建 `Scenes/`、`Resources/`，
> 写 `project.ron` 与默认场景「打开」走 `ProjectHandle::open`（读 `project.ron`）。
> 原生目录选择器来自 `rfd`。

### 独立游戏 (`xgame-game`)

```
main()                           crates/xgame-game/src/main.rs
  ├── resolve_project()                  三级定位（见下）
  │     ├── ① argv[1]                    命令行参数
  │     ├── ② exe 同目录的 project.ron   ← 编辑器 Build 产出的游戏走这条
  │     └── ③ projects/SampleScene       仓库内 cargo run 的兜底
  ├── ProjectHandle::open(...)           读 project.ron
  └── EventLoop::new()
       └── GameApp::new(&window, project)  异步初始化
             ├── wgpu Instance → Adapter → Device + Queue
             ├── Surface → SurfaceConfiguration（vsync 取自项目清单）
             ├── ScenePipeline::new()
             ├── project.load_entry_scene()
             └── PhysicsWorld::new() + entity→body 映射
```

> ② 是「把项目做成软件」的关键：构建产物把 `<项目名>.exe` 和 `project.ron` 放在同一个
> 目录里，于是双击 exe 就能跑，不需要参数，也不需要知道项目在哪。

---

## 二、每帧循环

### 编辑器每帧

eframe 每帧调用 `EditorApp::update(ctx)`。

**没有打开项目时**，只画一帧项目枢纽：`ProjectHub::ui(ctx)`（最近项目列表、
新建表单、打开表单）。**打开项目之后**，走下面几步：

```
 1  ViewportCommand::Title        窗口标题带上项目名（仅项目名变化时发送）
 2  toolbar::show() + 模式切换     项目 / 播放暂停停止 / 变换工具
 3  Ctrl+S                        保存当前场景回项目入口场景
 3b Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y 撤销 / 重做（Play 模式下不生效）
 4  physics_step() (仅 Playing)    固定步长物理模拟
 5  CentralPanel (egui_tiles)      4 面板 Dock 布局
 6  StatusBar                      底部状态栏（含项目名）
```

> **这几件事的频率各不相同，别搞混** —— 每帧、只启动一次、仅变化时，三种：
>
> | 动作 | 频率 | 为什么 |
> |---|---|---|
> | `theme::install(ctx)` | **只启动时一次** | `ThemePreference::Dark` 固定后 egui 不再按系统主题覆盖 style（过去每帧重设就是在对抗那个覆盖）|
> | `theme::install_fonts(ctx)` | **只启动时一次** | `set_fonts` 会重建整张字形图集，很贵 |
> | `ViewportCommand::Title` | **仅项目名变化时** | 一路走到 winit 的 `SetWindowText`，是真实的系统调用 |
> | 「最近项目」磁盘校验 | **仅列表变化时** | `is_file()` 是磁盘 IO，遇到失效的网络路径会阻塞 UI 线程 |
> | 离屏 3D 渲染 | **仅输入指纹变化时** | 指纹没变整段渲染跳过，连世界矩阵都不解算，见「阶段 4b」 |
> | Hierarchy 行模型 | **仅结构指纹变化时** | 行缓存按 (指纹, 折叠版本, 过滤器) 复用，不变零 String 分配 |
>
> 最后一行是最容易踩的：**"这一帧要走一遍界面"不等于"这一帧要重画 3D"**。

#### 中文字体（启动时装配一次）

```rust
// crates/xgame-editor/src/main.rs — EditorApp::new()
panels::theme::install_fonts(&cc.egui_ctx);   // 挂系统中文字体
```

`install_fonts` 从 `C:\Windows\Fonts` 按优先级探测一个中文字体（微软雅黑 → 等线
→ 黑体 → 宋体），**追加**到 egui 字体族末尾：西文和图标继续走 egui 自带字体
（字形更精致、体积更小），只有内置字体缺字时才回退到中文字体。egui 内置的
Ubuntu-Light / Hack **不含任何 CJK 字形**，少了这一步中文就是一排「豆腐块」□。

注意 `epaint` 解析字体失败时会**直接 panic**，所以加载前先校验文件头
（TrueType `0x00010000` / `true`、OpenType `OTTO`、字体集合 `ttcf`），把坏文件
挡在 `set_fonts` 之外 —— 否则编辑器会带着 panic 启动失败，用户侧只看到「双击没反应」。

#### 阶段 1：窗口标题

```rust
let project_name = session.state.project_name();
if session.window_title != project_name {          // 只在变化时发
    session.window_title = project_name.to_owned();
    ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
        "XGameEngine Editor — {project_name}"
    )));
}
```

`ViewportCommand::Title` 一路走到 winit 的 `SetWindowText`，是**真实的系统调用**，
所以缓存上一次的标题、只在变化时发送 —— 每帧发一次会平白给窗口管理器加活。

#### 阶段 2：Toolbar + 模式切换

```rust
let prev_mode = self.state.play_mode;
panels::toolbar::show(&mut self.state, ctx);
match (prev_mode, self.state.play_mode) {
    (Edit, Playing) | (Paused, Playing) => enter_play_mode(),
    (Playing, Edit) | (Paused, Edit)    => exit_play_mode(),
    _ => {}
}
```

**模式生命周期：**

```
Edit ──▶ [▶ Play] ──▶ Playing ──▶ [⏸ Pause] ──▶ Paused
  ▲                                                   │
  └────────────────── [⏹ Stop] ───────────────────────┘
```

- **Playing**：创建 `GameRuntime` → 保存实体变换 → 启动 `PhysicsWorld`
- **Paused**：暂停物理步进，场景视图保持最后状态
- **Edit**：销毁 `GameRuntime` → 恢复保存的变换

#### 阶段 3：保存快捷键

```rust
if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::S)) {
    let _ = session.state.save_scene();
}
```

`save_scene()` 把当前 `Scene` 写回**项目清单里的入口场景**（`project.entry_scene`），
路径由 `ProjectHandle::path` 解析，与编辑器的工作目录无关。

#### 阶段 3b：撤销 / 重做

```rust
// 三种组合一次读完再分派，省得在同一个 input 闭包里重复查键盘状态
let (want_undo, want_redo) = ctx.input(|i| { /* Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y */ });
```

`UndoStack` 是**快照式**的（整份 `Scene` 入栈，上限 100 步）。因此**任何新增的可撤销
操作，都必须在改动发生之前**调用 `state.push_undo()` —— 顺序反了，Ctrl+Z 会像「没反应」
（栈里存的已经是改完之后的状态）。Play 模式下不生效。

#### 阶段 4：物理步进

仅在 `play_mode == Playing` 时执行：

真正干活的是 `xgame_runtime::World`；编辑器只是它的一个宿主
（`game_runtime.rs` 现在是薄壳）：

```rust
// 先把 egui 的键鼠状态灌进运行时 —— 必须在 physics_step 之前
input_bridge::sync(ctx, runtime.input_mut());
// 用**平滑过的真实帧间隔**推进，而不是 predicted_dt（那是"下一帧大概多久"的预测，
// 预测偏了固定步长累加器就会多跑/少跑）。
let dt = ctx.input(|i| i.stable_dt);
session.state.physics_step(dt);
```

`World::update(dt)` 的帧序（**编辑器 Play 与打包出的游戏跑的是同一段**）：

```text
clock.tick(dt)
  ├─ dispatch_start                尚未启动过的实体（含上一帧脚本生成的）跑一次
  └─ 固定步长循环（与渲染帧率解耦，max_steps = 5 防死亡螺旋）
       ├─ dispatch_fixed_update    脚本的 fixed_update
       ├─ 消费脚本通知             补建 / 清理刚体、重定位换过父节点的刚体
       ├─ sync_bodies              一次无分配扫描：待补建为空 且 物理实体数 == 已有刚体数
       │                             ⇒ 判定"集合没变"，清理扫描与 sync_queries 整段跳过
       │                             顺带记下 has_kinematic / has_dynamic 供下面两步早退
       ├─ push_kinematic_bodies    运动学体：把脚本摆的世界变换推给物理（无运动学体则直接返回）
       ├─ physics.step(fixed_dt)   顺带维护查询流水线（所以稳态下不必再 sync_queries）
       └─ pull_dynamic_bodies      动态体：物理世界变换 → 局部（无动态体则直接返回）
  ├─ dispatch_update               脚本的 update（每渲染帧一次）
  ├─ 消费脚本通知 / sync_bodies
  └─ input.end_frame()             清掉"仅本帧有效"的输入边沿
```

> **`sync_bodies` 的稳态快路径是运行时性能的关键。** 绝大多数帧里"哪些实体有刚体"
> 根本没变，而旧实现无条件跑完三段扫描（补建 + 建存活 `HashSet` + 清理）并重建射线
> 加速结构 —— 在一个静态场景里是纯浪费。判据的完备性：新增实体一定会让"待补建"非空
> 或总数变化；删实体 / 去掉 `physics` 会让总数与刚体数不等；"一增一删抵消"的巧合里，
> 新增那个的 id 一定不在映射表里 ⇒ 待补建非空。
>
> 层级解算同理：`pull_dynamic_bodies` 原来先 `parent_indices()` 再 `world_matrices()`，
> 而后者内部又要算一遍父索引 —— 每个固定步白跑一次 `hierarchy()`。现在
> `world_matrices_with_parents()` 一次解算两样都给。

**动态体由物理驱动、运动学体由脚本驱动** —— 这条分工决定了上面两个方向的数据流。
一个刚体只属于其中一类，所以两者不会互相覆盖。

物理与渲染帧率**完全解耦**。渲染 30fps 时物理仍跑 60Hz。

**场景所有权是"交换"的，不是"克隆"的**：`World` 拥有场景，而编辑器的面板全都
直接读 `state.scene`。每帧用 `std::mem::swap` 交换整个 `Scene`（O(1)，换三个指针）——
`update` 之前换进去、之后换出来。**前提是 `update` 期间没人读 `state.scene`**，
改动帧序时要留意这一点。

#### 阶段 5：五面板 Dock 布局

```
egui_tiles::Tree 布局

┌────────────┬───────────────────────┬──────────────┐
│ Hierarchy  │     Scene View        │  Inspector   │
│   60%      │        75%            │    25%       │
├────────────┼───────────────────────┤              │
│ Prefabs    │     Console           │              │
│   40%      │        25%            │              │
└────────────┴───────────────────────┴──────────────┘
  ← 左列 22% →      ← 右列 78% →
```

左列上下分（Hierarchy 在上、Prefabs 在下），与 Unity 的层级树 + Project 浏览器
布局一致。

每个面板通过 `#[derive(Panel)]` 派生宏自动分发到 `show_content()`。

##### 4a. Hierarchy 面板

```
hierarchy::show_content(state, ui)
  ├── 搜索过滤器 (TextEdit，词存 state.hierarchy_filter)
  ├── [+] 创建按钮 (Play 模式下禁用 + 黄字提示)
  ├── 层级树 (ScrollArea)
  │     ├── 按 Scene::roots 从根往下走，压平成 (索引, 深度) 列表
  │     │     └── 折叠节点的子树直接跳过（state.collapsed_entities，按 ID 记）
  │     ├── 每行: [折叠箭头▼/▶] [图标] 名称，按深度缩进
  │     │     └── 图标: 🔷 预制体实例根 / 🔹 实例内部节点 / 📦 有模型的实体 / • 空实体
  │     ├── 点击=选中；点箭头/双击=折叠展开
  │     ├── 拖拽=重挂：拖到某行上→挂到它下面；拖到列表空白→移回根层；拖到面板外→取消
  │     │     └── 保世界变换 + 防环（成环被拒并在 Console 提示）
  │     └── 右键菜单: Create Empty / Create Primitive / Duplicate / Copy / Paste as Child
  │           / Create Prefab from This… / [实例根额外] Apply to Prefab · Revert from Prefab
  │           / Unparent / Delete
  ├── [Create Empty] + [Create Primitive ▾] 底部按钮（Cube / Sphere / Plane / Cylinder）
  └── 快捷键: Delete (删除), Ctrl+D (复制), Ctrl+C / Ctrl+V (复制/粘贴), 仅 Edit 模式有效
```

**缩进是渲染出来的，不是数据里的**：场景只存一条 `Entity::parent`，树每帧从
`Scene::roots()` 现算。这样"看到的树"永远等于"数据的树"。

**Play 模式守卫**：`create_entity()` / `duplicate_entity()` / `delete_selected()` /
`reparent_entity()` 在非 Edit 模式返回 early 并输出警告日志。

**删除 / 复制是子树语义**：`Delete` 删整棵子树（`Scene::remove_subtree`），
`Ctrl+D` / `Ctrl+C` 带着子树一起复制（副本重新分配 ID 并还原内部父子关系）。

##### 4b. Scene View 面板（核心）

```
scene_view::show_content(state, ui)
  │
  ├── 顶部栏: 场景名称 + 相机坐标
  │
  ├── setup_viewport → 分配响应区，量出视口尺寸
  │
  ├── ① handle_fly_camera(response)          ← **先处理输入**
  │     右键按下 → 飞行态；rotate(指针增量) | pan_forward(W/S) | pan_right(A/D)
  │     move_up(Q/E) | zoom(scroll)（连续帧由 frame_policy 统一维持，见后文）
  │
  ├── ② 用**本帧最新的相机**算 view_proj
  │     proj = perspective(60°, aspect, 0.1, 100)
  │     view = look_at(eye, eye + forward, Y)
  │
  ├── ③ PointerInput::read + handle_gizmo_interaction  ← 只改状态，**不落笔**
  │     ├── 非 Edit 态 / 无选中 → 清 gizmo 状态（连同 undo_pending 一起丢）
  │     ├── 悬停：primary_down 时**冻结**，只在没按住时重新 pick_gizmo
  │     ├── drag_started → undo_pending = scene.clone()，记 start_value /
  │     │                  start_hit / start_axis(+ axis_anchor) / start_rotation
  │     ├── 每帧          → compute_transform() → 世界增量换算回局部变换写回
  │     └── drag_stopped  → undo_pending 压进撤销栈（一次拖拽 = 一个撤销点）
  │
  ├── ④ 纯点击 → pick_entity(射线 × **世界 AABB**，取最近) → selected_entity
  │     点在 gizmo 上时不拾取（gizmo_active / gizmo_hovered 有值时跳过）
  │
  ├── ⑤ 工具快捷键 W/E/R —— 仅 Edit 且**非飞行模式**且已选中实体时生效
  │
  ├── ⑥ render_scene(用本帧最新的相机与场景)   ← **现在才画 3D（UI 线程同步）**
  │   │
  │   ├── 【输入指纹】相机 / 视口尺寸 / 环境光 + 有效光源（含阴影开关）+
  │   │     每个实体的 model + **局部变换** + **父引用**，收成一个 u64（逐位比较）
  │   │     └── 与上次**完全相同** → 整段跳过 ★
  │   │         ★ 指纹**排在解算世界矩阵之前**：静态帧连 `world_matrices()` 都省掉
  │   │
  │   ├── 指纹变了 → SceneRenderer::render（本帧输入之后、同一帧内完成）
  │   │     ├── 解算世界矩阵 + Phase 1: 模型加载（惰性，entity.model → GltfMesh::load）
  │   │     │   └── 新模型的包围盒记入 model_cache（拾取 mesh_bounds 读这里）
  │   │     ├── Phase 2: wgpu 渲染 (ScenePipeline::render)
  │   │     │     ├── 写入 CameraUniform / LightsUniform / ShadowUniform
  │   │     │     ├── 遍历实体：收集绘制项 + 把 ModelUniform **攒进复用缓冲**
  │   │     │     │   └── 攒完一次 `write_buffer` 写完整段（旧的逐实体写 = 每帧 N 次分配 + N 条命令）
  │   │     │     ├── [有阴影时] 阴影 RenderPass：只画不透明图元 → 2048² 深度图
  │   │     │     ├── 主 RenderPass (背景 #1E1E24)：
  │   │     │     │   ├── 颜色附件 = **多采样**纹理，resolve 到交给 egui 的单采样纹理
  │   │     │     │   ├── 逐 primitive: set_global_bg / set_material_bg / draw_indexed
  │   │     │     │   └── 片元按灯叠加，被标记的灯查阴影图（3×3 PCF）
  │   │     │     └── 提交命令（queue.submit）
  │   │     │
  │   │     └── eframe 上屏与离屏 pass 同在一条队列、按入队顺序执行 →
  │   │           **贴出来的必是本帧纹理**（输入→像素零管线延迟；
  │   │           稳态单帧 CPU 实测 ~0.4ms，无渲染线程、无在飞任务）
  │   │
  │   └── 没有跨帧状态：不存在贴旧图、轮询交卷或等待预算
  │
  ├── ⑦ painter.image(tex_id, rect, uv)      ← 叠加层，画在 3D 之上
  │     └── 贴的就是本帧 render 的纹理：输入到像素在同一线程同一帧内闭环
  │
  ├── ⑧ draw_gizmo(painter, &ctx, tool_mode)
  │
  ├── ⑨ [可选] Physics Debug 叠加（project_to_screen 把 3D 线投到 2D）
  │
  └── ⑩ 叠加层：FPS | Tool: Translate | Speed: X.X（滚轮后短暂显示）
```

### 编辑器「空闲时不重绘」是怎么保住的

```text
Windows 指针静止时仍会反复投递同一坐标的 WM_MOUSEMOVE
  → egui-winit 逐条转成 Event::PointerMoved
  → egui 帧末看到 !events.is_empty() → 请求**立即**重绘
  → 编辑器 60fps 空转（实测空闲 CPU 5~7%）
  → 窗口的模态移动/缩放循环抢不到消息线程 → 拖窗口卡顿
```

`EditorApp::update` 开头先过一道 `drop_redundant_pointer_moves`：丢掉**位置与上次
放行位置相同**的 `PointerMoved`。位置相同的指针事件不携带新信息（egui 的指针状态本来
就停在那儿），是语义上的 no-op；`PointerGone` 时把记录清空，否则"离开再回到同一点"
会被误判成没动、悬停从此失效。

同一道过滤还丢掉 `Event::MouseMoved`（**相对位移**，来自 winit 的
`DeviceEvent::MouseMotion` 原始输入，和绝对位置的 `PointerMoved` 是两回事）：
光电鼠标的传感器噪声以 ±1~2 像素的 `MouseMoved` 持续灌进来，会把帧调度的"刚有输入"
宽限期无限续命（2026-10-10 用事件日志实测：过滤掉它之前，指针完全静止时仍 ~16fps 出帧）。
编辑器没有任何代码消费原始 delta（相机旋转用的是 `pointer.delta()`，由 `PointerMoved`
产生），egui 内部也只把它累计进 `input.raw.motion`，丢弃是安全的。

> 判据只看"有没有真的动"。实测：空闲 0.00% CPU、鼠标移动时 ~8.7%（每帧重绘）。
> 回归测试 `identical_pointer_moves_are_dropped_but_real_moves_pass` /
> `pointer_returning_to_the_same_spot_passes_after_leaving` /
> `raw_mouse_motion_noise_is_dropped`。

**排查这类"谁在请求重绘"问题的正确工具是 `Context::repaint_causes()`**（egui 0.30 起，
带调用点 `file:line`）。逐段插桩那一步在这里是**无效**的 —— 请求发生在我们 UI 代码之外，
自己插的桩全都不认账。

### 帧调度：`frame_policy`（Hybrid 为默认）

上面那道过滤只负责"别把噪声当成输入"；**"要不要连续出帧"是另一个问题**，由
`frame_policy.rs` 集中决策。它存在的原因：eframe/egui 默认是**事件驱动重绘**，这对
表单界面是对的，但编辑器里嵌着实时 3D 视口 —— 鼠标一停画面就冻住、状态栏 FPS 不再
刷新、"来一个事件才算一帧"的节奏让操作发涩。反过来"永远满速"又会在真空闲时白烧 CPU
（还和窗口移动/缩放的模态循环抢消息线程）。

三档策略（环境变量 `XGAME_FRAME_POLICY` 切换，`always` / `hybrid` / `reactive`）：

| 策略 | 行为 | 用途 |
|---|---|---|
| `Always` | 只要开着项目就一直满速 | 最跟手，空闲持续吃 CPU |
| `Hybrid`（默认） | 交互中 / 刚交互过 220ms 内满速，其余停 | 兼顾跟手与空闲开销 |
| `Reactive` | 纯事件驱动（eframe 默认） | 仅 A/B 对照 |

决策是纯函数 `plan(policy, inputs, since_last_input) -> FramePlan`，四个输入
`playing`（Play 模式物理必须按帧推进，绝不能停）/ `interacting`（指针任意键按住 ——
用"按着键"这个单一信号，而不是分别去问 gizmo/相机/面板，漏一处就是"拖着拖着不跟手"）/
`had_input`（本帧有真实输入事件）/ `since_last_input`（最后一次输入距今，即宽限期）。
`FrameClock` 只负责维护"最后一次输入"的时间戳 —— 策略不持有时间，才好单测。

`EditorApp::update` 每帧跑完 `drop_redundant_pointer_moves` 后算一次 `live`，为真就
`ctx.request_repaint()`；`live` 同时传给状态栏（空闲时把 FPS 显示成"已空闲"，不再挂着
上一帧的读数装死）。**全工程只有这一处主动 `request_repaint`** —— Play 模式与飞行
相机都不再自己请求（散落多处的请求会互相打架、也难免漏）。

> 实测（同一台机器、同一驱动脚本，三档对比）：拖拽阶段 Hybrid 与 Always 同为
> ~60fps、p99 ≈ 35ms；Reactive（旧）拖拽中存在 30% 死时间 —— 这就是"操作发涩"的
> 量化。Hybrid 空闲帧数只有 Always 的 ~1/3，且完全无输入时整段归零
> （实测有 6.27s 的连续零帧空档）。
> （这组绝对值是 vsync 时代的 —— 上屏已改 AutoNoVsync，改后的量级见「上屏节奏」节。）
> 回归测试在 `frame_policy.rs`：`reactive_only_fires_on_actual_input` /
> `hybrid_keeps_running_while_interacting` / `hybrid_never_stops_during_play` /
> `hybrid_keeps_running_within_the_grace_window` / `hybrid_idles_once_the_grace_window_expires`。

> **量帧间隔用 `frame_probe.rs`**（环境变量 `XGAME_FRAME_LOG=<文件>`，每帧一行
> `t_us/dt_us/w_us/px/py/rt/d/p/live` —— 关键是 `w_us`（闭包内工作量）：**w 小而
> dt 巨 = 时间花在上屏/唤醒段而不是计算**，第三轮就是靠它把矛头从 UI/渲染转向
> present 的）。两个坑都踩过：探针**不能**用 `BufWriter`（驱动脚本用
> `taskkill /F` 收尾，进程没机会 flush，丢的恰好是被测的尾段）；统计要**只取阶段
> 窗口**并排除窗口内第一帧的 dt（它是跨窗口边界算来的，会把"死时间"算出 >100%）。
> 另外注意测量机环境里可能有**真实指针活动**（位置漂移、偶发按键），idle 档的数据
> 会被它污染 —— 判定"空闲是否真停"以"有无连续零帧空档"为准，别看平均 fps。

### 上屏节奏：`AutoNoVsync`（2026-10-12 第三轮定案）

"操作迟钝、拖动一顿一顿"的最后一块拼图不在 UI 也不在渲染，而在**交换链的等待结构**：

- vsync（Fifo / FifoRelaxed）+ `desired_maximum_frame_latency=1` 时，wgpu 的 acquire
  （取后缓冲）与 present 各等一个 vsync，两处等待叠成 **33ms 输入盲区**：`update()`
  节奏变成 33/1/16ms 的三连拍 —— 每 ~50ms 有一段 33ms **期间输入完全不被处理**、
  界面内容按三连拍刷新；提交→上屏 32.8ms、输入→光子 p90 ≈ 50ms。
  （PLAN 十四遗留的"p90≈33ms 双峰三档一致、策略管不到"就是它 —— 它跟帧策略无关，
  是 present 层的。）
- 修法：`main.rs` 默认 `present_mode = AutoNoVsync`（Immediate → Mailbox → Fifo 逐级
  回退），上屏等屏交给 DWM 按刷新率合成；窗口化应用由 DWM 合成、不撕裂。
  诊断开关保留：`XGAME_PRESENT_MODE` / `XGAME_FRAME_LATENCY`（A/B 后别改回 vsync 默认）。
- 实测（PresentMon 2.6 与帧探针对齐）：改后 `update()` 间隔 p50 0.5ms / p99 1.9ms
  无盲区，提交→上屏 17.5ms（p99 18.9，减半）；交互期代价 ~1 核 CPU + ~11% GPU
  （**不是 GPU 瓶颈**，GPUBusy 仅 3.6ms）；闲置仍由 frame_policy 停帧（0.21s 内归零）。
  状态栏 FPS 交互期显示 ~2000 是**不限帧的真实值**，不是 bug。
- 已排除、勿重测：`latency=2`（帧挤成串后整段停 33-50ms，更糟）、Vulkan 后端（同样
  三连拍）、DX11 后端（wgpu 没编译，surface 创建失败）、egui 多遍（探针 `rt=1` 全程、
  `d=0`）。

### 视口缩放：节流重建离屏目标

`SceneRenderer::set_viewport_size` 在拖边框时每帧都被调。旧实现每次都重建 3 张纹理
（单采样颜色 / **MSAA 颜色（4 倍采样）** / 深度）+ 注册新 egui 纹理 + 释放旧的，
而且尺寸一变指纹必变 → 还要每帧全量重绘 3D。**实测一次拖拽重建 43 次。**

现在由纯函数 `plan_resize` 决策（状态机与阈值集中在 `RESIZE_REBUILD_INTERVAL = 100ms`）：

| 情况 | 动作 |
|---|---|
| 还没有渲染目标（首帧） | 重建（否则第一帧空白） |
| 尺寸与上一帧请求相同 = 用户松手了 | 立刻按最终尺寸重建 |
| 距上次重建超过 100ms（还在拖） | 重建，视口保持约 10Hz 更新 |
| 其余 | 跳过 3D，调用方把上一帧纹理拉伸显示 |

> 实测一次拖拽：重建 **43 → 1**；缩放结束后画面比例正确、无拉伸残留。

> **帧序是视口手感的命门**：必须「先处理本帧输入 → 再按最新状态重画 3D → 最后画叠加层」。
> 旧的顺序是"先重画、后处理输入"，于是这一帧画的是**上一帧**的相机与场景 —— 拖物体、
> 转视角都白白慢一帧，手感发"黏"。而叠加层必须画在 3D 之上，只能排在渲染之后，
> 所以 gizmo 被拆成"更新状态"（③）与"绘制"（⑧）两步。
> 回归测试：`the_frame_is_rendered_after_this_frames_camera_input`（把渲染那一刻用的
> `view_proj` 记录下来，断言它等于本帧输入处理完之后的相机矩阵）。

###### 为什么不是"每帧都重画一遍"

编辑器是**事件驱动重绘**的：没有输入就不出帧。但"需要重绘"和"画面变了"是两回事，
两者之间差着一大截 —— 最典型的就是**拖动窗口**。`egui-winit` 把
`WindowEvent::Moved` 也归进"需要重绘"的事件里（0.30 起；0.20 时它的注释写得很清楚：
_"moving a window doesn't warrant a repaint"_，后来被并进大组里丢了），于是窗口每挪
一下，eframe 就重画一整帧。

窗口位置变化**不改变客户区内容**，这一帧里 3D 视图完全没必要重画。而 Windows 的窗口
拖动是在**主线程的模态移动循环**里跑的 —— 渲染越重，消息循环被饿得越狠，手感就是
"拖一下、顿一下"。同理，拖停靠面板、鼠标在界面里划过、点个复选框，都会带来一堆
"场景其实没变"的帧。

`SceneRenderer::render` 因此先算一个**输入指纹**：相机矩阵与位置、视口尺寸、环境光、
参与光照的光源（含 `casts_shadow` / `shadow_resolution`）、每个实体的模型标识、**局部变换**
与**父引用**。这些正是着色器会读到的东西；选中态、gizmo、网格开关、物理调试线都不在其中
（它们画在 egui 叠加层上，与离屏纹理无关）。
指纹与上一帧逐位相同 → 整段渲染跳过，`painter.image` 照旧贴上一帧那张纹理。

> **为什么收「局部变换 + 父引用」，而不是直接收世界矩阵。**
> 着色器读的是世界矩阵，但世界矩阵是 `(局部变换, 层级)` 的**纯函数** —— 两者一样，
> 世界矩阵必然一样，所以按前者哈希是**等价**的。收益是：不必为了算指纹先跑一遍
> `Scene::world_matrices()`（那要分配几条长度为 n 的 `Vec` 并做 n 次矩阵连乘，是
> 大场景里编辑器**每帧**的固定成本）；而且世界矩阵 16 个浮点、局部变换只要 10 个。
> **父引用必须一起哈希**：只哈希局部变换的话，"把子节点挂到另一个父节点下"会漏掉
> （子节点自己的局部变换一个字节没变，但世界矩阵变了）。回归测试
> `signature_notices_reparenting` 盯的就是这条。

> **实测**（2026-10-10，真机悬停流 + 进程内探针）：连续 666 次鼠标移动里，
> 离屏 3D **只重画了 1 次**，渲染目标也只重建了 1 次（初始那次）。也就是说
> "悬停"这条最常见的路径上，3D 视图一分钱都不花；编辑器此时的开销就是 egui 的
> 排版与镶嵌。

两个容易写错的地方：

- **用位模式比较，不用浮点相等**：`-0.0` / `NaN` 这类值按位比才是确定稳定的。这里
  宁可"多渲染一帧"（无害）也绝不"少渲染一帧"（画面上就是脏的）。
- **重建渲染目标后必须把指纹置回 `None`**：视口尺寸变了，旧纹理内容作废，下一帧无论如何都要重画。

> 顺带修掉的另一处：`recreate_targets` 以前每次重建都会新建一个**没人引用的 1×1 占位
> 纹理**，并且注册新原生纹理时**从不释放旧的 `TextureId`** —— 而窗口缩放期间这个函数
> 是每帧跑一次的，等于每次改尺寸漏一条 egui 内部表项。现在改成先 `free_texture` 再注册。

**WGSL 着色器**（位于 `xgame-render/src/scene_pipeline.rs`）：

绑定组布局（两个 group）：

```text
group(0)  全局，整帧一份            group(1)  材质，每个材质槽一份
  binding 0  camera  uniform          binding 0  base color texture
  binding 1  lights  uniform          binding 1  sampler
  binding 2  model   uniform          binding 2  material uniform
  binding 3  shadow  depth tex
  binding 4  shadow  cmp sampler
  binding 5  shadow  uniform
```

```wgsl
// 顶点阶段
world_pos  = model * position;
normal_mat = transpose(inverse(mat3(model)));   // 逆转置：非均匀缩放下法线才不歪
clip_pos   = view_proj * world_pos;

// 片元阶段（Blinn-Phong，最多 8 盏灯叠加）
albedo = textureSample(base_color_tex, base_color_smp, uv).rgb
       * material.base_color.rgb * input.color;
color  = ambient.rgb;
for 每盏灯:
    L      = 方向光取 -direction；点光取 normalize(light_pos - world_pos)
    atten  = 点光按 1/(1 + 0.09·d + 0.032·d²) 衰减；方向光恒为 1
    ndotl  = max(dot(N, L), 0)
    H      = normalize(L + V)                   // V = normalize(camera_pos - world_pos)
    spec   = pow(max(dot(N, H), 0), shininess) * specular * ndotl
    // 只有被标记的那盏灯查阴影图；3×3 PCF
    shade  = shadow_params.x > 0.5 ? pcf_3x3(uv_light, depth_light - bias) : 1.0
    color += (albedo * ndotl + spec) * radiance * shade;
```

**阴影 pass**（在主 pass 之前，同一个 encoder 里）：

```text
if 有开阴影的方向光 且 场景包围盒非空:
    1. CPU: fit_directional_shadow(dir, scene_bounds)
            → 用场景外接球拟合正交视锥（proj · look_at）
    2. GPU: 只画**不透明**图元 → 2048² Depth32Float（无片元管线，背面剔除）
    3. 片元阶段把 world_pos 变换回光源裁剪空间，取 xyz/w 得 UV 与深度，
       用比较采样器做 3×3 PCF（textureSampleCompareLevel，避开非一致控制流）
```

`shadow_bgl` 刻意**不含**深度贴图本身：阴影 pass 一边把深度画进 `shadow_view`、
一边若在 group(0) 里绑同一张图，wgpu 会判成"同一资源既作渲染目标又作绑定资源"。
用小布局绕开它。深度偏置用两处：着色器里的常数偏置 + 管线的硬件斜率偏置。

材质绑定组按 `GltfPrimitive::material_id` **惰性创建并缓存**——模型加载一次之后，
每帧只是查表 + `set_bind_group`，不会重复建 GPU 对象。

绘制顺序：**先不透明（写深度、背面剔除），后半透明（不写深度、关背面剔除、
按到相机距离远 → 近排序）**，两条路径各有一条 render pipeline。

**调试线投影**（将 3D 世界坐标映射到 egui 屏幕坐标）：

```rust
fn project_to_screen(point: Vec3, view_proj: &Mat4, screen_size: Vec2) -> Pos2 {
    let clip = *view_proj * Vec4::new(point.x, point.y, point.z, 1.0);
    let ndc = clip.truncate() / clip.w;               // [-1, 1]
    let x = (ndc.x + 1.0) * 0.5 * screen_size.x;
    let y = (1.0 - ndc.y) * 0.5 * screen_size.y;     // 翻转 Y
    egui::pos2(x, y)
}
```

**抗锯齿（MSAA）**：

```text
采样数 = supported_sample_count(adapter, format)   // 支持就 4×，否则退回 1
  ├── 编辑器：多采样颜色纹理(RENDER_ATTACHMENT) + 多采样深度
  │        渲染写多采样那张 → resolve_target = 交给 egui 的**单采样**纹理
  │        （egui 只认单采样纹理，注册的必须是 resolve 之后那张）
  └── 游戏：多采样颜色纹理 → resolve 到**交换链视图**
           （交换链的视图本身不能是多采样，所以必须多这一张）
```

采样数**必须由适配器能力决定**：某些格式/后端不支持 4×，硬开会让管线创建直接失败。
颜色附件与深度附件的采样数也必须与管线一致，否则渲染通道校验失败。阴影 pass 单独
一条管线、永远是 1× —— 它只存深度，不需要抗锯齿。

###### 飞行相机的两条手感规则

- **转视角不能每帧都要求"指针压在视口上"。** 飞行态是在"右键按下且指针在视口里"
  那一刻开启的；开启之后若还逐帧用 `response.hovered()` 当闸门，用户手一抖把光标
  带出视口，旋转就会**卡住**，移回来又**跳一下** —— 快速转视角时表现就是"抽搐"。
  飞行态一旦开着就一路转到松手。
- **位移用 `stable_dt`（平滑过的真实帧间隔），不用 `predicted_dt`。** 后者是"下一帧
  大概要多久"的预测；预测不准时同一段按键时长会走出不同距离，速度一冲一顿，平移与
  旋转同时进行时最明显。编辑器的物理步进与 FPS 读数同理（FPS 用预测值会把卡顿藏起来）。

###### Gizmo 的四个坑（都踩过）

**一、点击和拖拽是两条互斥的路，信号不能混用。**

egui 对 `Sense::click_and_drag()` 的控件，`drag_started_by()` **必须指针移动过一段距离**
才为 true（官方说明：*"If the widget also senses clicks, this will only become true if
the pointer has moved a bit."*）。所以：

- 「按下—原地松手」只会给出 `clicked()`，`drag_started_by()` 全程 false；
- 「按住并移动」只会给出 `drag_started_by()`，`clicked()` 为 false。

拾取必须看 `clicked()`。早先拿 `drag_started_by(Primary)` 当"点击"用，结果**在视口里
点一下永远选不中任何物体**，必须按住拖一下才行。

**拾取的范围必须按真实几何算（世界 AABB），不能用"包围球"。** 旧实现给每个实体造一个
球、半径取 `scale.length() * 0.5`，对**被拉伸过**的物体是灾难性高估：一块
`16 × 0.5 × 16` 的地面半径算成 11.3，而相机离场景中心只有约 8.7 —— **相机落在球内部**，
于是任何射线都命中它。后果是「点天空也选中地面」，也就是**取消选中根本做不到**；
真机表现就是"想选的选不中、想取消的取消不掉"。现在拿 `GltfMesh::bounds`（网格本地
包围盒）经世界矩阵变换成 AABB 再做 slab 求交，范围就是几何真正包住的那一块。

**二、按住左键期间必须冻结 gizmo 悬停。**

同一条"要移动过阈值"的规则反过来坑另一头：等 `drag_started` 成立时，指针已经离开按下
点了（阈值约 6px），而操作柄的命中半径只有 6–14px —— 这时若重新 `pick_gizmo`，很可能
拿到 `None`，于是 `drag_started` 已经用掉、`gizmo_active` 又设不上，**拖拽永远开始不了**
（拖得越快越抓不住）。悬停在"按下那一帧"才是准的，保持到松手即可。

**三、轴上的位移量只能由「直线投影」求，不能用「平面交点」。**

`pick.rs` 里的 `hit_position_on_axis` 求的是射线与「过实体、以轴为法线」的**平面交点**。
按定义，这个交点到实体的位移**永远垂直于轴**，投影到轴上恒为 0 —— 拿它算轴坐标，
得到的是与鼠标移动无关的常量：

| 用途 | 旧实现的后果 |
|---|---|
| 沿轴平移 | 位移恒等于 `-entity_pos·axis`，**拖红箭头物体纹丝不动** |
| 沿轴缩放 | 比值恒为 `0 / 常量`，被 `.max(0.01)` 夹住 → **一拖就塌成 1%** |
| 旋转环 | `start_hit` 落进 `_ => entity_pos`，起始方向是零向量 → `normalize()` 出 NaN → **拖动环后物体从画面上消失** |

正确做法是 `axis_param_under_mouse`：直接求「直线 `P(s) = entity_pos + axis·s`」与鼠标
射线的最近点对（Ericson《Real-Time Collision Detection》里 ClosestPtSegmentRay 的直线
版本），返回那个 `s`。`GizmoDrag` 因此同时存两个起始量：

- `start_axis: f32` —— 轴坐标，供**平移 / 缩放**用；
- `start_hit: Vec3` —— 平面交点，只供**平面拖拽**（求差得位移）与**旋转环**（求起始角度
  方向）用。

**四、轴拖拽的参考线必须锚死在拖拽起点，不能跟着物体跑。**

`axis_param_under_mouse` 求的是「过某个点、方向为轴」的直线与鼠标射线的最近点参数。
那个"某个点"如果用**每帧重建**的 `ctx.entity_pos`（= 物体被拖到的当前位置），参考线
就跟着物体一起走，`s_now` 里混进了"物体自己刚挪的那一段"：

```text
Δ_k = A_k − Δ_{k-1}        // A_k = 鼠标本帧"想"走的位移
```

这是增益 −1 的正反馈：极点在 `z = −1`（奈奎斯特）。**物体会在相邻两帧之间来回跳**
（"拖拽抽搐"），稳态只走鼠标位移的一半，还把鼠标的像素级噪声放大成可见抖动。
修法是把参考线锚在拖拽起点的世界位置（`GizmoDrag::axis_anchor`），`s_now` 于是只由
鼠标射线决定，环断开 —— 结果只取决于"鼠标现在在哪"，与它被采样成多少帧无关。

> 这个 bug 之前逃过测试，是因为**所有拖拽测试都用固定的 `ctx`**：它们复刻不了
> "每帧按新位置重建 ctx"这一步。现在有两条跨帧测试盯它：`gizmo/drag.rs` 里逐帧
> 重建 ctx 的那条，以及 `scene_view.rs` 里"1 大步与 6 小步必须落到同一位置"的端到端。

##### 4c. Inspector 面板

```
inspector::show_content(state, ui)
  ├── 没有选中实体 → 提示语，返回
  ├── working = scene.entities[选中索引].clone()      ← 只克隆**一个**实体
  ├── 控件全部渲染在 working 上（scene 整段不动）
  │     ├── Name: [TextEdit]
  │     ├── Transform:
  │     │     ├── Position:  [x] [y] [z]
  │     │     ├── Rotation:  [x] [y] [z]（欧拉角显示，内部四元数）
  │     │     └── Scale:     [x] [y] [z]
  │     └── Model: [TextEdit] + 内置图元快选
  ├── 撤销点（必须发生在写回**之前**，那时 scene 还是"改前"）
  │     ├── 拖拽起始  → undo_pending = scene.clone()
  │     ├── 拖拽结束  → 把 undo_pending 压进撤销栈（整次拖拽 = 一个撤销点）
  │     └── 离散编辑  → push_undo()（当前 scene 就是改前状态）
  └── 有改动才把 working 写回 scene（拖拽期间逐帧写回，3D 视图才会跟着动）
```

**为什么控件指向副本**：这样"改前快照"可以推迟到**确认真的编辑了**再拍。旧写法是
一进面板就 `scene.clone()` 当快照 —— 那意味着**每帧**都克隆整份场景，哪怕用户什么都
没动、只是鼠标划过界面。现在每帧只克隆选中实体（两个小字符串），真正需要整场景快照
时仍然是整场景克隆 —— 但那时是"确实编辑了一次"。

**拖拽为什么要 `undo_pending` 中转**：`DragValue` 拖动过程中每一帧 `changed()` 都是
true，逐帧入栈会让一次拖动塞进几十个撤销点，撤销变成"一像素一步"。

##### 4d. Console 面板

```
console::show_content(state, ui)
  ├── [Clear] 按钮 + 消息计数
  ├── 消息列表 (ScrollArea, 自动滚动)
  │     └── 每条: 级别图标 + 文本 + 去重计数 (×N)
  └── 最多保留 500 条
```

##### 4e. Prefabs 面板（预制体资源浏览器）

```
prefabs::show_content(state, ui)
  ├── 标题 "Prefabs" + [⟳] 重新扫描（手工在资源管理器里增删过文件时用）
  ├── 搜索框 (词存 state.prefab_filter)
  ├── [📦 Create from Selection] + 当前选中实体名提示
  │     └── 点击 → 开命名弹窗（state.prefab_save_dialog）→ 写资产 + 原地标记为实例
  ├── 资产列表 (ScrollArea)，每行 [📦 名称]（悬停显示完整路径）
  │     ├── 点击 = 实例化到场景（落在根层并选中）
  │     └── 右键: Instantiate / Copy Path / Delete Asset…
  └── 弹窗: Create Prefab（命名 + 路径预览）/ Delete Prefab（二次确认，标注不可撤销）
```

**列表是缓存的，不是每帧扫盘**：`state.prefab_list` 在打开项目（`EditorState::new`）、
创建 / 删除资产、点 ⟳ 时由 `refresh_prefabs()` 刷新。目录扫描是一串系统调用，
逐帧做就是「界面没动却在耗 CPU」。

#### 阶段 6：状态栏

```
StatusBar
  "v0.1.0 | Project: SampleScene | Edit Mode | Scene: Test Scene | 3 entities | FPS: 60"

  Playing 时额外显示:
  "▶ Playing | ... | Bodies: 3 | Colliders: 3 | FPS: 60"
```

---

### 独立游戏每帧 (`xgame-game`)

**先看驱动方式**：winit 的事件循环在**没有新事件时会阻塞等待**，不会自己连续出帧。
所以要在 `about_to_wait` 里主动 `window.request_redraw()` 把循环唤醒，真正的一帧则放在
`window_event(.., RedrawRequested)` 里：

```
ApplicationHandler::about_to_wait      window.request_redraw()   ← 排一次重绘，唤醒循环
WindowEvent::RedrawRequested
  ├── dt = now - last_frame            真实耗时，夹到 250 ms 上限
  ├── GameApp::update(dt)
  └── GameApp::render()
```

少了 `request_redraw()` 这一步，画面就**只在鼠标 / 键盘事件到来时才更新** ——
表现出来正是「鼠标不动，游戏就不跑」。

> **为什么是 `run_app` + `ApplicationHandler`，不是 `event_loop.run(闭包)`**：
> 闭包版本从 winit 0.30 起已废弃，而且窗口**只能**通过
> `ActiveEventLoop::create_window` 创建（`EventLoop::create_window` 同样废弃），
> 后者仅在回调内部拿得到 —— 所以窗口与 `GameApp` 都必须推迟到 `resumed` 里再建。
> `GameRunner` 的字段顺序即析构顺序的反序：`app` 声明在 `window` 之后，
> 于是先析构 `app`（它内部那个 `Surface<'static>` 是借窗口生命周期的）再析构窗口。

```
GameApp::update(dt)
  ├── clock.tick(dt)
  ├── while clock.consume_fixed_update():
  │     physics.step(fixed_dt)
  ├── clock.reset_step_counter()
  └── sync_transforms()

GameApp::render()
  ├── 计算 view_proj 矩阵 (固定轨道相机: (5,5,5) → 原点)
  ├── 惰性加载模型 (GltfMesh::load(device, queue, project.root(), entity.model))
  ├── surface.get_current_texture()
  ├── encoder = device.create_command_encoder()
  ├── ScenePipeline::render(queue, encoder, frame_view, depth_view,
  │                         entities, model_cache, view_proj, camera_pos)
  └── queue.submit() + frame.present()
```

> **示例项目在 Play / 独立运行时会"塌"**：`GameApp::new` 给**每个有模型的实体**都建
> 动态刚体（`RigidBody::dynamic_at`），而 `projects/SampleScene` 里没有静态地面 ——
> 于是全部物体在重力下无限下落，几秒后就掉出视野。这是既有行为，不是渲染问题。

```
GameApp::update(dt)
  ├── clock.tick(dt)
  ├── while clock.consume_fixed_update():
  │     physics.step(fixed_dt)
  ├── clock.reset_step_counter()
  └── sync_transforms()

GameApp::render()
  ├── 计算 view_proj 矩阵 (固定轨道相机: (5,5,5) → 原点)
  ├── 惰性加载模型 (GltfMesh::load(device, queue, project.root(), entity.model))
  ├── surface.get_current_texture()
  ├── encoder = device.create_command_encoder()
  ├── ScenePipeline::render(queue, encoder, frame_view, depth_view,
  │                         entities, model_cache, view_proj, camera_pos)
  └── queue.submit() + frame.present()
```

---

## 三、构建流程

这里的「构建」有两条**互相独立**的路，别混：

| | 把编辑器做成软件 | 把游戏项目做成软件 |
|---|---|---|
| 入口 | 双击根目录 `一键构建.cmd` | 编辑器工具栏的 🔨 Build |
| 实现 | `tools/build-release.ps1` | `crates/xgame-editor/src/builder.rs` |
| 产物 | `dist/`（编辑器 + 播放器 + 示例项目） | `<项目>/Build/<项目名>/` |

> **开发期用 `cargo run` 的话，根 `Cargo.toml` 里的 `[profile.dev]` 别删。**
> dev profile 默认 **零优化**，而这个项目的单帧开销几乎全在依赖里（egui 的排版/镶嵌、
> wgpu 的校验层、glam 的数学）—— 全量未优化时单帧能从个位数毫秒掉到几十毫秒，表现
> 出来就是"界面发黏、拖一下窗口就卡"。现在是自己的代码 `opt-level = 1`（保住编译速度）、
> 依赖 `opt-level = 3`。代价是首次构建要多花几分钟重编依赖，之后都是增量。

### 发布构建（一键构建.cmd → tools/build-release.ps1）

```
一键构建.cmd
  └─ powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\build-release.ps1"
       ├── Start-Transcript          → build-release.log
       ├── Resolve-Cargo             PATH 里没有时回退 %USERPROFILE%\.cargo\bin
       ├── cargo build --release --workspace
       │     └── 失败 → Enable-MsvcEnv 拼出 LIB / INCLUDE → 重试一次
       └── 收集产物
             ├── target/release/xgame-editor.exe → dist/
             ├── target/release/xgame-game.exe   → dist/   （编辑器 Build 时要复制它）
             ├── projects/SampleScene/           → dist/projects/
             └── 写 dist/README.txt
```

两个刻意的设计：

- **先裸构建，失败了才补 MSVC 环境**。这台机器的 VS 桌面版 lib 不完整（缺
  `msvcrt.lib`），得用 `lib/onecore/x64` 顶替；但把这种机器专属路径写死进脚本，
  换台机器就崩，所以只在需要时才去探测。
- **只删自己的产物目录**（`dist/`），且走 .NET API 而不是 `Remove-Item`——
  语义明确，也不受任何 shell 层的回收站包装影响。

> ⚠️ 脚本里的注释一律写英文。Windows PowerShell 5.1 在没有 BOM 时按系统 ANSI
> 代码页（这台机器是 GBK）解码 `.ps1`，中文注释会乱码；`.cmd` 同理。

### 编辑器构建项目（builder.rs）

```
工具栏 🔨 Build
  └─ EditorState::build_project()
       ├── save_scene()                先把场景存盘，否则打出去的是旧内容
       └── builder::build(&project)
            ├── 入口场景不存在 → 直接报错（快速失败，不留半成品目录）
            ├── locate_player()         编辑器同目录 → 兄弟 profile 目录
            ├── 清空 <项目>/Build/<项目名>/
            ├── 复制播放器 → <项目名>.exe
            ├── 复制 project.ron
            └── 复制 content_dirs()：清单里的内容目录（去重）+ 入口场景所在目录
```

产物布局是**为 `xgame-game` 的项目定位顺序服务的**：

```
<项目>/Build/<项目名>/
├── <项目名>.exe      ← 播放器副本，改了名
├── project.ron       ← 与 exe 同目录，这一点是关键
├── Scenes/
└── Resources/
```

`xgame-game` 的定位顺序：**命令行参数 → exe 同目录的 `project.ron` → 仓库示例项目**。
中间那条正是为这个布局准备的，所以构建出来的游戏**双击 exe 就能跑**，不需要参数，
也不需要启动脚本。

---

## 四、数据流与状态模型

```
EditorState（中央状态，所有面板共享可变引用）
  │
  ├── project: ProjectHandle            已打开的项目（根目录 + 清单）
  │     └── path(rel) → 绝对路径         所有资产路径的唯一解析入口
  │
  ├── scene: Scene
  │     ├── name: String
  │     ├── entities: Vec<Entity>
  │     │     ├── name: String            显示名称
  │     │     ├── transform: Transform    **局部**变换（相对父节点）
  │     │     ├── model: String           模型标识：项目相对路径，或 builtin://cube
  │     │     ├── id: EntityId            稳定标识符 (u64)
  │     │     ├── parent: Option<EntityId> 父实体（唯一真相；子节点现算）
  │     │     ├── prefab: Option<PrefabInstanceRef>  预制体来源（普通实体为 None，不落盘）
  │     │     └── components: ComponentMap 运行时组件
  │     ├── lights: Vec<Light>            场景光源（方向光/点光，渲染上限 8 盏）
  │     └── ambient: [f32; 3]             全局环境光
  │
  │   世界变换不进数据结构，按需由 Scene::world_matrix / world_matrices 沿父链解算。
  │   渲染、物理、gizmo 用世界变换；Inspector 显示局部、另给一行世界坐标。
  │
  ├── editor_camera: EditorCamera        自由飞行相机 (yaw/pitch/position)
  ├── selected_entity: Option<usize>     场景 entities 索引
  ├── play_mode: PlayMode                { Edit, Playing, Paused }
  │
  ├── undo_stack: UndoStack              快照式撤销栈（上限 100 步）
  ├── undo_pending: Option<Scene>        拖拽中的待入栈快照
  ├── clipboard: Option<Vec<Entity>>     复制/粘贴用的**子树**快照（根在前）
  │
  ├── prefab_list: Vec<String>           预制体资产清单缓存（项目相对路径，已排序）
  │     └── 打开项目 / 增删资产 / 点 ⟳ 时刷新，**不每帧扫盘**
  ├── prefab_filter / prefab_save_dialog / prefab_save_name / prefab_delete_pending
  │
  ├── show_grid: bool
  ├── show_physics_debug: bool
  ├── return_to_hub: bool                请求关闭项目回到枢纽
  │
  ├── scene_renderer: Option<SceneRenderer>   视口渲染器（UI 线程同步渲染，主循环内构造）
  │     ├── model_cache                      模型网格 + 包围盒（拾取 mesh_bounds 读这里）
  │     ├── last_signature                   上一帧输入指纹（静态帧整段跳过的依据）
  │     └── texture_id                       离屏纹理（本帧 render 后即为本帧画面）
  │
  └── game_runtime: Option<GameRuntime>      物理运行时 (仅 Playing)
        ├── physics: PhysicsWorld             rapier3d 世界
        ├── entity_to_body                  实体索引 → 刚体 ID
        ├── saved_states                     备份**局部**变换 (退出时恢复)
        └── clock: GameClock                  固定步长 60Hz

      物理只认世界空间：建刚体读 Scene::world_transform；回写时把世界变换左乘
      父世界矩阵的逆换算回局部（GameRuntime / xgame-game 的 sync_transforms）。
```

**预制体数据流**（`xgame-core::prefab`）：

```
Prefab { name, entities }          ← 与 Scene 同构的"小场景"，存 Prefabs/*.prefab.ron

创建    Prefab::from_subtree(scene, root)   提取子树（清掉指向子树外的 parent、清空来源）
        ProjectHandle::write_prefab(path)   落盘
        mark_subtree_as_instance            原地给这批实体贴上 PrefabInstanceRef

实例化  Prefab::instantiate(source)         分配新场景 ID + 重映射内部 parent + 贴来源
        Scene::instantiate_prefab           追加进场景，返回实例根索引

Apply   Prefab::from_subtree(实例根) → write_prefab   把实例当前样子覆盖回资产
Revert  load_prefab → remove_subtree(实例根) → instantiate_prefab
        └── 新实例根局部设为旧世界矩阵，再 set_parent 回原父节点 ⇒ 世界位置不变
```

**独立游戏状态**：

```
GameApp
  ├── project: ProjectHandle              已打开的项目（根目录 + 清单）
  ├── scene: Scene
  ├── physics: PhysicsWorld
  ├── entity_to_body: HashMap<usize, u64>
  ├── clock: GameClock
  ├── pipeline: ScenePipeline
  ├── device / queue / surface / config
  ├── depth_view / depth_texture
  └── model_cache: HashMap<String, GltfMesh>
```

**持久化**：每个项目一份清单 + 若干场景

```
projects/SampleScene/
├── project.ron          项目清单
├── Scenes/scene.ron     由 project.entry_scene 指向
└── Resources/…          entity.model 指向这里
```

```ron
// project.ron
(
    format_version: 1,
    name: "SampleScene",
    entry_scene: "Scenes/scene.ron",     // 相对项目根目录
    window: (title: "SampleScene", width: 1280, height: 720, resizable: true, vsync: true),
    content: (scenes_dir: "Scenes", models_dir: "Resources", …),
)
```

```ron
// Scenes/scene.ron —— model 同样是项目相对路径
Scene(
    name: "Test Scene",
    entities: [
        Entity(
            name: "Box1",
            transform: (position: (0,0,0), rotation: (0,0,0,1), scale: (1,1,1)),
            model: "Resources/Box.glb",
        ),
    ],
)
```

---

## 五、技术栈

| 层级 | 技术 | 版本 | 用途 |
|---|---|---|---|
| 窗口 + 事件 | winit | 0.30 | 窗口创建、输入事件 |
| GPU | wgpu | 23 | Vulkan/Metal/DX12 抽象 |
| 原生框架 | eframe (egui) | 0.30 | 窗口循环 + 即时模式 GUI |
| 布局 | egui_tiles | 0.11 | 可拖拽面板 dock 布局 |
| 过程宏 | syn / quote / proc-macro2 | 2/1/1 | `#[derive(Panel)]` |
| 数学 | glam | 0.29 | Vec3/Mat4/Quat |
| 物理 | rapier3d | 0.22 | 刚体动力学、碰撞检测、射线 |
| 脚本 | rhai | 1.26 | 嵌入式脚本（开了 `f32_float`，见下） |
| 3D 模型 | gltf (Rust) | 1 | glTF/glb 解析 |
| 纹理 | image | 0.24 | PNG/JPEG 解码 |
| 序列化 | ron + serde | 0.8 / 1 | 场景文件持久化 |

> **已知的上游问题（尚未在应用层根除）**：`egui-winit 0.30` 把 `WindowEvent::Moved`
> 归进"需要重绘"，而 `winit 0.30.13` 在 Windows 上的模态移动循环里又会持续投递这个
> 事件 —— 两者叠加，拖窗口时应用会**在消息循环线程上**不停出帧（上游对应
> `rust-windowing/winit#4708`、`emilk/egui#3953`）。当前的对策是**把每帧做轻**
> （见「阶段 4b」的输入指纹、以及 `[profile.dev]`），让渲染不足以饿死消息循环。
> 想彻底根除得改依赖：要么给 `egui-winit` 打一行补丁（`Moved` 应当是 `repaint: false`，
> 这也是它在 0.20 时的写法），要么整体升级 egui 系（锁在 0.30，升到 0.3x 是一整套
> API 迁移，得单独当一件事做）。

```
xgame-core     核心类型：Color,Config,Transform,Scene,Entity,Component,GameClock,AssetId
    ↑
xgame-math     glam 重导出 + projection 辅助
    ↑
xgame-render   wgpu 管线、glTF 加载、ScenePipeline（共享渲染管线）
xgame-physics  rapier3d 刚体、碰撞体、物理世界、调试绘制
    ↑
xgame-editor   egui 面板、SceneRenderer（ScenePipeline wrapper）、GameRuntime
xgame-game     独立游戏二进制（winit + wgpu + ScenePipeline + PhysicsWorld）
```

---

## 六、关键文件速查

| 文件 | 行数 | 核心函数 |
|---|---|---|
| `editor/main.rs` | 516 | `EditorApp::new()`、`update()`（开头过 `drop_redundant_pointer_moves`）、`open_project()`、`editor_ui()` |
| `editor/hub.rs` | 352 | `ProjectHub::ui()`、`open_project_at()`、`create_project_at()` |
| `editor/recents.rs` | 119 | `RecentProjects::load()`、`add()` |
| `editor/builder.rs` | 368 | `build()`、`locate_player()`、`content_dirs()` |
| `editor/state.rs` | 1438 | `EditorState::new()`、`UndoStack`（含上限）、`GizmoDrag`（含 `axis_anchor`）、`clamp_selection()`、`log()`（控制台带上限）、`create_primitive()`、`undo()`、`paste_entity()`、预制体操作 |
| `editor/scene_renderer.rs` | 804 | `SceneRenderer::new()`（含 `project_root`）、`scene_signature()`（局部变换 + 父引用，含阴影开关/分辨率）、`plan_resize()`（缩放节流状态机）、`render()`（收 `&Scene`，指纹没变整段跳过、连世界矩阵都不解算，返回新加载模型的包围盒）、`mesh_bounds()`（拾取用） |
| `editor/scene_view.rs` | 1143 | `show_content()`、`PointerInput::read()`、`handle_gizmo_interaction()`、`pick_entity()`（射线 × 世界 AABB） |
| `editor/gizmo/pick.rs` | 543 | `pick_gizmo()`、`axis_param_under_mouse()`、`ray_aabb()` / `UNIT_BOUNDS`（实体拾取） |
| `editor/gizmo/drag.rs` | 520 | `compute_transform()`、`axis_param_anchored()`（参考线锚定） |
| `editor/game_runtime.rs` | 93 | `start()`、`fixed_update()` |
| `core/project.rs` | 685 | `ProjectHandle::open()`、`create()`、`path()`、`list_prefabs()`、`load_prefab()`、`write_prefab()` |
| `core/prefab.rs` | 421 | `Prefab`、`PrefabInstanceRef`、`from_subtree()`、`instantiate()`、`prefab_display_name()` |
| `core/scene.rs` | 947 | `Scene::world_matrices()` / `world_matrices_with_parents()`（链缓冲复用、`Mat4` + done 位图）、`hierarchy()` / `parent_indices()` / `child_map()`（批量解父子，O(n)）、`set_parent()`（保世界变换+防环）、`children()`、`subtree()`、`remove_subtree()`、`instantiate_prefab()`、`prefab_instance_root()` |
| `core/light.rs` | 170 | `Light`、`LightKind`、`normalized_direction()`、`casts_shadow` / `shadow_resolution` |
| `render/scene_pipeline.rs` | 1575 | `ScenePipeline::new()` / `render()`（含阴影 pass、模型矩阵攒进复用缓冲后一次上传）、`scene_bounds()`、`fit_directional_shadow()`、`pick_shadow_slot()`、`build_lights()`、`material_binding()` |
| `render/gltf_loader.rs` | 546 | `GltfMesh::from_file()` / `load()`、`read_material()`、`Bounds`（网格本地包围盒） |
| `render/mesh.rs` | 365 | `BuiltinMesh`、`cube()`、`sphere()`、`plane()`、`cylinder()` |
| `physics/world.rs` | 900 | `step(dt)`（按传入 dt 积分、子步均分 dt）、`add_body()`（维护 `body_handles` 反查表）、`add_collider()`（未知 body 只警告不 panic）、`debug_lines()` |
| `editor/hierarchy.rs` | 410 | `show_content()`（树形 + 折叠 + 拖拽重挂 + 右键菜单 + 预制体标记/菜单）|
| `editor/inspector.rs` | 314 | `show_content()`（含撤销集成、父节点下拉、预制体区块）|
| `editor/prefabs.rs` | 258 | `show_content()`（资产列表 + 创建/实例化/删除弹窗）|
| `editor/toolbar.rs` | 144 | `show()`（含 ⌂ Projects / 🔨 Build） |
| `editor/console.rs` | 46 | `show_content()` |
| `editor/dock.rs` | 117 | `create_tile_tree()`、`EditorBehavior` |
| `editor/theme.rs` | 124 | `install()`、`unity_dark_visuals()` |
| `game/main.rs` | 517 | `GameRunner`（`ApplicationHandler`：`resumed` / `window_event` / `about_to_wait`）、`GameApp::new()`、`update()`、`render()`、`resolve_project()` |
| `tools/build-release.ps1` | 349 | `Resolve-Cargo`、`Enable-MsvcEnv`、`Invoke-CargoBuild` |
