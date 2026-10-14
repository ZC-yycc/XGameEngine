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
