# XGameEngine 开发计划

> 从"编辑器原型"到"可运行的游戏引擎"的演进路线。
>
> **这是一份历史实施记录（archive）**：每一节是"当时那一轮做了什么、为什么这样做、
> 量到了什么数据"。它不反映当前状态 —— 状态看 [`ROADMAP.md`](ROADMAP.md)，差距与
> 优先级看 [`Docs/差距分析.md`](Docs/差距分析.md)，不能推翻的结论看
> [`Docs/决策记录.md`](Docs/决策记录.md)。
>
> **索引**：零–十四 = 项目分离 / 编辑器 / 玩法层 / 阴影 / 交互硬化 / 帧调度；
> 十五–二十一 = 双轨语言架构 P0–P3（含性能闸门与删 Rhai）；
> 二十二–三十一 = 音频 / Agent-Native / PBR / HDR / Bloom / 天空 IBL / 实例化 /
> 编辑器 UI / 剔除 / FXAA。

---

## 零、引擎与游戏项目分离（已完成）

引擎（本仓库 `crates/`）与游戏项目（磁盘上任意目录）彻底分家。编辑器启动后先进入
**项目枢纽**，选定或新建项目之后才进编辑器主界面；所有资产路径都按**项目根目录**
解析。编辑器与独立游戏都不再依赖进程当前工作目录，项目目录可以随便移动。

```
XGameEngine/                  # 引擎源码仓库
├── crates/                   # 引擎 + 编辑器 + 游戏二进制
└── projects/<项目名>/         # 游戏项目（默认位置，可放到任意目录）
    ├── project.ron           # 项目清单：名称 / 入口场景 / 窗口 / 内容目录
    ├── Scenes/
    └── Resources/
```

关键改动：

| 位置 | 内容 |
|---|---|
| `xgame-core::project` | `Project` 清单 + `ProjectHandle`（open / create / save / 路径解析） |
| `xgame-editor::hub` | 项目枢纽界面：新建 / 打开 / 最近项目（原生目录选择器 `rfd`） |
| `xgame-editor::recents` | 最近项目持久化（`%APPDATA%\XGameEngine\recent_projects.ron`） |
| `xgame-editor::main` | `EditorApp` 拆成「项目枢纽 / 编辑器会话」两态 |
| `xgame-editor::scene_renderer` | 按项目根目录解析 `entity.model` |
| `xgame-game::main` | 接收项目路径，入口场景与资产均按项目根解析 |

> 遗留：`GameRuntime`（`xgame-editor/src/game_runtime.rs`）与 `xgame-game` 的物理
> 初始化 / 同步逻辑仍是两份重复实现。要彻底去重，需要把运行时提到独立 crate
> （见架构原则 3）。

---

## 一、现状：两个入口

当前架构经过两次重构，有两个可运行的二进制：

```
                          xgame-game（新）
                              |
xgame-core                    | 独立游戏二进制
    ↑                        ├── winit + wgpu 原生窗口
xgame-math                   ├── ScenePipeline 渲染
    ↑                        ├── PhysicsWorld 物理
xgame-render                 └── 场景从 RON 加载
    ↑
xgame-physics          xgame-editor（已有）
    ↑                       ├── eframe 窗口 + egui 面板
    └────┬──────            ├── ScenePipeline + 离屏纹理
         │                  ├── GameRuntime 封装物理
    xgame-editor            └── Play/Pause/Stop 模式
    xgame-game
```

### 渲染路径收拢

之前有两个平行的渲染实现，现在统一为 `ScenePipeline`：

```
旧：
  xgame-render::RenderContext（无人使用）
  xgame-editor::SceneRenderer（实际在用，内联 WGSL/pipeline）

新：
  xgame-render::scene_pipeline::ScenePipeline（共享）
    ├── WGSL 着色器
    ├── Pipeline + BindGroup + Uniform buffers
    └── 逐实体遍历 + draw call 发放
  xgame-editor::SceneRenderer → 薄 wrapper（离屏纹理 + egui）
  xgame-game::GameApp → 直接使用 ScenePipeline（swapchain）
```

### 当前能力

| 能力 | 编辑器 | 独立游戏 |
|---|---|---|
| 场景加载 (RON) | ✅ | ✅ |
| glTF 模型渲染 | ✅ | ✅ |
| Blinn-Phong 光照 | ✅ | ✅ |
| 固定步长物理 (60Hz) | ✅ | ✅ |
| 飞行相机 | ✅ | ❌（固定轨道相机）|
| Physics Debug 叠加 | ✅ | ❌ |
| Play/Pause/Stop | ✅ | N/A |
| 实体增删 | ✅（Play 模式守卫）| N/A |
| ScenePipeline 共享 | ✅ | ✅ |

---

## 二、三阶段路线图

```
阶段一（已完成）        阶段二（当前目标）        阶段三（远期）
┌─────────────┐       ┌─────────────┐       ┌─────────────┐
│ 层重构       │       │ 固定步长循环  │       │ 独立游戏二进制 │
│ GameRuntime  │ ───→  │ 编辑器持续集成 │ ───→  │ IPC/分离架构  │
│ 物理集成     │       │ Physics Debug │       │ 完整游戏管线  │
│ 渲染路径统一  │       │ 独立游戏二进制 │       │ 资源浏览器   │
│ 共享管线     │       │              │       │ Gizmo 操作  │
└─────────────┘       └─────────────┘       └─────────────┘
```

---

## 三、阶段二：编辑器强化 + 独立游戏

### 3.1 固定步长物理（已完成）

`GameClock` + `GameRuntime` 内部累加器，物理 60Hz 与渲染帧率解耦。

### 3.2 编辑器期间实体增删保护（已完成）

Play 模式下 Hierarchy 面板禁用增删。

### 3.3 渲染路径统一 + ScenePipeline（已完成）

从 `SceneRenderer` 提取 WGSL、pipeline 创建、uniform 管理和绘制逻辑到
`xgame-render::scene_pipeline::ScenePipeline`。编辑器离屏渲染器和独立
游戏二进制共用同一份渲染代码。

### 3.4 独立游戏二进制（已完成）

创建 `xgame-game` 二进制入口，使用原生 winit + wgpu 循环，
通过 `ScenePipeline` 渲染，直接运行 `Scenes/scene.ron` 中的场景。

### 3.5 Physics Debug 绘制（待完成）

- `GameRuntime` 已暴露 `debug_lines()`
- `PhysicsWorld::debug_lines()` 已实现
- `scene_view.rs` 已有叠加绘制代码
- 需要确认叠加层的视觉正确性（碰撞体线框对齐）

### 3.6 状态栏 Play 模式信息（已完成）

状态栏显示 `Playing | 3 entities | 60.0 FPS | Bodies: 3 | Colliders: 3`

### 3.7 场景层级 Scene Graph（已完成）

`Entity::parent`（按 ID 单向引用）+ `Scene::world_matrix` / `world_matrices` 世界矩阵
解算；层级面板是**可折叠的树**，拖拽重挂父节点保留世界变换并自动防环；渲染 / 物理 /
Gizmo 全部走世界变换；复制 / 粘贴 / 删除是**子树**语义。

### 3.8 预制体 Prefab（已完成）

把一棵实体子树存成 `Prefabs/*.prefab.ron` 资产并反复实例化：

- `xgame-core::prefab`：`Prefab { name, entities }` + `from_subtree()` 提取 +
  `instantiate()` 实例化（重映射 ID / 父引用，并贴上 `PrefabInstanceRef` 来源标记）；
- `Entity::prefab` 记录「来自哪份资产、对应资产里哪个实体、是不是实例根」；
- 编辑器 Prefabs 面板：列资产 / 搜索 / 点击实例化 / 从选中创建 / 删除；
- **Apply** 把实例改动写回资产，**Revert** 用资产覆盖回实例（保留实例根的世界位置）；
- 构建（🔨 Build）会把 `Prefabs/` 一并打进产物。

---

## 四、阶段三：完整游戏管线

### 4.1 资源浏览器

文件浏览器面板，拖拽 glTF 到场景中创建实体。

> 进度：**Prefabs 面板已落地**（列资产 / 搜索 / 实例化 / 删除）；
> 还缺 `Resources/` 里模型资产的列举与"拖进场景"。

### 4.2 Gizmo 变换工具

Scene View 中的拖拽操作柄：平移 / 旋转 / 缩放。

> 进度：三轴平移 / 旋转 / 缩放已可用（世界空间换算回局部）；**Rect 工具**与
> **吸附**（`gizmo_snap`）尚未接线。
>
> 轴拖拽的数学见 [FLOW.md](FLOW.md) 的「Gizmo 的三个坑」——轴上的位移量必须由
> **直线投影**（`axis_param_under_mouse`）求，不能用「过实体、垂直于轴的平面交点」，
> 后者在轴上的投影恒为 0。一次拖拽进撤销栈**一步**（拖拽前快照，松手才入栈）。

### 4.3 编辑器 - 运行时互操作

- 当前：编辑器内嵌 `GameRuntime`，原地运行
- 未来：子进程方案，编辑器启动 `xgame-game` 并通过 IPC 通信

### 4.4 后续大特性

按开发优先级：

1. **动画系统**：glTF 动画通道解析、关键帧插值、骨骼蒙皮
2. **音频系统**：kira/rodio 集成、3D 空间音频
3. **后处理**：阴影（CSM）、Bloom、ToneMapping、天空盒
4. **脚本系统**：Lua/Rhai 嵌入或 Rust 热重载

---

## 五、架构原则

1. **层间单向依赖**：core ← math ← {render, physics} ← editor / game
2. **渲染路径共享**：ScenePipeline 是唯一渲染入口，编辑器（离屏）和游戏（swapchain）共用
3. **GameRuntime 不依赖编辑器**：核心业务逻辑与 UI 解耦（当前位于 editor crate，后续可提取）
4. **固定步长优先**：物理 / 未来动画系统使用固定时间步长
5. **功能先于性能**：先跑通，再优化
6. **引擎与项目解耦**：引擎不假设游戏内容在哪；`project.ron` + `ProjectHandle`
   是访问资产的唯一入口，资产路径一律相对项目根目录解析
7. **编辑器是工具，不是项目的一部分**：编辑器二进制可打开磁盘上任意项目，
   游戏项目目录不出现在 `crates/` 里

---

## 六、文件清单

### 已完成

| 文件 | 阶段 | 说明 |
|---|---|---|
| `crates/xgame-core/src/transform.rs` | 一 | Transform 从 xgame-math 移入 |
| `crates/xgame-core/src/scene.rs` | 一 | Entity/Scene 从 xgame-render 移入 |
| `crates/xgame-editor/src/game_runtime.rs` | 一 | 编辑器无关的游戏运行时 |
| `crates/xgame-render/src/scene_pipeline.rs` | 二 | 共享渲染管线（WGSL + pipeline + uniform）|
| `crates/xgame-game/Cargo.toml` | 二 | 独立游戏二进制 |
| `crates/xgame-game/src/main.rs` | 二 | 独立游戏循环（现按项目根解析资产）|
| `crates/xgame-core/src/project.rs` | 零 | 项目清单 + `ProjectHandle` |
| `crates/xgame-editor/src/hub.rs` | 零 | 项目枢纽界面 |
| `crates/xgame-editor/src/recents.rs` | 零 | 最近项目持久化 |
| `projects/SampleScene/` | 零 | 从仓库根迁出的示例项目（原 `Scenes/` + `Resources/`）|

### 后续需创建

| 文件 | 说明 |
|---|---|
| `crates/xgame-editor/src/panels/resource_browser.rs` | 资源浏览器面板 |

### 后续需修改

| 文件 | 说明 |
|---|---|
| `crates/xgame-editor/src/panels/state.rs` | Gizmo 状态 |
| `crates/xgame-editor/src/panels/scene_view.rs` | Gizmo 交互叠加层 |

---

## 七、商业引擎对齐（本轮）

对照商业引擎补齐了一批最显眼的短板，详见 `ROADMAP.md` 的「商业引擎对齐」表。

关键改动：

| 位置 | 内容 |
|---|---|
| `xgame-core/src/light.rs` | 新增 `Light` / `LightKind`；光源是场景级数据 |
| `xgame-core/src/scene.rs` | `Scene` 增加 `lights` / `ambient`（`#[serde(default)]` 兼容旧文件）|
| `xgame-render/src/scene_pipeline.rs` | 引入 `group(1)` 材质绑定组、Blinn-Phong、多光源数组、透明管线 |
| `xgame-render/src/gltf_loader.rs` | 读 glTF 材质因子（base color / metallic / roughness / alphaMode）；新增 `GltfMesh::load` 统一入口 |
| `xgame-render/src/mesh.rs` | 新增内置图元生成（Cube / Sphere / Plane / Cylinder）与 `BuiltinMesh` |
| `xgame-editor/src/state.rs` | 新增 `UndoStack`（快照式撤销）、实体剪贴板、`create_primitive` |
| `xgame-editor/src/panels/inspector.rs` | 所有属性编辑接入撤销栈；新增内置图元快捷切换 |
| `xgame-editor/src/panels/hierarchy.rs` | 创建菜单加入 Create Primitive；新增 Copy / Paste 与快捷键 |

> 渲染管线改动是**破坏性 API 变更**：`ScenePipeline::render` 现在多收
> `device` 与 `SceneLighting` 两个参数，调用点（`scene_renderer.rs`、
> `xgame-game/src/main.rs`）已同步更新。

---

## 八、玩法层（本轮完成）

到目前为止引擎"能摆场景"，但还"不能做游戏"—— 缺的是**输入**、**脚本**，
以及一个把各子系统编排起来的**运行时**。这一轮的四个新 crate：

```
core ← math ← {render, physics}
  ↑        ↑
  └── input / script        ← 设备抽象与脚本宿主
             ↑
         runtime            ← 编排：场景 + 物理 + 输入 + 脚本
             ↑
     editor(Play) / game    ← 两个宿主共用同一段 update
```

| 位置 | 内容 |
|---|---|
| `xgame-input` | `KeyCode`/`MouseButton`（与 winit 解耦）、`InputState`（按住 / 本帧按下 / 本帧松开）、`InputMap`（项目级 `input.ron`：多绑定 / 修饰键组合 / 双向轴）；窗口库翻译只在接窗口的二进制里发生 |
| `xgame-script` | 嵌入式 Rhai；`ScriptHost` 负责编译缓存、生命周期派发、命令回收；宿主 API 覆盖变换 / 实体 / 物理 / 输入 / 时间 / 日志；`state_*` 提供按 `(实体, 脚本)` 分开的持久状态 |
| `xgame-runtime` | `World`：唯一编排者。固定步长与渲染帧解耦；**动态体由物理驱动、运动学体由脚本驱动**；`FrameReport` 供 UI 读 |
| `xgame-core` | `Entity::{scripts, physics}`、`PhysicsBody`/`CollisionShape`/`BodyKind`、`PhysicsHandle`（运行时组件）、`Ray`/`RayHit`、`Light::{casts_shadow, shadow_resolution}`、`ContentConfig::scripts_dir` |
| `xgame-physics` | `scene_body::body_from_scene`（声明 → rapier 刚体）、`raycast` / `raycast_with_filter` / `RaycastFilter`、`set_velocity` / `teleport` / `set_rotation` / `sync_queries` |
| `xgame-editor` | `game_runtime.rs` 变成 `World` 的薄壳（场景所有权用 `mem::swap` 交接，O(1)）；Inspector 的 Scripts / Physics 区块；Hierarchy 徽标；工具栏 `⟳ Scripts`；`input_bridge` 把 egui 键鼠灌进运行时 |
| `xgame-game` | 改用 `World`；窗口事件翻译成引擎输入；读项目 `input.ron`；脚本日志转发到 `log` |

### 两个刻意的取舍

1. **场景所有权用 `mem::swap` 交接，而不是把场景包进 `Rc<RefCell<_>>`。**
   `World` 拥有场景，但编辑器几十处面板代码都直接读 `state.scene`。改所有权模型
   要动几十个调用点；包 `RefCell` 会把借用检查推到运行期。交换整个 `Scene` 是
   **O(1)**（换三个指针），比每帧克隆整份场景便宜几个数量级。
   前提是"`update` 期间没人读 `state.scene`"——这一点写在 `update_with_scene` 的注释里。

2. **脚本的结构性改动立即生效，而不是攒成命令延后执行。**
   延后执行会让 `let id = spawn_entity(...)` 之后的 `set_entity_pos(id, ...)`
   静默失效 —— 而"先生成、再摆位置"是最自然的写法。立即执行之所以安全，
   是因为派发脚本时遍历的是一份 `(实体 ID, 脚本路径)` 快照，且所有查找都按 ID，
   增删实体最多让后面的脚本少看到一个实体。运行时通过一份**通知**
   （`EntitySpawned` / `EntitiesRemoved` / `Reparented`）知道该补建 / 清理哪些刚体。

<!-- markdownlint-disable-file -->

---

## 九、渲染质量：方向光阴影（本轮）

`Light::casts_shadow` / `shadow_resolution` 从"已进场景、渲染侧没接"变成真的生效。
做法是经典 shadow mapping，落在共享的 `ScenePipeline` 里 —— 编辑器离屏渲染与独立
游戏因此**同时**获得阴影，不需要各自接一遍。

| 位置 | 内容 |
|---|---|
| `xgame-render/src/gltf_loader.rs` | 新增 `Bounds`（AABB）；`GltfMesh` 记录本地包围盒（三条构造路径都填） |
| `xgame-render/src/scene_pipeline.rs` | 阴影深度图 + 比较采样器 + `ShadowUniform`；独立的深度-only 阴影 pass 与 `shadow_bgl` / `shadow_pipeline`；`GpuLight.shadow_params` 标记使用阴影的灯；主片元用 3×3 PCF；纯函数 `scene_bounds` / `fit_directional_shadow` / `pick_shadow_slot` |
| `xgame-editor/src/scene_renderer.rs` | 指纹加入 `casts_shadow` / `shadow_resolution`（漏掉的话勾选后画面不动） |

### 三个刻意的取舍（别推翻）

1. **正交视锥按"场景世界包围盒的外接球"拟合**，而不是固定大小的盒子。阴影贴图分辨率
   固定，视锥开太大 → 精度被稀释成噪点；开太小 → 边缘物体没影子。用球（而不是贴合
   方向的盒）是因为光源角度任取时球在各个方向投影都一样大，不必为每个角度重算 ——
   代价是浪费一些纹素，对"先把阴影做出来"是划算的。
2. **只支持一盏方向光**（第一盏 `casts_shadow` 的方向光）。多盏各自的阴影贴图意味着
   每盏一次全场景重绘。点光阴影（6 面立方体贴图）明确不在这一版范围内。
3. **阴影 pass 用独立的 `shadow_bgl`**，不含深度贴图本身。若沿用全局布局，就会在
   "把深度画进 `shadow_view`"的同时又在 group(0) 里绑同一张图 —— wgpu 判为
   "同一资源既作渲染目标又作绑定"。

### 验证

- 单测 +12：`Bounds` 求并/变换/空盒退化；`fit_directional_shadow` 把场景包围盒的
  八个角点全部映射进 NDC `[-1,1]²` 与深度 `[0,1]`（**这是阴影能工作的前提**）、
  深度顺序与"离光源远近"一致、零方向不产生 NaN、空场景返回 `None`；
  `pick_shadow_slot` 跳过点光与 `casts_shadow=false`；`build_lights` 把标记落在
  正确的下标上。
- 真机 A/B：同一场景、同一机位，只切 `casts_shadow` → 14343 像素变化，且差异区域
  **正好**是柱子在地面上的投影范围。
- 编辑器打开示例项目：60 FPS、无 wgpu 校验错误。

### 遗留

- **无色调映射**，太阳强度一高地面就过曝成纯白（示例项目 `intensity: 3.2` 就是）。
  这属于第九阶段剩下的 ToneMapping（ACES）。
- 阴影没有级联（CSM），大场景远处会糊。

---

## 十、稳定性与交互修复（本轮）

用户报「界面卡顿 / 物体拖拽抽搐」。先量后改：给管线与编辑器加临时探针，用真机悬停流
和关 vsync 的播放器拿到数据，再逐个定位。

### 一、找到并修掉的真 bug

**1. 轴拖拽是正反馈 —— 这就是"拖拽抽搐"的根因**

`axis_param_under_mouse` 量的是「过 `ctx.entity_pos`、方向为轴」的直线，而 `ctx` 每帧
按**已被拖走的**当前位置重建 → 参考线跟着物体跑，`s_now` 里混进"物体自己刚挪的那一段"：

```text
Δ_k = A_k − Δ_{k-1}     ← 增益 −1 的正反馈，极点 z = −1（奈奎斯特）
```

物体会在相邻两帧之间**来回跳**，稳态只走到鼠标位移的一半，还把鼠标的像素级噪声放大成
可见抖动。修法：`GizmoDrag` 新增 `axis_anchor`（拖拽起点的世界位置），轴坐标一律以它
为参考线原点（`axis_param_anchored`）。

**2. 拾取用包围球 —— "想选的选不中、想取消的取消不掉"**

旧实现半径取 `scale.length() * 0.5`。一块 `16 × 0.5 × 16` 的地面半径算成 11.3，而相机
离场景中心只有 8.7 —— **相机落在球内部**，任何射线都命中它：点天空也选中那块地面。
改成「网格本地包围盒（`GltfMesh::bounds`）经世界矩阵变换成 AABB」后做 slab 求交。

**3. 选中索引可能越界 → 编辑器直接 panic 退出**

`selected_entity` 是按位置记的索引，而改实体数量/顺序的操作有十几处（撤销 / 删除 /
复制 / 粘贴 / 预制体实例化与还原……）。任何一处漏同步，下一帧 `entities[idx]` 都会
panic。新增 `EditorState::clamp_selection()` 在**每帧入口**夹一次（O(1)），并让
gizmo 路径改用 `get` / `get_mut` 兜底。

**4. 阴影视锥半径不封顶 → 有物体飞远时深度精度崩掉**

示例项目里就有自由落体掉出视野的箱子。包围盒被它拉大 → 深度范围拉长 → 固定分辨率的
阴影贴图精度当场崩。加 `MAX_SHADOW_RADIUS` 封顶（超出范围的不投影）。

**5. `hub_is_quiet_when_idle` 是不稳定测试（既有）**

枢纽里的 `ScrollArea` 滚动条淡入动画期间会请求重绘（正常、短暂），而旧测试用墙钟只跑
4 帧 —— 帧快就"动画没结束"、帧慢就"已结束"，于是时灵时不灵（实测单独跑必挂、和几十个
测试并行反而常过）。改成给 egui 显式时钟推进到动画结束再断言。

### 二、测出来的结论（都没问题，附证据）

| 项目 | 数据 |
|---|---|
| 阴影 pass 的代价 | 同一静态场景、关 vsync，只切 `casts_shadow`：p50 **0.32ms vs 0.29ms** —— 差 ~0.03ms，可忽略 |
| 悬停时的离屏重画 | 666 次鼠标移动里**只重画 1 次**；渲染目标只重建 1 次（初始） |
| 应用侧每帧开销 | 无热点；`dock.rs` / `theme::install` / 面板都没有每帧磁盘 IO 或整场景克隆 |

遗留（外部因素，非应用代码）：

- 拖动窗口时 Windows 的模态移动循环（`WM_ENTERSIZEMOVE`）会节流出帧 —— winit 0.30 上游
  问题。
- 窗口被遮挡时 DXGI present 会阻塞（实测：无人值守跑播放器 6 秒只有 272 帧、dt 被
  250ms clamp 反复触发），前台运行不受影响。

### 三、验证方式的一个新教训

真机投递合成鼠标输入在这个环境里**不可靠**：同一脚本同样的坐标，两次运行结果不同
（一次点中、一次没反应），`SetForegroundWindow` 在调用方不是前台进程时会静默失败。
所以交互类修复以**进程内测试**为准（`egui::Context::run` 合成事件驱动 `show_content`），
真机只做"能起来、渲染正常、无 GPU 校验错误"的冒烟。

配套要求：**每条回归测试都必须先证明它在旧实现下会红**。本轮 4 条新测试全部做过这个
对照（临时把实现改回旧写法，确认失败，再改回来）。

---

## 十一、手感优化：MSAA + 帧序 + 飞行相机（本轮）

用户报「右键转视角 + 同时移动相机时抽搐卡顿、画面锯齿严重、拖拽有严重延迟」。
三件事三个原因，都不是"性能不够"：

### 一、画面锯齿严重 → 全流程 sample_count = 1

一次都没开过 MSAA。现在：

- `ScenePipeline::new(device, format, sample_count)`；`render()` 多收一个
  `msaa_color_view`，开了就把它当颜色附件、把最终视图设为 `resolve_target`。
- 采样数**由适配器能力决定**（`supported_sample_count`）：不支持的格式硬开 4×
  会让管线创建直接失败。颜色附件与深度附件的采样数必须与管线一致。
- 编辑器：多采样颜色 + 多采样深度 → resolve 到交给 egui 的**单采样**纹理（egui 只认
  单采样）。游戏：多采样颜色 → resolve 到**交换链视图**（交换链视图本身不能多采样）。
- 阴影 pass 单独一条管线、永远 1×（只存深度，不需要抗锯齿）。

**验证**：同一场景同机位，旧（无 MSAA）vs 新（4×）。边缘放大图肉眼可见台阶消失；
场景视图上半区（避开 gizmo）的"硬台阶"像素数 438 → 307（−30%）。

### 二、拖拽有严重延迟 → 离屏渲染排在输入处理**之前**

`show_content` 原来是「先重画 3D → 再处理输入」，于是这一帧画出来的是**上一帧**的
相机与场景 —— 拖物体、转视角都白白多一帧延迟，手感发"黏"。

改成「飞行相机 → 交互（gizmo / 拾取 / 快捷键）→ 才重画 → 叠加层」。叠加层必须画在
3D 之上，所以 gizmo 被拆成"更新状态"与"绘制"两步（`handle_gizmo_interaction` 现在
只改状态并返回 `GizmoCtx`）。

**验证**：`the_frame_is_rendered_after_this_frames_camera_input` —— 把渲染那一刻用的
`view_proj` 记进 `EditorState::last_render_view_proj`（`#[cfg(test)]` 字段），断言它等于
本帧输入处理完之后的相机矩阵。旧帧序下这条会红（已对照验证）。

### 三、转视角抽搐 → 飞行相机自身的两条规则

1. **旋转不再逐帧要求"指针压在视口上"**。飞行态是"右键按下且指针在视口里"那一刻开启
   的；开启之后仍逐帧用 `hovered()` 当闸门的话，光标被带出视口时旋转**卡住**、移回来
   又**跳一下** —— 快速转视角就是"抽搐"。现在飞行态一开就一路转到松手。
2. **位移改用 `stable_dt`（平滑真实帧间隔），不用 `predicted_dt`**。后者是"下一帧大概
   多久"的预测，预测不准时同一段按键时长走出不同距离，速度一冲一顿。编辑器的物理步进
   与 FPS 读数一并换成 `stable_dt`（FPS 用预测值会把卡顿藏起来）。

副带影响：`fly_state` 现在是**每帧由右键推导**的，`tool_shortcuts_yield_to_fly_camera`
测试里"直接写死 `fly_state`"的写法不再成立，已改成真的按住右键（更贴近真实路径）。

**验证**：`check` / `clippy` 0 警告；`test --workspace` **148 全绿**；真机截图：
编辑器与播放器都 `MSAA x4`、无 GPU 校验错误。

---

## 十二、全面硬化：稳定 / 流畅 / 性能（本轮，无新功能）

用户要求"全面完善整个引擎，暂不添加新功能，先做到稳定、流畅、性能强劲"。
所以这一轮**不加任何能力**，只做三件事：先量后改、把每帧的重复开销拿掉、把会崩 /
会涨的地方堵上。

### 一、先建量尺（这是本轮最重要的产出物）

新增 `crates/xgame-runtime/tests/update_throughput.rs` —— 一个分四档的微基准，
默认 `#[ignore]`，手动跑：

```sh
cargo test -p xgame-runtime --release --test update_throughput -- --ignored --nocapture
```

刻意分档的理由：只跑一档很容易把 **rapier 的耗时误记到引擎头上**。
`纯视觉` 档把物理完全排除在外，量的才是引擎自己的每帧开销。

| 场景（2000 实体） | 改前 | 改后 | |
|---|---|---|---|
| 纯视觉（无物理） | 226 µs/帧 | **13 µs/帧** | ≈17× |
| 全静态体 | 1026 µs/帧 | **89 µs/帧** | ≈11.5× |
| 稀疏动态 ×64 | 1685 µs/帧 | **724 µs/帧** | ≈2.3× |
| 密集接触 ×1000 | 13989 µs/帧 | 13005 µs/帧 | 物理主导 |

**第四行同样重要**：1000 个互相挤压的动态体吃掉 13 ms/帧，那是 rapier 求解器的成本，
不是编排层的。这一条把"下一步该优化哪儿"讲清楚了 —— 想在那档提速得动作业系统 /
减少接触岛，不是继续抠编排。

### 二、运行时热路径（收益主要来自这里）

1. **`sync_bodies` 的稳态快路径。** 旧实现无条件跑三段扫描（补建 + 建存活 `HashSet` +
   清理），最后还无条件 `sync_queries()` 重建射线 BVH —— 而 `physics.step()` 每步已经
   维护了查询流水线，稳态再重建一次纯属浪费。现在先做**一次无分配扫描**同时数出
   "待补建"与"物理实体总数"，两者都表明集合没变就整段跳过。
   判据完备性写在 `sync_bodies` 的文档注释里（新增 / 删除 / 一增一删抵消 三种情形都覆盖）。
2. **`push_kinematic_bodies` / `pull_dynamic_bodies` 早退。** "场景里有没有运动学 /
   动态体"由上面那次扫描顺带记进 `has_kinematic` / `has_dynamic`，两步据此直接返回 ——
   纯静态场景连世界矩阵都不解算，也不再各自扫一遍全场。
3. **`world_matrices_with_parents()`。** `pull_dynamic_bodies` 每步两样都要（父索引取父
   矩阵、世界矩阵做局部换算），旧写法先 `parent_indices()` 再 `world_matrices()`，而后者
   内部又要算一遍父索引 —— 每个固定步白跑一次 `hierarchy()`。
4. **`world_matrices` 自身两处。** (a) 链缓冲从循环内提到循环外复用 —— 平铺场景
   （很常见）原来每算一个实体就为一条长度 1 的链分配一次，整次调用 n 次小分配；
   (b) 输出用 `Mat4` + done 位图取代 `Vec<Option<Mat4>>`（`Mat4` 没有 niche，
   `Option` 会从 64 字节涨到 80）。
5. **`Scene::parent_indices()` 不再白建子表。** 原来它转调 `hierarchy().0`，而
   `hierarchy` 会顺手 `vec![Vec::new(); n]` 建一整张子表 —— 平铺场景全是空 `Vec`，
   纯浪费。

### 三、渲染与编辑器热路径

6. **模型矩阵批量上传。** `ScenePipeline::render` 原来每个实体一次 `queue.write_buffer`
   （每次都要从 staging belt 领一段 + 记一条拷贝命令，256 实体就是 256 次）。
   现在攒进管线持有的复用 `Vec<u8>`（按 `uniform_alignment` 排布），最后**一次**写完。
7. **指纹不再需要先解算世界矩阵。** `SceneRenderer::scene_signature` 原来收世界矩阵，
   而算世界矩阵要先跑一遍 `world_matrices()`（几条长度为 n 的 `Vec` + n 次矩阵连乘），
   是**每一帧**（哪怕什么都没动）都要付的固定成本。世界矩阵是 `(局部变换, 层级)` 的
   纯函数 ⇒ 改哈希「局部变换 + 父引用」是等效的，却让静态帧的指纹变成零分配、零矩阵
   运算。父引用必须一起哈希，回归测试 `signature_notices_reparenting` 盯这条
   （已做"去掉父引用即变红"的对照验证）。

### 四、稳定性

8. **`PhysicsWorld::add_collider` 不再 panic。** 旧实现 `.expect("unknown body")` ——
   一次把 body_id 传串（或脚本在刚体已删之后挂碰撞体）就带走整个进程。现在返回
   `Option`、记警告、退化成"少一个碰撞体"。配套测试确认不 panic 且不留残留记录。
9. **控制台消息表加上限（1000 条，成批丢最旧）。** 去重只挡得住"连续完全相同"的消息；
   一个每帧打一条**不同**日志的脚本能在几分钟内把表撑到几十万条。成批丢（200 条/次）
   而不是逐条 `remove(0)`：后者是 O(n) 整表搬运，而 `log` 在刷屏时就是每帧热点。
10. **复核过、判定不需要动的两处**（记录决策，免得下次又查一遍）：
    - `draw_physics_debug_lines` 只在用户显式打开 `show_physics_debug`（默认 false）时
      才跑，不在常规热路径上；改成复用缓冲要把 `&EditorState` 变 `&mut`，风险大于收益。
    - 层级面板的 `scene.hierarchy()` 里 `vec![Vec::new(); n]` 只有 **1 次**堆分配
      （`Vec::new()` 不分配堆），不是热点。面板真正的成本是 egui 逐行渲染，
      那属于"面板虚拟化"，不在本轮范围内。
    - 撤销栈本来就有 100 步上限；模型 / 材质缓存按"每个模型路径只加载一次"有界，
      当前没有模型热重载路径，所以不会无界增长。

### 五、验证

- `check` / `clippy --workspace --all-targets`：**0 警告 0 错误**。
- `test --workspace --all-targets`：**154 全绿**（新增 6 条：控制台上限、clamp_selection、
  `add_collider` 不 panic、指纹认父引用、指纹认旋转/缩放、深链世界矩阵、成环世界矩阵）。
- 归因对照：`signature_notices_reparenting` 已做"去掉父引用即变红"验证。
- 基准：见上表；`dist/` 刷新后做真机冒烟（编辑器打开示例项目正常渲染、无 GPU 校验错误）。

---

## 十三、交互延迟：空闲空转 + 缩放重建（本轮）

用户报「窗口操作有延迟、拖拽物体有延迟、窗口拖拽会卡顿」。
上一轮我把"拖窗口卡顿"归给了 winit 上游，这次用**先量后改**推翻了这个结论 ——
真正的原因是编辑器**从来没闲下来过**。

### 一、怎么找到的（方法本身值得记下来）

1. `cpu_probe.py` 量空闲 CPU：**5.2–7.2%**。一个什么都不做的编辑器不该吃 CPU。
2. 加临时探针量"每秒帧数"：**空闲时 61 FPS**，且 `renders=0`（3D 没重画）——
   说明是 egui 在空转，不是 3D。
3. 逐段插桩问"谁请求的重绘"：全都不认账 —— 说明请求发生在**我们的 UI 代码之外**。
4. 换成 **egui 自带的 `Context::repaint_causes()`**（0.30 有，带调用点 `file:line`），
   直接指名道姓。这一步把"猜"变成了"看"。
5. 再量"每帧有几个输入事件"：**空闲时约 160 个/秒**，且种类只有一种 ——
   `PointerMoved([1073.0 517.0])`，**坐标始终不变**。

### 二、根因与修法

**Windows 在指针静止时会反复投递同一坐标的 `WM_MOUSEMOVE`**，egui-winit 逐条转成
`PointerMoved`；而 egui 在帧末只要看到「本帧有输入事件」就请求**立即**重绘
（`InputState::wants_repaint_after` 的 `!events.is_empty()` 那一支）。于是：

```text
WM_MOUSEMOVE(同一坐标) → PointerMoved → egui 认为"要再画一帧"
   → 编辑器每 16.7ms present 一次 → 窗口的模态移动/缩放循环抢不到消息线程
   → 拖窗口卡顿、拖拽/缩放有延迟
```

**修法**（`drop_redundant_pointer_moves`）：在 `EditorApp::update` 开头丢掉**位置与
上次放行位置相同**的 `PointerMoved`。位置相同的指针事件不携带任何新信息 —— egui 的
指针状态本来就停在那儿，所以这是语义上的 no-op。`PointerGone` 时把记录清空，
否则"离开再回到同一点"会被误判成没动、悬停从此失效。

> 实测：空闲 CPU **5.2–7.2% → 0.00–0.20%**；编辑器真正闲下来了。
> 交互侧没有退化：合成悬停时 `events≈59/s`（真实移动逐帧放行），拖拽正常。

### 三、顺带修掉的第二处：缩放窗口每帧重建离屏目标

`set_viewport_size` 在拖边框时每帧都被调，旧实现每次都重建 3 张纹理
（单采样颜色 / **MSAA 颜色（4 倍采样，最贵）** / 深度）+ 注册一张新 egui 原生纹理 +
释放旧的；尺寸一变指纹必变，于是还要每帧全量重绘一次 3D。

**实测一次拖拽（约 1.7 秒）：重建离屏目标 43 次、重绘 43 帧。**

改成**节流重建**（`plan_resize` 纯函数 + `RESIZE_REBUILD_INTERVAL = 100ms`）：

| 情况 | 动作 |
|---|---|
| 还没有渲染目标（首帧） | 立刻重建（否则第一帧是空白） |
| 尺寸与上一帧请求相同（= 用户松手了） | 立刻按最终尺寸重建 |
| 距上次重建已超过 100ms（还在拖） | 重建一次，让视口保持约 10Hz 更新 |
| 其余（还在拖、又刚重建过） | 本帧跳过 3D，画面由调用方拉伸上一帧纹理 |

> 实测：**43 次 → 1 次**。缩放结束后截图确认视口比例正确、无拉伸残留、阴影正常。

### 四、验证

- `check` / `clippy --workspace --all-targets`：0 警告 0 错误。
- `test --workspace --all-targets`：**161 全绿**（新增 7 条：指针过滤 2 条 +
  缩放节流状态机 5 条）。指针过滤那条已做"把过滤器改成 no-op 即变红"的对照验证。
- 真机：空闲 CPU 5.2–7.2% → 0.00–0.20%；缩放重建 43 → 1；缩放后画面正确。

### 五、下一层能做的（本轮没做，先记下）

- 窗口**移动**期间的出帧仍然由 winit 的模态循环决定（上游行为）。若要进一步压到
  零，需要 `SetWindowLongPtr` 子类化窗口、拦截 `WM_ENTERSIZEMOVE`/`WM_EXITSIZEMOVE`
  主动停帧 —— 平台相关且侵入，收益待量。
- 游戏侧（`xgame-game`）本来就是连续出帧，未受影响。

## 十四、帧调度器：frame_policy（本轮）

### 一、问题

用户报"鼠标没事件时 FPS 不刷新、编辑器用起来还是发涩"。定位：上一轮修掉空闲空转后，
编辑器退化成**纯事件驱动**（eframe 默认）—— 全工程只剩 Play 模式与飞行相机两处
`request_repaint`，Edit 模式下"来一个事件才算一帧"，鼠标一停画面冻住。

### 二、方案

- `crates/xgame-editor/src/frame_policy.rs`：三档策略（Always / Hybrid / Reactive，
  `XGAME_FRAME_POLICY` 切换）+ `FrameClock`（宽限期 220ms）+ 纯函数 `plan()`。
  决策输入：`playing` / `interacting`（指针任意键按住）/ `had_input` / 宽限期。
- `frame_probe.rs`：`XGAME_FRAME_LOG` 每帧一行帧间隔日志（探针**不用 BufWriter**，
  `taskkill /F` 收尾会丢尾段数据）。
- `EditorApp::update` 统一决策，**全工程唯一的主动 `request_repaint`**；Play 模式与
  飞行相机的散装请求全部移除。
- 顺带修掉：`drop_redundant_pointer_moves` 同时丢弃 `Event::MouseMoved`（原始位移，
  传感器噪声载体，实测指针静止时以它续命宽限期 ~16fps）。

### 三、实测（同机同脚本三档对比）

- 拖拽：Hybrid 61.2fps / p99 35.6ms ≈ Always（60.0 / 34.7）；Reactive（旧）58.6fps
  但死时间 30%（发涩的量化）。
- 空闲：Hybrid 帧数约为 Always 的 1/3；完全无输入时整段归零（实测 6.27s 零帧空档）。
- 测量注意：这台机器有真实指针活动（漂移 + 偶发按键），idle 平均 fps 会被污染，
  判"是否真停"要看有没有连续零帧空档。

### 四、验证

- clippy 0 警告；workspace 测试全绿（新增 frame_policy 7 条 + MouseMoved 过滤 1 条，
  后者做过"禁用过滤即红"的对照）。
- 真机冒烟：无 panic、无 GPU 校验错误。

### 五、遗留

- ~~p90≈33ms 的双峰（vsync 相位）三档一致，属 eframe/vsync 层，策略管不到。~~
  已在 10-12 第三轮定位为交换链 acquire+present 双重等待（33ms 输入盲区），
  以 `present_mode = AutoNoVsync` 治本，见 FLOW「上屏节奏」节。
- eframe 层"事件到达即请求重绘"无法从应用侧拦截（噪声事件仍会各触发一帧，
  但不再连锁续命）。

---

## 十五、双轨语言架构 P0：门面 + 方法层 + Rust 组件 + 插件边界（本轮）

按 `Docs/双轨语言架构计划.md` §五 的六件事执行，**方法层先行**（其余五项都依赖
它的形状，现有 Rhai 测试当平移回归网）。六件事全部完成，验收七条全过。

### 一、六个产物

1. **`GameCtx` 方法层**（`xgame-script/src/ctx.rs`）：api.rs 约 750 行的函数体
   平移为 `impl GameCtx` 方法（含 辅助函数 + RayHit）；`api.rs` 每个函数退化为
   ≤3 行转发壳；`ScriptCtx` 瘦身为 { GameCtx 地址束, rhai state }；
   `host.rs::run_hook` 构造 GameCtx。15 条 Rhai 测试语义零改动，全绿。
2. **`xgame` 门面 crate**：`ENGINE_VERSION`（版本握手基准）+ prelude 拍平深路径；
   `xgame-game/main.rs` 的 7 行 `xgame_render::`/`xgame_script::` 直引清零。
3. **`Lifecycle` trait**（`xgame-runtime/src/lifecycle.rs`）：四钩子默认空实现，
   与 Rhai `Hook` 同名同语义；`Ctx` 是 `GameCtx` 的类型别名。
4. **组件分发**（`world.rs`）：`run_dispatch` 平行加组件路径，沿用字段级拆借；
   `on_destroy` 接线在命令消费时（实体已离场，与脚本轨同款时机）。
5. **注册表 + `Entity.behaviors`**：`register::<T>("Name")`；behaviors 与 scripts
   完全同款的 serde 兼容模式，旧场景文件加载测试通过。
6. **插件边界**（`xgame/src/plugin.rs` + `examples/spinner-plugin`）：
   `export_plugin!` 宏生成 `unsafe extern "C" fn xgame_plugin_entry`；
   `#[repr(C)] PluginHost { EngineApi{version, log}, registry 句柄 }`；
   版本码 = `0x58` 魔数 + 三段 8 位（永不为 0，不与错误码撞）；`load_plugin`
   双侧握手（插件拒载 + 日志报自身版本，宿主翻译成可读报错）。

### 二、刻意的取舍（三处对计划文档的偏离，已回写其 D6）

1. **`GameCtx` 落在 `xgame-script` 而非计划文档 C1 写的 `xgame-runtime`**：
   api.rs 转发壳要调 GameCtx，放 runtime 会形成 script → runtime 反向依赖；
   script 现有依赖恰好覆盖 GameCtx 所需，零新增。
2. **`EngineApi` 只带 `version` + `log`**，不带 C4 列的 pos/set_rot 等 ~10 函数：
   C1 落地后那些**全是 `Ctx` 的方法**，钩子本来就拿到 `&mut Ctx` —— 再铺裸函数表
   等于同一套能力两份真相源。只留钩子之外（入口期）用得到的东西。
3. **组件热路径零哈希**：钩子函数指针在装配期拷进 `BehaviorInstance`，`start`
   标记从 `HashSet<EntityId>` 下沉到组件自身。理由与数据见下节。

### 三、先量后改：组件分发微基准（验收第 7 条）

新增 `crates/xgame-runtime/tests/behavior_dispatch_throughput.rs`（`#[ignore]`，
1000 实体纯视觉三档：无轨道基线 / no-op 组件 / no-op Rhai）：

| 档 | µs/帧 |
|---|---|
| 无轨道基线 | 4.36 |
| 组件轨分发（扣基线） | 68.95 → **37.61**（验收线 50 µs） |
| 脚本轨分发（扣基线） | 493.89（= 组件的 **13.1×**） |

首跑 68.95 µs 超线（验收线先红），归因：每帧 3 阶段 × 1000 实体 × 两次
`HashMap<String>` 字符串哈希（装配期解析好的钩子每帧重新查表）+ Start 阶段
`HashSet<EntityId>` 哈希。改法即取舍 3，两处哈希归零；脚本轨不受影响。

### 四、先证红：组件测试首跑全红揪出装配 bug

三条组件测试（驱动实体 / on_destroy / 未注册跳过）首跑全红 —— 钩子一个没跑。
根因：`World::new` 在空注册表下装配，在实例表里插入"未注册"空占坑；
`instantiate_behaviors` 的 `contains_key` 短路让注入注册表后的重装永不发生。
修法：`World::new` 不再装配（注释写明理由）；`with_behavior_registry` 先清实例
再重装配；分发早退判据改 `declared == 0`。

### 五、验证

- workspace **280 项测试全绿**；clippy 0 警告（顺手修掉 editor 一条既有的
  `collapsible_match`）。
- 端到端：`examples/spinner-plugin`（cdylib）经 libloading 加载 → 版本握手 →
  注册 Spinner/Reaper → `World` 驱动四钩子（调用计数经 `#[no_mangle]` 导出符号
  回读：start 恰好 2、on_destroy 恰好 1）→ Reaper 销毁 target 全链路走通。
- dll 缺失回退：单独 `cargo test -p xgame` 时集成测试现场触发嵌套
  `cargo build -p spinner-plugin`（工作线程 + 超时，防与外层 cargo 抢锁死锁），
  实测可用。
- 版本不匹配拒载：`run_entry` 单测（拒载 + 经日志报自身版本 + 注册表不被写入）
  + `handshake` 报错文案含两端版本，可读性断言在案。

### 六、遗留

- 脚本轨 `dispatch_destroy` 仍无调用方（历史遗留，勿提前接线）：组件轨的
  `on_destroy` 在命令消费时派发；脚本轨接线需要命令携带脚本路径，留待后续
  （避免"命令在帧内何时带路径"的语义漂移），已写进 lifecycle.rs 文档。
- 插件 panic 隔离（D5 的 `catch_unwind` 纪律）本轮未做：P0 边界是同版本工程内
  自用（示例插件 + 集成测试），隔离留到插件生态开放时（P0.5 PIE 之后）。
- 编辑器只编辑数据：Behaviors 字段 Inspector 暂不显示（C5 范围声明）；
  编辑器内 Play 跑 Rust 插件（PIE）是 P0.5。

---

## 十六、双轨语言架构 P0.5：编辑器 PIE（后台编译 + 插件会话）

接 §十五。目标是把 UE 模式那条"改代码 → 按 Play → 自己就跑起来了"的链路打通，
验收七条全部完成。四个产物：

### 一、四个产物

1. **`plugin_build.rs`（编辑器）**——探测 / 新鲜度 / 后台构建 / 产物同步。
   探测只看"项目根有没有 `Cargo.toml`"（一次 `stat`）；新鲜度是纯时间戳比较的
   纯函数 `is_fresh`（三个输入：`Plugins/` 里每个 DLL 的 mtime、项目源 mtime、
   编辑器 exe mtime），新鲜就**连 cargo 进程都不起**；过期才 `spawn_build` 起
   后台线程跑 `cargo build --message-format=json-render-diagnostics`，产物路径
   从 stdout 的 JSON 行解析（比扫 `target/` 可靠），诊断走 stderr 直接进 Console。
2. **`plugin_session.rs`（编辑器）**——DLL 句柄保活 + 每次 Play 重建注册表。
   句柄只增不减：旧代挪进 `_retired`，`Drop` 里 `mem::forget`，**永不 FreeLibrary**
   （UE Hot Reload 同款策略，理由见 D7）。每次 Play 调 `build_registry()` 在已
   加载的 DLL 上重跑一遍入口（`LoadedPlugin::register_into`，微秒级）拿到全新
   `BehaviorRegistry`，保证"停止再播放"回到干净状态。
3. **编辑器状态机**——`enter_play_mode` 改成"先看要不要编译"：过期就转后台
   构建 + `pending_play = true` + 退回 Edit，编译完成后由每帧入口的
   `poll_plugin_build` 同步产物并自动进 Play。UI 全程不阻塞，工具栏显示
   转圈 + 已用秒数，并提供手动「🔨 Build」按钮。编译失败留在编辑态、错误进
   Console，**不阻塞 Play**（项目里可能还有上次的产物）。
4. **玩家与打包器**——`xgame-game` 启动时 `load_plugins_dir("Plugins")`；打包器
   把 `Plugins/` 复制进 `Build/<项目名>/Plugins/`，并在"有 Cargo.toml 却没 DLL"
   时把警告塞进 `BuildReport.warnings`（走报告而不是 `log::warn!`，因为那条只到
   stderr，而这个警告恰恰是点图形界面按钮的人需要看见的）。

另外补了一块 UI：Inspector 在 Scripts 之后加 **Behaviors** 区（组件名列表 + ✕ +
`+ Add Behavior` 菜单，未注册的名字标红 ⚠）—— 与 Scripts 完全同构。

### 二、两个真 bug：都由测试先证红抓出

1. **`needs_build` 写反**（返回了 `is_fresh` 而不是 `!is_fresh`）。语义完全颠倒，
   而两种写法的返回类型都是 `bool`，编译器一声不吭。`freshness_against_real_files`
   首跑红，改对后绿。这条同时说明了那个函数存在的理由：调用方只该问"要不要编"，
   不该各自再翻转一次。
2. **cargo 参数拼错**：`json-render-diagnostic`（单数）—— cargo 的可选值是
   `json-render-diagnostics`（复数）。这个错**单测抓不到**（`run_cargo_build`
   不在单测覆盖里），只有真按 Play 才会炸。手工端到端验证时抓出并修正，
   同时把 `parse_cdylib_artifacts` 的固定 JSON 换成实测形状（`crate-type =
   ["cdylib","rlib"]` 的目标一次给五个文件名，`.dll.lib` / `.pdb` / `.rlib`
   都要靠 `Path::extension` 正确排除，不能用 `contains("dll")`）。

### 三、一个差点上线的死循环

新鲜度里"编辑器 exe 比 DLL 新 ⇒ 要重建"这条，与"cargo 源码没变就不重写产物"
会打架：引擎改了 → 编辑器重编 → 按 Play → 检查说 DLL 旧 → 起 cargo → cargo 说
没改动、什么都不做 → DLL 还是旧的 → 再按 Play 又来一遍，**每次 Play 都白跑一轮
cargo**。

解药是 `sync_plugins_dir` 同步完盖一个"现在"时间戳（`touch`）。回归测试
`sync_makes_plugins_fresh_even_when_cargo_did_nothing` 把待同步产物的时间戳
**倒退回一小时前**来模拟"cargo 说没改动"的情形 —— 注释掉 `touch` 该测试立刻变红
（已实测验证红）。

### 四、演示项目 `projects/BehaviorsDemo`

**它本身就是一个 cdylib crate**（workspace 成员），是"用户项目形态"的插件 ——
与 `examples/spinner-plugin`（引擎自带示例）互补。场景里同时挂：

- `RustSpinner`（Rust 组件，75°/s）+ `RustBobber`（Rust 组件，上下浮动，
  组件自己持有跨帧状态）；
- `RhaiSpinner`（Rhai 脚本，同款 75°/s）—— **两条轨道共用一个运行时**的证明。

三条单元测试在 crate 内直接跑组件逻辑；另有两条集成测试在 `xgame` 里经
**真实 dlopen** 加载 `behaviors_demo.dll`，验证"跨边界注册的表"与"单测里那张表"
完全一致（否则会出现"单测绿、场景里挂组件却纹丝不动"）。

### 五、验证

- workspace **296 项测试全绿**；clippy 0 警告。
- 端到端手工验证：在 `projects/BehaviorsDemo` 里跑真实
  `cargo build --message-format=json-render-diagnostics`，确认产物 JSON 形状、
  只挑出 `behaviors_demo.dll`。
- 无头检查：`xgame-editor --check projects/BehaviorsDemo` →
  `entities=4 lights=1 script_refs=1`，`OK`。

### 六、遗留

- **Play 中途热换插件**未做（现在是"停止 → 编译 → 再播放"），Inspector 的
  组件徽章、release profile 的插件出货（P3）同属遗留。
- cargo 构建**没有超时**：首次拉依赖可能几分钟，用户只能干等（工具栏有秒数）。
  加超时需要决定"杀不杀 cargo 子进程"，留到有真实诉求时。
- 新鲜度把"编译期间恰好又存了文件"这个窄窗口交给下一次存盘（cargo 的指纹
  本来也有同款窗口），已在 `sync_plugins_dir` 的文档里写明。

### 七、一条工程教训

本轮用 PowerShell 的 `Get-Content`/`Set-Content` 做了一次文本替换，**把
`plugin_build.rs` 的中文注释全毁了**（PS 5.1 默认按 GBK 读、按 UTF-8 写，
且部分字符是有损的）。教训：改本仓库的中文源文件一律走编辑器工具，
shell 只做只读操作。

## 十七、双轨语言架构 P1：`#[derive(Inspect)]` + 编辑器调参（本轮）

### 一、"可检视"为什么是组件契约的一部分

编辑器**不链接用户代码**（插件是用户自己编的 DLL）。要在 Inspector 里画出一个
组件的字段，编辑器只有两条路：要么拿到类型信息（跨 DLL 边界做不到），要么让
**实例自己带着字段表**。所以：

```rust
pub trait Behavior: Lifecycle + Inspect {}   // 空 body + blanket impl
```

关键在 `Box<dyn Behavior>` 能直接 `as_mut()` 成 `&mut dyn Inspect`。若存的是
`Box<dyn Lifecycle>`，编辑器想要 `&mut dyn Inspect` 就必须 downcast ——
而 downcast 需要两边共享类型信息，跨 DLL 边界正是这条不通。**"可检视"因此不是
锦上添花，是让编辑器这条边能成立的前提**，注册约束相应从 `Lifecycle` 提到
`Behavior`（`impl_no_inspect!` 是"我不开放字段"的一行逃生舱）。

### 二、`Inspect` 的形状：表 + 下标，不是回调

`crates/xgame-core/src/inspect.rs`：`FieldDesc`（`'static` 描述）/
`FieldKind`（10 种）/ `FieldValue`（拥有值）/ trait `Inspect`。

- **为什么不是"每个字段一对闭包"**：检视对象活在插件 DLL 里，跨边界传闭包要
  处理 panic 隔离与函数指针生命周期；而"读第 i 个 / 写第 i 个"是两个对象安全的
  方法，下标与表一一对应，热路径零哈希。
- **`inspect_field` 返回 `&'static FieldDesc`**：调用方（编辑器）可以跨组件变更
  持有描述符而不借用实例。表是编译期常量，`'static` 是白给的。
- **`FieldValue` 是拥有的**：字段里可能有 `String`，借用版需要 `FieldValueRef`
  与 `Cow`，把 API 复杂度推给每一个消费者。要优化将来加
  `inspect_get_into(&self, i, &mut FieldValue)` 即可，表结构不动。
- **检视是旁路**：`inspect_set` 遇到类型不符的变体**静默忽略**（Lua 传错类型是
  日常），越界 `inspect_get` 返回该类型的零值 —— 一个下标算错不该拖垮宿主进程。

### 三、derive：三处下标必须同源

`crates/xgame-derive` 生成 `impl Inspect for T`。**表、get、set 全部由同一个
`index` 变量生成** —— 这就是"不要手写这个 trait"的理由：手写最常犯的错是三处
下标错位，表现是"改 A 字段动了 B 字段"，两边都是 `f32` 时编译器一声不吭。

- **硬编码 `::xgame::` 路径**（serde 惯例）：派生出来的代码要能在任何 crate 里
  编译。引擎内部测试组件解析不到 `::xgame`，用 `xgame_core::impl_no_inspect!`。
- **`#[inspect(skip / label= / min= / max= / speed= / step= / unit= / tooltip=)]`**。
  `label` 只改**显示名**，寻址名永远是 Rust 字段名 —— 重命名显示名不能破坏已有脚本。
- 回归测试 `crates/xgame/tests/inspect_derive.rs`：核心是"把每个字段写成一个
  **可辨认**的值，再逐个读回来"（数值刻意不取 0，否则错位了也读到 0 测不出来）。

### 四、踩到的四个坑（共同点：报错指向用户代码，而不是宏本身）

1. **`INSPECTABLE` 两列都写了小写类型名** → 生成 `FieldKind::f32` →
   报"no variant named `f32`"，而错误位置指向**用户的结构体**。
2. **`descs` 里字面量之间漏了一个尾逗号** → `expected one of ',' ... found '::`。
   只有两个以上字段才复现，所以第一个字段的用例全绿。
3. **`split_for_impl` 摆错位**（`for #name #impl_g`）—— `DeriveInput` 上没有
   `split_for_impl`，要先 `let (impl_g, ty_g, where_g) = input.generics.split_for_impl()`。
4. **`String` 字段在 `inspect_get` 里是 `&self` 上的移动** → E0507。
   数值 / bool / glam 向量都是 `Copy`，只有 `String` 要 `.clone()`。

**诊断方法**（下次照做）：proc macro 里 `eprintln!("{ts}")`，用环境变量门控，
再 `cargo check -p <pkg>` 读 stderr。`cargo expand` 本仓库没装，别指望它。

### 五、编辑器：Edit 模式也要有"实例"

组件实例只在 Play 时由注册表构造，Edit 模式下没有东西可改 —— 但"先拖滑块再按
Play 看效果"是 Unity 工作流的常态。于是 `EditorState.edit_components`（惰性构造，
声明位置在 `game_runtime` 之后、`plugins` **之前** —— 值里有插件函数指针）。

- **名字对齐校验**：用户把 `behaviors[0]` 从 A 换成 B 后，旧副本必须作废重建。
  否则会把 A 的参数写进 B 的字段里 —— 两边都是 `f32`，编译器管不着。
- **`seed_play_from_edit_components` 逐字段复制，不换实例**：World 那份已经装配
  完毕，换掉等于作废装配状态；而且只有可检视字段该被外部影响，
  `#[inspect(skip)]` 的状态字段本来就属于实例自己。
- **Play 中的改动不回写**：停止 Play 即丢弃（Unity 默认语义）。
- **组件字段改动刻意不碰 `touched`**：那个标志意味着"改了 `working`，要写回场景
  +推一条撤销"。组件参数不在场景文件里，动它会凭空多出一条场景撤销记录，
  按 Ctrl+Z 会撤掉一个并不存在的改动。

**顺带修掉的隐患**：`PluginSession` 现在把加载时已经装配好的注册表**留了下来**
（原先读完名字就丢）。Inspector 每帧要查字段表、要构造编辑态副本，若走
`build_registry()`（每次现跑插件入口），一旦出现"名字在 `names` 里、重跑却没
注册上"的错位，构造会**每帧失败**，而入口重跑失败会 `log::error!` —— 日志洪水。
常驻缓存把它变成一次 `HashMap` 查询。

### 六、一个会崩进程的测试陷阱（P0.5 埋的，P1 才暴露）

`plugin_loading` 集成测试直接 dlopen `target/*.dll`。而 **`cargo test` 根本不
产出 cdylib**（它只要 rlib 来链接测试），所以那些 DLL 停在上次 `cargo build`
的那一刻。P1 给 `BehaviorEntry` 加了 `fields` 字段 —— **插件注册表是跨编译单元
的内存布局**：DLL 那侧写、宿主这侧读。旧 DLL 按三段写、新宿主按四段解，
`HashMap` 的桶直接读歪，下一次经构造函数指针跳过去就是
**`STATUS_ACCESS_VIOLATION`（0xc0000005，进程暴毙，连 panic 都没有）**。

修法：取 DLL 前先**嵌套构建一次**（`build_package_once`，每包每进程一次，
持锁跨整次构建以串行化并行测试），把"新鲜"交给 cargo 的完整指纹去判断。

**教训**：P0.5 当时没踩到纯属运气 —— 那会儿 DLL 刚由验证流程里的 `cargo build`
产出、恰好新鲜。**产物与源码是否一致不能靠运气**，尤其当它变成内存布局的时候。

### 七、性能：先量后改（这次差点白改一版架构）

换 `Box<dyn Lifecycle>` → `Box<dyn Behavior>` 时担心：supertrait 方法调用要多一层
vtable 间接（`dyn Behavior` 的 v表里存着 `Lifecycle` 的 v表指针）。量之前先回测：

| 轨 | P0.5 `3d77f12` 同机 | P1 同机背靠背 |
|---|---|---|
| Rust 组件轨分发 | 68.4 µs | **65.2 µs** |
| Rhai 脚本轨分发 | 728.8 µs | 729.3 µs |

**P1 无回归**（甚至略低，在噪声内）—— 所以没有为这个猜想动架构。

注意 50 µs 的验收线在本机是**红**的（P0.5 也红，同机测得 68.4 µs），而 P0 当时
记录的是 37.6 µs：三条轨同步变慢而基线不动，判定为当时机器状态不同，不是某次
提交把分发搞慢了。**没有为迁就机器而上调验收线**；数字连同"必须带同机对照"
的说明一起写进了 `behavior_dispatch_throughput.rs` 的模块文档。

### 八、验证

- `cargo check --workspace --all-targets` / `cargo clippy`：0 错误 0 警告。
- `cargo test --workspace --all-targets`：**307 项全绿**（P0.5 是 296）。
- 无头检查：`xgame-editor --check projects/BehaviorsDemo` → `OK`。

### 九、遗留

- `FieldKind` v1 只有 10 种：`Quat` / `Color` 未进（等有真实组件需要再说）。
- **组件字段不进场景文件**（P1.5）：现在 Edit 态调的参数活在编辑器内存里，
  存盘不带走。序列化格式与 Lua 侧的绑定都等 P1.5 / P2。
- 字段级编辑**没有撤销**（场景级有）—— 接上场景撤销时再一起做。
- 没有 `trybuild`：「类型不支持又没标 `skip` ⇒ 编译期报错」这条无法断言，
  只能靠报错文案本身足够指向性。

---

## 十八、双轨语言架构 P1.5：组件参数落盘进场景文件（本轮）

接 §十七。P1 打通了「可检视 + 可调参」，但参数只活在编辑器内存里 —— 关掉即丢、
不能进版本库、prefab 没有字段位。本轮把组件参数变成**场景数据**，验收四条全部完成。

### 一、数据模型：注册名做键，不是下标

`Entity::behavior_params: BTreeMap<String, FieldValues>`（注册名 → 字段寻址名 → 值）。

- **为什么是注册名而不是 `behaviors` 数组下标**：下标会错位（用户在中间插一个组件，
  所有后续下标全变），而参数表是**持久化数据** —— 错位写参的表现是"改 A 组件的参数
  写进了 B 组件"，两边字段都是 `f32` 时编译器和运行时都发现不了。注册名天然稳定。
- **`FieldValues = BTreeMap<String, FieldValues>` 的有序性**：`BTreeMap` 保证 key 有序，
  序列化顺序稳定 → 场景文件 diff 不会因为哈希随机序而抖动。
- **`FieldKind` / `FieldValue` 补 serde**：变体标签即磁盘格式，钉死变体名测试
  （`field_value_ron_variant_labels_are_stable`）—— 改标签名等于作废所有旧场景。

### 二、capture 存全部，apply 吸收陈旧

- **capture 存全部可检视字段**（不做"非零才存"的半截规则）：半截规则会让"用户显式
  调回默认值"与"从没碰过"无法区分，且 diff 不可读。
- **apply 静默吸收陈旧/错型条目**（`apply_field_values`）：组件字段表改了之后，
  旧场景里的陈旧参数不该让加载崩溃。这与世界层 `apply_field_values` 既有取舍一致
  （检视是旁路，一个下标算错不该拖垮宿主）。

### 三、装配期套用 + 编辑副本种子

- **`World::instantiate_behaviors`**：装配时按注册名查 `behavior_params` 并套用。
  注册不到的名字**静默跳过**（与"参数表只是数据"的既有取舍一致，不 panic）。
  行为字段级拆借未动（Lua 侧仍走 `FieldValue`）。
- **`EditorState::inspect_component`**：构造编辑态副本时种子场景参数（找不到字段表
  时不种）。这样 Edit 模式下打开 Inspector 看到的就是场景里的真实值，而不是组件默认值。

### 四、撤销语义的推翻（P1 遗留的"不进撤销栈"）

P1 刻意让组件字段改动**不进**场景撤销栈（理由：参数不在场景文件里，动它会凭空多出
一条撤销记录）。参数进场景后这条取舍**失效** —— 它已成为场景数据，不进栈会凭空丢
一条场景编辑。改法：组件字段编辑接上既有撤销两段式（拖拽挂 `undo_pending`、松手入栈；
离散编辑当场 push），与 Name/Transform 同一通道，不单开支路。

**Play 中的改动仍不写回**：停止 Play 靠场景快照还原（既有语义，与本轮无关）。

### 五、一个测试后门

`PluginSession` 加 `#[cfg(test)] register_for_test<T>`：编辑器测试要构造带字段表的
组件实例，但生产路径只有 `build_registry()`（重跑插件入口）。后门只在测试编译期存在，
保住"注册表只构建一次"的每帧性能取舍。

### 六、验证

- 先证红（仓库惯例）：`behavior_params_from_scene_are_applied_at_assembly`
  （旧实现场景参数被丢弃 → 红）、`edit_component_is_seeded_from_scene_params`
  （无种子路径 → 红）。改后全绿。
- 新增回归：capture roundtrip、陈旧条目容错、RON 变体标签钉死、场景 roundtrip/
  旧文件兼容、装配套用、编辑副本种子 —— 共 6 条。
- `cargo test --workspace --all-targets`：**312 项全绿**（P1 是 307）。
  clippy 0 警告；`cargo check` 0 错误。
- 无头检查：`xgame-editor --check` 对 BehaviorsDemo 与 SampleScene 均 `OK`。

### 七、遗留

- **prefab 的组件字段**未做（prefab 目前只覆盖 Transform 等基础数据）——
  待 prefab 系统扩展到组件层时一起。
- `FieldKind` 仍只有 10 种（`Quat` / `Color` 未进），序列化变体随之受限。
- Play 中热调参**不写回**是既有语义；若将来要"Play 中调参也持久化"需另开决策。

## 十九、双轨语言架构 P2：Lua 宿主 + 路径驻留表（本轮）

接 §十八。完成计划文档 §四的 **P2** 两件事（Lua 宿主、修每帧 String 克隆）
与双轨的编辑器接入。验收：`cargo test --workspace --all-targets` **332 项
0 失败**（2 个手动基准 ignore）、clippy **0 警告**、`xgame-editor --check`
OK。决策注记已回写计划文档 D10。

### 一、Lua 宿主（`lua_host.rs` + `lua_api.rs`，两文件共约 1600 行）

- **mlua 0.10**（`lua54` + `vendored` + `send`）：没有 `'lua` 生命周期，
  `Value` / `Table` / `Function` 全是 `'static` —— 与 Rhai 一样能把句柄放进
  跨帧结构。`ScriptCtx` 泛型化 `ScriptCtx<S: 'static>`（Rhai 侧
  `ScriptCtx<rhai::Map>` **零改动**，Lua 用 `ScriptCtx<mlua::Table>`）。
- **每脚本一张环境表（沙箱）**，钩子从 env 里查。关键：`__index` 必须挂
  **真元表**（`meta.set("__index", lua.globals())` 后 `env.set_metatable(...)`）
  —— 往 env 塞名叫 `__index` 的普通字段对读取完全无效（本轮最贵的坑：
  脚本里所有宿主 API 变成 nil，9 条测试因此红，查元表语义才定位到）。
- **指令计数沙箱**：`set_hook(every_nth_instruction(1024))`，预算 200_000，
  扣到 0 返回 `Error::RuntimeError` 中断本次调用 —— 死循环脚本只毁掉自己
  这一帧，不卡死进程。与 Rhai 的 `set_max_operations(200_000)` 同量级。
- **StdLib 裁剪**：只开 `TABLE | STRING | MATH | UTF8 | COROUTINE`；
  io / os / package / debug / ffi 不给 —— 脚本是用户内容（可能是下载的 mod）。
  四个钩子与 Rhai 同名同语义；顶层 `local` 脚本级共享，每实体状态走
  `state_get_or` / `state_set`。
- mlua API 细节坑（留档）：`load(chunk)` 只收 1 个参数（chunk 名用
  `.set_name(...)`）、`AsChunk` 只对 `&[u8]` 实现、`set_metatable` 返回 `()`、
  `Table::set` 无 turbofish。测试 15 条（沙箱/死循环/状态隔离/钩子，与
  Rhai 侧对称）。

### 二、两轨按扩展名分流（`world.rs` 合并）

- `.lua` → `LuaHost`，其余 → `ScriptHost`（`is_rhai = !is_lua`）。两轨
  **合起来必须恰好覆盖全部脚本**：漏了是"两边都不管、脚本静默不执行"，
  重了是"两边都编译、假报编译错"。证红测试 `rhai_host_skips_lua_scripts`
  （旧实现把 `.lua` 丢给 Rhai → 每帧一条"脚本编译失败"日志 → 红）。
- `World` 帧管线依次派发两轨；日志/命令缓冲**合并但不共享指针**
  （各持各的 `Vec`，每帧取走追加到 world 侧）。
- `dispatch_destroy` / `forget_entity` 两宿主都**未接线** —— 与 Rhai 对称的
  既有遗留（运行时本来就没调用它们），不借 P2 之名单方面接线。

### 三、热路径 String 克隆 → 路径驻留表（先量后改）

**量**（临时探针 `clone_cost_probe`，量完即删，数据在此留档）：1000 实体
场景、每帧 2 次派发 × 300 帧，同机会话：

| 热路径点 | String 版 | 驻留 u32 版 | 克隆贵出 |
|---|---|---|---|
| 建派发队列（`collect_*_queue` / `dispatch_start`） | 53.52 µs/帧 | 20.91 µs/帧 | **32.61 µs/帧** |
| 状态表 entry（`state_key`） | 93.84 µs/帧 | 16.00 µs/帧 | **77.84 µs/帧** |

合计上限 ≈ **110 µs/帧** —— 占当时 Rhai 轨分发（819.6 µs）的 13%。
三处克隆：两宿主各自的 `collect_*_queue`、`dispatch_start` 内联队列、
`run_hook` 的 `state_key`（Lua 侧与 Rhai 侧一一对称）。

**改**（`xgame-script/src/intern.rs`，两宿主各持一份 `PathIntern`）：

- 路径 → 稳定 `u32`；队列存 `(EntityId, u32)`，状态键 `(EntityId, u32)`；
  `resolve(id)` 用 `Vec` 下标换回 `&str`（日志 / `GameCtx::source`），零分配。
  每帧每路径只多一次哈希查表（探针已计入），换掉一次 String 分配+复制。
- **id 永不回收、驻留表永不清空**：状态表按 id 找状态，`invalidate` /
  `invalidate_all` 只清编译缓存。表一清，热重载后每实体状态会**静默错位**
  （A 脚本读到 B 的计时器，两边都是 `f32` 时无从发现）。回归测试
  `state_survives_hot_reload_even_when_intern_order_changes`（两宿主各一条）：
  跑帧 → `invalidate_all` + **把实体顺序倒过来**（逼驻留顺序改变）→
  断言状态延续（a=2、b=200）。**证红**：把"清空驻留表"这个看似合理的实现
  写进 `invalidate_all`，两条测试都红（b=100，即状态从 0 重计）——
  这是新不变量的红对照（旧实现没有驻留表，性能改动本身在行为测试下恒绿，
  正因此需要这条不变量网）。
- **借用检查的形状**（实现要点）：借 `self.intern` 得到的 `&str` 没法跨
  `&mut self` 方法调用传（借用检查器看不出"它就是 self 身上的一块"）。
  解法：`ensure_loaded(path)` 拆成"先 intern 再转"的外壳 + `ensure_loaded_id`
  内核（在方法体内自己 resolve）；`run_hook` 改收 `path_id`，体内 resolve
  （O(1) 下标）。`CompiledScript.path` 字段随之删除（两个用途都被驻留表
  取代，留着是死字段）。

**验**（端到端，`behavior_dispatch_throughput` 4 轮背靠背）：Rhai 轨分发
819.6 → **713 µs/帧**（-106 µs，-13%），与探针预测 ≈110 µs 吻合 ——
微基准与端到端交叉验证，该对照已写进基准模块文档（P2 列）。

### 四、编辑器 / 工具链接入

- `Project::list_scripts` 扫 `rhai` **和** `lua`（证红测试
  `list_scripts_includes_both_tracks`：旧实现只列出 `["Scripts/a.rhai"]` → 红）。
  Inspector 的 Add Script 菜单、MCP 项目信息、热重载扫描都走它，一处改完全通。
- 三处文案（state 扫描说明、工具栏 ⟳ Scripts 提示、Inspector 空态）改为
  "双轨"；热重载链（state → game_runtime → world）本就两个宿主各
  `invalidate_all`，无需改。
- `xgame-editor --check`：SampleScene OK（entities=14、script_refs=3）。

### 五、验证汇总

- 全量：**332 项 0 失败**、clippy 0 警告、`--check` OK。
- 新增回归 5 条：intern 稳定性 2、双宿主状态延续 2、list_scripts 双轨 1
  （其中 3 条按惯例先证红再改绿，状态延续 2 条的红对照见上文）。
- 基准 P2 列：基线 7.43 / 组件分发 64.7 / Rhai 分发 713 µs。基线与组件轨
  相对上一轮的漂移（10.39 → 7.43、57.1 → 64.7）是机器状态 —— 那两档的
  执行路径不经过驻留表（脚本列表为空，扫描循环逐字未变）；判回归只看
  Rhai 轨的 -106 µs 是否消失。

### 六、遗留

- **下一步 = P2.5 基准闸门**：扩 `behavior_dispatch_throughput` 加 Lua 轨、
  三负载对比出报告（规则见计划文档 §六）。Rhai 去留在闸门结论前不动。
- `dispatch_destroy` / `forget_entity` 双轨都未接线（既有遗留，等运行时
  真正需要"销毁钩子"时两轨一起）。
- 编辑器没有"新建脚本"功能（两轨一致）：`.lua` 文件放进 `Scripts/` 即被
  扫描、挂载、执行。
- Lua 轨的**每帧性能还没有实测**（本轮基准只加了驻留表对照）——P2.5 会出
  第一个数；若 Lua 轨明显慢于预期，优先怀疑方向是每钩子的 `LuaCtx` 构造
  与 env 查表，不是驻留表（已修的部分有交叉验证）。

## 二十、双轨语言架构 P2.5：基准闸门 Rhai vs Lua（本轮）

接 §十九。完成计划文档 §六 的 **P2.5** 基准闸门：三负载实测 Rhai vs Lua，
按事先声明的规则出判定。**判定结果：Lua 三负载全占优（1.31× / 1.26× /
9.16×，比值 >1 = Lua 更快）→ 按 §六 规则进入 P3「删 Rhai、样例文档切
Lua」**。闸门过程中顺带修掉了 Lua 宿主热路径的两处实现开销（先量后改，
数据在下文），把负载 1 从"Rhai 反超 2.25×"翻转为"Lua 快 31%"。

### 一、闸门工具（`language_gate.rs`，`#[ignore]` 手动基准）

三负载、同进程背靠背（同一台机器同一时刻，消除跨会话机器状态漂移）：

| 负载 | 场景 | 测什么 |
|---|---|---|
| 1 dispatch | 1000 实体 × no-op 四钩子脚本 | 宿主调度 + 边界开销 |
| 2 api | 500 实体 × 每帧 20 组 `pos/set_pos/action_pressed` | API 边界穿越 |
| 3 compute | 200 实体 × 每帧 1000 次浮点循环 | 解释器本体 |

- **脚本两轨同源同循环数**：`api_rhai()/api_lua()`、`compute_rhai()/compute_lua()`
  用 `format!` 从同一组常量（`K_API`/`I_COMPUTE`）拼出来 —— 单一真相源，
  改负载只改一处，两轨永远跑同样的工作量。
- **错误日志守卫**（首跑栽在这，见下）：`measure()` 预热后 `take_logs()`，
  有 `Error` 级日志直接 panic —— 脚本没真跑时测出来的"快"是假数据，
  不许端到端通过。证红：把 Rhai 脚本改回 `let mut`，断言如期触发。

### 二、首跑假数据事故（Rhai 侧，已修）

首跑"Rhai 负载 2/3 大胜"是假的：**Rhai 语言没有 `mut` 关键字**（变量天然
可变，`let x = 0.0; x = 1.0;` 直接重赋值），我写脚本时按 Rust 习惯加了
`let mut x` —— Rhai 把 `mut` 当变量名解析后撞上下一个 token 报编译错
（`Expecting ';' to terminate this statement`），脚本体根本没执行，
"快"的只是空调度。铁证：假数据里 Rhai 计算档 0.28 µs/实体 < no-op 档
0.73 µs/实体（计算档比空跑还快不可能）。`repo` 里既有 `.rhai` 脚本
（`FoxLocomotion.rhai:23-25`）都是 `let` 直接重赋值，无一用 `mut`。

**教训**：宿主对编译失败只记日志不 panic（产品语义正确 —— 一个坏脚本
不该拖垮整个游戏），但基准工具必须自己断言"脚本真的跑了"—— 这就是
`measure()` 日志守卫存在的理由。

### 三、负载 1 的 Lua 慢 2.25×：定位 + 修复（先量后改）

修好脚本后负载 1 真实数据：Rhai 743.59 vs Lua 1670.43 µs/帧（Lua 慢 2.3×）。
纯调度不该差这么多 —— 分两步量证：

**探针 1**（`_probe_lua_call_cost.rs`，量完即删，数据留档）复刻宿主环境
（同 StdLib 裁剪、同 1024 指令 hook、同 env+元表），分项量单次钩子调用：

| 分项 | 成本 |
|---|---|
| `env.get` 从环境表取函数 | **195.4 ns** |
| `call` 无参 | 84.0 ns |
| `call` 带 userdata | **368.5 ns**（比无参多 284） |
| 带 userdata 关指令 hook 对照 | 357.5 ns（**hook 税仅 ~11，排除嫌疑**） |
| get+store+entry+call（≈宿主全形状） | 588.6 ns |

**探针 2**（`_probe_run_hook_segments.rs`，量完即删）在 `run_hook` 上分段
计时（1000 实体 × 300 帧）：`func.call` 442.7 µs/帧（39.8%）、`env.get`
192.3 µs/帧（17.3%）是大头。两处合计 ≈478 ns/钩子 × 每帧 2000 钩子
（update + fixed_update 两相）≈ 956 µs/帧 —— 与实测差值 927 µs 严丝合缝。

**userdata 方案对比**（探针 1 第二轮）：现造 365.2 / 预建 `AnyUserData`
克隆 159.8 / **轻量指针 81.3** / 无参参照 78.0 ns。

**改**（`lua_host.rs` + `lua_api.rs`）：

1. **钩子函数句柄编译期缓存**：`Hooks`（bool 版）→ `HookFuncs`
   （`Option<Function>` × 4）。`detect_funcs` 本来就要 `raw_get` 验存在性，
   句柄直接留下 —— 热路径零查表。
2. **ctx 改宿主常驻槽 + 轻量指针传参**：`LuaHost` 持
   `Box<MaybeUninit<LuaCtx>>`，每钩子入口写入本次地址束、传槽地址
   （`LightUserData`，mlua 在 `send` 下 `unsafe impl Send` 在案）。
   安全论证写在 `LuaCtx` 类型文档：指针永远指向宿主持有的槽；任何脚本
   可见的执行都发生在钩子调用窗口内（窗口内槽 = 本次 ctx）；脚本违约
   存句柄下帧再用，读到的也是下帧调用的**有效** ctx（语义="当前运行中的
   上下文"，不悬垂）—— 比旧方案（userdata 存过期拷贝、下帧解引用已死
   栈帧）更不容易踩坑；脚本无法伪造 lightuserdata（Lua 语言层无此操作），
   `FromLua` 手写校验仍给清晰报错。
3. `CompiledScript` 不再存 env（钩子函数的 `_ENV` upvalue 持有它）。

**验**（同机背靠背，`behavior_dispatch_throughput` + `language_gate` 复跑）：

| 负载 | 修复前 | 修复后 |
|---|---|---|
| 1 dispatch（1000 实体） | Rhai 743.59 / **Lua 1670.43** | Rhai 753.94 / **Lua 575.92** |
| 2 api（500 实体） | Rhai 18984.57 / Lua 18367.18 | Rhai 19402.71 / **Lua 15452.31** |
| 3 compute（200 实体） | Rhai 44962.36 / Lua 5092.78 | Rhai 45788.25 / **Lua 5001.30** |

负载 1 的 Lua **-65%**（1670 → 576），负载 2 的 Lua -16%（`FromLua` 走
轻userdata 解引用也省了每次 API 调用的 `as_userdata+borrow`），
负载 3 基本不动（纯解释器，不吃边界开销）。行为回归 34 条全绿
（沙箱/死循环/状态隔离/钩子派发均未受影响）。

### 四、闸门判定（规则事先声明，无中间地带）

§六 规则："Lua 三负载全占优 → P3 删 Rhai；任何一负载 Rhai 反超 >20% →
保留双宿主并重新评估"。最终数据（同进程背靠背）：

| 负载 | Rhai | Lua | 比值（Rhai/Lua） |
|---|---|---|---|
| 1 dispatch | 753.94 µs | **575.92 µs** | 1.31× |
| 2 api | 19402.71 µs | **15452.31 µs** | 1.26× |
| 3 compute | 45788.25 µs | **5001.30 µs** | **9.16×** |

**三负载全占优 → 判定：P3 删 Rhai**。判定稳健性：负载 3 的 9.16× 是
AST 解释 vs 字节码 VM 的**语言级代差**，任何宿主侧派发优化都翻不了盘；
负载 1/2 的 1.31×/1.26× 也都在 20% 线外 —— 即使对 Rhai 侧做对称优化
（它也有 `call_fn` 每次按名字查 AST 的空间），量级上也无法扭转负载 3。

### 五、验证汇总与遗留

- 全量：clippy 0 警告、`xgame-script` 34 项 0 失败、闸门与分发基准跑通。
- 临时探针 3 个（`_probe_gate_diag` / `_probe_lua_call_cost` /
  `_probe_run_hook_segments`）全部删除，原始数据留在本文档。
- 组件轨 71.7 µs 与历史 64.7 的漂移在机器噪声内（同机基线也漂了 2%），
  本机验收线持续红是 P0.5 起已知状态，不修线。
- **下一步 = P3**：按判定删 Rhai 宿主、样例/文档切 Lua（范围见计划文档
  §四 P3 与 ROADMAP）。`language_gate.rs` 在 P3 删 Rhai 后自然退役
  （判定已出，它的一次性使命完成）。

## 二十一、双轨语言架构 P3：删 Rhai、脚本层收敛单一 Lua 轨（本轮）

接 §二十。按闸门判定（§二十 四、计划文档 §六 / D11）执行删除：Rhai 宿主整体
下线，脚本层从"双轨按扩展名分流"收敛为**单一 Lua 轨**。全量验证绿
（clippy 0 警告、`cargo test --workspace --all-targets` 全绿、`--check` OK）。

### 一、删了什么

| 删除物 | 位置 | 说明 |
|---|---|---|
| Rhai 宿主本体 | `xgame-script/src/host.rs` | `ScriptHost` / `CompiledScript` / bool 版 `Hooks` 及其派发、编译缓存、测试全删 |
| Rhai 转发壳 | `xgame-script/src/api.rs` | 整文件（P0 起就是"每函数一行转发"，函数体早在 ctx.rs） |
| `rhai` 依赖 | `xgame-script/Cargo.toml` | 连带 `f32_float` 注释（那是 Rhai 的 f64 问题，Lua 侧无此事） |
| 闸门工具 | `tests/language_gate.rs` | 一次性使命完成，判定数据已入档（§二十 + 计划文档 §六） |
| 基准 Rhai 档 | `tests/behavior_dispatch_throughput.rs` | 保留基线 / 组件 / Lua 三档；模块文档的 Rhai 列**留作历史记录表** |
| `LuaPreloadReport` 别名 | `lua_host.rs` | 双轨对称期的纯别名，单轨后无人使用（连 `PreloadReport` 都在同模块导出） |

**保留物**：`host.rs` 瘦身为**宿主公共类型**模块（`Hook` / `PreloadReport` /
`CommandReport`）—— `lua_host.rs` 一直 `use crate::host::{...}` 共用这三个类型，
模块路径与 `lib.rs` 再导出不变，纯删代码不制造无关 diff。

### 二、单轨化改动

1. **`World` 单字段**：`scripts: ScriptHost` + `lua: LuaHost` → 一个
   `scripts: LuaHost`。`take_logs` / `reload_scripts` / `consume_script_commands`
   的"两轨缓冲合并"代码全删（合并语义本来就是"消费方不关心来源"，单轨后
   天然成立）；组件轨的日志 / 命令缓冲仍走 `scripts.buffers_mut()`，通道未变。
2. **`ScriptCtx<S>` 收成非泛型**：状态表直接持 `mlua::Table`。泛型是双轨期
   "Rhai `Map` / Lua `Table` 各一个实例化"的产物，单轨后无意义。**地址束模式、
   `ErasedMut`、unsafe 调用窗口约定原样保留** —— 不在删语言的同一轮里动
   unsafe 核心（churn 风险 > 收益）。
3. **`list_scripts` 只扫 `.lua`**：残留 `.rhai` 不进编辑器下拉。证红测试
   `list_scripts_includes_both_tracks`（P2 引入）使命完成，更新为
   `list_scripts_scans_lua_only` —— 断言方向**反转**（写一个 `.rhai` 进去，
   必须不出现在列表里；旧实现在这条断言下会红）。
4. **编辑器三处双轨文案**收敛（state.rs / inspector.rs / toolbar.rs）；
   `xgame-core` 的注释示例（`Scripts/Player.rhai` → `.lua` 等）同步。

### 三、样例与文档迁移

- **4 个 `.rhai` 移植 `.lua`**（`git mv` 语义：删旧建新）：SampleScene 的
  Spinner / Jumper / FoxLocomotion + BehaviorsDemo 的 Spinner。语法点：
  `let` → `local`、`fn` → `function ... end`、`!` → `not`、方法式调用本就
  没有（Rhai 函数式写法同形，迁移零摩擦）、`sin` → **`math.sin`**（沙箱
  MATH 库已开 —— `sample_project_runs` 跑 FoxLocomotion.lua 180 帧无错误日志，
  链路验证在案）。场景 ron 引用（scene.ron ×3 + 注释 ×2、main.ron ×1）同步改。
- **`Docs/脚本API.md` 整篇重写为 Lua 版**：原"两个语法坑"一节收敛 ——
  "全局 `let` 在函数里看不见"随 Rhai 消失（Lua 顶层 `local` 是 upvalue，
  函数体天然可见），换成"顶层 `local` 是**脚本级共享**、每实体状态走
  `state_*`"的语义说明；数学节改 `v*` 具名函数 + `math.*`；容错表去掉
  Rhai 专属的"递归 32 层"数字，按 Lua 指令预算语义写；`raycast` / 向量的
  返回形状按 `lua_api.rs` 真实实现核对过（表、全字段恒填）。
- README（脚本章节 + 目录树 + Fox 示例）、差距分析两处、ROADMAP 架构图同步。
  `Docs/双轨语言架构计划.md` 补 **D11 落地注记**、路线图表 P2.5/P3 打勾、
  一页结论与 §六 收尾更新。

### 四、刻意的取舍

1. **API 名一字不改**：`spawn_entity` 的 `_entity` 后缀是 Rhai 保留字时期
   的历史产物，`state_*` / `set_anim_*` 等全部保持原名 —— 迁移成本
   （既有脚本 + 文档 + 测试）远大于后缀的违和感。注释里写明历史原因。
2. **`host.rs` 不改名不搬家**：有人会想把它并进 `context.rs` 或改名
   `types.rs` —— 但 `lua_host.rs` 的 `use crate::host::` 与 `lib.rs`
   再导出路径全都不用动，纯删代码的 diff 最小、最好审。
3. **`ScriptCtx` 泛型收掉，但 unsafe 半步不迈**：见上文二.2。地址束 +
   `ErasedMut<'static>` 是 P2 安全论证的原物，本轮只把 `S = rhai::Map`
   这个默认参数摘了。
4. **`language_gate.rs` 直接删**而不是留作"将来接第三门语言再用"：
   一次性工具，脚本两轨同源的写法与 Rhai 侧强耦合（`api_rhai()` /
   `compute_rhai()`），留着就是死代码 + 误导（仓库有"不留死代码"惯例）。
   判定数据已完整入档，未来要做语言对比按 §二十 的方法重写即可。
5. **`check.rs` 不加扩展名校验**：悬空引用检查仍只查"文件在不在"，
   不查"是不是 `.lua` / 能不能编译"—— 旧项目场景里残留的 `.rhai` 引用
   （文件还在）会继续通过存在性检查，不被误报；真正的"这脚本不会跑了"
   会在 Play 时以"没定义钩子"的静默形态出现（与"空脚本"同语义）。
   迁移期不制造惊吓，是刻意选择。

### 五、验证汇总与遗留

- 全量：`cargo check --workspace --all-targets` 绿、clippy **0 警告**、
  `cargo test --workspace --all-targets` 全绿（xgame-script 16 项 Lua 测试
  = 原 11 项 Rhai 测试的等价覆盖：沙箱 / 死循环预算 / 状态隔离 / 驻留表
  热重载不变量 / 分流判定 / 顶层 local 可见性）；
  `xgame-editor --check` OK（`script_refs=3`，三个 Lua 脚本全解析）。
- 基准复跑（同机）：Lua 轨分发与 P2.5 的 562 µs/帧在噪声内，组件轨 /
  基线同机器状态口径不变 —— 删 Rhai 没有触碰 Lua 轨热路径。
- 行为验证：`sample_project_runs`（Spinner.lua 转动 + Jumper.lua +
  FoxLocomotion.lua 的 `math.sin` / `set_anim_*` 全链路）、
  `sample_project_assets_all_load`（三脚本编译 0 失败）、BehaviorsDemo
  场景加载。
- 遗留不变：`dispatch_destroy` / `forget_entity` 仍未接线（单轨后只剩
  Lua 宿主一份，语义同 P2）；组件轨 <50 µs 验收线本机持续红（P0.5 起已知）。
- **双轨语言架构计划 P0–P3 全部分期完成**。

## 二十二、第八阶段：音频系统（本轮）

引擎第一次有声音。选型 **kira 0.12.5**（纯 Rust、CPAL 后端、自带空间化轨道
与 symphonia 解码；对比 rodio —— rodio 无内置 3D 定位，空间化得自己拿
`SpatialSink` 手搓距离衰减，等于把本阶段最核心的一块自己写一遍）。

全量验证绿：`cargo check --workspace --all-targets` 绿、clippy **0 警告**、
`cargo test --workspace --all-targets` 全绿（新增 21 条音频测试，全部先证红）。

### 一、分层与数据流

```
xgame-core::audio   纯数据（无 kira 依赖）
  AudioSource（组件） / ListenerPose / PlaySound / AudioCommand / AudioPort / AudioStats
        ↑ 只依赖 core
xgame-audio         AudioEngine：kira 封装（设备 / 空间轨道 / 解码缓存 / GC）
        ↑
xgame-runtime       World 持 port + engine，帧末 drain 结算
        ↑
xgame-script        GameCtx.audio: Option<&mut AudioPort>（脚本 6 个 API 只进 port）
```

**脚本层不引 kira** —— 脚本 API 的实现是"往 `AudioPort` 里塞命令"，与
物理射线 / 生成实体同一模式：脚本产生**意图**，`World` 帧末统一结算。
这样设备失败 / 沙箱 / 热重载全都与音频实现解耦。

### 二、核心设计决策

1. **监听者与主音量是状态，不进命令束**。`AudioPort` 持
   `listener: ListenerPose` + `master_volume: f32` + 私有 `commands`。
   每帧被反复覆盖（编辑器每帧推相机姿态），命令束语义是"发生过的事"，
   姿态是"现在是什么"—— 混在一起会让"第 3 帧听者在 A、第 5 帧在 B"
   变成一串需要回放的状态更新。

2. **`PlaySound.follow` 是实体绑定，不是空间化开关**。`spatial` 只看
   `position.is_some()`；`follow` 只决定 `StopEntity` / `EmitterPosition`
   按哪个实体匹配。**非空间化的声音也绑实体** —— UI 音挂实体时照样要能
   "实体删了音就停"。实现期踩过：初版只给空间声源绑实体，导致
   "非空间化 autoplay + 删实体"音频还在响（`removed_entity_or_component_stops_its_sound`
   红后修复，`play.follow` 恒设挂载实体）。

3. **无声降级而不是报错**。`AudioEngine::open` 失败（无设备 / 无头 CI /
   容器里没有音频服务）进入 `Playback::Silent`：照常受理命令、解码、计数、
   GC（墙钟），只是不出声。**所有音频回归测试跑在 silent 引擎上** ——
   不用 MockBackend（那会把 kira 内部结构钉死在测试里），用**真 WAV 文件**
   走完整解码路径，测试因此同时验证了"解码 + 缓存 + 停止 + GC"整条链。

4. **`sync_audio` 每渲染帧一次，不跟物理固定步同频**。人耳对 16 ms 的
   声源位置更新完全不敏感，固定步一帧可能跑好几个 —— 音频不值得同频。

5. **帧末结算**：`sync_audio()`（发命令）→ …物理 / 脚本… → `flush_audio()`
   （drain → `engine.apply`）。放在 `World::update` 内部，编辑器 Play 与
   独立游戏天然一致（仓库铁律）。

### 三、组件与落盘

`Entity::audio: Option<AudioSource>`（一等字段，`#[serde(default, skip_serializing_if)]`，
与 `physics` / `animator` 完全同款的旧文件兼容策略）。字段：
`clip / volume(1.0) / pitch(1.0) / looping(false) / autoplay(true) / spatial(false) /
min_distance(1.0) / max_distance(50.0)`，结构体级 `#[serde(default)]`。

`autoplay` 默认 `true` 是刻意的：给实体挂音频源的作者意图几乎总是"进场景就响"；
要"等脚本触发"就关掉 autoplay、脚本里 `play_sound`。首播去重靠
`World.audio_autoplayed: HashSet<EntityId>`（标记进集合后不再触发，组件摘除 /
实体删除 → `StopEntity` + 清标记 → **重挂再响**，`removed_entity_or_component_stops_its_sound`
的后半段钉死这个来回）。

### 四、空间化与监听者

- kira spatial sub-track：`add_spatial_sub_track(listener.id(), pos,
  SpatialTrackBuilder{distances{min,max}})`，Drop 即从混音树移除；
  `set_position` 每帧推发射位置（组件 spatial 声源每帧从世界矩阵取位置）。
- 监听者姿态：`ListenerPose::new(position, forward)` —— **只收位置与朝前向量**，
  up 恒 +Y、`right = forward × up`（相机帧约定）；`forward` 存归一化值，
  竖直朝向（与 up 平行）退化取 +X 而不是 NaN（`vertical_forward_does_not_produce_nan`）。
- 编辑器 Play 每帧 `set_listener(编辑器相机 eye/forward)`；`xgame-game`
  固定机位与 `ListenerPose::default()` 一致，不需每帧推。
- kira 类型（glam 0.33）不出 `xgame-audio` 墙 —— 跨界只传 `[f32;3]` / `[f32;4]` 数组
  （引擎 glam 是 0.29，两个版本不能混类型）。

### 五、脚本 API（6 个，`Docs/脚本API.md` §4.11）

`play_sound(ctx, clip, volume, pitch)` / `play_sound_at(ctx, clip, x, y, z, volume)` /
`stop_sound(ctx, clip)` / `stop_all_sounds(ctx)` / `set_listener(ctx, x, y, z, fx, fy, fz)` /
`set_master_volume(ctx, v)`。全部是 `GameCtx` 方法（`ctx.rs` 真相源，
`lua_api.rs` 每函数一行转发）；`FrameCtx.audio` / `GameCtx.audio` 为 `None` 时
静默 no-op（编辑态、测试构造不需音频也能跑）。

### 六、编辑器接入

- Inspector 新增 **Audio** 区块：clip 路径 / volume / pitch / loop / autoplay /
  spatial / min-max distance，离散编辑走场景撤销栈（与 Name/Transform 同通道）；
- `GameRuntime` 注入 `AudioEngine::open(root)`，每帧 `set_listener(编辑器相机)`；
- 状态栏加 **Audio** 行（`AudioStats`：设备可用 / 活跃声数 / 缓存 clip 数）；
- `xgame-core::check` 加 `audio_refs` / `missing_audio`（clip 断链进悬空引用报告，
  `--check` 与 MCP `project_check` 同源）；
- `ProjectHandle::list_audio()` 扫 wav/ogg/mp3（`content.audio_dir` 子树）。

### 七、示例与资产

SampleScene：`Resources/Audio/Beep.wav`（44.1 kHz / 16 bit / mono 880 Hz
衰减正弦，`tools/gen-beep-wav.ps1` 生成，88 KB 入库）。Spinner 实体挂
空间化循环蜂鸣（走近响、走远轻），Jumper 起跳播一次性音效
（`play_sound` pitch 1.5）。`bundled_sample_project_loads` 补了音频引用
存在性断言（clip 断链在编辑器里不显形 —— 解码失败只记日志然后无声，
必须在加载前断言才能被 CI 抓住）。

### 八、回归测试清单（21 条新增，全部先证红）

| 层 | 测试 | 钉死什么 |
|---|---|---|
| core | `audio_source_ron_round_trips` / `_defaults_when_fields_omitted` / `audio_source_survives_ron_roundtrip` / `plain_entity_omits_audio_field` / `audio_participates_in_entity_equality` | 落盘格式 + 旧文件兼容 |
| core | `audio_port_collects_commands_in_order` | 命令束顺序 |
| core | `listener_orientation_matches_camera_convention` / `vertical_forward_does_not_produce_nan` | 姿态数学（竖直朝向不 NaN） |
| core | `source_maps_to_play_sound` | 组件 → 播放请求映射（spatial 只看 position） |
| audio | `plays_and_stops_a_real_wav_in_silent_mode` | 真 WAV 解码 + 停止 + GC（墙钟） |
| audio | `missing_clip_is_counted_but_not_retried` | 解码失败进 failed 表、不重试 |
| audio | `entity_scoped_stops_and_emitter_updates` | StopEntity / EmitterPosition 按实体匹配 |
| audio | `linear_volume_maps_to_decibels_safely` | 音量→分贝换算 + clamp（0 音量不炸） |
| script | `audio_api_buffers_commands_or_noops` | GameCtx 音频方法缓冲 / None 静默 |
| script | `script_audio_calls_reach_the_port` | Lua 6 个函数 → port 端到端 |
| runtime | `autoplay_sound_requests_play_exactly_once` | 首播一次（标记表去重） |
| runtime | `real_wav_autoplay_becomes_active` | 真 WAV 进 ActiveSound |
| runtime | `spatial_source_pushes_emitter_positions_each_frame` | 空间声源每帧推位置 |
| runtime | `removed_entity_or_component_stops_its_sound` | 删实体 / 摘组件停音 + 重挂再响 |
| runtime | `script_play_sound_reaches_the_engine` | 脚本 → World → 引擎全链路 |
| runtime | `audio_rescan_picks_up_new_entities` | 快速路径的重扫时机（实体数变化必须重新武装扫描） |

### 九、性能：音频同步"先量后改"（同机背靠背）

`update_throughput`（2000 实体 × 300 帧，同机交替跑 before / after 二进制）：

| 档位 | 音频前（均值） | 音频后·无快速路径 | 音频后·有快速路径 |
|---|---|---|---|
| 纯视觉 | **19.3 µs/帧** | 25.0（**+5.7**） | **21.9**（含跨二进制布局漂移 ~2.5，见下） |
| 全静态体 | 101.0 | 109.7 | 105.5 |
| 稀疏动态 64 | 737.7 | 747.2 | 742.5 |
| 密集接触 1000 | 13281.7 | 13256.5 | 13282.5 |

- 起因：`sync_audio` 每帧扫一遍实体表找音频源 —— 2000 实体纯视觉场景下
  这是一条额外的内存流（实体结构每条几百字节），实测 +5.7 µs/帧。
- 修复：`audio_sources_seen` / `audio_scan_count` 快速路径 —— 没挂过音频源、
  没有 autoplay 标记、实体数也没变 ⇒ 整段跳过；实体数变化（脚本 spawn /
  destroy）重新武装一次扫描（`audio_rescan_picks_up_new_entities` 钉死）。
  **刻意取舍**：给已有实体在运行中直接贴 `audio` 组件（不增删实体）不会触发
  重扫 —— 动态发声的正路是脚本 `play_sound`（不经扫描），组件走场景配置。
- 收尾判定：把 `sync_audio` / `flush_audio` 两个调用整体注释掉再量得
  21.5~21.9 µs/帧 ≈ 快速路径版同值 —— 残余的 ~2.5 µs 差距来自**跨二进制
  的代码布局漂移**（同一份源码 batch 重编也会浮动），不是每帧音频工作。
  按仓库口径：±1.5× 的系统性漂移内，音频对热路径的增量 ≈ 噪声。

### 十、刻意的取舍

1. **不用 MockBackend**：无声模式走真实 `AudioEngine` 代码路径（只是不碰设备），
   测试用真 WAV。Mock 会把 kira 内部 trait 结构钉进测试，升级即碎。
2. **不做混音总线分组**（BGM / SFX 各自音量）：主音量 + 每声源音量已够 v1；
   总线要动 kira 的 track 层级设计，等有真实需求（游戏要"音乐音量滑块"）再开。
3. **不做流式长音频**：`StaticSoundData` 全量解码进内存（Beep.wav 88 KB 无感）；
   一小时 BGM 会吃几十 MB —— 流式播放（`StreamingSoundData`）留给需要时。
4. **`play_sound` 固定参数不接表**：Lua 侧签名 `(ctx, clip, volume, pitch)` 是
   位置参数，不支持 `play_sound(ctx, {clip=..., volume=...})` 的表式可选参数 ——
   与 `raycast` / `spawn_ball` 等既有 API 的固定参数风格一致，转发壳每函数一行
   的模式也不被破坏。
5. **`AudioEngine` 藏在 `xgame-audio`，`World` 持有但不暴露**：外部只能
   `with_audio_engine` 注入、`audio_stats` 读数 —— 防止有人绕过 port 直接
   摆弄 kira 句柄（那会绕开命令束的时序保证）。

### 十一、实现期踩的坑（防好心人翻案）

- **非空间化 autoplay 没绑实体** → 删实体音还在响。修复：`play.follow`
  恒设挂载实体（见二.2）。
- **测试 WAV 太短被 GC 干扰**：初版测试用 50 ms WAV + `Instant` 墙钟 GC，
  断言"active == 1"时声音已自然结束 → 红。改用 1~5 s WAV 后绿。
  教训：**GC 语义（墙钟 vs 句柄状态）要在测试里显式选择**。
- **`AudioManager` 六百多字节内联进 `Playback` 枚举** → clippy
  `large_enum_variant`。`manager` 装箱（热路径只按引用解引用，零成本）。
- **`map_or(true, …)`** → clippy `unnecessary_map_or`，改 `is_none_or`。

## 二十三、Agent-Native：MCP 工具面 + 无头运行（本轮）

差距分析「近期计划」第 ② 条落地：**无头运行**。判据还是那一条 ——「智能体没有
鼠标时，这条路走得通吗？」。`--check` 已回答"项目能不能加载"，本轮回答后半句：
**"跑起来会怎样"**。MCP 侧同时收获第 7 个工具 `run_headless`（透传壳），
至此工具面覆盖「读 → 改 → 查 → 看效果」完整闭环。

全量验证绿：`cargo check --workspace --all-targets` 绿、clippy **0 警告**、
`cargo test --workspace --all-targets` 全绿（**359 条**，较上轮 +20：headless
模块 15 条自带测试 + MCP 工具 5 条；其中 MCP 侧 4 条 spawn 行为测试在接线前
**先证红** —— 红的是 `UnknownTool("run_headless")` 与工具数 6≠7，随后实现转绿）。

### 一、形态与契约

```sh
xgame-editor --headless [--frames N] [--dt 秒] [--report 路径] [--dump-scene 路径] <项目>
```

- **实现位置在编辑器二进制**（`headless.rs`，`--check` 的邻居）：跑的是
  `World::update` —— 与编辑器 ▶ Play、独立游戏**同一段**代码，无头跑出的行为
  与玩家实跑一致。MCP 只做 spawn 透传，不做第二份帧循环。
- **退出码契约**（与 `--check` 对齐，给 CI / 智能体）：
  **0 = 干净跑完 / 1 = 加载失败或参数错误（报告不产出）/ 2 = 跑完但观察到问题
  （脚本 error 日志、非有限变换；报告照常输出）**。`2` 与 `1` 的分野是刻意的：
  "发现问题"的报告是完整可用的，智能体读 `error_samples` 就能定位。
- **报告字段即契约**：`frame_us`（最近秩百分位 —— 报告里每个数都是真实出现过
  的一帧，不是插值）、`logs`（含装载期脚本编译失败这种"帧 0"日志）、
  `error_samples`（前 10 条）、`non_finite_transforms`、`audio`（silent 引擎的
  真实解码 / 缓存计数）、`fixed_steps`。`json_contract_has_stable_keys` 钉死。
- **`--dump-scene`**：跑完后的场景落盘为 RON（与 `write_scene` 同一序列化配置）
  —— 观察物理落点 / 脚本改过的变换的出口，也可直接喂 MCP `scene_write`。

### 二、确定性（刻意的）

每帧固定 `dt`（默认 = 项目固定步长）、无输入、无墙钟依赖 —— 同一项目两次运行
**逐位一致**（测试里两次运行落盘场景 SHA256 相等钉死；是"改前改后对 diff"的
底座）。`--dt` 允许偏离固定步长（时钟补多个固定步），但**要确定性就沿用默认**。

### 三、实测（先量后改的"量"；本机样本，看数量级）

| 场景 | 300 帧墙钟 | 帧循环 | 帧均值 | p50 / p99 |
|---|---|---|---|---|
| `projects/SampleScene`（14 实体 / 10 刚体 / 3 脚本 / 1 音源） | 78~120 ms | 8.6 ms | 28.5 µs | 26 / 37~61 µs |
| `projects/BehaviorsDemo`（Rust 插件 + Lua 共存） | 62~82 ms | 1.3 ms | 4.4 µs | 3.7 / ~10 µs |
| 基线：`--frames 1` / `--check`（进程启动 + 装载） | 62~72 ms | — | — | — |

读法：**一次无头调用的固定成本 ≈ 进程启动 60 ms**（Windows 加载链接的 DLL），
帧循环本身个位数毫秒 —— "跑一下看看"在智能体循环里是廉价操作。

### 四、MCP `run_headless`（第 7 个工具，spawn 透传）

- **信封**：`{ "exit_code": 0|2, "report": {…}, "scene_ron": "…（include_scene
  才带）" }`。`report.ok=false`（退出码 2）**不是工具失败** —— 问题表达在报告里；
  只有打不开项目（退出码 1）才走 `isError`。
- **为什么走 spawn**：无头实现住在编辑器二进制里（同一段 `World::update`）；
  server 装在哪编辑器就在哪（`dist/`、`target/debug/` 都是同目录布局）——
  `find_editor_exe` 同时找同目录与**上一级**（`cargo test` 时 server 在
  `target/*/deps/` 下跑，编辑器 exe 在它上一级）。
- **校验归属编辑器唯一**：`frames` / `dt` 的合法性不做双侧校验（必然漂移），
  MCP 只做 JSON 类型检查、其余透传。
- **Windows 细节**：spawn 带 `CREATE_NO_WINDOW`（MCP server 常被 GUI 客户端
  无控制台拉起，不设会闪控制台窗）；孩子 stderr 进错误消息前**截尾 2000 字符**
  （`RUST_LOG` 开着时头部是成片日志噪声，不拿它灌智能体上下文）。

### 五、实现期踩的坑（防好心人翻案）

- **过期的编辑器 exe 会把测试挂死**：旧版编辑器不认 `--headless`，会把它当
  项目路径继续 `eframe::run_native` 弹窗 → 测试进程挂住。而 `cargo test` 不产出
  常规 bin 产物，`target/*/xgame-editor.exe` 可能停在上次 `cargo build`。
  处理：MCP 测试沿用 `xgame/tests/plugin_loading.rs` 的纪律 —— 取 exe 前先
  **嵌套 `cargo build -p xgame-editor` 一次**（OnceLock，每进程一次，带 240s
  超时防与外层 cargo 抢目标目录锁）。
- **PowerShell 管道会把子进程 stdout 的 UTF-8 按 GBK 解码弄坏**（中文变 `?`，
  JSON 直接解析失败）—— 手工冒烟要 `cmd /c "exe > file"` 做字节级重定向，
  或在管道前先 `[Console]::OutputEncoding = UTF8`。这是本仓库的老坑（PS 5.1），
  与 AGENTS.md 的工具链注意同源。
- **`find_editor_exe` 的上一级命中路径拼写带 `..`**（`deps/../xgame-editor.exe`）：
  测试若按字节比较路径会假红（功能是对的）。断言应比较**解析后的同一文件**
  （`canonicalize` 后比较），或者干脆只断 `is_some` + 文件名 —— 别把 `join` 的
  拼写行为写死进契约。

### 六、刻意的取舍

1. **`include_scene` 默认关**：默认响应只带报告（智能体大多只要"跑通没有"）；
   场景 RON 可能很大，要观察模拟结果再显式打开 —— 不拿大响应占上下文。
2. **`--headless` 不重编 Rust 插件**：`Plugins/*.dll` 按现状加载（与打包游戏
   同路径）；改了插件源码要先在编辑器 Play 一次或走 PIE 构建。
3. **不做输入模拟、不推进动画时钟**：无头没有输入源；动画采样在渲染宿主侧
   （`AnimationDriver`），脚本写的 `set_anim_param` 照常进场景、只是没人消费。
   两条都写进了 `Docs/无头运行.md` 的边界一节，不给"它应该能"的误解留空间。
4. **`--frames` 上限 100000**：防"手滑写个大数挂住一个智能体"；100k 帧 @60Hz
   已是 ~28 分钟模拟时长，真实验证远用不到。
5. **Worktree 工作区状态**：验证 BehaviorsDemo 时把 `target/debug/behaviors_demo.dll`
   同步进了 `projects/BehaviorsDemo/Plugins/`（该 DLL 是构建产物、未入 git；
   编辑器 PIE 平时也这么干）—— 过期 DLL 跨 ABI 加载是 0xc0000005 的已知雷
   （P1 记录在案），验证前刷新是纪律不是可选项。

## 二十四、渲染层 PBR + 逐实体材质覆盖 + 无头截图（本轮）

差距分析里"观感落后一代"的 P0 主因：光照模型停在 Blinn-Phong，材质参数
**读进来就丢掉**。本轮三件事一次闭环：① Cook-Torrance 替换 Blinn-Phong；
② 逐实体材质覆盖（`MaterialOverride`，"材质系统"的入口）；③ `--screenshot`
无头截图（渲染改动的唯一可靠验证手段）——截图接上 MCP 后，智能体能**看到**
自己造的画面。全量验证绿：`cargo check --workspace --all-targets` 绿、
clippy **0 警告**、`cargo test --workspace --all-targets` **391 条全绿**
（2 条基准默认 ignore；较上轮 +32：截图 CLI 18 种边角、材质 7 条、
合并语义 3 条、逐实体 tint 1 条、指纹 1 条、MCP 3 条）。

### 一、PBR：Cook-Torrance 替换 Blinn-Phong

- 材质参数（metallic / roughness）**原样进 GPU**，片元着色器实现完整
  Cook-Torrance：GGX 分布 + Smith 几何 + Schlick 菲涅尔；`F0 = mix(0.04,
  albedo, metallic)`、漫反射项 `kd = (1 - F)(1 - metallic)`。金属没有漫反射、
  绝缘体有 4% 基础反射 —— 从"每盏灯两个经验项"变成有物理意义的模型。
- **环境项 = `ambient × (albedo×(1-metallic) + F(N·V))`**：均匀环境近似。
  真 IBL（环境贴图 / 预过滤）留给 RenderGraph 轮 —— 但结构上先给金属留了
  环境反射的挂点，到时候只换这一项。
- **radiance × π 的亮度标定**：历史场景的灯强度是按 Lambert 手调的
  （`diffuse = albedo × intensity × ndotl`）。PBR 的漫反射是
  `albedo/π × radiance`：着色器里取 `radiance = intensity × π`，与旧公式
  **数值等价** —— 换模型不换亮度，材质调参才有可比较的基线（否则全场先暗
  3.14 倍，所有场景内容都得重调）。
- **`MIN_ROUGHNESS = 0.04`**：GGX 在 roughness = 0 时分母为 0（除零 → NaN
  随第一个像素扩散到整帧），夹到 0.04 的镜面感与真镜面肉眼无差。
- **色调映射（ACES）刻意暂缓**：HDR 目标要动 Presenter / 游戏 / 编辑器三个
  宿主的渲染目标，超出本轮；当前高光仍会裁剪，属已知限制（差距分析同步
  改了措辞）。

### 二、逐实体材质覆盖（`MaterialOverride`）

- **场景格式**：`Entity.material: Option<MaterialOverride>`，三个字段全
  `Option`（base_color / metallic / roughness），serde
  `default + skip_serializing_if` —— 旧场景逐位兼容；RON 里 `Option` 显式写
  `Some(...)`（与 `physics` / `audio` 同款，见 `material.rs` 文档的示例）。
- **字段语义**：base_color **相乘**（tint 乘子：内置白模 = 直接上色，
  glTF 带贴图的模型 = 调整体色调，a 乘透明度）；metallic / roughness
  **替换**（"乘 0.5 的金属度"没有直觉含义）；**blend 不参与**（透明与否是
  管线级行为，不是颜色参数）。
- **渲染侧结构改动（防好心人翻案）**：材质参数从"每材质一个绑定组"改为
  **逐 draw uniform** —— `DrawUniform { model, params, base_color, material }`
  （112 字节），staging 按**图元**分段、动态偏移取段；group(1) 只剩
  贴图 + 采样器。为什么必须换：逐实体覆盖让"同一 material_id 的两个实体
  材质不同"，按资产缓存参数要么改色时重建绑定组、要么缓存键随数值无限膨胀；
  逐 draw 段天然正确。上限 `MAX_RENDER_DRAWS = MAX_RENDER_ENTITIES × 2`
  （多材质网格一只实体出十几个 draw，不是一对一），超出静默丢弃 ——
  动态偏移越过缓冲 = 整帧校验失败，宁缺勿炸。
- **编辑器**：Inspector 新增 Material 区（Override 勾选 → Tint 色盘 +
  metallic / roughness 勾选式 DragValue）；`scene_signature` 纳入材质哈希
  （漏掉的话"在 Inspector 里调颜色"视口纹丝不动 —— 指纹门控的缓存帧）；
  组件字段编辑走既有撤销通道（P1.5 结论不变：参数是场景数据，编辑进撤销栈）。

### 三、无头截图 `--screenshot`

```sh
xgame-editor --headless --frames 150 --screenshot out.png \
    [--shot-size 1600x900] [--camera-pos x,y,z --camera-target x,y,z] <项目>
```

- 尺寸默认 1280×720、上限 4096²；`--camera-pos` / `--camera-target` **必须
  成对**（单给一侧 = 参数错误）；`--shot-size` 等只在配合 `--screenshot` 时
  才合法（给了没截图 = 报错，不给"静默忽略"留歧义）。
- **默认取景 = 拟合全场 + 顺光机位**：先 `frame_bounds` 框住全部可绘制实体
  （含背景模型），再算**相机方位 = 主方向光的 XZ 方位角 + 35°、仰角 26°**。
  为什么是顺光：太阳在 -x/-z 时把相机放在 +x/+z，画面全是背光面 + 一丝环境
  光，白模看起来像"渲染坏了"——实测第一版截图如此，不是 PBR 的问题而是机位
  问题。无可用方向光时退回固定方向（纯函数 + 两条测试钉死）。
- **无 GPU：退出码 1、不降级**。音频可以 silent 降级（照常计数只是不出声），
  截图没有"静默降级"的余地 —— 拿不到适配器就是拿不到图。
- 报告新增 `screenshot { path, width, height, eye, target }`：`eye/target` 是
  **实际使用**的相机（自动取景时是拟合结果）——"这张图从哪拍的"必须可复现。
- **确定性**：同机两次进程运行 PNG **逐字节一致**（本轮实测 SHA256 相等）；
  跨机器不承诺（GPU 光栅化差异），文档按此口径。
- **`--screenshot` 是推进动画时钟的唯一例外**：静态截图里角色停在绑定姿态
  没有信息量，截图路径每帧 tick `AnimationDriver`；其余无头路径维持"不推进
  时钟"的边界（脚本 `set_anim_param` 照常进场景、没人消费）。

### 四、MCP：`run_headless` 加 `screenshot`

- `screenshot: true` → spawn 编辑器带 `--screenshot <临时 PNG>`；读完即删，
  PNG 以 **MCP `image` 内容块**（base64）随结果返回 —— 视觉模型直接看到
  画面本身，而不是"渲染没报错"的口头保证。信封加
  `{"screenshot": {"bytes": n}}`，实际机位在报告的 `screenshot` 节里。
- **只有一张图、默认取景，不给相机参数开洞**：智能体很难凭一组 xyz 取景，
  自动取景 + 可复现机位报告已经够用；要精细取景走 CLI。
- base64 自写 30 行（不引新依赖），RFC 4648 已知向量 + 长度关系测试钉死；
  「ToolOutput → content 块」抽成纯函数 `tool_result_content` 单独测 ——
  `mimeType` 是协议拼写，写错客户端会把图片块静默丢掉。

### 五、示例内容（SampleScene 上色）

- 内置白模第一次有颜色：地面混凝土（0.55/0.58/0.62，rough 0.9）、平台三级
  暖沙、木箱三兄弟深浅、蓝半光泽 Spinner（rough 0.35）、红塑料球
  （rough 0.30）、抛光金属柱（metallic 1.0 / rough 0.35，与红球对照
  "金属 vs 塑料"）。
- **太阳强度 3.2 → 1.4**：3.2 是 Blinn-Phong 时代"白模也要亮"的手调值 ——
  白 albedo 地面在 ndotl 0.87 × 强度 3.2 下辐射度 ≈ 2.7，早已是裁剪纯白。
  PBR 的 π 标定保持了这个亮度（不是回归），所以过曝是**内容**问题：上色 +
  回调强度后，受光面落在"高光但不死白"的区间。
- 实拍：`target/shots/after3_sample.png`（自动取景，顺光）、`hero1.png`
  （近景：高光、软影、材质差异全部可见）。

### 六、测试与先证红

- 先证红（仓库纪律）：材质 uniform 两条测试在旧实现下红（旧代码把
  metallic=1 写成 params[0]=0.6）；金属/绝缘像素测试在旧 Blinn-Phong 下
  **方向相反**（184 vs 152 luma；PBR 后 95 vs 147）——换实现后转绿即证。
- 新增：core 7 条（material.rs 4：字段级解析 / 空表 / None 不落盘 / 往返；
  scene.rs 3：落盘往返 / 普通实体不写字段 + 旧文件解析 / 脏检查）、
  gltf_loader 合并 3 条、render 逐实体 tint GPU 测试 1 条（覆盖 → 像素变蓝、
  清掉覆盖 → 与基线**逐位一致**）、editor 指纹 1 条、MCP 3 条
  （base64 向量 / 截图像素块 / content 块协议形状）。
- **像素测试的一个实测账**：tint 测试最初把光与相机放同轴，GGX 副瓣在中心
  像素叠了 ~0.24 线性白（roughness 0.45 时 D≈7.8），看去像"乘子没生效"
  （实测 176 vs 基线 255，比值 0.69 > 阈值 0.5 假红）。把光源错开轴线后
  中心像素是纯漫反射，断言才有判别力 —— **写像素测试必须算一遍镜面项**。

### 七、刻意的取舍

1. **不做 IBL**：金属在均匀环境近似下偏暗（只有 ambient×F 兜底），这是
   物理上诚实的"没有环境可反射"；真 IBL 属 RenderGraph/后处理轮。
2. **不做 ACES 色调映射**：见第一节；现在的高光裁剪是已知限制。
3. **材质不接脚本 API**：本轮入口 = Inspector / 场景 RON / MCP
   `scene_write`；脚本 `set_material` 留给下一轮（连带动画/材质曲线一起做）。
4. **blend 不可覆盖**：见第二节；要"开关透明"得换材质而不是改参数。
5. **材质随 prefab 自动走**：实例化走 `entity.clone()`，新字段零改动自动
   进预制体实例（这是"Entity 字段一律 Clone + PartialEq"惯例的红利）。

## 二十五、第九阶段：HDR 渲染路径 + ACES 色调映射 + 曝光控制（本轮）

二十四节的"刻意的取舍 2"（不做 ACES 色调映射）在本轮兑现 —— 差距分析里
"PBR 落地后最划算的下一项"（高光直接裁剪是最扎眼的已知限制）。三件事一次
闭环：① 场景 pass 从直写 sRGB 目标改为**渲进 `Rgba16Float` HDR 中间标**；
② 新增全屏后处理 pass 做**曝光 + ACES 色调映射**写进 LDR 显示目标；③ 画面
设置落 `project.ron` 新增的 `render` 节，三家宿主（编辑器视口 / 独立游戏 /
无头截图）**同一份配置、同一条链**。全量验证绿：`cargo check --workspace
--all-targets` 绿、clippy **0 警告**、`cargo test --workspace --all-targets`
**407 条全绿**（3 条基准默认 ignore；较上轮 +16 条、+1 条 GPU 基准）。

### 一、HDR 渲染路径：为什么中间标是必须的

- LDR 目标的写入是**钳制**的：8 位 sRGB 存不下 4.0 的高光，超出的部分在写
  下去那一刻就被裁掉 —— 高光细节不是"丢了"，而是**从来没被看见过**。色调
  映射的全部意义就是在这次裁剪**之前**拿到真实亮度。
- `HDR_FORMAT = Rgba16Float`：WebGPU 核心格式里唯一同时满足"浮点、可混合、
  可 MSAA、可线性过滤"的颜色目标；存储带宽只有 `Rgba32Float` 的一半。
- **`HdrTargets` 收拢三家宿主的附件创建**（HDR 颜色 + MSAA 颜色 + 深度）：
  编辑器要节流重建、游戏跟 resize、截图建一次，但"需要哪几张纹理、采样数
  怎么对"是同一件事 —— 三份手写纹理描述迟早漂移，而采样数不匹配的渲染通道
  在提交时**直接校验失败**。
- MSAA 采样数按 **HDR 格式**问 `supported_sample_count`，不是显示格式：采样
  数是格式相关的，拿显示格式问出来的答案在 HDR 附件上不一定成立。

### 二、后处理 pass 与"不做 RenderGraph"

- 后处理是**一趟全屏单采样 pass**（全屏三角形，位置由 `vertex_index` 现算）：
  采样 HDR → ×曝光 → ACES（或 Off）→ 钳制 →（必要时手动 sRGB 编码）→ LDR。
- **为什么不做通用 RenderGraph**：通用帧图是给"十几个 pass 互相借纹理"的
  规模准备的；当前链是线性的一串。现在抽图只是抽象税 —— 等出现真正带依赖
  分叉的 pass（SSAO 要深度、TAA 要历史缓冲、CSM 要重投影）再抽，那时是
  **有信息**地抽。这条写进 `postfx.rs` 模块文档，防好心人提前上框架。
- 绑定组按宿主维护的 `target_gen`/`source_gen` 缓存：源视图只在目标重建时
  才换，每帧新建绑定组是白给的分配；但**不能用指针判等** —— `TextureView`
  没有稳定身份，指针在释放后可能被复用（表现是画面停在旧尺寸的最后一帧）。
- `needs_manual_srgb(format)`：sRGB 目标交给硬件编码（着色器输出线性值），
  非 sRGB 目标（`Bgra8Unorm` 之类，交换链格式由系统挑）必须着色器手动编码 ——
  赌"交换链一定是 sRGB"会在部分机器上整幅偏暗。

### 三、`render` 配置节（project.ron）

```ron
render: (exposure: 1.0, tone_map: Aces),
```

- `RenderConfig { exposure, tone_map }`，serde `default` +
  `skip_serializing_if`：**旧清单没有这一节照常加载**
  （`old_manifest_without_render_defaults`）。
- `sanitized()` 把曝光夹进 `[0.05, 20]`（0 或负数会让整帧发黑 / 负值进曲
  线），NaN/∞ 落回 1.0 —— 与 `runtime` 节同款契约，入口（设置面板 / MCP）
  统一走它。
- `ToneMapMode` 的 **RON 变体标签即磁盘格式**（`tone_map_ron_labels_are_stable`
  钉死）：改标签名 = 作废所有旧清单。
- 编辑器：设置面板新增"画面（曝光 / 色调映射）"区；视口 `scene_signature`
  纳入 `RenderConfig`（漏了表现为"在设置里调曝光视口纹丝不动" —— 指纹门控
  的缓存帧，`signature_tracks_render_settings` 钉死）。
- 无头运行报告与 MCP `project_info` 各带一个 `render` 节（字段名进稳定契约，
  `json_contract_has_stable_keys` 同步）："这张图按什么设置拍的"与机位一起
  可复现。

### 四、ACES 与亮度基线

- **曲线 = Stephen Hill 的 RRT + ODT 有理函数拟合**（三.js / Unity 同款近似）：
  先转到 ACEScg（AP1）做曲线，再回 sRGB。比"对 RGB 直接套一条曲线"的朴素
  做法最大收益是**过曝处的色相漂移小**：纯红高光烧出来仍红偏白，不会变橙黄。
- **曲线前 /0.6**：拟合把 0.18 中灰映到 ~0.10（整幅暗一档）。本引擎亮度按
  "白面 ≈ 1.0 线性"标定（PBR 的 π 校准），除以 0.6 把中灰拉回 ~0.21 ——
  **整体亮度基线不因接入色调映射而变**，变化只在高光那一端（`mid_gray_stays_mid_gray`
  钉死：0.18 中灰显示值仍落 110~150）。
- **`ToneMapMode::Off` 是兼容模式不是调试开关**：曝光 + 钳制的输出与色调映射
  上线前的旧管线**逐位一致**（`off_mode_matches_the_pre_tonemap_pipeline`）——
  旧项目迁移与像素回归对照都靠它。
- 曝光是**曲线之前**的亮度乘子：挪到曲线之后会把高光端钳平，不是风格问题
  而是语义问题（`exposure_scales_before_the_curve` 与 GPU 侧同款测试双钉）。

### 五、测试与先证红

- **先证红（实测）**：把 `HDR_FORMAT` 换回 `Rgba8UnormSrgb` 后
  `aces_keeps_highlight_detail_that_off_clips` 立刻红（ACES 像素 226 vs
  预期 243）—— 场景直写 sRGB 目标时写入即钳制，"ACES 与 Off 的差异"根本
  无从谈起；换回 HDR 路径转绿即证接通。
- CPU 镜像 `postfx::cpu`（与 WGSL 逐式对应，改一边必须改另一边）：8 条纯函数
  测试 —— 端点、单调性（0..32 扫描无回折）、滚降（4.0 与 16.0 映射到两个不同
  的非 255 值）、曝光在曲线前、Off = 旧管线、手动 sRGB 编码对表（0.5 → 188）、
  `needs_manual_srgb` 判定、中灰基线。
- GPU 像素测试 2 条（`screenshot.rs`）：
  `aces_keeps_highlight_detail_that_off_clips`（Off 必须纯白 255、ACES 必须
  <250 且与 CPU 镜像对表 ±4）、
  `exposure_scales_highlights_before_the_curve_on_gpu`（0.25/1/4 三档显示亮度
  严格递增 + 对表一档）。
- core 5 条：`render_config_defaults_to_aces_and_unit_exposure` /
  `render_config_sanitized_rejects_garbage` / `old_manifest_without_render_defaults` /
  `render_survives_manifest_roundtrip` / `tone_map_ron_labels_are_stable`；
  editor 1 条（指纹纳入画面设置）；headless `json_contract_has_stable_keys`
  扩展 `render` 节断言；MCP 断言并入 `project_info` 既有用例。

### 六、性能实测（先量后改：GPU 时间戳口径）

新增基准 `crates/xgame-render/tests/postfx_cost.rs`（`#[ignore]`，手动跑）：
同机背靠背测"旧 LDR 直渲"与"新 HDR + ACES"的每帧成本（1280×720，地面 +
64 立方体 + 跟随阴影，MSAA x4）。**口径教训**：首版用墙上时钟，被本机噪声
打穿（同一路径轮间 207~515 µs、胜负都能翻转），改为 **GPU 时间戳查询**
（帧头 / 场景末 / 帧末三个点）后轮间波动 <1 µs —— 基准测的是 GPU 成本就
别让 CPU 与调度混进来。

| 段 | 旧 LDR 直渲 | HDR + ACES |
|---|---|---|
| 场景 pass（含 MSAA resolve） | 121.2 µs | 135.2 µs |
| 后处理 pass | 0（无） | 22.6 µs |
| **合计（GPU）** | **122.0 µs** | **157.8 µs** |
| CPU 编码 + 提交 | 137.2 µs | 155.5 µs |

**差额 +35.8 µs/帧（+29.3%）**：后处理 pass 22.6 µs 是纯新增；场景段
+13.9 µs 是 fp16 附件写带宽 + fp16 resolve。折算：60 FPS 预算（16.6 ms）的
0.22% —— 换来高光不再裁剪，账很清楚。CPU 侧 +18 µs 是"多记录一趟 pass +
多写一只 uniform"。

### 七、实拍对照（曝光 4.0）

- `target/shot_off4.png`（Off）：地面 / 顶面整体爆白，**24747 个纯白像素**
  （1280×720 隔点抽样 10.7%）；红盒成为生硬纯红。
- `target/shot_aces4.png`（ACES）：纯白像素 **0 个**；地面保留明暗层次、
  红盒高光向珊瑚色柔和滚降（高光去饱和是 ACES 的特征形态）。
- 校准基线：线性 1.0 → 显示 ~226/255、4.0 → ~250（不裁顶）；0.18 中灰 →
  ~127（Off 模式下 4.0 与 16.0 都是 255）。

### 八、刻意的取舍（防好心人翻案）

1. **不做通用 RenderGraph**：见第二节 —— 线性链抽图只是抽象税。
2. **`Off` 保留而不是删掉**：它是旧管线的兼容出口与像素对照基准；默认
   `Aces`，但"想看裁剪语义"是合法需求。
3. **1/0.6 的亮度基线不许动**：动它 = 全部旧场景整体明暗挪一档，与 PBR 的
   π 校准是同一条"换模型不换亮度"的原则。
4. **不做自动曝光**：需要帧间状态的亮度统计（直方图读回或计算着色器 + 历史
   缓冲），先把静态曝光 + ACES 铺平 —— 与 Bloom 一起留在后处理链的下一片。
5. **无 IBL 的限制仍在**：金属的世界只有 `ambient × F` 兜底 —— 色调映射把
   高光滚下来后它更好看了，但"没有环境可反射"这件事没变（差距分析同步记）。
6. **不给任何宿主留"绕过色调映射"的旁路**：截图 / 视口 / 交换链三个目标
   同源，旁路会让三者画面分叉 —— 与"单一运行时"是同一条纪律。

## 二十六、第九阶段：Bloom 泛光（本轮）

二十五节把 Bloom 与自动曝光一起留给了"后处理链的下一片"，本轮兑现 Bloom：
**亮部提取（软膝）→ 三级 mip 降采样 → 可分离高斯模糊 → 与色调映射同一趟
合成 pass 叠加**。默认**关**（`intensity == 0` 是唯一关闭形态），开启才惰性
建链；参数落进 `project.ron` 的 `render.bloom`（intensity / threshold / knee），
编辑器设置面板可调、智能体改一行文本即可，三宿主（编辑器视口 / 独立游戏 /
无头截图）同一条链。全量验证绿：`cargo check` / clippy **0 警告**、
`cargo test --workspace --all-targets` **419 条全绿**（3 条基准默认 ignore；
较上轮 +12 条：core 配置 2 + CPU 镜像 8 + GPU 像素 2）。

### 一、链的形状：为什么是"亮部提取 + 三级 mip + 可分离高斯"

```text
HDR ──亮部提取(软膝)·½降采样──▶ a0 ──降采样──▶ a1 ──降采样──▶ a2
                                 │            │            │
                                 ├─横/纵模糊──┤（每级 a、b 两块纹理乒乓，
                                 ▼            ▼   同纹理不能既当输入又当输出）
                           a0/a1/a2（三级模糊结果，合成 pass 读取）

合成（同一趟全屏 pass）：HDR ─▶ 曝光 ─▶ + Σ(aᵢ×wᵢ)×intensity ─▶ ACES ─▶ LDR
```

- **可分离**：N×N 高斯要 N² 次采样，横一趟 + 纵一趟各 9 抽头只要 18 次 ——
  同一视觉结果，采样数降 4.5×；分辨率减半等效核宽翻倍，三级 mip（½、¼、⅛）
  用同一套核叠出由近及远的柔和光晕。九趟 pass 全部在 ≤½ 分辨率上（三级合计
  像素量 = 主画面的 21/64 ≈ 33%）。
- **9 抽头核 `[1,8,28,56,70,56,28,8,1]/256`**（二项式）：近似正态、系数和恰为
  256，归一化在纸面可验（镜像测试断言合成权重和为 1）。
- **降采样一次双线性到位**：输出像素中心恰好落在源 2×2 texel 块的中点 ——
  一次 `textureSample` 就是一次箱式平均，比四次点采样便宜，也没有接缝。
- **合成权重 `[0.5, 0.3, 0.2]`**：小 mip 更模糊、覆盖更广，权重更低；乘
  intensity 后是"光晕总能量"的线性度量。

### 二、亮部提取：阈值在曝光后判 + 软膝 + 按最大通道

- **阈值是"显示值"语义**：摄影上"多亮算亮"取决于曝光后的画面（threshold = 1.0
  就是显示白点）。提取时先乘曝光，合成时光晕也加在曝光后的颜色上 —— 改曝光
  光晕跟着走，不用重调阈值（`..._applies_exposure_first` 钉死）。
- **软膝**（Unity URP 同款口径）：

  ```text
  soft = clamp(brightness − threshold + knee, 0, 2·knee)
  soft = soft² / (4·knee + 1e-5)
  weight = max(soft, brightness − threshold) / max(brightness, 1e-5)
  ```

  `knee = 0` 退化为硬阈值（`..._knee_zero_is_a_hard_threshold`）；分母的
  `1e-5` 兜住除零。硬阈值的亮斑边缘是一圈"刀切"，软膝让贡献在阈值附近
  二次淡入 —— 那种"比不做还难看"的硬边就是它治的。
- **权重 ≤ 1 的归一化**：软膝区按比例淡入、过膝后全额参与，光晕不会比源像素
  还亮。提取保留原色相（乘权重而非替换颜色）：白斑发白光，红斑发红光。
- **按最大通道判亮度**：纯红的高光不该因为"平均亮度低"而不发光 —— 彩灯的
  光晕是彩色的（`..._selects_by_the_max_channel`）。

### 三、默认关：风格参数不进默认画面

- **默认 `intensity = 0`**：Bloom 是**风格**参数（写实档也常关），不是
  "正确性"参数 —— 与色调映射相对照：后者回答"高光怎么落进显示区间"，是必须
  回答的问题，所以默认 ACES；前者是"要不要发光"的口味问题。要发光的项目把
  intensity 调到 0.4~0.8 即可。
- **不设独立开关**：`intensity == 0` 就是关 —— 两个字段表达同一状态迟早
  互相矛盾。
- **关着时整段跳过**：`PostFx::render` 不建纹理、不记录 pass，还把链丢回
  `None`（不养显存）；是真正的零成本，而不是"跑一遍然后乘 0"。开启时惰性
  建链，链的重建判据是**源尺寸**（尺寸不变就只记录 pass）。
- **合成绑定组缓存键 =（源代, 尺寸, 开关）**：源视图换代 / 尺寸变化 / 开关
  翻转都会拿到新绑定组；关着时 mip 槽位用**源视图占位**（绑定组要求槽位逐项
  填），着色器按 `bloom_intensity`（uniform）分支不去采样 —— uniform 值 ⇒
  整个 draw 走同一分支，`textureSample` 在一致性控制流里才合法。
- **入口三件套**：`project.ron` 的 `render.bloom`（`sanitized()` 夹取安全范围）、
  编辑器 ⚙ 设置面板三控件（范围来自 `BloomConfig::MAX_*`，hover 有说明）、
  MCP / 报告 JSON 只读投影 —— 与曝光 / 色调映射同一条"单入口"纪律。

### 四、wgpu 边界：几个踩过就懂的细节

- **同一张纹理不能既当输入又当输出**（wgpu 提交时校验）：每级 mip 配一对
  a/b 纹理乒乓，横向模糊 a→b、纵向 b→a，最终结果回到 a。
- **每趟 pass 一律 `LoadOp::Clear`**：全屏三角形铺满目标，内容每帧被完整
  覆写 —— "建时清黑"那点状态不需要，还消灭了"采样到未初始化内容"的窗口。
- **纹理本体放手是安全的**：wgpu 的视图持纹理的 `Arc`、绑定组持缓冲的
  `Arc`（查过 wgpu-core 23 源码）；链的字段只存视图与绑定组，不存纹理句柄。
- **`wgpu::Sampler` 不是 Clone**：`BloomChain::record` 收采样器参数而不是
  自己持有一份。
- **pass 顺序就是执行顺序**：链记录在合成 pass 之前，同队列上的隐式屏障
  保证合成当帧读到链的结果，不需要手动同步。
- **亮度提取的源是外部（HDR 附件）**：它的绑定组按 `source_gen` 单独缓存；
  降采样 / 模糊的源与目标都在链内，绑定组建链时一次性建好。

### 五、测试与先证红

- **先证红（两条实测）**：① 强制 bloom 分支为关 → `bloom_glows_around_a_bright_spot`
  红（开 / 关同一点逐位相同，差 0）—— 证明"光晕"不是场景自身的形态；
  ② 把 `fs_bright` 的权重改成恒 1.0 → `bloom_leaves_sub_threshold_content_dark`
  红（阈值以下的微光被错误点亮）。
- **CPU 镜像新增 8 条**（`postfx::cpu`，与 WGSL 逐式对应）：阈值下全黑、
  软膝形状（单调 + 与硬阈值可区分）、knee=0 硬阈值、最大通道选择、曝光先行、
  权重和为 1、合成在曲线前（同权重下"先加光晕再 ACES" ≠ "先 ACES 再加"）、
  关时与纯色调映射逐位一致。
- **GPU 像素测试 2 条**（`screenshot.rs`）：光块轮廓外 ~5px 处开 > 关 +25、
  远角差 < 4（光晕围着亮源、不照亮全场）、强度 2.0 > 0.6 + 5（单调）；
  微光内容（受光面线性 ≈ 0.19，低于软膝下限 0.4）开关两档差 < 3。
- **core 2 条**：默认关 + 默认不落盘（`skip_serializing_if`） / `sanitized`
  容错（NaN / 负数 / 超限；`intensity` 负数 = 关，不是"反向光晕"）。
- **编辑器**：`scene_signature` 纳入 bloom 三字段（3 条 `assert_ne`）——
  防"调泛光视口纹丝不动"的指纹门控 bug。
- **报告契约**：headless 报告与 MCP `project_info` 的 `render.bloom` 三字段
  进稳定契约（`json_contract_has_stable_keys` 同步）；`two_runs_produce_identical_png_bytes`
  改用 bloom 开启（0.7）跑 —— 光晕链逐字节确定。

### 六、性能实测（GPU 时间戳口径）

沿用 `postfx_cost.rs`（1280×720、MSAA ×4、地面 + 64 立方体 + 阴影），
三路径交替 ×8 轮 ×64 帧，取中位数：

| 段 | 旧 LDR 直渲 | HDR + ACES | HDR + ACES + Bloom |
|---|---|---|---|
| 场景 pass（含 MSAA resolve） | 121.8 µs | 134.6 µs | 135.2 µs |
| 后处理 pass | 0.7 µs（连打两点残留） | 24.6 µs | 85.0 µs |
| **合计（GPU）** | **122.5 µs** | **159.2 µs** | **220.2 µs** |
| CPU 编码 + 提交 | 140.4 µs | 157.1 µs | 233.8 µs |

- **Bloom 相对纯 ACES +61.0 µs/帧（+38.3%）**：1 趟亮部提取 + 2 趟降采样 +
  6 趟可分离模糊，全在 ≤½ 分辨率。折算 60 FPS 预算（16.6 ms）的 0.37%。
  同会话重跑合计 220.3 µs、相对 ACES +60.8（+38.1%）—— 两跑互证。
- 三条路径的场景段在断言容差（< 5 µs）内一致（134.6 / 135.2）—— 链没有
  回头改场景渲染。
- CPU 侧 +76.7 µs 是"多记录 9 趟 pass + 每趟写 uniform"的编码开销；链的
  纹理与绑定组只建一次，不进每帧成本。
- ACES 相对旧路径 +36.7 µs（+30.0%），与上一轮（+35.8 / +29.3%）及同会话
  重跑（+37.8 / +31.1%）同量级 —— 三次读数互证，口径稳定。
- 基准自带防"空调度"断言（P2.5 教训）：`bloom_post > hdr_post + 1.0`（链
  缺席必红）、场景段差 < 5 µs、时间戳读数非零。
- 口径残留：旧路径"连打两个点"的后处理段读数不是 0（本次 0.7 µs），那
  就是 `write_timestamp` 本身的开销 —— 把它当噪声底看。

### 七、实拍对照

- `target/bloom_off.png`（默认关）：平台边缘干净利落、阴影锐利。
- `target/bloom_on.png`（演示**刻意调强**：threshold 0.8 / intensity 1.2）：
  整个亮面的边缘往外"渗光"，台上物件带柔和光晕，与环境光的灰底连成雾感。
- 逐像素比对（1280×720 隔点 230400 样本；两图 eye/target 逐位相同，差异里
  没有第二个变量）：全部样本**变亮或持平**（变暗 0 个）、平均 +4.9/通道、
  最大 +122 —— 与"光晕加在曲线前、ACES 单调"的预期一致。
- 对照参考：默认参数（threshold 1.0 / intensity 0.8）下差异更含蓄 ——
  ~1% 像素 Δ > 3（maxΔ 45）。threshold = 1.0 是显示白点，SampleScene 里
  只有高光面越得过去 —— 这与"默认关"呼应：想看见光晕，先把艺术参数往上调。

### 八、刻意的取舍（防好心人翻案）

1. **默认关**：见第三节 —— 风格参数不该改变默认画面；"要不要发光"与
   "高光怎么落进显示区间"是两类问题。
2. **阈值在曝光后判**：挪到曝光前会让"调曝光"与"调阈值"耦合（每次改曝光都得
   重调阈值），摄影语义只有一种直觉。
3. **光晕加在曲线前、不加在曲线后**：曲线后加是"贴纸"（暗部会蒙一层灰、
   高阈值时光晕先在 LDR 端被裁出硬边），曲线前加才等价于"场景里真的有那么
   亮的光"。
4. **不抽 RenderGraph**：链仍是线性固定序列（提取 → 2 降采样 → 6 模糊 →
   合成），理由与二十五节相同 —— 等出现带依赖分叉的 pass 再抽。
5. **不做自动曝光 / 亮度统计**：需要直方图读回或计算着色器 + 历史缓冲，
   继续留给后处理链的下一片（与 FXAA/TAA 排一起）。
6. **不引 Kawase 双滤波 / 各向异性核 / 镜头鬼影**：先把"正确、便宜、可解释"
   铺平；换核是风格问题不是功能问题。

## 二十七、第九阶段：程序化天空盒 + 环境光照（IBL）（本轮）

差距分析 P1 表里的 **IBL / 天空盒** 兑现：天空是一个**解析函数**（天顶 / 地平线 /
地面三段渐变 + 太阳盘 + 太阳光晕），同一个函数喂三个消费者 —— 背景天空盒、
漫反射环境光（L2 球谐）与镜面环境光（GGX 重要性采样预过滤立方体 + BRDF LUT）。
参数落进 `project.ron` 的 `render.sky`（七字段），编辑器 ⚙ 面板可调、智能体改
一行文本即可、三宿主（编辑器视口 / 独立游戏 / 无头截图）同一条链。
**默认关**（`enabled = false`）：关着时只写 80 字节 uniform（开关位 0），
不建内容、不采样三张环境纹理 —— 旧项目画面与上线前**逐位一致**；SampleScene 里
开启作为演示（那里有一个 metallic 1.0 / roughness 0.35 的金属件，正好展示镜面
环境反射）。全量验证绿：`cargo check` / clippy **0 警告**、
`cargo test --workspace --all-targets` **434 条全绿**（4 条基准默认 ignore；
较上轮 +15 条：天空 CPU 镜像 8 + GPU 像素 5 + core 配置 2）。

### 一、天空是一个解析函数：一个真相源喂三个消费者

```text
                      ┌─ 背景：天空盒 pass（全屏三角形，场景 pass 内、不透明之后）
sky_radiance(dir) ────┼─ 漫反射：CPU 数值积分 L2 球谐（存 E/π）→ binding 8
                      └─ 镜面：GPU GGX 重要性采样 → 预过滤立方体（5 级 mip）→ binding 9
```

- **不做 Preetham / Hosek 那类物理大气**：前者回答"长什么样"，后者回答"为什么
  长这样"。参数化渐变 + 太阳盘已经能交出可信的画面，而出口结构（"方向 → 辐射
  亮度"的一个函数）与物理模型**完全同构** —— 以后想换模型，下游（天空盒 /
  IBL / 测试）一行都不用改。
- **渐变在 `dir.y == 0` 处连续**：天空段与地面段在视线水平方向**都**取地平线色
  （`mix(horizon, zenith, y^0.5)` / `mix(horizon, ground, (-y)^0.5)`），没有
  "缝合线"这种需要专门修的伪影（`sky_gradient_is_continuous_at_the_horizon`
  从上下两侧夹逼地平线，断言各自与地平线色差 < 2e-3）。
- **太阳盘的面辐射亮度写死 40**（常数名 `SKY_SUN_DISK_RADIANCE`）：太阳在现实里
  远超"白面 1.0"，40 让它在 ACES 后是一小块近白、同时给 Bloom 留下可抓的过曝。
  盘面半径（度）来自 `sun_size`，光晕 `0.35 × cos^16` 是宽而软的近似二次散射。
- **太阳的位置与颜色来自场景**：`packed`（渲染管线实际用的 ≤8 盏灯）里**第一盏
  active 的方向光** —— 方向取传播方向的反向，颜色取灯色。没有方向光就没有太阳
  （`sun_intensity` 写 0，盘面与光晕整段不出现）。"背景里有太阳、物体上却没有
  对应光"这类错位从数据层就不存在：两边读的是同一盏灯。
- 亮部（相机朝上）与暗部（朝下）在 `t = 0` 处共享地平线色，因此"把相机抬起来
  看"只会让地平线色占据更多画面 —— 这正是无头默认机位（俯角 26°）下截图背景
  偏地平线色的原因，不是天空没生效。

### 二、漫反射 IBL：L2 球谐，存 E/π（与 ambient 同标定）

- 漫反射环境是**低频**的（余弦卷积本身就是个低通），L2 球谐（9 个系数）足够；
  预过滤立方体的"漫反射 mip"方案要多建一张纹理，这里不需要。
- **CPU 数值积分**（`cpu::sh_project`，1024 个**斐波那契球**采样点、等权、
  零随机数）：确定性 = 同参数逐位同结果，热重载 / 双跑截图才可能逐字节一致。
  代价摊在重建时刻（实测见第八节），不进每帧。
- **余弦卷积折进系数**：`sh_project` 乘 `[1, 2/3, 1/4]`（L0/L1/L2 的卷积因子），
  存的是 **E/π** —— 着色器端只做"求值"。这让它与常量 `ambient` **同一标定**：
  均匀天空下 `albedo × sh_eval` 逐值等于 `albedo × ambient`，两套环境光因此能
  直接相加（`sh_projection_of_a_constant_sky_reproduces_the_constant` 用常量
  天空断言这一点）；加进场景的 IBL 项走 `kd_env`（`(1-F_rough)(1-metallic)`）
  而不是旧环境项的 `(1-metallic)`，金属不拿漫反射这条物理约束顺带落实。
- 9 个 coeff × vec4 = 3 个 std140 vec4 槽位（`sh: array<vec4<f32>, 9>`）。
- `sh_eval` 的基函数在 WGSL 与 Rust 两侧**逐项同表**（`[Y00, Y1-1(y), Y10(z),
  Y11(x), …]`，系数 0.282095 / 0.488603 / 1.092548 / 0.315392 / 0.546274）。

### 三、镜面 IBL：GGX 预过滤立方体 + BRDF LUT（split-sum 两半）

- **预过滤 128² 基 + 5 级 mip（8² 封顶，共 30 趟小 pass）**：每个 mip 级按
  roughness 做 GGX 重要性采样（Hammersley 序列，**无随机数** ⇒ 逐位确定、
  不会逐帧闪烁），`V = N = R` 是 split-sum 的标准近似（把"看向光源的积分"
  从 BRDF 里解耦出来）。采样数 96 / 48 / 24 / 16 / 12（分辨率低采样跟着降；
  基级给足是为了粗糙度 0 时太阳盘在反射里清晰而不抖）。
- **粗糙度连续靠 mip 线性插值**：场景 FS 采样
  `textureSampleLevel(env_cube, env_smp, R, roughness × ENV_MAX_LOD)`，采样器
  `mipmap_filter: Linear`（这一项是硬的）。5 级是"128² → 8²"的折中：更少的级
  会让粗糙材质在级间跳变。
- **BRDF LUT 128² / Rgba16Float / 256 样本**（Karis 积分，`A / B` 二分表），
  坐标语义 `(N·V, roughness)`；**惰性**：首次真正开启天空时才生成（关着天空的
  项目永远不付这笔钱），生成后常驻。
- **面基向量由 CPU 给**（`cpu::cube_face_basis`）：`dir = forward + right·(2u-1)
  + up·(1-2v)`（v = 0 是面的顶行），面序 = WebGPU 立方体贴图约定
  （+X, -X, +Y, -Y, +Z, -Z）。着色器不现算 `cross` —— ±Y 面那两处"换 up 轴"的
  特判就不存在了，镜像测试与 GPU 走**同一张表**。这条错位的表现是"反射里北方
  的太阳跑到南方"，GPU 测试用"粗糙金属上的太阳反光"专门钉住（先证红见第七节）。
- **预过滤 / LUT 各写各的绑定**：预过滤管线自己的布局（0 = 天空 uniform、
  1 = 动态偏移的 `PrefilterParams`，30 槽一次写全）；LUT 无绑定（采样数是编译期
  常量）。两张表共用同一个 `env_sampler` 与 `env_view` 对象身份 —— 内容重建时
  **不换绑定组对象**（见第四节）。

### 四、生命周期：固定分配 + 按 key 重建（为什么不做每帧重建）

- 环境立方体与 LUT **一次性分配**（合计 < 1 MB），**内容**按
  `uniform_key`（天空 uniform 的 80 字节 FNV-1a 位模式哈希，太阳方向 / 颜色 /
  太阳尺寸 / 三个颜色 / 开关全在内）重建：key 不变 ⇒ 每帧只有一次 80 字节
  `write_buffer`；key 变了（编辑器调参、灯光转向、昼夜循环）才重跑
  brdf（仅首次）+ 球谐投影 + 30 趟预过滤。
- **位模式哈希而不是浮点比较**：`-0.0` / `0.0` 会判成两个 key，代价只是一次
  多余的重建，比"改了参数却没重建"安全得多（`uniform_key_tracks_every_field_…`
  对每个字段各断言一次"改了必换 key"）。
- **绑定组对象身份必须稳定**：全局绑定组是每帧路径（每个 draw 一次
  `set_bind_group`），内容重建若换对象就得每帧重建绑定组。固定分配时视图与
  纹理句柄不变，内容换代对绑定组完全透明 —— 这也是"不做每帧重建"的真正原因：
  重建本身不贵，贵的是它会逼着每帧路径换对象。
- **关着天空时什么都不重建**（只写 uniform）：`built_key` 也**不清空** ——
  关了再开、参数没变就免一次重建。"默认关不改变旧画面"与"开关不浪费"两条
  一起成立。

### 五、集成：绑定号契约、三处各自声明、画在哪里

- **`SKY_COMMON_WGSL`（`sky_radiance` / `sky_sh_eval` + 两个结构体）刻意不声明
  任何绑定**：天空盒 FS（全局组 0/7）、场景 FS（0/8–11）、预过滤 FS（自身布局
  0/1）三处绑定号各不相同，公共片段只带类型与函数（uniform 按值传参）。这样
  "公式只有一份"不靠人肉同步 —— 三处各自 `format!` 拼装，
  `SKY_FRAG_PRELUDE` / `ENV_PRELUDE` 声明各自的绑定与输入结构。
  全局组的 7–11 号契约集中在 `sky.rs::global_layout_entries()`。
- **`CameraUniform` 新增 `inv_view_proj`**（32 字节）：天空盒片元由 NDC 反推
  世界方向（`inv_view_proj · (ndc, 1, 1)` 减去相机位置再归一化 —— 远平面上的
  点与视线同向，透视除法不进这个式子）。场景 FS 不读它，代价是一行声明。
- **天空盒画在场景 pass 内、不透明之后、半透明之前**：全屏三角形（顶点
  `(0,0)(2,0)(0,2)` → NDC 覆盖全屏），片元位置 z = 1.0 + `LessEqual` +
  **不写深度** ⇒ 被几何盖住的像素在这里失败、只有空像素作画；半透明随后混在
  天空之上（半透明物体背后是天，不是清屏色）。
- **`prepare` 必须排在 `begin_render_pass` 之前**（同一 encoder 里 pass 不能
  嵌套）：预过滤 / LUT 各自开自己的 pass；`write_buffer` 与那些 pass 同属一次
  提交，wgpu 按队列顺序保证 pass 读到新 uniform。
- **`ScenePipeline::render` 新增 `sky: SkyConfig` 参数**：编辑器视口、独立游戏、
  无头截图、GPU 基准四个调用点全部接线；`scene_signature` 哈希 sky 全字段
  （漏了 = "调天空视口纹丝不动"的指纹门控 bug）。
- **场景 FS 的 IBL 项与 ambient 相加**，且整段由 `sky.sun_dir.w > 0.5` 开关位
  控制 —— 开关位是 uniform 里的常量，整个 draw 走同一分支，`textureSampleLevel`
  在一致性控制流里才合法（也才便宜）。

### 六、入口三件套（与曝光 / 泛光同一条"单入口"纪律）

- `project.ron` 的 `render.sky`：七字段 + `serde(default, skip_serializing_if)`，
  老清单照常加载（默认关）；`sanitized()` 容错 —— NaN / 无穷整组退回默认，
  有限但离谱的量级夹进安全范围（颜色 [0, 50]、强度 [0, 10]、太阳尺寸
  [0.1°, 20°]）。
- 编辑器 ⚙ Project Settings 新增「天空（程序化天空盒 + 环境光照 IBL）」区：
  启用复选框 + 强度 / 三色拾取 / 太阳盘大小 / 太阳强度，全部带范围与悬停说明
  （范围取自 `SkyConfig::MIN_* / MAX_*` 常量）。
- MCP `project_info` 与无头报告 `render.sky` 只读投影（报告契约测试
  `json_contract_has_stable_keys` 同步断言字段与三通道数组；数值同 Bloom ——
  f32 存储、JSON 里是精确加宽的 f64）。

### 七、测试与先证红

- **先证红（两条实测）**：① 只把 GPU 侧渐变指数 `0.5` 改成 `0.4`
  （CPU 镜像不动）→ `sky_background_matches_cpu_mirror` 红（(160,12) 处
  GPU 174 vs CPU 181，超 ±2 容差）—— 证明"对 CPU 镜像逐值比较"这条测试真的
  敏感；② 把 `cpu::cube_face_basis` 的 -Z 面 up 轴翻成 -Y →
  `metal_reflects_the_sky_sun_at_the_mirrored_screen_position` 红（反射里的
  太阳跑到了反侧）—— 证明面约定测试盯的就是这根轴。两条均已回退。
- **CPU 镜像 8 条**（`sky.rs::tests`）：渐变在地平线连续、太阳盘局部性 +
  无方向光不出现、常量天空的 SH 投影复现常量、SH 漫反射跟随渐变方向、
  面基向量与 WebGPU 约定逐项对齐、key 对每个字段敏感 + 投影确定性、
  太阳 = 第一盏 active 方向光（跳过点光与熄灭灯）、开关位进 uniform。
- **GPU 像素 5 条**（`screenshot.rs`）：背景 5 点对 CPU 镜像（±2）；关天空时
  背景 = 旧清屏色 (0.12/0.12/0.14)（"默认关逐位一致"的钉子）；无光绝缘体
  开天空被球谐照亮（关时 luma < 2、开时 > 20 —— 只建管线不写数据时两边都黑，
  方向性断言必红）；光滑金属平面（metallic 1 / roughness 0.05、全黑天空色）上
  的太阳盘反光出现在**几何镜像点**（饱和像素质心 vs 期望屏幕位置 < 15 px，
  太阳盘 8°；那条 1e-30 强度的方向光只为提供太阳方向 / 颜色）；开天空两跑
  PNG 逐字节一致（无随机数重构）。
- **core 2 条**：`sky_defaults_to_off`（默认关 + 默认不落盘）、
  `sky_config_sanitized_rejects_garbage`（NaN 退回 / 越界夹取 / 合法值原样保留）。
- **编辑器 / 报告契约**：`scene_signature` 纳入 sky 全字段（`assert_ne` 组）；
  无头报告与 MCP `project_info` 的 `render.sky` 进稳定契约。

### 八、性能实测（GPU 时间戳口径）

`postfx_cost.rs` 新增第二条入口 **`sky_ibl_cost`**（同款口径：1280×720、MSAA ×4、
地面 + 64 立方体 + 阴影，三档交替换轮 ×8 轮 ×64 帧取中位数）：

| 段 | 关（基线） | 稳态开（key 不变） | 每帧重建（key 每帧变） |
|---|---|---|---|
| 场景 pass（含 MSAA resolve） | 136.7 µs | 166.2 µs | 507.9 µs |
| 后处理 pass | 24.6 µs | 24.7 µs | 24.8 µs |
| **合计（GPU）** | **161.3 µs** | **190.9 µs** | **532.7 µs** |
| CPU 编码 + 提交 | 168.1 µs | 168.8 µs | 617.8 µs |

- **稳态开相对关 +29.5 µs/帧**（场景段 +21.6%；合计 +18.3%）= 天空盒全屏 pass
  + 场景 FS 的 IBL 采样（环境立方体 + LUT 各一次）。折算 60 FPS 预算的 0.18%，
  "背景与照明同源"买到的是这个价。后处理段三档一致（24.6–24.8）—— 链没被
  天空触碰。
- **每帧重建相对稳态 +341.7 µs GPU / +449 µs CPU**（30 趟预过滤 + 球谐投影
  1024 点 + 30 槽 uniform）。这是"太阳每帧动"的最坏情况：昼夜循环每帧改参数
  就要付这笔账，而上限理由是**每帧路径**（重建本身不贵，CPU 编码 30 趟 pass
  才是大头）。日常形态（参数恒定）只付 uniform 的 80 字节。
- **开启天空那一帧**（BRDF LUT + 首次预过滤 + 球谐）：579.6 µs 场景段 =
  重建档 + 71.6 µs（LUT 256 样本 × 128² + 冷态）。一次性成本，编进 LUT 的
  惰性生成 —— 不在首帧支付就要在别处支付。
- 基准自带防"空调度"断言（P2.5 教训）：稳态场景段 > 关 + 1 µs（天空盒 / IBL
  真在跑）、重建场景段 > 稳态 + 10 µs（预过滤真在重跑）、重建 CPU > 稳态 +
  5 µs（球谐与 30 趟编码真在跑）、时间戳读数非零。key 一旦因回归不再变化，
  后两条必红。

### 九、实拍对照（SampleScene，无头截图）

同一场景开 / 关天空各拍一张（`--headless --frames 150 --screenshot`，机位自动
拟合 —— 两份报告的 `eye` / `target` 逐位相同：eye (-42.5946, 21.7024, -2.1760)、
target (0, 1.1, 0)，差异里没有第二个变量）：

- 1280×720 隔点取 230400 样本，**全部变亮、0 个变暗**：平均 +68.8/255 每通道，
  最大 +119；整幅均值 (110.7, 110.5, 119.5) → (177.0, 181.4, 188.7)。
- 观感：背景从深灰清屏色变成地平线附近的淡蓝 → 天顶方向渐深；金属件的面上
  出现天色反射；所有受光与背光面都抬了一档环境光（ambient + IBL 相加的直接
  读数）。背光面不再发死黑 —— 这正是"环境反射来源"这一片要解决的事。
- 默认机位俯角 26°，画面上沿只略高于地平线，因此天空在默认截图里主要展示
  地平线色（见第一节末）。

### 十、刻意的取舍（防好心人翻案）

1. **默认关**：见 `SkyConfig` 文档 —— 天空是风格参数（写实 / 卡通 / 夜景），
   与 Bloom 同理，"默认改变画面不可接受"。要开就改一行 `project.ron`。
2. **太阳 = 场景第一盏方向光**，不引入独立的"天空光源"：再开一个太阳参数
   必然与灯光漂移（改灯忘了改天空 = 阴影方向与太阳盘不一致）。方向光本来就
   是太阳语义。
3. **IBL 与常量 ambient 相加**，不把 ambient 顶掉：老场景调好的环境底色不能被
   悄悄改写；想只留 IBL 就把 ambient 清零（数学上两者同标定，见第二节）。
4. **固定分配 + 按 key 重建**：`built_key` 不清空、绑定组对象身份不变（第四节）。
   别改成"每帧重建"或"重建时换绑定组"。
5. **不做 compute shader 预过滤**：30 趟 render pass + 全屏三角形在 128² 尺度上
   已经够便宜（实测 +341.7 µs 是在**每帧**重建的最坏情况下；日常只在调参时付），
   引 compute 要动管线布局与队列语义，收益不成比例。等有更大环境贴图 / 运行时
   重烘焙的需求再评估。
6. **不做 Preetham/Hosek、不做脚本 `set_sky`**：天空是画面设置不是玩法参数；
   昼夜循环等真出现"逐帧改天空"的需求时再开 API（那时第一节的出口结构原样
   可用，代价已在第八节量过）。

### 十一、已知限制（写进差距分析）

- **单一全球环境**：没有反射探针 / 局部环境（室内空间看不到室内色，金属反射
  的是室外天空）。
- **天空不投阴影**：太阳盘只是背景辐射；直接光与阴影仍来自场景里的方向光。
- **脚本侧没有 `set_sky` / `set_material`**：入口 = Inspector / 场景 RON /
  `project.ron` / MCP `scene_write`。
- 无折射 / 透射（材质系统本身没有 transmission），IBL 的镜面项是单次反射。

## 二十八、渲染实例化（draw batching）（本轮）

差距分析 P1 表里的 **实例化** 兑现：渲染收集阶段把"同图元"的绘制项并成一批，
每批一次 `draw_indexed(0..n_indices, 0, first..first+count)` —— 逐实例数据
（模型矩阵 + 蒙皮槽位 + 有效材质）走 **storage buffer**，顶点着色器用
`@builtin(instance_index)` 直接索引，**动态偏移 uniform 整个退役**。
流程按"先量后改"走：先把帧绘制统计接口 + 基准作为准备提交（`81ff6e9`）落盘，
旧实现（2000 实体同网格场景 **2001 批 / CPU 2225 µs/帧**）的数字进案；落成后
同机交替复测 **2 批 / 344 µs/帧（-84%）**；画面端到端**逐字节不变**。
全量验证绿：`cargo check` / clippy **0 警告**、`cargo test --workspace
--all-targets` **439 条全绿**（5 条基准默认 ignore；较上轮 +5 = 实例化 GPU 测试组）。

### 一、批次 = 一次实例化 draw：分组键与顺序都是"确定性"的一部分

```text
收集循环（逐实体 → 逐图元）：
  不透明 → batch_index[图元地址] 查/建批次；实例数据攒进 pending_draws
  半透明 → blended_draws（各自一批，带距离键）
           ↓
  blended_draws 按距离远→近排序
  槽位前缀和：批次 i 占 [first, first+count)，半透明接在后面
  一次 queue.write_buffer 写完整段实例数据
           ↓
主 pass：全局组绑一次 → 逐批 [材质组 + 顶点/索引缓冲] → draw_indexed(..., first..first+count)
阴影 pass：同一份批次表逐批重放（不重复收集、不重复计 draw call）
```

- **分组键 = 图元地址**（`prim as *const GltfPrimitive as usize`，只做相等
  判定）：同一份 `GltfMesh` 的图元指针相同 —— 共享网格的实体自然并批，
  各自一份网格的实体自然不合批。指针相等判定是 O(1) 且零分配；换成
  "网格路径字符串哈希"每帧 2000 次是纯浪费，换成"材质进键"则会因为逐实体
  材质覆盖（常态）把合批拆碎 —— 材质本来就要逐实例走（见第三节）。
- **批次顺序 = 首次遇到顺序**（`Vec` 追加，不是 `HashMap` 迭代序）：槽位布局
  因此逐帧确定、跨进程确定 —— 无头截图"同机逐字节确定"契约与基准读数的
  可复现性都依赖这条。别改成哈希序 / 按材质排序。
- **半透明不参与合批**（排序语义优先，见第四节）；**每图元各自成批**
  （不跨图元合并 —— 每个图元通常各有材质，跨并反而要拆材质）。
- 批次表是跨帧复用的字段：`opaque_batches` / `pending_draws` / `batch_index`
  全部 `clear()` 复用容量（稳态零分配，与收集缓冲同一纪律）。

### 二、逐实例数据：storage buffer 取代"逐 draw 动态偏移 uniform"

- 旧方案：每绘制项一段对齐到 256 B 的 uniform，逐项 `queue.write_buffer` +
  逐项 `set_bind_group(0, &bg, &[offset])`。2000 实体 = 2000 次小写 + 2000 次
  带动态偏移的绑定 —— 这就是"每帧 CPU 税"的大头（实测在案）。
- 新方案：`DrawUniform`（**112 B** = 模型矩阵 64 + 蒙皮槽位 16 + 材质
  base_color/material 32）在 `draw_buffer`（`STORAGE | COPY_DST`）里**紧密
  排布**，一次 `write_buffer` 写完整段；全局绑定组与阴影绑定组对它的
  绑定都是**整缓冲 + `has_dynamic_offset: false`**。
- **112 B 必须等于 WGSL 数组步长**：storage 数组元素步长 = align16 后的
  结构大小，112 = 7×16 恰好无填充 —— 改组内字段 = 改 ABI，CPU / WGSL 两侧
  一起改（`size_of` 已有单测钉死）。
- 槽位容量上限仍然是 `MAX_RENDER_DRAWS`（超限静默丢弃的旧语义原样保留）。

### 三、着色器：`instance_index` 索引 + flat 插值传材质

- 顶点着色器 `let draw = draws[instance_index];` —— WebGPU 的
  `@builtin(instance_index)` 从 `first_instance` 起算，与槽位前缀和分配的
  "批起始槽位"正好对齐；`draw_indexed` 的实例范围与数据段一一对应。
- **材质随实例数据到片元，靠 `@interpolate(flat)`**（`@location(4/5)`）：
  片元阶段拿不到 `instance_index`，直接读 storage 会在片元阶段引入 storage
  绑定类别（且每个片元重复索引）；flat 插值取 provoking vertex 的值，同一
  实例所有顶点同值，"取谁都一样"。片元阶段保持"只吃顶点输出"的旧接口。
- 半透明与不透明走**同一套**顶点路径（同一 storage 布局），差别只在管线
  状态（混合 + 不写深度）与 draw 粒度。

### 四、半透明与阴影

- **半透明逐项一次 draw call、按到相机的距离远→近**：alpha 混合顺序敏感，
  排序是语义的一部分（两条同轴半透明立方体的钉子测试在第六节）。项数与
  批次数无关（不合批）。
- **阴影 pass 复用同一批批次表**（同一份实例数据、逐批同样一次 draw）：
  零额外收集、零额外准备；统计口径（`FrameStats`）只记主 pass 的 draw call，
  阴影不重复计。只画不透明的旧语义不变（半透明不投实心影）。
- 收集阶段对每个新批次 / 半透明项**预热材质绑定组**（`material_binding`
  要 `&mut self`，绘制循环里只有 `&self`）—— 绘制循环用
  `material_cache.get(..).expect("材质绑定组缺失")`：这是故意的硬失败，
  静默拿着上一批的材质画会得到"看起来能跑"的错色画面。
- 顺带修一处遗留接线：`sky.rs` 的 `set_bind_group(0, global, &[0])` 改
  `&[]` —— 全局组已无任何动态偏移，多传偏移 wgpu 会直接校验报错。

### 五、统计接口与"先量后改"的流程

- `FrameStats { opaque_batches, opaque_instances, blended_draws }` +
  `draw_calls()`（= 批次数 + 半透明项数）先于实现落盘（`81ff6e9`）：
  合批是否生效、画面是否正确，两者互不覆盖 —— 像素测试守"画得对"，
  计数守"是不是一批画完的"。`ScreenshotRenderer::frame_stats()` 是测试探针
  （与 `ScenePipeline::frame_stats` 直连，无缓存）。
- 微基准 `tests/instancing_cost.rs`（手动、`#[ignore]`）：两个**几何完全
  相同**的场景 —— 共享网格（`builtin://cube` ×2000）与各自网格（同几何、
  各自一份 `GltfMesh`）。后者不是假想最坏情况，而是**回归护栏**：合批逻辑
  一旦把不同顶点缓冲错并成一批，这里与画面测试会同时红。读数口径与
  `postfx_cost` 一致：GPU 时间戳取场景段 + CPU 段单记"编码 + 提交"，
  两档交替 ×8 轮取中位数。

### 六、测试与先证红

- **旧实现下红（3 条，实测）**：把 5 条新测试原样移植进旧实现 worktree
  （`81ff6e9`，仅补一个 `frame_stats()` 访问器）→ ①
  `instancing_batches_shared_mesh_entities` 红（65 批 vs 期望 2）；
  ② `instancing_matches_per_draw_rendering` 红（共享 65 批不小于各自的 65）；
  ③ `batching_keeps_per_instance_material_overrides` 红（2 批 vs 期望 1）。
- **突变证红（2 条契约钉，新实现里人为破坏再回退）**：④ 把 `shadow_enabled`
  写死 false → `instanced_draws_still_cast_shadows` 红（变暗像素 0 = 影子
  没了）；⑤ 把半透明排序翻成近→远 → `blended_draws_render_far_to_near` 红
  （中心像素 r > b）。两条均已回退、复跑绿。
- **GPU 测试组 5 条**（`screenshot.rs`）：批计数（64 立方体 + 地面 = 2 批 /
  65 实例 / `draw_calls() == 2`）；**逐 draw 等价**（共用网格 vs 4 份注入的
  同几何网格，两图**逐字节一致**，且共享档批次更少 —— 开着阴影跑，阴影
  pass 的批复用一起覆盖）；**同批各调各的色**（红/绿实体级覆盖物在同批,
  两个采样点颜色各异 —— 防"材质被提到批次级"）；**合批投阴影**（开/关
  阴影两图比对：变暗像素 ≥100、无像素变亮；统计口径与主 pass 一致）；
  **半透明远→近**（近蓝远红，中心 b > r）。
- **基准自带反退化断言**（P2.5 纪律，印完摘要再断言）：共享档
  `batches × 100 ≤ instances`（退化成逐 draw 必红）、各自档
  `batches ≥ ACTORS`（错并批必红）、两档实例数相等、`s_cpu × 2 < u_cpu`
  （合批没省下 CPU 必红）。

### 七、性能实测（同机交替 A/B，3 轮取中位数）

方法：`81ff6e9` 在独立 worktree 构建旧二进制，与工作区新二进制**交替**
（旧/新/旧/新/旧/新）跑同一基准，抵消机器状态漂移。口径：RTX 2060 /
Vulkan / 1280×720 / MSAA ×4 / 2000 实体 ×（共享网格、各自网格）两场景 ×
8 轮 ×64 帧取中位数；CPU 段 = "编码 + 提交"（实例化真正要砍的税）。

| 指标 | 旧（2001 批） | 新（2 批） | 变化 |
|---|---|---|---|
| 共享网格：批次数 | 2001 | **2** | −99.9% |
| 共享网格：CPU 编码 + 提交 | 2225 µs/帧 | **344 µs/帧** | **−84%** |
| 共享网格：场景段（GPU） | 287 µs/帧 | 241 µs/帧 | −16% |
| 各自网格护栏：批次数 | 2001 | 2001 | 持平（护栏语义） |
| 各自网格护栏：CPU | 4139 µs/帧 | 3287 µs/帧 | −21% |
| 各自网格护栏：场景段 | 292 µs/帧 | 267 µs/帧 | −9% |

- 三个数字逐轮一致（旧 CPU 三次 2225.0 / 2206.5 / 2234.1，新 CPU 340.8–344.3，
  交替六轮无交叉）—— 不是机器状态的巧合。两档的轮内读数离散度也小（旧
  2202–2292、新 334–368 单轮制）。
- **各自网格对照档也快了 21%**：那不是合批的功劳（它一批都没合成），而是
  砍掉了每 draw 的 2000 次动态偏移 `set_bind_group(0)` 与 2000 次
  `write_buffer`（逐项 uniform 上传）。新方案每帧只有一次整段实例数据上传。
- 后处理段时间戳两版间有稳定的 ~3.5 µs 差（旧 28 µs 档 / 新 24.7 µs 档，
  后处理代码未动、机制不明），**不计入任何收益口径** —— 上面只引用场景段
  与 CPU 段。
- 半透明项两档均为 0（基准场景无半透明）；阴影开关不进 CPU 段（同一份批次
  表重放，不额外编码命令）。

### 八、端到端（SampleScene 无头截图逐字节一致）

两版编辑器的 `--headless --frames 150 --screenshot` 同一场景（SampleScene，
含共享网格实体 8×cube + 3×Box、Lua 脚本、方向光阴影、天空 IBL、ACES）：

- **两张 PNG 逐字节一致**（SHA256 同为 `314E310E…`）—— 实例化没有改动一个
  像素；连"模拟 150 帧后的动画姿态与脚本位移"都逐位相同。
- 两份 JSON 报告逐行 diff：**仅 18 行不同，全部是耗时字段**
  （`max/mean/p50/p95/p99/run_ms/setup_ms`）与环境路径（worktree 根、截图
  文件名）—— 实体计数 / 刚体计数 / 脚本日志（含 error 样本）/ 非有限变换
  全部逐字符一致。

### 九、刻意的取舍（防好心人翻案）

1. **分组键 = 图元地址、批次顺序 = 首次遇到**：见第一节 —— 槽位布局的
   确定性是截图契约与基准可复现性的地基；别改成哈希序 / 按材质排序 / 排序
   批次表。
2. **逐实例数据只走 storage buffer**：别再引入"逐 draw 动态偏移 uniform"
   的写法 —— 本轮实测它是 2000 实体场景 ~1.9 ms/帧 的纯 CPU 税；且
   storage 数组让"实例槽位"与"数据段"一一对应，`first_instance` 直接就是
   数据下标，没有第二套偏移书签。
3. **半透明不参与合批**：排序是 alpha 混合语义的一部分，逐项一次 draw 是
   正确性优先的选择；将来半透明规模大到 CPU 吃紧再谈（当前无此需求）。
4. **阴影 pass 复用批次表、不为阴影单独收集**：换了收集路径就有了"主 pass
   与阴影 pass 实例数据错位"的可能；复用 = 不可能错位。
5. **材质 flat 插值而不是片元读 storage**：片元阶段保持"只吃顶点输出"，
   不引入 storage 绑定类别；同实例所有顶点取同值，flat 是语义最贴的插值。
6. **统计接口直读字段、不做每帧记账**：`frame_stats()` 只是拷贝 3 个
   `usize`，不进热路径；它是基准与测试的公共读数口（别让消费方从耗时反推）。
7. **每图元各自成批（不跨图元合并）**：图元通常各有材质，"跨图元合并"会
   立刻需要按材质二次拆分，得不偿失。

### 十、已知限制（写进差距分析）

- **LOD / 视锥剔除仍缺失**（差距分析"实例化 / LOD / 剔除"行改为部分完成）。
- 合批粒度是"图元指针"，**跨网格资产不合并**（同一几何两份 glTF = 两批）——
  与 Unity 的资产级实例化对照时注意口径。
- **半透明逐项 draw call**：数量大时是新的 CPU 税源（本轮场景未触及）。
- 无 GPU-driven / indirect draw、无材质排序（同材质跨网格不合批）——
  当前瓶颈数字（见 ROADMAP"每帧开销"行）不需要它们。

## 二十九、编辑器 UI：UE 风格重做（本轮）

### 一、要解决的是什么

用户的原话是「调整画面风格，有些元素大小不合适，有些地方排版不好看，有些地方
不整齐」。三条抱怨里只有第一条是审美，后两条是**几何问题**：行高在 20–26px 之间
飘、标签列宽随文字长短变化、值列左右各错开一档、emoji 字形宽度不一致导致图标
永远对不齐。所以这一轮的主线不是"换个配色"，而是**把尺寸与对齐收进一套常量**。

判据（可截图验证，不靠感觉）：
1. 同一面板里所有输入框的左边缘落在同一条竖线上；
2. 所有行高、控件高度、面板内边距只来自 `theme` 常量；
3. 界面上不再出现 emoji / 字形图标（矢量绘制，尺寸与基线可控）；
4. 改完不引入任何帧调度与渲染路径上的变化（纯 UI 层）。

### 二、尺寸体系（`panels/theme.rs`）

| 常量 | 值 | 用在哪 |
|---|---|---|
| `TOOLBAR_HEIGHT` | 34 | 顶部工具栏 |
| `TAB_BAR_HEIGHT` | 26 | 面板页签条 |
| `CATEGORY_HEIGHT` | 22 | Inspector 分类条 |
| `SECTION_GAP` | 6 | 分类条与上一组内容之间 |
| `ROW_HEIGHT` | 22 | Hierarchy / Prefabs / Console 行 |
| `FIELD_HEIGHT` | 20 | DragValue / TextEdit / ComboBox |
| `STATUS_BAR_HEIGHT` | 24 | 底部状态栏 |
| `PANEL_MARGIN` | 6 | 每个停靠面板的内边距 |
| `LABEL_WIDTH` | 58 | Inspector 标签列 |
| `AXIS_TAG_WIDTH` | 9 | X/Y/Z 轴标占位 |
| `FIELD_WIDTH` | 54 | 数值框标准宽度 |

行高统一之后"紧凑但不挤"是自然结果 —— UE 的信息密度来自**间距的规律性**，
不是把字号压小。

### 三、可复用件

- `theme::section_bar`：22px 分类条 + 左 2px 强调条 + 下沿 1px 线 + 6px 上留白。
  分组靠留白切开，不靠更粗的线。
- `theme::field_row(ui, label, f)`：定宽标签列 + 值区，**返回闭包的返回值**
  （向量行要拿 `changed`）。标签 `truncate()`，长参数名不会顶出控件列。
- `theme::note(ui, text)`：分类内容里的说明文字，**自己排一次版**再交给 Label。
- `theme::drag_field` / `axis_label` / `axis_color`：数值框与轴色。
- `theme::card_frame` / `card_title` / `bold` / `bold_family`：枢纽卡片与粗体族。
- `icons::*`：`paint(painter, rect, Icon, color)` 全几何绘制（18 个图标）、
  `icon_button` / `icon_text_button` / `text_button` / `add_row` / `chip` /
  `close_button` / `small_icon` / `v_separator`。

### 四、对齐的四个陷阱（这轮踩到的，全是"看不出原因"的那种）

1. **`allocate_ui_with_layout` 只按子 ui 的 `min_rect` 占位**，不按你给的
   `desired_size`。于是"58px 标签列"实际只有标签文字那么宽：`Type` 行与
   `Radius` 行的输入框差出 12px，一列控件左边缘参差。修法是在子 ui 里
   `ui.set_min_width(w)` 把占位拉回目标宽度。`field_row` 与枢纽的
   "名字列 / 主列"都吃这一条。
2. **`ComboBox::width(w)` 是内宽，控件实宽 ≥ w**（自带箭头与内边距）。所以
   拿"请求宽度"去排一张列头，偏移会逐列累积：`Weight` 表头最终偏出 10px。
   结论：**不排这张表头**，改成每个控件一条悬停说明。
3. **`FontFamily::Name` 指向未注册的族 = epaint 直接 panic**。`install_fonts`
   里粗体族改成**无条件注册**（找不到粗体文件就用 proportional 链兜底）；
   裸 `egui::Context`（测试）里画的东西只能用 `Proportional`。
4. **`allocate_ui_with_layout(vec2(w, 0.0), ...)`** 会把子 ui 的 `max_rect`
   高度设成 0，内部的 `ScrollArea` 按"可用高度 0"夹视口 —— 表现是列表只画出
   前三行。要给真实可用高度。

### 五、逐面板

- **工具栏**：34px 扁平分组（项目 / 保存 / 播放 / 插件 / 变换工具 / 显示 / 右
  侧 Build + 设置 + 场景名），分隔线用 1px 竖线，激活工具是蓝底。
- **页签**：自绘 `tab_ui`（选中 = 面板底色 + 顶部 2px 强调线 + 粗体），
  页签名与枚举名解耦（`SceneView → Viewport`、`Console → Output Log`）。
- **Hierarchy**：22px 行、缩进 12、选中行直角填充、矢量箭头、行尾 P/S 徽标
  （**选中行改半透明黑底 + 提亮字色**，否则徽标糊进蓝底）。
- **Inspector**：整条滚动；分类内容**零缩进**（`spacing.indent = 0`，代码里
  的 `ui.indent` 只做分块）；标量行补 `AXIS_TAG_WIDTH` 占位，与 Transform 的
  三个轴框共用同一条值列；Physics / Animation / Audio 全部 `field_row` +
  ComboBox；层栈去掉列头改悬停说明。
- **Prefabs**：标题 + 计数 + 刷新图标；搜索框整宽；`Create from Selection` 是
  扁平行；资产行 22px + 紫色菱形 + 右侧子目录暗字。
- **Output Log**：矢量等级图标（Info 圆点 / Warning 三角 / Error 叉 / Success
  对勾）+ 截断 + 右对齐计数。
- **状态栏**：24px 深条 + 分组 + 右侧 fps/版本；**编辑态用小圆点而不是暂停
  图标**（暂停符号在"没在跑"的语境里会被读成"运行中已暂停"）。
- **视口 HUD**：药丸底（`from_black_alpha(96)`）+ 左上场景名 / 右上相机 /
  左下工具 + fps。
- **枢纽**：三张卡片（新建 / 打开 / 最近），固定标签列、蓝底主按钮、最近项目
  名字列定宽 112px。

### 六、自检工具链（这轮新加的）

```
xgame-editor.exe <project> --ui-shot <png> [--ui-frames N] [--ui-select <实体名>]
                            [--ui-settings] [--ui-play] [--ui-size 1280x800]
```
- 首帧 `ViewportCommand::InnerSize(<--ui-size 或 1440x900>)` 固定窗口尺寸；
- **连续两帧逻辑尺寸相同**才发 `ViewportCommand::Screenshot`；
- 像素尺寸 ≠ 逻辑尺寸 × ppp 时最多重抓 3 次（pidpi 缩放下的经典陷阱）；
- 落盘走 `xgame_render::screenshot::save_rgba_png`（与场景截图同一份实现）；
- `--ui-settings` 在抓图前把模态的项目设置窗口拉开 —— 没有它，那个窗口
  **永远截不到**（只能靠点 ⚙）。
- `--ui-play` 抓图前先进 Play。**必须自己把 `play_mode` 推上去**：平时这条
  跃迁由 `editor_ui` 监听到工具栏改了枚举之后补上，截图路径不经过工具栏，
  只调 `enter_play_mode()` 会得到"运行时在跑、界面写着 Edit Mode"的僵尸态。
- `--ui-size` 逐档扫窗口宽度做排版回归（1280 / 1440 / 1920 各跑一遍）。
  解析失败要**报错**不能静默退化，否则那一档截图其实还是 1440×900（有测试钉住）。

顺带修掉设置窗口的两个硬伤：滚动区没有高度上限（窗口按内容长到屏幕外，
标题栏和 Apply/Cancel 一起被裁掉）、文本字段只有 egui 默认宽度（右边缘空出
一大截）。现在滚动区上限 = 窗口高 − 190，字段宽度 = 网格剩余宽度。

配套：`.workbuddy/tools/crop.py`（局部放大）与 `rowscan.py`（按行扫描颜色分段，
用来**量**像素位置而不是靠眼睛判断"对齐了没有"）。

### 七、第二轮：窄窗口与运行态（同日续做）

第一轮只在 1440×900 / 扫描过 1280 一次，第二轮把三档都跑了一遍，修掉三类问题：

1. **Transform 三轴在 1280 下顶出面板**（Z 输入框只剩半个）。根因是
   `FIELD_WIDTH=54` 写死：标签列 58 + 3×(轴标 9 + 框 54) + 6 个间距 ≈ 276px，
   而 1280 时 Inspector 内容宽只有 ~244px。改为 `theme::axis_field_width(ui)`
   按剩余宽度均分（`clamp(MIN_AXIS_FIELD=30, FIELD_WIDTH)`），三行共用**同一个**
   算出的宽度（放完第一个控件再算会越算越窄，列就不齐了）。同时把
   Inspector 的 share 从 25% 提到 28% —— 1440 下正好稳住 54px（与旧布局逐像素
   一致），1280 下从 37px 提到 43px。留 4px 余量给滚动条与取整。
2. **Play 态的三个语义错误**（`--ui-play` 做出来才看见）：
   - Play 中标题/工具栏一直显示 `*`（未保存）—— 运行时的物理每帧都在改
     `self.scene`，但那不是用户的改动。`scene_dirty_memo` 在非 Edit 态直接
     返回 `false`。
   - Play 中 Save 按钮可用，按下去会把"跑了三秒之后的现场"写进场景文件。
     `save_scene` / `save_scene_as` 现在硬拒绝，工具栏按钮跟着禁用。
   - Pause / Stop 在编辑态也是亮的，点了什么都不发生。改用 `add_enabled_ui`
     按 `play_mode` 禁用。
3. **三个图标在 14px 下读不出来**：`Translate` 的箭头只有 2.1×1.6px（整个图标
   是个加号）、`Save` 画了三个框糊成"回"字、`Folder` 的页签 4×1.7px（看着是
   空心方块）。分别改成：箭头放大到臂长的 1/4、软盘只留"外框 + 上半实心"、
   页签占宽度一半且单独提亮 + 接缝压暗。

### 八、刻意的取舍

1. **不引第三方图标库**：18 个图标全用 `Painter` 画。字形图标的宽度/基线/粗细
   不受控，正是"不整齐"的来源；矢量图标永远落在给定方框里。
2. **不恢复列头**：见陷阱 2。表头对不齐比没有表头更显乱。
3. **Inspector 分类内容零缩进**：UE 的 Details 就是平的；缩进会让值列分层。
4. **粗体族无条件注册**：宁可"没有粗体效果"也不能"找不到族就 panic"。
5. **UI 层不动帧调度**：截图与量帧数据表明这一轮没有引入新的重绘源。
6. **Inspector 不设最小宽度**：egui_tiles 的 `Behavior::min_size` 是**全局一个
   值**（0.11 的签名就是 `fn min_size(&self) -> f32`），没法按面板给；所以窄窗口
   靠"三轴自适应 + 调 share"扛，而不是靠 dock 保证宽度。

### 九、已知限制

- 视口 gizmo 与选中高亮的配色未重做（属于 3D 叠加层，不在这轮范围）。
- 窗口小于 1280×800 的排版未验证（Dock 四列到这个尺寸已经很难看，UE 同尺寸
  也一样）。
- Console 的错误/警告等级图标只在高亮时验证过；空列表没有专门的空状态插画。
- 动画层栈在 1280 下四个控件（名字 / 模式 / 剪辑 / 权重）仍然偏挤，
  但都完整可见（列头已按陷阱 2 删除）。

## 三十、渲染剔除：视锥剔除 + 阴影补投 + 绘制读数（本轮）

差距分析 P1 表里的 **视锥剔除** 兑现 —— 实例化解决"同网格的绘制项合成几批"，
剔除解决"这些实体根本不用进批次"。落成后 2000 实体、30° 窄视场（**56.6% 在
视锥外**）的实测：

| 场景 | 批次 / 实例 | CPU（编码 + 提交） | 场景段 GPU |
|---|---|---|---|
| 共享网格（合批成 1 批） | 1 / 2000 → **1 / 868** | 187.6 → **181.7 µs/帧（-3%）** | 76.8 → **71.7 µs（-7%）** |
| 各自网格（2000 批） | 2000 / 2000 → **868 / 868** | 3295.3 → **1146.3 µs/帧（-65%）** | 158.0 → **108.5 µs（-31%）** |

同一基准紧接着复测的方向与比例一致（各自网格 CPU 2290.8 → 982.8 µs/帧，-57%；
共享网格 CPU -3%、场景段 -8%）；绝对值随机器状态浮动 1.4× 是常态，跨版本比较
必须同机背靠背。全量验证绿：`cargo check` / clippy **0 警告**、
`cargo test --workspace --all-targets` **456 条全绿**（5 条手动基准默认 ignore）。

### 一、判据：六个归一化平面 + 网格级世界 AABB

- `frustum.rs` 从 `view_proj` 按 **Gribb–Hartmann** 提六平面，法线朝视锥内侧、
  且**逐平面归一化** —— 点积因此就是世界单位的有符号距离，球体测试可以直接拿
  半径比，AABB 测试也不受平面尺度影响。
- **近平面取第 3 行**（wgpu 的 NDC 深度是 `[0, 1]`，`glam::perspective_rh` 的
  约定），不是 OpenGL 的 `row3 + row2`。搞反的表现是"相机贴脸时近处物体整片
  消失"，单测专门钉了相机原点附近的点。
- `Frustum::from_view_proj` 返回 `Option`：矩阵含 NaN / Inf、或任一平面法线长度
  为零时返回 `None`，调用方**当作不剔除**。相机矩阵坏掉时宁可白画一帧，也不能
  把整个场景判成不可见（那是黑屏）。
- 剔除粒度是**实体 × 网格级 AABB**（`mesh.bounds` 经世界矩阵变换），不是逐图元：
  同一网格内的图元共享一个物体，逐图元求交省下的实例写不值得再乘一次矩阵。
  每个实体只算一次包围盒，随后主 pass / 阴影 pass 共用。
- `Bounds::transformed_affine` 是 8 角点法的**解析等价式**（`|M| · 半长` 再乘
  2 加中心），每帧 2000 个实体时省掉 7 次点变换；`frustum.rs` 里有一条
  "与角点参考实现逐值一致"的交叉单测钉住（旋转 + 缩放 + 平移的矩阵）。
- AABB 测试只测每个平面**最靠内侧的角点**（p-vertex）：它在外侧则整盒在外侧，
  与"8 个角点全测"等价，省 7/8 的点积。

### 二、三种去向：主 pass / 仅阴影 / 整实体跳过

只按相机视锥剔会制造最容易漏的画面 bug —— **相机看不见、但影子落在画面里**
的物体跟着影子一起消失。收集阶段因此对每个实体算两个视锥：

```text
相机视锥内                    → 主 pass（阴影 pass 复用同一批）
相机外、阴影视锥内（不透明）  → shadow_batches（只进阴影 pass）
两边都外                      → 整实体跳过（不蒙皮 / 不建材质组 / 不写实例）
```

- 阴影视锥来自本帧的 `shadow_matrix`（与阴影贴图同一份正交拟合）；阴影关着时
  不保留任何"仅阴影"实体。阴影视锥矩阵退化时**保守地当作全都在内**
  （`is_none_or`），否则一次坏矩阵会让全场影子消失。
- **半透明不投实心影**的旧语义原样保留：仅阴影路径遇到 `material.blend` 直接
  跳过（它不写主深度，混进阴影图会投出一块错误的黑斑）。
- `shadow_batches` 是独立批次表，**不占主 pass 的实例槽位、不预热材质绑定组**
  （深度 pass 不采样材质）；按图元地址分组的规则与主 pass 相同。实例槽位布局
  是"主 pass 不透明 → 半透明 → 仅阴影"三段连续，仍只做一次
  `queue.write_buffer`；阴影 pass 依次遍历 `opaque_batches.chain(shadow_batches)`。
- 统计口径：`opaque_instances` **只算主 pass**；仅阴影实例单独记
  `shadow_only_instances`；`cull_ratio()` 的分母把两类都算进去（仅阴影的投影物
  不是"被剔除"，不能从分母里漏掉）。

### 三、蒙皮：绑定姿态包围盒 + 50% 余量

蒙皮发生在 GPU 顶点着色器里，CPU 侧的 `GltfMesh::bounds` 只是**绑定姿态**的
外壳；走路、挥拳、翻滚都会把肢体甩到盒外。给 `animator.is_some()` 的蒙皮网格
留 **1.5× 半长**（3× 体积）余量：

- 精确做法是逐帧用当前骨骼重算蒙皮包围盒 —— 要在 CPU 上再跑一遍蒙皮，收益
  抵不上成本（剔除省下的是实例写与光栅化，不是骨骼计算）。
- 余量只乘在**蒙皮网格**上：静态网格（含内置图元）走精确解析式，不平白多画
  3× 体积。`cull_bounds_pads_skinned_meshes_only` 与
  `cull_bounds_leaves_static_meshes_exact` 两条单测钉死这两半。
- 余量随包围盒尺寸缩放，所以"离原点很远的大角色"会被保得更宽 —— 这是保守
  方向的误差（多画），不是错判。

### 四、测试与先证红

**纯几何单测 10 条**（`frustum.rs`）：正前方 / 相机背后、近平面两侧、远平面、
跨近平面的盒子、侧面外沿与贴边盒子、球体半径、正交阴影视锥罩住整盒、退化矩阵
（全零 / NaN）不剔除、解析 AABB 与角点法一致。

**GPU 测试 3 条**（`screenshot.rs`）：

1. `frustum_culling_skips_offscreen_instances`：16 可见 + 16 相机背后的交错
   立方体 → 关剔除 32 实例 / 0 剔除，开剔除 16 / 16、`cull_ratio == 0.5`；
   两档批次数相同（剔除只少实例，不重排批次）。
2. `frustum_culling_matches_unculled_pixels`：开 / 关剔除两帧**逐字节一致**
   —— 保守性的硬合同。
3. `offscreen_casters_still_cast_shadows`：相机外的投影物把影子投在可见地面上
   （红灯时"有阴影 / 无阴影"两档都是 236.0；修复后 `lit + 20 < unlit`），
   `shadow_only_instances == 1`、`culled_entities == 0`。

**旧实现 / 破坏性突变下红（3 条，均实测后回退复绿）**：

- 把 `camera_frustum` 强制成 `None`（等价于旧的无剔除实现）→ ① 红：
  `assertion left == right failed: 只剩可见的 16 个实例, left: 32, right: 16`。
- 把阴影 pass 的 `opaque_batches.chain(shadow_batches)` 退回只遍历
  `opaque_batches` → ③ 红：`屏幕外投影物的影子必须落在地面上：有阴影 236.0
  vs 无阴影 236.0`（影子完全消失）。
- 把 `SKINNED_BOUNDS_PADDING` 改成 1.0（旧的无余量行为）→ 蒙皮余量单测红：
  `left: Vec3(1.0, 1.0, 1.0), right: Vec3(1.5, 1.5, 1.5)`。

**微基准 1 条**：`tests/culling_cost.rs`（手动、`#[ignore]`）。同一场景、同一
相机，只切 `set_frustum_culling`；共享网格与各自网格各测一组（原因见下节）。
基准自己断言"关剔除 0 剔除 / 开剔除 > 一半被剔 / 画了 + 剔了 == 2000"，
防止剔除没生效时得到一张假快的表（P2.5 的教训）。

### 五、读数出口：编辑器 HUD 与无头截图

- 视口右下角新增一行 `Draws N   Inst M   Cull K`（等宽 10.5，与左下 fps / 右上
  相机同一套药丸底）：相机一转这个数就变，剔除有没有在干活不用翻日志。
  与缩放提示（`bottom - 26`）同角叠放，不压字。
- `SceneRenderer::frame_stats()` 直连管线；读的是**上一次真正渲染**的结果 ——
  静态场景被指纹门控整帧跳过时，画面就是上一帧那张，统计也停在上一帧。
- 无头截图报告新增 `screenshot.render_stats`
  （`draw_calls` / `opaque_batches` / `opaque_instances` / `blended_draws` /
  `culled_entities` / `culled_instances` / `shadow_only_instances` /
  `cull_ratio`）。自动取景会把整场框进视锥，所以默认截图 `culled_* == 0`；
  显式 `--camera-pos/--camera-target` 时它解释"图里为什么少了东西"。
  JSON 字段名与顶层一样是**稳定契约**（测试钉字段存在性）。

### 六、刻意的取舍

1. **先剔实体、不剔图元**：一个网格的内部图元通常同生共死；逐图元求交的收益
   在"整网格挡住/没挡住"的粒度上几乎为零，还要为每个图元保存世界 AABB。
2. **AABB 而不是包围球**：球半径取半对角线，对长条物体（地面 / 墙）会大到
   永远与视锥相交；AABB 的 p-vertex 测试既准确又只多几次比较。
3. **蒙皮余量而不是精确蒙皮包围盒**：见第三节。50% 是"常见骨骼摆幅能盖住、
   又不至于让角色永远不可剔"的工程值；要改先拿狐狸走路 / 挥拳的截图量。
4. **仅阴影批次独立成表**，而不是"把相机外实体留在主批次里让 GPU 裁"：那样
   主 pass 仍会写实例数据、绑材质、发 draw call，CPU 侧一分不省；也不能把
   它们剔除后只补一遍阴影 pass（会重复收集）。
5. **剔除默认开、开关只给测试/基准**：保守性由逐字节像素等价测试守，生产路径
   没有关闭的理由。相机矩阵退化自动关闭，不暴露"剔除失败"这种状态。
6. **不做遮挡剔除**：视锥剔除只回答"在不在画面里"，不回答"被不被挡住"；
   Hi-Z / 深度预pass 是另一个量级的工程（要读回上一帧深度 + 重写实例表），
   留到批次结构再演进时做。
7. **基准把共享 / 各自网格分两张表**：只报共享网格会严重低估剔除在"素材没做
   合批"项目里的价值（-3% vs -65%），只报各自网格又会把合批的收益算到剔除
   头上。两个数字一起看才知道该先做哪件事。
8. **基准关阴影**：相机外投影物按设计必须保留，混进来测的是"剔除 + 阴影补投"
   的组合而不是剔除本身；阴影正确性另有 GPU 测试。

### 七、已知限制

- **无 LOD**：距离远的物体仍按全精度网格画；剔除只去掉视锥外的，不降低视锥内
  的几何量。差距分析里 LOD 仍是缺口。
- **无遮挡剔除 / Hi-Z**：被墙挡住的一整排物体仍在主 pass 里。遮挡剔除需要
  深度预 pass 或上一帧深度回读，当前管线还没有可用的深度金字塔。
- **剔除是 O(实体数 × 6 平面)**：2000 实体每帧多 ~6 µs 量级的 CPU 成本（已
  包含在"开剔除"的 CPU 读数里），没有空间索引；万级实体前要配合 ECS / 分区
  做 broad phase，而不是给每个实体再乘常数。
- **蒙皮包围盒仍是绑定姿态**：余量覆盖常见摆幅，但"长矛突刺"这类极端姿势
  仍可能在屏幕边缘提前消失；要精确就得 CPU 蒙皮求盒或让 GPU 回读。
- **仅阴影实体仍付蒙皮骨骼计算**（必须算，阴影顶点着色器要用）；剔除省的是
  主 pass 的实例写、材质组与 draw call，不是骨骼。
- **阴影贴图的拟合上限 `MAX_SHADOW_RADIUS` 之外**：物体已不在阴影图里，剔除
  与阴影补投都不会救它（与旧行为一致）。

## 三十一、渲染：FXAA 抗锯齿（2026-10-20）

补齐差距分析里"后处理链"的 FXAA 一格。引擎的场景 pass 已经走 MSAA（默认
按适配器能力取最高档，常见 4×），但 MSAA 只解决**几何边缘**的覆盖问题：
高光的高频细节、细长几何、低采样档位下的斜边仍然是一格一格的硬台阶。FXAA
是叠在合成之后的第二趟全屏 pass，把"显示空间的台阶"磨成中间调。

```text
场景 pass（HDR, MSAA） ──resolve──▶ HDR ──合成（曝光 / Bloom / ACES）──▶
    ┌─ FXAA 关：直接写 LDR 显示目标（与上线前逐位一致）
    └─ FXAA 开：先写 LDR scratch，再 FXAA → LDR 显示目标
```

**默认关**（`RenderConfig.fxaa.enabled = false`）：FXAA 会改变画面（哪怕只在
边缘），与 Bloom / 天空同一条纪律 —— 画质增强不能默认动旧场景。关着时渲染器
**整段跳过**：不建 scratch、不记录 pass、不写 uniform，是真正的零成本，而不是
"跑一遍然后原样输出"。

### 一、算法：FXAA 3.11 的质量路径，阈值保守一档

- **5 抽头 + 亚像素项**（`FXAA_QUALITY__PRESET 12` 一族）：中心 + 四角判边缘
  方向，沿方向取 1/3、2/3 两点平均成 A，再用 ±½ 两点平均成 B；B 跑出局部
  亮度范围时退回 A。亚像素项把中心与四邻均值按局部对比度混入（0.75），
  专治"细高光闪烁"这类 MSAA 管不到的走样。
- **常量取公开口径，边缘阈值保守一档**：亚像素 0.75、方向收缩 1/8（下界
  1/128）、跨度上限 8 texel 都是 FXAA 3.11 原值；边缘阈值 stock 是 1/24 与
  1/8，这里取 **1/12 与 1/6** —— 默认已有 MSAA，stock 阈值叠上去会整幅柔化，
  而 FXAA 只该收拾 MSAA 覆盖不到的走样。这些阈值是 Lottes 按 8 位显示值标定
  的，改一个就要重新评估"平坦区域是否被无辜柔化"—— 因此 `FxaaConfig` 只暴露
  开关，不把调参负担转给用户。
- **luma / 混合在显示空间做**：阈值回答的是"人眼在屏幕上看到的对比度"。
  在线性光里算，暗部边缘会被过度平滑、亮部欠平滑。sRGB 目标上采样值由硬件
  解码成线性，着色器先 `srgb_encode` 到显示空间，混合完再 `srgb_decode` 回
  线性交给硬件第二次编码；非 sRGB 目标（合成 pass 已手动编码）直接原样处理。
- **平坦区必须逐字节不变**：五抽头范围低于阈值且亚像素项为 0 时，原样返回
  中心采样值，不走 encode/decode 往返 —— 8 位量化边界上的往返误差会让"没开
  FXAA 的角落"悄悄变一个 LSB。
- **采样一律 `textureSampleLevel(..., 0.0)`**：边缘检测的分支建立在采样值上，
  WGSL 只允许显式 LOD 的采样出现在非一致控制流里。后处理是 1:1，LOD 恒为 0，
  没有画质代价。

### 二、接线：独立第二趟 pass + 惰性 scratch

- 合成 pass 的目标从"显示目标"变成"FXAA 关时 = 显示目标 / 开时 = scratch"；
  FXAA pass 再把 scratch 写进显示目标。两条路径共用同一条合成管线，管线状态
  没有分叉。
- scratch 是**目标格式**的 LDR 纹理（`RENDER_ATTACHMENT | TEXTURE_BINDING`），
  尺寸跟源走；尺寸变化时整块重建（绑定组引用视图，视图对象换了必须重建）。
  关着时直接 `None`，不养这份显存。
- FXAA 的 uniform（texel 尺寸 + sRGB 标志）在 scratch 建好时一次写死，之后
  每帧只记录 pass —— 参数不随帧变化，没有理由每帧上传。

### 三、先证红

实现之前，`fxaa_smooths_edges_without_touching_flat_areas`（1× MSAA、
320×240、绕 Z 轴旋转的薄盒）在旧实现下**红得干脆**：`fxaa` 只是个未接线的
配置字段，开 / 关两张图逐字节相同，第一条断言直接报
`FXAA 开关没接进后处理链：两张图逐字节相同`。

实现后的读数（同机一次运行）：

| 指标 | 关 | 开 |
|---|---|---|
| 硬台阶数（相邻亮度差 > 0.25，横 + 纵） | 672 | **374（-44%）** |
| 中间调像素（背景 0.389 / 前景 0.953 之间） | 0 | **804** |
| 平坦背景角点 `(4,4)` | — | **逐字节不变** |

同组测试还钉住三条前提：前景 / 背景亮度差 > 0.15（不然测不出边缘）、
关着时硬台阶 > 200（1× MSAA 的斜边必须真有台阶）、开着的平坦角点必须与
关着逐字节相同（早期退出的第一道闸门）。

### 四、确定性与成本

- `two_runs_produce_identical_png_bytes` 改为**同时开满 Bloom + FXAA**：
  scratch 的惰性建、两段 pass 的记录顺序、sRGB 往返都必须确定 —— 两次进程
  运行（连渲染器都重建）PNG 仍逐字节一致。
- 桌面设置里加 `FXAA` 勾选（Inspector 的渲染区块），`scene_signature` 哈希
  `render.fxaa.enabled`（暂停时勾选必须让视口刷新）；无头报告与 MCP
  `project_info` 都投影 `render.fxaa.enabled`，报告字段名是稳定契约。
- **实测**（RTX 2060 / 1280×720 / MSAA×4，`postfx_cost` 新增第四档）：

  | 路径 | 场景段 | 后处理段 | 合计 |
  |---|---|---|---|
  | HDR + ACES | 131.1 µs | 24.7 µs | 155.8 µs |
  | HDR + ACES + Bloom | 131.1 µs | 85.8 µs | 216.9 µs |
  | HDR + ACES + FXAA | 131.3 µs | **72.2 µs** | **203.5 µs** |

  FXAA 相对纯 ACES **+47.6 µs/帧（+30.6% 端到端）**：一张全尺寸 LDR scratch
  + 一趟 9 次采样（5 抽头 + 亚像素 + sRGB 换算）的全屏 pass。这是"开"的
  价格；关着时后处理段仍是 24.7 µs（整段跳过）。

### 五、刻意的取舍

1. **只做 FXAA、不做 TAA**：TAA 要历史缓冲、重投影与运动向量，是另一个量级的
   工程（还要处理镜头移动时的鬼影）。FXAA 是"一张当前帧就能做完"的方案，
   先把低采样下的台阶问题关掉，TAA 留到有速度缓冲时再做。
2. **默认关而不是"MSAA 低档才开"**：自动策略会让同一项目在不同 GPU 上画面
   不同，也违背"画质增强默认不动画面"的纪律。开关交给项目和用户。
3. **不做质量档位**：FXAA 3.11 的各档差异只在边缘搜索的采样数，当前带宽下
   质量档的收益（更长的边缘）抵不上多出来的采样；要档位时再加字段，默认值
   保持不变。
4. **scratch 不池化复用**：开关是手动操作，重建一次无感；池化会让"关着时
   零显存"这条承诺失效。
5. **测试强制 1× MSAA**：4× MSAA 的斜边本来就有中间调，FXAA 的效果会被
   硬件抗锯齿掩盖，测不出"开关是否接线"。1× 才能让 672 个硬台阶暴露出来。

### 六、已知限制

- **FXAA 会柔化高频细节**：细文字、极细的几何、镜面高光边缘都会被抹一点；
  这是空间抗锯齿的固有代价，不是实现缺陷。
- **没有质量档 / 锐化**：不做"FXAA + 锐化"的组合，也不暴露阈值；需要更锐的
  项目应优先提高 MSAA / 分辨率。
- **scratch 是一张全尺寸 LDR 纹理**：4K 下约 33 MB（`Rgba8`），开关打开时
  常驻。关掉即释放。
- **不处理运动**：FXAA 是纯空间滤波，镜头快速平移时仍可能有高频闪烁；
  TAA / 运动模糊是另一条路。
- **不对 HDR 中间标做 FXAA**：边缘检测在显示空间做，HDR 高光在曲线前不参与
  判断；这是有意为之（见"显示空间"一节），但也意味着"曲线前的高频"不会被
  这一步处理。



---

## 三十二、P0 资产管线：GUID + `.meta` + 导入 + 资源浏览器（2026-10-20）

优先级来自 `Docs/差距分析.md`：**唯一的 P0**（资产管线）。不做的代价是
"场景 / 预制体多起来之后改引用格式 = 重写全部资产文件与所有外部工具"。

### 一、为什么先做这一刀

盘点结论（`差距分析.md` 第四节）：场景里 `model: "Resources/Box.glb"` 是路径，
资产改名 / 挪目录即断链，只能靠 `--check` 事后发现再手改。而引用格式是**数据格式**，
越晚改越贵。同时"资源浏览器"这一端完全缺失（只有 Prefabs 面板），
每加一个资产都要手改 RON —— 对 Agent-Native 路线尤其刺眼。

### 二、做法（完整推理见决策记录 §十四）

1. **磁盘存 GUID、内存存路径**。转换只在 `ProjectHandle` 的 load / write 边界：
   `xgame_core::asset` 的 `encode_scene_references` / `decode_scene_references`
   （预制体同构复用）。243 个消费点一行没改。
2. **`.meta`**（`<资产>.meta`，RON）：`guid` + `kind` + `import`。
   GUID 首次导入生成，之后永不变；改名 / 移动只搬文件。
3. **`AssetDatabase`**：一次扫描建 `guid ↔ path` 双向索引 + 诊断
   （缺 meta / 孤儿 meta / 坏 meta / GUID 冲突）。构建产物目录跳过。
4. **`--check` 扩展**：`dangling_guids` / `missing_metas` / `orphan_metas` /
   `invalid_metas` / `guid_conflicts` / `missing_prefabs`。检查仍**严格只读**。
5. **`--migrate-assets <项目>`**：老项目一次性升级。用**字符串字面量替换**
   而不是整份重序列化 —— 示例场景是带注释的教程，重写会吃光注释
   （第一版实现踩了，靠 `git diff` 发现后改成替换）。
6. **资源浏览器**（Assets 面板，与 Prefabs 共页签）：目录树 + 类型图标 +
   搜索 + 详情（路径 / GUID / 导入设置）+ 双击放入场景 + **拖到视口按射线落点放置**。
7. **MCP `asset_list`**：智能体的资产树视野（只读，含诊断）。
8. **仓库内三个项目已迁移**：8 份 `.meta`，`SampleScene` 场景 14 行引用改写，
   注释一字未动。

### 三、验证

- 新回归测试 **33 条**（core 22：资产库 15 / 项目端到端 4 / check 诊断 3；
  编辑器 9：状态 5 / 面板 2 / 落点 2；MCP 2），全量 **491 passed / 6 ignored**
  （基线 458）。
- 核心验收测试：`renaming_asset_keeps_scene_reference_alive`
  （挪资产 + `.meta` → 场景仍解析到新路径）—— 旧实现下必红。
- 冒烟：`--check projects/SampleScene` → `entities=14 lights=1 assets=8` 全绿；
  `--headless --frames 60` 正常；`--ui-shot` 三档尺寸截图检查资源浏览器布局。

### 四、刻意的取舍

1. **内存不引入 `AssetRef` 类型**：见决策记录 §十四.1，避免改 243 个消费点与热路径。
2. **场景文件不登记资产**：它由 `entry_scene` 按路径引用，没有改名需求；
   强行要 `.meta` 会让"新建项目"立刻带一条待处理诊断。
3. **导入设置只落盘不消费**：`model_scale` 的正确实现要连骨骼 / 逆绑定矩阵 /
   动画轨道一起缩放，半吊子接线会与物理不一致 —— 留到下一刀（文档已标注）。
4. **视口落点用 y=0 地面平面**，不做任意表面吸附：项目约定地面在 y=0；
   射线朝上时退回相机前方 5 米。表面吸附等做角色控制器时一起补射线查询。
5. **拖拽不用 egui `DragAndDrop` payload**：视口是自绘离屏纹理，命中与落点本来
   就要自己算；`Option<String>` 更直白，也更好测（收尾放在 `main.rs` 帧末，
   面板被拖成非活动页签时不会漏）。

### 五、已知限制

- 资产被删后，场景里留 `guid://…`（这是刻意的，`--check` 会点名）。
- `Fox.gltf` 的外部 buffer `Fox.bin` 不是独立资产：移动 gltf 时必须连它一起搬
  （`.meta` 目前不记依赖）。
- 重命名资产要在编辑器外做（还没有 in-editor 的 rename / move API）。
- 编辑器保存是全量序列化，会吃掉场景注释（既有行为，见决策记录 §十四.7）。

## 三十三、角色控制器 + 碰撞事件进脚本（2026-10-20）

优先级来自 `Docs/差距分析.md` P1 第 3 项：3D 游戏的主角移动与"踩到触发器"是玩法
闭环的第一块。路线是**封装 rapier 的 `KinematicCharacterController`**，不手写扫掠。

### 一、四个上游缺陷（这一节是本轮的主要成本）

rapier 0.22 + parry 0.17.6 的这条链路对**胶囊角色**有四处硬缺陷，全部先用诊断
测试跑出真实数据、再决定绕过方式（完整推理见决策记录 §十五与
`xgame-physics/src/character.rs` 的模块文档）：

1. `(Capsule, Cuboid)` 的 contact manifold 在 parry 里被注释掉 → `grounded`
   恒为 false，连带 `snap_to_ground` 整体失效。
2. `check_and_fix_penetrations()` 是空函数 → 一旦陷地，扫掠返回 `None`、整段
   位移放行，实测 60 帧沉到 y=-41 并穿墙。**根因链**：静止间隙恰好等于 `offset`
   时本来就会退化 → 所以静止间隙取 `skin_width × 1.1`，着地窗口再宽 20%。
3. 60° 薄板上胶囊扫掠返回 `None`（0°/30° 正常）→ 陡坡滑落自实现（投影到坡面）。
4. `autostep` 的 `handle_stairs` 只在"第一次扫掠撞墙"时触发，且 `min_width`
   不足以把胶囊底面推上台阶 → 关掉，自实现"抬升 → 前进一个半径 → 贴回"。

### 二、分层

| 层 | 内容 |
|---|---|
| `xgame-core` | `CharacterController`（可序列化 + `sanitized()` + `center_height()`/`rest_gap` 约定）、`CharacterState` / `CharacterContacts`（运行时组件，零分配） |
| `xgame-physics` | `move_character`（grounded / snap / 皮肤层硬约束 / 陡坡滑落 / 台阶）、`probe_ground`（胶囊中心出发、法向间隙） |
| `xgame-script` | `character_*` 8 个方法（Lua 与 Rust 组件共用真相源）、`Hook::{CollisionEnter, CollisionExit}` + 带参数派发 |
| `xgame-runtime` | controller 实体的刚体托管、接触面差分 → enter/exit 双向派发、`Lifecycle::{on_collision_enter, on_collision_exit}` |
| `xgame-editor` | Inspector 的 Character 区块 |

### 三、验证

- 物理新增 **21 条**（4 条配置翻译 + 17 条行为）：站立 / 吸附 / 悬空 / 撞墙 /
  沿墙滑 / 上台阶 / 高台阶挡停 / 爬坡 / 陡坡滑落 / 坡面法线 / 跳跃 / 传感器 /
  高速下冲 / 高速横冲 / 陷地自救 / 零位移 / 零台阶高度 / 缺失刚体。
- 运行时新增 **3 条**：角色行走（实体变换跟随 + 状态组件）、跳跃（最高点 + 落回）、
  角色碰撞事件（运动学 vs 静态）；脚本层 **1 条** Lua 端到端（脚本驱动 + 收事件）。
- 全量：`cargo test --workspace --lib` **412 passed / 0 failed**；集成测试
  （runtime / script / xgame / render）另 16 条全绿，6 条 GPU / 截图用例 skip。
  编辑器测试二进制在本机被安全策略拦下，未纳入本次统计。

## 三十四、游戏 UI：屏幕空间 UI 运行时 + 渲染 + 脚本 API（2026-10-20）

优先级来自 `Docs/差距分析.md` P1 第 4 项。评价很直接：**任何可玩的 demo 都需要
HUD、菜单、血条**，而这是"内容层三件套"里唯一完全没有技术底子的一件
（角色控制器有物理底座、粒子有渲染底座，UI 什么都没有）。

路线刻意**先做运行时、不做可视化编辑器**（差距分析里写的就是这句）：egui 的
即时模式布局思路不能复用（没有锚点、没有九宫格、不能做游戏内交互），而
"把锚点算对、把字排对、把点击判对"才是真正难的部分。

### 一、分层

| 层 | 内容 |
|---|---|
| `xgame-core::ui` | `UiElement` / `UiStyle` / `UiTextStyle` / `UiImageStyle` / `UiState`（可序列化；**运行时状态 `#[serde(skip)]`**） |
| `xgame-ui`（新 crate） | 锚点布局（`layout`）、指针状态机（`input`）、文本排版 + 字形图集（`text`）、绘制数据合批（`draw`）。**纯 CPU，不认识 wgpu** |
| `xgame-render::ui_renderer` | 一条实例化管线（圆角 SDF + 纹理采样）、图集 / 图片上传、UI pass |
| `xgame-runtime` | 每帧布局 + 命中测试 → 写回 `UiState`；`set_ui_viewport` / `set_ui_pointer` |
| `xgame-script` | 23 个 `ui_*` API（`ui_ctx.rs` 方法层 + `ui_api.rs` 转发壳） |
| `xgame-editor` | Inspector 的 UI 区块（锚点预设 / 样式 / 文本 / 图片）、Create UI 菜单、Play 态指针映射 |

### 二、五个关键决定

**1. UI 元素就是实体（Unity `RectTransform` 模式），不是独立的 UI 树。**

层级面板、撤销栈、预制体、复制粘贴、Inspector 全部免费复用 —— 对一个小引擎
来说，"少一套平行的编辑器模型"比少写几百行布局代码重要得多。代价是 `Entity`
多了一个字段，`transform` 对 UI 元素不参与布局（位置由锚点决定）。

**2. 锚点模型只做 Unity 的最小可用子集：anchor_min/max + offset_min/max。**

```text
rect.min = parent.min + anchor_min * parent.size + offset_min
rect.max = parent.min + anchor_max * parent.size + offset_max
```

四行公式覆盖"贴角 / 居中 / 四边拉伸 / 通栏"。**故意不做 pivot 与旋转缩放** ——
它们只有做动画 / 世界空间 UI 时才需要，却会让命中测试与文本绘制复杂一档。
分辨率适配靠锚点（Unity 的 "Constant Pixel Size" 档），"设计分辨率缩放"
（Canvas Scaler）留到有需要时再加。

**3. 运行时状态（悬停 / 按下 / 点击）放进 `UiElement` 本身，`#[serde(skip)]`。**

三个好处：脚本查询（`ui_clicked(ctx, id)`）只需要 `&Scene`（不用给 `GameCtx`
加字段，也就不会波及七个构造点）；渲染器读同一份数据画悬停态；实体删除时
状态跟着走，不会留悬垂条目。

**代价是一个必须记住的坑**：`Entity::eq` 是脏检查的真相源，把 `state` 算进去
会让"鼠标划过按钮"变成"场景有未保存改动"。所以 `UiElement` 额外提供
`data_eq()`（只比落盘字段），`Entity::eq` 用它 —— 回归测试
`state_changes_do_not_count_as_edits` 锁着这一条。

**4. 矩形的绘制顺序 = 实体层级的 DFS 先序，命中测试反向扫描。**

不引入 z 序字段：同层按实体在场景里的顺序，后画的盖住先画的。这是"唯一真相"，
没有排序步骤，也不需要处理"两个元素 z 相同"的歧义。

**5. 所有 UI 图元走一条实例化管线（这一版最重要的性能决定）。**

一开始是两条管线（矩形 SDF / 纹理精灵），实测一屏 900 个元素切出 **1200 次
draw call** —— 因为"背景 / 文字"交替出现，而**绘制顺序就是语义**，不能按类型
重排（后一个元素的面板会盖掉前一个元素的文字）。

合并成一条管线后，矩形不采样纹理但绑定组与字形相同（图集），于是相邻的
矩形与文字天然同批：**示例 HUD（7 个元素 / 4 个矩形 / 47 个字形）= 1 次 draw**。
只有图片会打断批次（它是另一张纹理）。无头报告的 `render_stats.ui.draws`
就是这条读数，CI 能直接断言。

### 三、文本排版

自己写（`ab_glyph` 提供度量与轮廓光栅化，不引 cosmic-text / swash）：

- **字形缓存 + R8 图集**：1024² 起、按需翻倍到 2048²，shelf 打包，字形四周留
  1px 透明边防渗色；UV 用 **texel 坐标**存，图集翻倍不影响旧字形。
- **换行**：拉丁按空格断词、中日韩任意字间可断（UAX #14 有一百多条规则，对
  HUD 没有收益）；找不到断点就硬断 —— 溢出到别的元素上比断在单词中间更难看。
- **对齐**：水平三档 + 垂直三档，行宽按 advance + kerning 重算。
- **落整像素**：字形按整像素光栅化，小数坐标只会让双线性采样把边缘糊掉。
- **字体来源**：系统字体（与编辑器同一套候选），找不到**不崩**、只是不画字
  （告警一次）。不内嵌中文字体：10–20 MB + 授权条款各不相同。

已知限制：不做复杂文字整形（阿拉伯文 / 天城文连写不会正确成形）、没有
粗体合成（缺粗体字重时退回常规）、没有字距微调之外的字形变体。

### 四、颜色空间（最容易搞错的一处）

- 组件里存的是**线性 RGBA**（与材质 / 灯光同一约定），RON 里写
  `(0.0070, 0.0086, 0.0116, 0.78)` 这种值；
- **脚本 API 收 sRGB 0..1**（美术 / CSS 的直觉），入口统一 `to_linear()`；
- **egui 的取色器收线性值**（`color_edit_button_rgba_unmultiplied`）——
  第一版多套了一次 `to_srgb()`，色块比画面亮一大截（截图里按钮显示成浅蓝），
  已修正并写进注释。

实测校验：截图里按钮底色 `(38, 92, 168)` = 场景文件里写的 `#265ca8`，逐分量一致。

### 五、验证

- `xgame-core`：序列化兼容（最小 RON）、运行时状态不落盘、`data_eq` 忽略状态、
  坏数据夹取。
- `xgame-ui`：28 条 —— 锚点数学（四角 / 拉伸 / 嵌套 / 尺寸不随窗口变 / NaN
  几何拦截）、DFS 顺序与隐藏子树、命中测试的层级与穿透、点击状态机（按下 →
  拖出 → 拖回 → 松开仍算点击；拖出松开取消；离开视口清悬停）、文本（换行 /
  对齐 / 透明度 / 缓存命中 / 图集翻倍保内容）、悬停与按下明度、合批（矩形与
  字形同批、图片切断批次）。
- `xgame-runtime`（集成，5 条）：**真 Lua 脚本**端到端 —— `ui_clicked` 触发、
  点外部不触发、无视口不交互、脚本 `ui_spawn_button` 生成的元素下一帧可点、
  隐藏元素不可点。
- 编辑器：`viewport_pointer` 的坐标换算（DPI / 视口偏移 / 视口外返回 None）。
- **截图自检**：`--screenshot` 出图（HUD 在 1280×720 下排版正确、中文 / 粗体 /
  圆角 / 血条 / 按钮全部正常）；`--ui-shot --ui-play` 验证编辑器 Play 态视口；
  `--ui-select HUD.Button` 验证 Inspector 区块（并据此修掉了四个被截断的标签、
  取色器色彩空间、以及"UI 区块落在折叠线以下"的布局问题）。
- **坏数据防线**：加载边界 `ProjectHandle::load_scene/load_prefab` 夹取一次
  （NaN / 越界 → 默认值），布局入口再拦一道非有限矩形（整棵子树跳过 + 告警）——
  一个手写的 `nan` 不会变成"整屏 UI 消失"。
- **性能**（`cargo test -p xgame-ui --release --test ui_cost -- --ignored`）：
  900 元素 / 3190 字形 → **753 µs/帧**（占 60fps 预算 4.5%），1200 个命令段
  被渲染器合并成 **1 次 draw**。空场景（1000 个纯 3D 实体）走 `any` 快路径，
  一次布局都不做。

### 六、已知限制（有意为之，不是遗漏）

- 没有裁剪（`RectMask2D` 等价物）：子元素溢出父矩形会画到外面 —— 滚动列表
  要做之前必须先补它；
- 没有 pivot / 旋转 / 缩放动画；
- 没有九宫格（9-slice）与图集打包；
- 没有文字输入框、没有手柄 / 键盘导航；
- UI 不吃指针时游戏输入照常（"点 UI 时不要开枪"需要脚本自己用 `ui_hovered` 判断）；
- 无 Canvas Scaler（设计分辨率缩放）。
