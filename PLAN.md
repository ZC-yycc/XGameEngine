# XGameEngine 开发计划

> 从"编辑器原型"到"可运行的游戏引擎"的演进路线。

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
