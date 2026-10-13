# XGameEngine

> Rust 实现的类 Unity 游戏引擎，基于 wgpu + egui + rapier3d。

## 快速开始

```powershell
# 编辑器 —— 启动后进入「项目枢纽」，可新建 / 打开项目
cargo run -p xgame-editor

# 编辑器 —— 直接打开指定项目（跳过枢纽）
cargo run -p xgame-editor -- projects/SampleScene

# 独立游戏 —— 运行一个项目
cargo run -p xgame-game -- projects/SampleScene

# 无头项目检查 —— 不开窗口，验证清单 / 场景 / 资产引用
# （退出码 0=干净 · 1=加载失败 · 2=悬空引用；给 CI / AI 智能体用）
cargo run -p xgame-editor -- --check projects/SampleScene

# MCP server —— AI 智能体接口：stdio 起服务，客户端可读写场景、动实体、跑检查
# （工具参考 Docs/MCP.md）
cargo run -p xgame-mcp

# 一键构建可分发软件包（等价于双击根目录的「一键构建.cmd」）
powershell -NoProfile -ExecutionPolicy Bypass -File tools\build-release.ps1
```

## 引擎与游戏项目是分开的

**引擎**（本仓库的 `crates/`）与**游戏项目**（磁盘上任意目录）彻底分家。
编辑器不再假设自己旁边就躺着场景文件，而是先**打开一个项目**。

```
XGameEngine/                  # 引擎源码仓库
├── crates/
│   ├── xgame-core/           # 核心类型 + Project（项目清单、路径解析）
│   ├── xgame-math/           # 数学库：glam 重导出 + projection 辅助
│   ├── xgame-render/         # 渲染管线：wgpu 上下文、glTF 加载、ScenePipeline
│   ├── xgame-physics/        # 物理引擎：rapier3d 刚体、碰撞体、射线、调试绘制
│   ├── xgame-input/          # 输入层：设备状态 + 动作映射（与窗口库解耦）
│   ├── xgame-script/         # 脚本层：嵌入式 Rhai + 生命周期钩子 + 宿主 API
│   ├── xgame-runtime/        # 运行时：把场景/物理/输入/脚本编排成一条帧管线
│   ├── xgame-derive/         # 过程宏：#[derive(Panel)]
│   ├── xgame-editor/         # 编辑器程序：项目枢纽 + egui 面板
│   └── xgame-game/           # 独立游戏二进制
└── projects/                 # 默认的项目存放位置（可放在任意目录）
    └── SampleScene/          # 示例项目
        ├── project.ron       # 项目清单
        ├── input.ron         # 输入映射（动作 / 轴）
        ├── Scenes/
        ├── Scripts/          # 实体脚本（.rhai）
        └── Resources/
```

依赖方向是单向的，新增的四个 crate 都在中间层：

```text
core ← math ← {render, physics} ← editor / game
  ↑        ↑         ↑
  └── input / script ┘
             ↑
         runtime（编排上面全部）
```

一个**游戏项目**是自包含的目录：

```
<项目名>/
├── project.ron      # 清单：项目名、入口场景、窗口设置、内容目录
├── Scenes/          # 场景文件（.ron）
└── Resources/       # 模型 / 纹理 / 音频等资产
```

场景里的 `model: "Resources/Box.glb"` 是**项目相对路径**，统一由 `ProjectHandle`
解析成绝对路径。编辑器与游戏都不依赖进程当前工作目录，项目目录可以随便移动、
拷贝、进版本库。

### 编辑器界面

编辑器二进制承担两个界面，打开项目后互相切换：

```
┌──────────────────────────┐   新建 / 打开项目   ┌──────────────────────────┐
│ 项目枢纽                  │ ─────────────────▶ │ 编辑器                    │
│  · 新建项目               │                    │  · Hierarchy / Scene View │
│  · 打开项目               │ ◀───────────────── │  · Inspector / Console    │
│  · 最近项目列表           │     ⌂ Projects      │  · Play / Pause / Stop    │
└──────────────────────────┘                    └──────────────────────────┘
```

最近项目列表持久化在 `%APPDATA%\XGameEngine\recent_projects.ron`（Windows）。

## 渲染与编辑能力

**渲染**（`xgame-render`，编辑器与独立游戏共用同一条管线）

- glTF 模型加载：顶点 / 法线 / 顶点色 / UV + base color 贴图 + PBR 材质因子
- Blinn-Phong 光照：环境光 + 漫反射 + 半程向量高光，单帧最多 8 盏方向光 / 点光
- 光照是**场景级数据**（`Scene::lights` / `Scene::ambient`），随场景一起存盘
- **方向光阴影贴图**：`Light::casts_shadow` 打开后，那盏方向光会先渲染一遍场景深度，
  主 pass 再用 3×3 PCF 比较出阴影；`shadow_resolution` 可调贴图分辨率（0 = 2048）
- **MSAA 4×**：颜色附件走多采样、resolve 回单采样纹理，物体轮廓不再是硬台阶。
  采样数由适配器能力决定（不支持的格式自动退回 1×）
- 半透明（glTF `alphaMode`）与不透明分两条管线，半透明按到相机距离远 → 近排序
- 内置基础图元（Cube / Sphere / Plane / Cylinder）不依赖外部资产文件
- **骨骼蒙皮动画**（GPU skinning，详见「动画」）：关节矩阵在顶点着色器里按权重合成，
  阴影 pass 同步蒙皮

**编辑器**

- Hierarchy / Prefabs / Scene View / Inspector / Console 五面板 Dock 布局
- **场景层级（父子关系）**：层级面板是一棵可折叠的树，拖拽节点即重挂父节点
  （保留世界变换），拖到空白处移回根层；Inspector 有父节点下拉框
- **预制体（Prefab）**：把一棵子树存成可复用资产并反复实例化，支持 Apply / Revert
- 变换 Gizmo（平移 / 旋转 / 缩放）；**一次拖拽 = 一步撤销**，与 Inspector 的拖拽编辑一致；
  拖拽结果**只取决于"鼠标现在在哪"**，与帧率无关（轴上的参考线锚在拖拽起点，见下）
- 视口里**单击即选中**：射线 × 实体的**世界 AABB**（不是"包围球"）。被拉伸过的物体
  （地板、平台）用包围球会严重高估，连相机都落进球里，导致"点哪儿都选中它、取消不掉"
- 撤销 / 重做：`Ctrl+Z` · `Ctrl+Shift+Z` · `Ctrl+Y`
- 复制 / 粘贴：`Ctrl+C` · `Ctrl+V`；复制实体 / 删除：`Ctrl+D` · `Delete`
- **项目设置（工具栏 ⚙）**：窗口 / 重力 / 固定步长 / 内容目录可视化编辑，草稿式
  Apply 写回 `project.ron`；重力与步长不再是引擎源码里的硬编码，编辑器 Play 与
  打包游戏读的是**同一份**配置（`WorldConfig::from_project_runtime`）
- **未保存保护**：场景有改动时标题栏与工具栏显示 `*`（按帧 memo 的落盘快照比较），
  关窗 / 返回枢纽前弹确认（保存 / 不保存 / 取消）；`Ctrl+S` 保存、`Ctrl+Shift+S`
  **另存为**新场景文件并切过去（旧文件原样保留）
- **无头检查**：`xgame-editor --check <项目>` 秒级验证项目完整性（见快速开始），
  退出码可判 —— 一条命令跑通"改 → 查 → 自证"闭环
- 🔨 Build 把项目打包成独立游戏
- **帧序**：每帧"先处理输入（飞行相机 / 拖拽 / 拾取）→ 再按最新状态发渲染任务 → 最后画叠加层"，
  拖物体与转视角都不会多慢一帧
- **同步渲染（同帧闭环）**：3D 视口在 UI 线程**同一帧内**画完 —— 本帧输入处理完才发
  渲染任务、同帧贴到视口，输入到像素零管线延迟（2026-10-12 架构反转：曾试过独立渲染
  线程，真机复验引入一帧滞后与 UI 阻塞，已回退）。静态帧靠输入指纹整段跳过，
  稳态单帧 CPU 实测 ~0.4ms

## 阴影（方向光）

场景里的方向光可以投射**实时阴影**——这正是给场景"压下去一层"、让物体之间产生
空间关系的东西：

```ron
lights: [
    (
        name: "Sun",
        kind: Directional,
        direction: (0.45, -1.0, 0.35),   // 光**射出去**的方向
        intensity: 3.2,
        casts_shadow: true,              // 这盏灯投射阴影
        shadow_resolution: 0,            // 0 = 引擎默认（2048²）
    ),
],
```

做法是经典的 **shadow mapping**，每次出帧多一个 pass：

```text
1. 阴影 pass   用光源的正交视图投影矩阵，只把深度画进一张 2048² 深度图
               （场景世界包围盒 → 拟合正交视锥，见 fit_directional_shadow）
2. 主 pass     片元变换回光源裁剪空间，与深度图比较（3×3 PCF 软化边缘），
               被挡住的片元只留环境光
```

几个刻意的取舍：

- **只做方向光**。点光阴影要 6 面立方体贴图，成本高得多；`Light::casts_shadow`
  在点光上会被忽略。
- **只做一盏**。第一盏开了 `casts_shadow` 的方向光是唯一投影的灯。多盏各自的
  阴影贴图意味着每盏一次全场景重绘 —— 先把一盏做扎实，比铺开十几盏半成品值得。
- **正交视锥按场景世界包围盒拟合**，而不是拍一个固定大小的盒子：贴图分辨率固定，
  视锥开太大精度就被稀释成噪点，开太小边缘的物体直接没影子。
- **半透明图元不投阴影**。玻璃本来也不该在地上投一块实心黑斑。
- 没有开阴影的灯、或场景里没有任何带模型的实体时，整个阴影 pass 被跳过（零开销）。

编辑器里在 Inspector 的光照区块就能勾 `casts_shadow`，改完立刻重画。

## 动画

给实体挂上 `animator`，它引用的 glTF 模型就会动起来（骨骼蒙皮）：

```ron
(
    name: "Fox",
    model: "Resources/Fox.gltf",
    transform: (
        position: (0.0, 0.0, 0.0),
        rotation: (0.0, 0.0, 0.0, 1.0),
        // Khronos Fox 样例按厘米建模（骨骼跨度 ~100 单位，且 glTF 里没有归一化
        // 根节点），scale 0.01 才是真实狐狸体型（约 1 米），否则有百倍立方体大。
        scale: (0.01, 0.01, 0.01),
    ),
    animator: Some((
        clip: "Walk",      // glTF 里的动画剪辑名
        speed: 1.0,        // 播放倍率；0 = 冻结，1 = 原速
        looping: true,     // true 取模循环；false 播完停在最后一帧
        autoplay: true,    // 载入即自动播放；false = 停在第 0 帧摆姿势
        fade_time: 0.25,   // 换剪辑交叉淡入时长（秒）；0 = 硬切
    )),
),
```

**怎么工作**：

- **解析**：glTF 的 animation / skin / 节点层级在加载时转成 `AnimClip`（轨道 + 关键帧）
  与 `Skin`（关节、逆绑定矩阵）；LINEAR / STEP 运行时插值，CUBICSPLINE 读入时降采样。
  位置 / 缩放线性插值，**旋转走 slerp**。
- **播放**：每实体一个播放时钟（`AnimationDriver`）。编辑器用墙钟推进（视口在播就
  持续出帧），独立游戏跟帧 dt 走 —— 与物理 / 脚本同一拍。**换剪辑自动交叉淡入**
  （`fade_time`，默认 0.25s）：旧剪辑连同它自己的时钟一起淡出，两份局部姿态按权重
  混合（平移 lerp、旋转 slerp）；暂停中切换或 `fade_time = 0` 则硬切。
- **蒙皮**：每帧把当前姿势的关节矩阵（关节全局 × 逆绑定矩阵）上传到骨骼存储缓冲，
  顶点着色器按顶点的 `joints / weights` 加权合成模型空间坐标。**阴影 pass 一样蒙皮**，
  否则跑动的角色会投出一具绑定姿态的静止影子。每个动画实体占一个定长槽位
  （256 骨），多个角色同时播互不干扰。
- **编辑器**：Inspector 的 **Animation 区块**直接选剪辑、调速度、勾 Loop / Autoplay，
  暂停状态下换剪辑也立刻看到新姿势（动画配置进了渲染指纹）；视口有剪辑在播时帧
  策略自动保持连续出帧，停播后回到"没动就不画"的静态门控。

v1 的边界（ROADMAP 阶段七有完整清单）：**单剪辑直读 + 换剪辑淡入** —— 没有状态机、
没有叠加（additive）混合、没有 morph target；负速度不做倒放（停在第 0 帧）。每个模型
取第一张蒙皮，超过 256 骨的骨架会在加载时告警并截断。

## 场景层级

实体之间可以有父子关系，与 Unity / Godot 的场景树一致：

```ron
Entity(
    name: "Box2",
    transform: (position: (3.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0), scale: (1.5, 1.5, 1.5)),
    model: "Resources/Box.glb",
    id: (2),
    parent: Some((1)),      // 挂到 id 为 1 的实体下
)
```

几个刻意的设计：

- **层级只存一条 `parent`（按 ID 引用）**，子节点不内嵌列表。子节点永远由
  `Scene::children` 现算——唯一真相只有那条 `parent`，不会出现"看到的树"和
  "数据的树"对不上。要**遍历整棵树**时用 `Scene::hierarchy()` 一次拿到
  `parent_indices` + `child_map`，别在循环里逐节点调 `children()`
  ——后者每次都要扫一遍实体表，整棵树就成了 O(n²)。
- **`Entity::transform` 是局部变换**，渲染 / 物理用的是 `Scene::world_matrix`
  解算出的世界变换。父节点一动，整棵子树跟着动。
- **重挂父节点保留世界变换**（Unity 的 "keep world transform"）：拖到新父节点下时，
  局部变换会被自动重算，物体在画面上的位置不跳。成环（把父节点拖进自己子树）
  会被拒绝并在控制台提示。
- **旧场景文件自动兼容**：没有 `id` / `parent` 字段的老 `.ron` 直接加载，全部落
  在根层。

## 预制体（Prefab）

把一棵实体子树（含层级）存成**可复用资产**，之后能在场景里反复实例化——对应
Unity 的 Prefab、Godot 的 PackedScene。

```
<项目>/
├── Prefabs/
│   └── Pillar.prefab.ron     # 一份预制体资产 = 一个"小场景"
├── Scenes/
└── Resources/
```

工作流（都在编辑器里完成）：

1. 在 Hierarchy 里选中一棵子树（比如父节点 + 几个子节点）；
2. **Prefabs 面板**（左下）→ `Create from Selection`，填个名字 → 生成资产，
   **同时把场景里这批实体原地变成该预制体的实例**（与 Unity 一致）；
3. 面板里点一下资产 = 实例化一份到场景，可以放多少个都行；
4. 改动某个实例后：**Apply** 把这次的改动写回资产（所有实例下次 Revert 都会拿到），
   **Revert** 用资产把当前实例覆盖回去（**保留它自己的位置**）。

场景文件里的实例长这样：

```ron
Entity(
    name: "Pillar",
    transform: (position: (6.0, 0.0, 0.0), rotation: (0.0, 0.0, 0.0, 1.0), scale: (1.5, 1.5, 1.5)),
    model: "Resources/Box.glb",
    id: (7),
    parent: None,
    prefab: Some((
        source: "Prefabs/Pillar.prefab.ron",   // 来自哪份资产（项目相对路径）
        source_id: (1),                        // 对应资产里哪个实体
        is_root: true,                         // 是不是这个实例的根
    )),
)
```

几个刻意的设计：

- **实例 = 资产的一份副本 + 一行来源标记**，不是"活引用"。所以实例化本身不依赖
  资产文件随时在场，场景存盘后就算资产被挪走也还能打开（只是 Apply/Revert 会报错）。
  这样避免了"资产一改，场景里几十个实例被隐式改掉"的意外。
- **`source_id` 指向资产内部的稳定 ID**，是 Apply / Revert 能精确落位的前提；
  资产里的实体 ID 与我们生成它时的场景实体 ID 一一对应，不需要再做模糊匹配。
- **`is_root` 把实例根和内部节点分开**：只有根接受 Apply / Revert / 整棵连坐删除。
- **资产文件里不保留实例来源**（写盘前会清掉），否则预制体套预制体会把实例元数据
  腌进资产，一实例化就自带错误的来源路径。
- **普通实体不写 `prefab` 字段**（`skip_serializing_if`），场景文件不会被一堆
  `prefab: None` 灌满；旧文件缺这个字段也能照常加载。
- 构建（🔨 Build）会把 `Prefabs/` 一并打进产物。

## 脚本（Rhai）

实体可以挂 `.rhai` 脚本，引擎在固定步长 / 每帧 / 销毁时回调：

```rhai
// Scripts/Jumper.rhai
const JUMP_SPEED = 6.5;

fn update(ctx) {
    if !action_pressed(ctx, "Jump") { return; }
    let me = self_id(ctx);
    let v = velocity(ctx, me);
    set_velocity(ctx, me, v.x, JUMP_SPEED, v.z);   // 保留水平速度，只跳
}
```

- 脚本按**项目相对路径**挂在实体上（`Entity::scripts`），所以一份脚本能挂到
  几十个实体，改一次逻辑全部生效
- 四个钩子全部可选：`start` / `update` / `fixed_update` / `on_destroy`
- Play 模式下点工具栏 **⟳ Scripts** 即可热重载，不必退出重进
- 脚本出错**不会**拖垮引擎：编译错误、运行时错误、死循环超限都只记一条日志，
  这一帧该钩子不生效，游戏继续跑
- 完整的函数清单、坐标约定、以及两个 Rhai 语法坑
  （全局 `let` 在函数里看不见、每实体状态要用 `state_*`）见
  **[Docs/脚本API.md](Docs/脚本API.md)**

## 输入映射

项目根目录放一个 `input.ron`，把物理按键翻译成语义动作 —— 游戏逻辑写
`action_down("Jump")`，改按键不用改代码、更不用重编引擎：

```ron
InputMap(
    actions: { "Jump": [Key(Space)], "Fire": [Mouse(Left)] },
    axes:    { "MoveX": (positive: [Key(KeyD)], negative: [Key(KeyA)]) },
)
```

输入层（`xgame-input`）刻意**不依赖 winit**：它自己定义一套与键盘布局无关的
`KeyCode`，窗口库的翻译只发生在编辑器 / 独立游戏这两个真正接窗口的地方。
这让输入可以被无头测试驱动，也为将来录放、网络同步留好了口子。

编辑器里按 Play 试玩时，egui 的键鼠状态会通过 `input_bridge` 灌进运行时，
所以脚本里的 `action_pressed` 在编辑器里同样能用。

## 物理

物理属性是**场景数据**（`Entity::physics`），不是"凡是带模型的实体都变成盒子"：

```ron
physics: Some((kind: Dynamic, shape: Ball(radius: 0.5), restitution: 0.55)),
```

- `kind`：`Static`（地面 / 墙）/ `Dynamic`（受重力）/ `Kinematic`（脚本驱动）
- `shape`：`Auto`（按实体缩放生成盒子）/ `Cuboid` / `Ball` / `Capsule` / `Cylinder`
- 不写 `physics` = **纯视觉实体**，不参与模拟（旧场景文件正是这个语义，不会被重力拽走）
- 提供射线检测（`PhysicsWorld::raycast`），脚本里是 `raycast(ctx, ...)`，
  自动忽略传感器并排除自身

**动态体由物理驱动、运动学体由脚本驱动** —— 这条分工决定了数据往哪个方向流，
两者不会互相覆盖。

## 运行时（编辑器和游戏共用的那一段）

`xgame-runtime::World` 是唯一编排者，编辑器按下 ▶ Play 与打包出去的游戏跑的是
**同一段** `update`：

```text
clock.tick(dt)
  ├─ dispatch_start                 新实体跑一次 start
  └─ 固定步长循环（与渲染帧率解耦）
       ├─ dispatch_fixed_update → 补/清刚体 → 推运动学体 → physics.step → 回写动态体
  ├─ dispatch_update                每渲染帧一次
  └─ input.end_frame()              清掉"仅本帧有效"的输入边沿
```

在这之前，"物理步进 + 变换回写"在编辑器和独立游戏里各有一份实现，
迟早漂移（事实上漂过一次）。现在只有一份。

## 性能

引擎自身每帧的开销刻意做得很低：**静态内容不付出任何重复代价**。几条硬规则：

| 位置 | 规则 |
|---|---|
| `World::update` | `sync_bodies` 先做一次**无分配扫描**，判定"刚体集合没有任何结构性变化"就整段跳过（含射线加速结构重建）。`physics.step()` 每步已经维护了查询流水线，稳态再重建一次纯属浪费 |
| `World::update` | 运动学 / 动态体的存在与否由上述扫描顺带记下（`has_kinematic` / `has_dynamic`），`push_kinematic_bodies` / `pull_dynamic_bodies` 据此早退 —— 纯静态场景连世界矩阵都不解算 |
| `Scene::world_matrices` | 链缓冲提到循环外复用；`out` 用 `Mat4` + 一张 `done` 位图而不是 `Option<Mat4>`（后者 80 字节/个）。这个函数在游戏里**每帧**都要跑 |
| `SceneRenderer`（编辑器） | 离屏渲染有**输入指纹**门控，指纹只哈希「局部变换 + 父引用」（等价于世界矩阵）+ Animator 配置 + 动画时钟签名，**不**为算指纹先解算世界矩阵 —— 静态帧零分配；渲染在 UI 线程同帧完成，有剪辑在播时时间每帧都变 → 指纹必变 → 持续出帧 |
| Hierarchy 面板 | 行模型按（结构指纹, 折叠版本, 过滤器）缓存 —— 树没变就零 String 分配，不再每帧重建整棵树的行 |
| `ScenePipeline::render` | 所有实体的模型矩阵先攒进一个复用缓冲，最后**一次** `write_buffer`（旧的逐实体写 = 每帧 N 次 staging 分配 + N 条拷贝命令） |
| 控制台 | 消息表有上限（1000 条，成批丢最旧）—— 每帧打一条不同日志的脚本不该把内存拖垮 |
| 编辑器空闲 | **空闲时完全不重绘**（实测 CPU 0.00%）。Windows 在指针静止时会反复投递同一坐标的 `WM_MOUSEMOVE`，而 egui 把"本帧有输入"当作"要立刻再画一帧"，于是编辑器会一直 60fps 空转、拖窗口时跟模态循环抢消息线程。现在位置未变的 `PointerMoved` 直接丢掉（语义 no-op）；**原始位移 `Event::MouseMoved` 整类丢弃**（光电鼠标传感器噪声以 ±1~2px 灌进来会续命帧调度宽限期，编辑器无人消费原始 delta） |
| 编辑器帧调度 | `frame_policy.rs` 集中决策「要不要连续出帧」：**Hybrid**（默认，`XGAME_FRAME_POLICY` 可切 `always`/`reactive`）—— 交互中 / 刚输入过 220ms 内满速出帧，其余停帧。拖拽阶段与永远满速同手感（~60fps、p99≈35ms），纯事件驱动的旧实现拖拽中有 30% 死时间（操作发涩的根源）。全工程只此一处主动 `request_repaint` |
| 编辑器缩放 | 拖边框时**节流重建**离屏目标（≤10Hz），尺寸一稳定立刻按最终尺寸重建。实测一次拖拽重建次数 43 → 1 |

可复现的微基准（2000 实体）：

```sh
cargo test -p xgame-runtime --release --test update_throughput -- --ignored --nocapture
```

| 场景（2000 实体） | 优化前 | 优化后 |
|---|---|---|
| 纯视觉（无物理） | 226 µs/帧 | **13 µs/帧**（≈17×） |
| 全静态体 | 1026 µs/帧 | **89 µs/帧**（≈11.5×） |
| 稀疏动态 ×64 | 1685 µs/帧 | **724 µs/帧**（≈2.3×） |
| 密集接触 ×1000 | 13989 µs/帧 | 13005 µs/帧 |

最后一行说明一件事：真正吃满 CPU 的是 **rapier 求解器**（1000 个互相挤压的动态体），
不是引擎编排。想在那档继续提速得换思路（作业系统 / 减少接触岛），不在"编排层优化"的范围内。

## 一键构建

这里的「构建」有两种，分别对应**把编辑器做成软件**和**把游戏项目做成软件**。

### 把引擎构建成可分发的软件包

双击仓库根目录的 **`一键构建.cmd`**，产物落在 `dist/`：

```
dist/
├── xgame-editor.exe          # 编辑器，双击即用
├── xgame-game.exe            # 播放器（编辑器按 Build 时会复制它，必须和编辑器放在一起）
├── projects/SampleScene/     # 示例项目，打开枢纽后可直接选它
└── README.txt                # 分发时附带的使用说明
```

等价的命令行写法：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\build-release.ps1
```

脚本自己处理了两件容易翻车的事：

- **cargo 不在 PATH** —— rustup 装完之后默认不会写进 PATH，于是新开的 shell（双击 `.cmd`
  拿到的正是这种）根本没有 `cargo`。脚本会回退到 `%USERPROFILE%\.cargo\bin`。
- **链接时 MSVC 库不全** —— 先按原样构建一次；**只有失败**才去找 MSVC toolset + Windows SDK，
  拼出 `LIB` / `INCLUDE` 重试。这样既能救这台机器，又不会写死任何机器专属路径。

构建日志写在 `build-release.log`。

### 把游戏项目构建成独立游戏

在编辑器里打开项目，点右上角的 **🔨 Build**。产物落在 `<项目目录>/Build/<项目名>/`：

```
Build/<项目名>/
├── <项目名>.exe      # 播放器的副本（改了名，看起来就是这款游戏）
├── project.ron       # 项目清单
├── Scenes/
└── Resources/
```

这个布局是刻意的：`xgame-game` 在没有命令行参数时，会优先去找**自己可执行文件旁边**的
`project.ron`。所以构建出来的游戏**双击 exe 就能跑**，不需要参数，也不需要启动脚本。

## MCP（AI 智能体接口）

`xgame-mcp` 是本引擎的 **MCP server** —— Agent-Native 路线的第一块：把"读项目、改场景、
动实体、跑检查"暴露成标准 MCP 工具，Cursor / Claude 等任何 MCP 客户端接上就能**不用鼠标
直接操纵引擎**做游戏开发。无窗口、无 GPU，stdio 起服务（按行 JSON-RPC）。

```json
{ "mcpServers": { "xgame": { "command": "D:\\...\\dist\\xgame-mcp.exe" } } }
```

| 工具 | 干什么 |
|---|---|
| `project_info` | 读清单概要：入口场景、窗口/重力/步长、场景/脚本/预制体清单 |
| `scene_read` | 读场景的完整 RON 文本（与文件同构，可原样写回） |
| `scene_write` | 整体替换场景；**先解析后落盘**，坏数据一个字节不写 |
| `entity_upsert` | 按名字创建/部分更新实体（变换、模型、脚本、父子） |
| `entity_remove` | 按名字删除实体（重名歧义拒绝；子节点提升不连坐） |
| `project_check` | 无头验证（与 `--check` 同一份逻辑），返回 JSON 报告 |

改完就 `project_check` 自证没改坏 —— 这是智能体的「改 → 查 → 验」闭环。
完整协议面、参数语义与已知边界（编辑器并发写、清单只读）见 [`Docs/MCP.md`](Docs/MCP.md)。

## 依赖方向

```
core ← math ← {render, physics} ← editor / game
core ← mcp                      （MCP server 只依赖 core：无窗口、无 GPU）
```
