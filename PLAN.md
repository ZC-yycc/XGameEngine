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
