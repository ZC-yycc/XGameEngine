# XGameEngine

> Rust 实现的类 Unity 游戏引擎，基于 wgpu + egui + rapier3d。

**文档导航**：想跑起来看本文「快速开始」；想查某个功能怎么用，看本文对应小节；
想知道**还差什么、先做哪个**看 [`Docs/差距分析.md`](Docs/差距分析.md)；阶段状态看
[`ROADMAP.md`](ROADMAP.md)；改引擎代码前先查 [`Docs/决策记录.md`](Docs/决策记录.md)
的约束表；历史决策与实测数据在 `PLAN.md` 与 `FLOW.md`。

## 快速开始

```powershell
# 编辑器 —— 启动后进入「项目枢纽」，可新建 / 打开项目
cargo run -p xgame-editor

# 编辑器 —— 直接打开指定项目（跳过枢纽）
cargo run -p xgame-editor -- projects/SampleScene

# 独立游戏 —— 运行一个项目
cargo run -p xgame-game -- projects/SampleScene

# 无头项目检查 —— 不开窗口，验证清单 / 全部场景 / 资产引用 / 资产身份证
# （退出码 0=干净 · 1=加载失败 · 2=有问题；给 CI / AI 智能体用）
cargo run -p xgame-editor -- --check projects/SampleScene

# 场景格式说明书 —— 导出机器可读 JSON（字段 / 默认值 / 示例 / 资产引用）
# （无 --out 打 stdout；同一份代码两次导出逐字节相同）
cargo run -p xgame-editor -- --export-schema --out schema.json

# 脚本 API 清单 —— 导出 178 个函数 / 6 个钩子 / 参数 / 返回值 / 约定（JSON）
# （与 MCP `script_api_read` 同源；无 --out 打 stdout）
cargo run -p xgame-editor -- --export-script-api --out script_api.json

# 老项目迁移 —— 给资产生成 .meta、把场景 / 预制体里的路径引用改写成 GUID
# （幂等；只替换引用字面量，不动注释与排版）
cargo run -p xgame-editor -- --migrate-assets projects/MyOldGame

# 无头运行 —— 不开窗口跑 N 帧，stdout 出 JSON 统计（帧耗时 / 实体 / 脚本日志）
# （退出码 0=干净 · 1=加载失败 · 2=跑完有问题；见 Docs/无头运行.md）
cargo run -p xgame-editor -- --headless --frames 300 projects/SampleScene

# MCP server —— AI 智能体接口：stdio 起服务，客户端可读写场景、动实体、跑检查、无头实跑
# （工具参考 Docs/MCP.md）
cargo run -p xgame-mcp

# 一键构建可分发软件包（等价于双击根目录的「一键构建.cmd」）
powershell -NoProfile -ExecutionPolicy Bypass -File tools\build-release.ps1
```

## 真实发布包验收（Windows 桌面 + GPU）

先完成同批 workspace release 构建并刷新 dist，再运行：

```powershell
powershell -NoProfile -File tools/test-published-player.ps1
```

脚本调用真实 `--build`，将生成的包移至独立目录，保留源项目但使原路径失效；从非项目工作目录无参数启动包内 EXE。通过 Windows 键盘消息走 winit 输入桥，逐关验证自定义 J 键动作/轴、松开后切关、真实 Spinner DLL 的旋转和销毁日志。三关各保存窗口截图，`outputs/published-player-<唯一编号>/` 保留包、源项目、日志与 result.json；失败抛错，不清理证据。参数 `-Fault input` / `-Fault plugin` 为预期失败的负对照，`-TimeoutSeconds` 控制单个断言等待上限。

这覆盖真实播放器和 GPU 窗口，但消息注入不等于物理键盘/系统焦点/IME 验证；截图需人工查看。现有 hosted CI 未接此桌面 GPU 门禁，不用 headless 代替。若 Windows PowerShell 因环境同时存在大小写重复的代理变量拒绝 Start-Process，需在运行进程内消除重复键，勿修改系统代理配置。脚本为 UTF-8 BOM，保证 Windows PowerShell 5.1 正确解析中文注释。

## Rust 插件兼容检查

使用 `xgame::export_plugin!` 导出插件；宏同时生成原入口与版本化 C 描述符 `xgame_plugin_descriptor_v1`。宿主在注册前校验构建指纹，缺少描述符或指纹不匹配时拒绝调用原入口，并报告插件路径与重编提示。旧 DLL 即使版本号相同也需要重新构建。

在当前 workspace 中，将宿主与插件按同一工具链、源码、锁文件及 profile 构建。例如在 PowerShell 中运行：

```powershell
$env:CARGO_INCREMENTAL = '0'
cargo build --workspace --release --locked -j2
```

将同批 `target/release` 的宿主程序用于运行，并将 `behaviors_demo.dll` 放到 `projects/BehaviorsDemo/Plugins/`；替换已有插件前先备份，避免混用 debug/release 或历史 DLL。遇到“构建指纹不匹配”时应统一配置并重编两侧，不要改写描述符绕过检查。

此检查仅比较当前 workspace 中 xgame 构建单元可见的信息，是保守拒绝门禁，**不是完整 Rust ABI 兼容证明**。下游全部 flags、依赖 feature 合并、第三方源码状态以及 OS 加载动态库时执行的初始化均不在保证范围内；也不防伪造描述符或恶意插件。详细契约见 `Docs/决策记录.md` §四。

## CI

推送到 `master`、开 PR 或手动 dispatch 时，GitHub Actions（`.github/workflows/ci.yml`）会在
`windows-latest` 上跑 `cargo check` / `clippy -D warnings` / `cargo test`，再跑
示例项目的 `--check` / `--headless` 与 `--export-schema` / `--export-script-api`
二进制冒烟。所有 cargo 命令带 `--locked`；没有 GPU 的 runner 上渲染测试会自动
skip，`--screenshot` 留给带显卡的自托管 runner。完整取舍见 `PLAN.md` 五十二与
`Docs/决策记录.md` §三十四。

## 引擎与游戏项目是分开的

**引擎**（本仓库的 `crates/`）与**游戏项目**（磁盘上任意目录）彻底分家。
编辑器不再假设自己旁边就躺着场景文件，而是先**打开一个项目**。

```
XGameEngine/                  # 引擎源码仓库
├── .github/                  # GitHub Actions（CI workflow）
├── crates/
│   ├── xgame-core/           # 核心类型 + Project（项目清单、路径解析）
│   ├── xgame-math/           # 数学库：glam 重导出 + projection 辅助
│   ├── xgame-render/         # 渲染管线：wgpu 上下文、glTF 加载、ScenePipeline
│   ├── xgame-physics/        # 物理引擎：rapier3d 刚体、碰撞体、射线、调试绘制
│   ├── xgame-input/          # 输入层：设备状态 + 动作映射（与窗口库解耦）
│   ├── xgame-script/         # 脚本层：嵌入式 Lua + 生命周期钩子 + 宿主 API
│   ├── xgame-audio/          # 音频引擎：kira 封装（设备 / 3D 空间轨道 / 解码缓存 / 无声降级）
│   ├── xgame-particle/       # 粒子：确定性 CPU 模拟（按实体分桶 / 上限 / burst），输出 billboard 四边形
│   ├── xgame-runtime/        # 运行时：把场景/物理/输入/脚本/音频编排成一条帧管线
│   ├── xgame/                # 门面 crate：prelude + Lifecycle / Inspect + 插件边界
│   ├── xgame-derive/         # 过程宏：#[derive(Panel)] / #[derive(Inspect)]
│   ├── xgame-editor/         # 编辑器程序：项目枢纽 + egui 面板
│   ├── xgame-game/           # 独立游戏二进制
│   └── xgame-mcp/            # MCP server：AI 智能体工具面（stdio JSON-RPC）
├── examples/spinner-plugin/  # 示例 Rust 组件插件（cdylib）
├── projects/                 # 默认的项目存放位置（可放在任意目录）
│   ├── SampleScene/          # 示例项目
│   │   ├── project.ron       # 项目清单
│   │   ├── input.ron         # 输入映射（动作 / 轴）
│   │   ├── Scenes/
│   │   ├── Scripts/          # 实体脚本（.lua）
│   │   └── Resources/
│   └── BehaviorsDemo/        # Rust 组件插件演示项目（本身是 workspace 成员）
```

上面各 crate 的依赖方向是单向的（不许反向依赖）；逐层关系见本文末
[「依赖方向」](#依赖方向)，避免同一张图在文件里出现两次。

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

编辑器偏好与面板布局保存在同一目录的 `editor.ron`：工具栏的滑块按钮打开
**Preferences** 窗口，可调相机速度 / 鼠标灵敏度 / 视口网格；拖动页签、调整分隔条后
自动保存，下次启动恢复，窗口里的「恢复默认布局」立即重建出厂布局。`XGAME_CONFIG_DIR`
可以把整个配置目录指到别处（便携运行 / 隔离验证）。

做无头截图时，`--ui-shot` 默认**只读**：不读也不写 `editor.ron`，保证截图永远对应
出厂界面；要验证偏好恢复加 `--ui-restore-prefs`，要在截图里打开偏好窗口加
`--ui-preferences`（见 `AGENTS.md` 的截图自检约定）。

**自动保存**是另一份独立的东西：每 60 秒（Preferences 里可调，设 0 关闭）把**有改动**
的场景写进项目根的 `.autosave/` 副本（原子写、与真保存同源 GUID 编码）。它与"保存"
无关 —— 标题栏的 `*` 还在、关窗照样要确认，`Ctrl+S` 仍然是唯一真正落盘的动作。
如果编辑器崩溃 / 被强杀，下次打开项目会弹「发现自动保存」，可以**恢复**（恢复后仍需
`Ctrl+S`）、**丢弃**或**稍后**；手动保存成功或正常退出时副本会被自动清掉（副本只服务
崩溃）。`.autosave/` 新建项目时就写进了 `.gitignore`。

**日志**有第三个去处：`%APPDATA%\XGameEngine\logs\Editor.log`（同一目录，`XGAME_CONFIG_DIR`
可覆盖）。引擎的 `log::warn!` / `log::error!` 以前只写 stderr —— 双击 `dist` 里的 exe
时没有终端，等于什么都没留下。现在同一条日志同时进入 stderr（`RUST_LOG` 语义不变）、
Output Log 面板和这个文件：追加写、4 MiB 轮转（只留一份 `Editor.log.1`）、每行带
`[+启动以来秒数] 定宽等级 模块路径: 消息`。面板右上角的「Log 目录」一键打开所在文件夹。
落档范围比 stderr 宽：自家 crate 的 `info` 与所有人的 `warn/error` 都收，第三方 `info`
和 `debug/trace` 不收（`RUST_LOG=trace` 不该把日志写成 GB 级）。面板上的等级开关与
关键词过滤只影响**显示**，`Clear` 才是清消息。

报错行可以**点开源码**：脚本报错自带 `Scripts/player.lua:42:` 这样的位置（按项目根解析），
引擎的 `log::warn!` 带 `file:line`，点击就弹出源码查看器窗口并高亮、滚到那一行
（只读、单文件上限 512 KiB；打包后的 exe 没有源码，窗口会如实说打不开哪个文件）。
只认 `.lua:` 这一种格式 —— 认不出来的不猜，宁可不可点。

**资产 / 脚本自动热重载**：编辑器每秒扫一次项目内容目录（只看路径 / 修改时间 / 长度，
不读文件内容）。改 `Scripts/*.lua`：编辑态刷新脚本列表，**Play 中自动重新编译**；改模型 /
贴图 / 材质 / `.meta`：重新扫描资产库并丢弃渲染缓存（模型 / 材质 / 贴图下次加载生效）；
新增或删除预制体：刷新资产与预制体列表。**场景文件不会自动重载** —— 磁盘上的 `.ron`
被外部改写只记一条 debug 日志，避免冲掉未保存的编辑（Scenes 菜单本身现扫，新场景会自动
出现）。当前打开的材质草稿如果有未保存改动，外部修改只提示、不覆盖（Revert 可载入磁盘版）。
`--ui-shot` 等只读会话不启用热重载，保证截图可复现。

## 渲染与编辑能力

**渲染**（`xgame-render`，编辑器与独立游戏共用同一条管线）

- glTF 模型加载：顶点 / 法线 / 顶点色 / UV + base color 贴图 + metallic / roughness 材质因子
- **PBR 着色（Cook-Torrance）**：GGX 分布 + Smith 几何 + Schlick 菲涅尔，金属度工作流；
  单帧最多 8 盏方向光 / 点光。灯强度按"radiance = intensity × π"标定，换模型不换亮度
- **逐实体材质覆盖**（`Entity.material`）：base_color 乘子 / metallic / roughness 字段级
  覆盖，Inspector 可调、随场景落盘 —— 内置白模也能上色、金属化
- **共享材质资产**（`.mat`）：颜色 / 金属度 / 粗糙度 / albedo 贴图存成可复用资产，
  场景里按 GUID 引用；优先级「模型 → 资产 → 实体覆盖」，Inspector 选择、Assets 拖到实体指派
- 光照是**场景级数据**（`Scene::lights` / `Scene::ambient`），随场景一起存盘
- **方向光阴影贴图**：`Light::casts_shadow` 打开后，那盏方向光会先渲染一遍场景深度，
  主 pass 再用 3×3 PCF 比较出阴影；`shadow_resolution` 可调贴图分辨率（0 = 2048）
- **HDR 渲染路径 + ACES 色调映射 + 曝光**：场景 pass 渲进 `Rgba16Float` 中间标，
  曝光与曲线后写 LDR；`project.ron` 的 `render` 节（三宿主同源，`ToneMapMode::Off`
  是旧管线出口）
- **Bloom 泛光**（可选，默认关）：亮部软膝提取 + 三级 mip + 可分离高斯，与色调映射
  同一趟合成；`render.bloom` 三参数，`intensity = 0` 即关闭（整段跳过）
- **程序化天空盒 + 环境光照（IBL）**（可选，默认关）：天顶 / 地平线 / 地面渐变 +
  太阳盘 / 光晕；同一个解析函数喂背景、漫反射球谐与镜面预过滤环境，`render.sky` 七参数
- **MSAA**：颜色附件走多采样、resolve 回单采样纹理，物体轮廓不再是硬台阶。采样数取
  适配器支持的最高档（常见 4×；不支持的格式自动退回 1×）
- **质量档位**（`project.ron` 的 `quality` 节，可选）：低 / 中 / 高预设收拢
  "机器向"的三颗螺丝 —— MSAA（1× / 4×）、阴影贴图默认边长（1024 / 2048 / 4096）、
  LOD 阈值缩放（0.6 / 1.0 / 1.25），另有 `msaa` / `shadow_resolution` / `lod_scale`
  显式覆盖。不配这一节 = 与引入之前逐位一致；改档位只改预算不改画面风格
  （曝光 / Bloom / TAA / FXAA 仍由 `render` 节决定）。三档实测场景段 GPU
  102 / 156 / 223 µs（2000 方块 + 4 级 CSM）
- **FXAA 抗锯齿**（可选，默认关）：合成后的画面再走一趟 FXAA 3.11 质量路径，
  补 MSAA 管不到的高频细节；`project.ron` 的 `render.fxaa.enabled` 开关，
  关着时整段跳过（不建 scratch、不记录 pass）
- **TAA 时域抗锯齿**（可选，默认关）：每帧给相机矩阵叠一个 Halton(2,3) 亚像素抖动
  （8 帧一循环），用上一帧的历史与重投影结果混合，把 MSAA + FXAA 都消不掉的边缘
  台阶压成中间调；`render.taa` 两个参数（`enabled` / `history_weight`，默认 0.9）。
  抖动**只进相机**，剔除与阴影级联仍用未抖动矩阵（否则纹素吸附会抖）；3×3 邻域
  钳制防鬼影；关着时整段跳过、逐位一致。开着时 FXAA 自动让位
- 半透明（glTF `alphaMode`）与不透明分两条管线，半透明按到相机距离远 → 近排序
- 内置基础图元（Cube / Sphere / Plane / Cylinder）不依赖外部资产文件
- **骨骼蒙皮动画**（GPU skinning，详见「动画」）：关节矩阵在顶点着色器里按权重合成，
  阴影 pass 同步蒙皮
- **距离 LOD + 绘制距离**（`Entity.lod`，可选）：逐级"模型 + 生效距离"，超出最后一级
  直接不画（最后一级写 0 = 无限远）；距离从相机量到实体**轴心**，`render.lod_bias`
  乘在阈值上（`<= 0` = 关掉 LOD，排查用）。各级模型与 `model` 走同一条 GUID 管线，
  Inspector 有 LOD 区块。2000 实体实测场景段 GPU **-85.3%**、顶点量 **-95.0%**

**编辑器**

- Hierarchy / Prefabs / Scene View / Inspector / Output Log / Profiler 六面板 Dock 布局
- **Profiler 面板**（与 Output Log 同页签组）：帧间隔 / 1% low / 主循环分段耗时
  （运行时 · 界面+视口）+ draw / 剔除 / 粒子读数，600 帧滚动窗口、Pause / Clear。
  只在编辑器里看，不落盘；长时段趋势仍用 `XGAME_FRAME_LOG` 探针
- **场景层级（父子关系）**：层级面板是一棵可折叠的树，拖拽节点即重挂父节点
  （保留世界变换），拖到空白处移回根层；Inspector 有父节点下拉框
- **预制体（Prefab）**：把一棵子树存成可复用资产并反复实例化，支持 Apply / Revert
- 变换 Gizmo（平移 / 旋转 / 缩放）；**一次拖拽 = 一步撤销**，与 Inspector 的拖拽编辑一致；
  拖拽结果**只取决于"鼠标现在在哪"**，与帧率无关（轴上的参考线锚在拖拽起点，见下）
- 视口里**单击即选中**：射线 × 实体的**世界 AABB**（不是"包围球"）。被拉伸过的物体
  （地板、平台）用包围球会严重高估，连相机都落进球里，导致"点哪儿都选中它、取消不掉"
- 撤销 / 重做：`Ctrl+Z` · `Ctrl+Shift+Z` · `Ctrl+Y`
- **视口导航**：`F` 聚焦选中子树（没选中就框全场景）—— 只把目标移到画面中心、**不改朝向**，用包围球装框所以转视角也不会突然出画；`1` / `3` / `7` = 前视 / 右视 / 顶视，配 `Ctrl` 取反（后 / 左 / 底），只转朝向不动位置，方便「先对轴再按 F」；**`5` 切正交投影**（可见高度 = 视口高度，滚轮指数缩放，正交下 F 改的是这个高度），对轴视角与正交是两件事；`[` / `]` 调飞行速度。搜索框等文本输入获得焦点时这些键不生效
- 复制 / 粘贴：`Ctrl+C` · `Ctrl+V`；复制实体 / 删除：`Ctrl+D` · `Delete`
- **项目设置（工具栏 ⚙）**：项目名 / 启动场景（`entry_scene`）/ 窗口 / 重力 / 固定步长 / 画面（曝光、
  ACES、Bloom、FXAA、TAA）可视化编辑，草稿式 Apply 写回 `project.ron`；内容目录目前只在
  `project.ron` 里手改（设置窗口未暴露）。重力与步长不再是引擎源码里的硬编码，编辑器
  Play 与打包游戏读的是**同一份**配置（`WorldConfig::from_project`）
- **多场景编辑（工具栏 Scenes）**：菜单列出 `Scenes/*.ron`，点击切换；`New Scene…`
  新建空场景（主相机 + 方向光）；`Set Current as Startup` 显式改启动场景。
  **Save 永远写当前场景，Save As / 新建都不改启动场景** —— 要改必须显式设，
  不会在备份关卡时悄悄改掉游戏入口
- **运行期场景切换**：脚本 `load_scene(ctx, "Scenes/Level2.ron")` 在**本帧末尾**
  换场景（新场景的 `start` 下一帧跑），`current_scene(ctx)` 读当前路径；
  物理刚体 / 粒子 / 音频 autoplay / UI 交互 / `state_*` 一并重置，脚本模块级全局
  变量保留。编辑器 Play、独立游戏、无头运行三处都接好了 ——
  "菜单 → 关卡"这条流程终于能用脚本写出来（详见 [`Docs/脚本API.md`](Docs/脚本API.md) §4.16）
- **关卡顺序（构建设置）**：`project.ron` 的 `build.scenes` 是一份**有序**清单
  （`⚙ 项目设置 → 构建` 里上下移 / 增删，旁边还有版本号）。顺序即关卡索引：
  `scene_index(ctx)` 取当前是第几关、`load_scene_at(ctx, i + 1)` 进下一关 ——
  关卡编排是数据，不是散落在一堆脚本里的路径。`--check` 会校验清单
  （缺场景算问题、盘上有但没列只提示），构建器打包前**逐关试装**，
  `--build` 让这条路径不开窗口也能跑
- **时间缩放 / 暂停**：脚本 `set_time_scale(ctx, 0)` 暂停模拟、`0.3` 慢动作、`2.0`
  二倍速，`time_scale(ctx)` 读当前值。缩放**只在时钟一处**生效，所以物理固定步、
  脚本 `delta` / `time`、粒子、动画钟同帧同源（不会出现"物理冻住、角色还在原地跑步"）；
  音频与 UI 不受影响，暂停菜单照样能点。Play 态视口的预览动画也跟手
  （详见 [`Docs/脚本API.md`](Docs/脚本API.md) §4.6）
- **层与碰撞矩阵**：每个实体有一个 `layer`（Inspector 顶部下拉，32 个），
  `⚙ 项目设置 → 层与碰撞矩阵` 里给层起名、点格子决定「这两层碰不碰」——
  **对称写、两侧相与**（和 rapier 一致），矩阵只保存"改过的部分"，
  默认全通 ⇒ 老工程读进来逐位一致。脚本侧 `layer(ctx, id)` / `set_layer(ctx, id, n)`
  读写、`raycast_masked(...)` 按层掩码查（只看成员位、**不看矩阵** —— Unity
  `layerMask` 语义，所以"不与任何层碰撞、只给射线用"的幽灵层是可表达的）。
  越界层号取模到 0..31，并由 `--check` / MCP `project_check` 的 `invalid_layers` 报出来
  （详见 [`Docs/脚本API.md`](Docs/脚本API.md) §4.4）
- **未保存保护**：场景有改动时标题栏与工具栏显示 `*`（按帧 memo 的落盘快照比较），
  关窗 / 返回枢纽 / 切换场景前弹确认（保存 / 不保存 / 取消）；`Ctrl+S` 保存**当前场景**、
  `Ctrl+Shift+S` **另存为**新场景文件并切过去（旧文件原样保留、启动场景不变）
- **自动保存 / 崩溃恢复**：每 60 秒（Preferences 可调、可关）把脏场景写进
  `<项目>/.autosave/` 副本；崩溃后重开项目弹「恢复 / 丢弃 / 稍后」。副本**不改变**
  未保存状态，手动保存 / 正常退出时清除 —— 详见上文「项目枢纽与编辑器偏好」
- **无头检查**：`xgame-editor --check <项目>` 秒级验证项目完整性（见快速开始），
  退出码可判 —— 一条命令跑通"改 → 查 → 自证"闭环
- **无头构建**：`xgame-editor --build <项目>` 与工具栏 Build 按钮同一条路
  （退出码 0/1，stdout 打产物路径与 `scene_count`），CI 里打包不用人点按钮
- **无头运行**：`xgame-editor --headless --frames N <项目>` 不开窗口跑 N 帧，
  stdout 出 JSON 统计（帧耗时分布 / 实体与刚体 / 脚本日志含 error 样本 /
  非有限变换），固定 dt 两次运行逐位一致；`--dump-scene` 落盘"跑完后的世界"，
  `--screenshot out.png` 再离屏渲染一帧（默认顺光取景，同机逐字节可复现）
- 🔨 Build 把项目打包成独立游戏。打包只认**和编辑器同一份引擎源码**编出来的播放器
  （`xgame-game` 二进制里自带源码指纹）：找到的播放器是旧引擎时会跳过它、全都不匹配
  就直接拒绝并提示重编 —— 免得把"跑旧引擎的成品"当成成功交付
- **打包会验自包含**：只带清单里的内容目录（外加发布清单里每一关的目录），发布前再对
  产物跑一遍完整性检查，与源码工程比对 —— **源码里有、包里没有**就直接失败并点名
  文件（`guid://…` 会翻回路径），不会发出一个"编辑器里正常、玩家手里缺东西"的包。
  源码工程**本来就缺**的引用只由 `--check` 报，不挡构建
- **帧序**：每帧"先处理输入（飞行相机 / F 聚焦与数字键对轴 / 拖拽 / 拾取）→ 再按最新状态发渲染任务 → 最后画叠加层"，
  拖物体与转视角都不会多慢一帧
- **同步渲染（同帧闭环）**：3D 视口在 UI 线程**同一帧内**画完 —— 本帧输入处理完才发
  渲染任务、同帧贴到视口，输入到像素零管线延迟（2026-10-12 架构反转：曾试过独立渲染
  线程，真机复验引入一帧滞后与 UI 阻塞，已回退）。静态帧靠输入指纹整段跳过，
  稳态单帧 CPU 实测 ~0.4ms

## 阴影（方向光 + 点光）

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

### 点光阴影（立方体贴图）

点光也能投影（`kind: Point` + `casts_shadow: true`）：

```ron
lights: [
    (
        name: "Lamp",
        kind: Point,
        position: (0.0, 3.0, 0.0),
        intensity: 6.0,
        range: 12.0,                     // 影响半径，也是立方体阴影的远平面
        casts_shadow: true,
        shadow_resolution: 0,            // 0 = 方向光默认值的一半（1024²/面）
    ),
],
```

做法是 **6 面深度图 + 立方体采样**：每面 90° 透视，六面各画一遍投射阴影的物体，
主 pass 按"光源 → 片元"的方向让硬件挑面，再做 3×3 PCF。与方向光**并列** ——
一盏方向光 + 一盏点光可以同时投影。

几个刻意的取舍：

- **每种只做一盏**。第一盏开了 `casts_shadow` 的方向光 / 点光各自投影。多盏各自的
  阴影贴图意味着每盏一遍全场景重绘 —— 先把一盏做扎实，比铺开十几盏半成品值得。
- **点光六面不做逐面剔除**（v1）：漏画一面的症状（某个方向没影子、其余五面正常）
  比浪费难查得多。实测（2000 方块 / 1280×720 / RTX 2060）六面共提交 8070 实例、
  场景段 GPU +89.7 µs —— 比方向光"4 级全画"的 8000 实例 / +99.8 µs 还便宜一点。
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
        clip: "Walk",      // 单剪辑模式：glTF 里的动画剪辑名
        speed: 1.0,        // 播放倍率；0 = 冻结，1 = 原速
        looping: true,     // true 取模循环；false 播完停在最后一帧
        autoplay: true,    // 载入即自动播放；false = 停在第 0 帧摆姿势
        fade_time: 0.25,   // 换剪辑/换状态交叉淡入时长（秒）；0 = 硬切
        // 可选：动画状态机 —— 配了它，clip/speed/looping 由状态表决定
        machine: Some((
            entry: "Idle",
            parameters: { "speed": 0.0 },   // 初值；脚本 set_anim_param / Inspector 可改
            states: [
                (name: "Idle", clip: "Survey", speed: 1.0, looping: true),
                (name: "Walk", clip: "Walk"),
                (name: "Run",  clip: "Run"),
            ],
            transitions: [
                // AtLeast 是 ≥、Below 是严格 <；进出阈值各配各的（滞回），0.3~0.5 之间不动
                (from: "Idle", to: "Walk", when: AtLeast(param: "speed", value: 0.5)),
                (from: "Walk", to: "Idle", when: Below(param: "speed", value: 0.3)),
                (from: "Walk", to: "Run",  when: AtLeast(param: "speed", value: 2.0)),
                (from: "Run",  to: "Walk", when: Below(param: "speed", value: 1.8)),
            ],
        )),
        // 可选：叠加层（additive）—— 在基础姿势（单剪辑或状态机当前状态）之上加一层**增量**，
        // 增量 = 叠加剪辑当前帧 − 它的第 0 帧（参考姿势），按 weight 加权。与机器正交。
        // 这是**单层快捷方式**：等价于排在 layers 之前求值的一个 Additive 层，两者可并存。
        additive: Some((
            clip: "Run",      // 用哪个剪辑的增量（呼吸/弹跳/晃动这类"小动作"）
            weight: 0.3,      // 0 = 不叠，1 = 全额；脚本 set_anim_additive_weight / Inspector 可推
            speed: 1.5,       // 叠加层自己的时钟速度
            looping: true,
        )),
        // 可选：混合层栈 —— 每层独立时钟 + Additive/Override 模式 + 骨骼遮罩，按数组
        // 顺序依次合到当前姿势上（后层盖前层）。求值次序：基础 → additive → layers。
        layers: [
            (
                name: "upper",       // 脚本 set_anim_layer_weight(ctx, "upper", w) 按名查找
                mode: Override,      // Override = 向这层姿势整段 lerp（Additive = 加增量）
                clip: "Survey",
                weight: 0.0,         // 0 = 这层不参与；走路段脚本推到 0.6（见 FoxLocomotion.lua）
                speed: 1.0,
                looping: true,
                mask: ["b_Spine01_02"],  // 骨骼遮罩 = 子树根名单：该骨及其全部子孙
                                          // （脊柱/颈/头/双臂）被这层作用；留空 = 整副骨架
            ),
        ],
    )),
    scripts: ["Scripts/FoxLocomotion.lua"],  // 推 speed/层栈权重（见 Docs/脚本API.md 4.9/4.10）
),
```

**怎么工作**：

- **解析**：glTF 的 animation / skin / 节点层级在加载时转成 `AnimClip`（轨道 + 关键帧）
  与 `Skin`（关节、逆绑定矩阵）；三种插值**都保留** —— LINEAR / STEP /
  **CUBICSPLINE 走 Hermite 三次样条**（入 / 出切线按"每秒"乘段长，旋转结果归一化）。
  位置 / 缩放按插值方式求值，线性档的旋转走 slerp。
- **播放**：每实体一个播放时钟（`AnimationDriver`）。编辑器用墙钟推进（视口在播就
  持续出帧），独立游戏跟帧 dt 走 —— 与物理 / 脚本同一拍。**换剪辑自动交叉淡入**
  （`fade_time`，默认 0.25s）：旧剪辑连同它自己的时钟一起淡出，两份局部姿态按权重
  混合（平移 lerp、旋转 slerp）；暂停中切换或 `fade_time = 0` 则硬切。
- **状态机**：配了 `machine` 就进入机器模式 —— 每帧从当前状态求值**一步**转移
  （参数 ≥ / < 阈值，按书写顺序取第一条命中；打错名的参数永不匹配、坏转移被跳过、
  entry 指不到状态则整台退回单剪辑模式），换状态 = 换剪辑、复用同一条交叉淡入。
  状态的 `speed` / `looping` 覆盖 `Animator` 的同名字段。参数表随场景落盘：
  脚本 `set_anim_param` 推、Inspector 里也能拖，Play 模式跑在场景快照上不污染文件。
- **脚本直接控制**：没配状态机时，`set_anim_clip` 直接换基础剪辑 —— 它写的是
  `Animator.clip`，下一帧动画钟看到变化就走**同一条交叉淡入**（同一个 `fade_time`），
  和 Inspector / RON 改剪辑没有第二套实现；`set_anim_speed` / `set_anim_looping` /
  `set_anim_fade_time` 调基础层播放参数。`set_anim_additive_clip` /
  `set_anim_layer_clip` 换叠加层与层栈剪辑（硬切，想平滑先拉权重）。配了可用状态机时
  `clip` / `speed` / `looping` 由状态表决定，直接 setter 只写字段、不改实际播放。
- **叠加混合**：配了 `additive` 就在基础姿势上**加一层增量** ——
  `最终姿势 = 基础姿势 + weight × (叠加剪辑当前帧 − 它的第 0 帧)`（第 0 帧是参考
  姿势/"零增量"帧；平移/缩放线性差、旋转按权重 slerp 一个局部增量）。走路边呼吸、
  跑路边回头这类"同时发生"的动作靠它，不必烘进每条移动剪辑（O(M+N) 取代 O(M×N)）。
  叠加层有**自己的时钟**（`speed` / `looping`）但与基础层共用同一个播放闸门；与
  状态机正交 —— 状态机换状态照常换基础层，叠加层不受惊。换叠加剪辑是硬切（想平滑
  就把 weight 脚本拉 0 再拉回）；权重 0 的叠加层零成本、与没配逐位一致。
- **混合层栈**：`layers` 里每层是 `name + mode + clip + weight + speed + looping + mask`，
  按数组顺序**依次合到当前已合成的姿势**上（后层盖前层）。两种模式 ——
  `Additive` 加增量（与 `additive` 快捷字段同语义），`Override` 向该层姿势整段 lerp；
  **骨骼遮罩**是子树根名单（`mask: ["b_Spine01_02"]` = 该骨及其全部子孙；留空 =
  整副骨架），"走路上半身张望、腿照走"这类分区域重定向靠它。每层独立时钟按**层名**
  与配置对齐（Inspector 重排不丢各自进度、换本层剪辑只归零那一层），与基础层共用
  同一个播放闸门；脚本 `set_anim_layer_weight(ctx, name, w)` 按名推权重（重名取第一个
  命中、未知名字静默 no-op）。零权重或剪辑不在模型里的层直接跳过、不付采样代价。
- **形变目标（morph target / blend shape）**：`0` = 基准形状，`1` = 完全变成那个
  目标，多个目标按权重**相加**。来源有两处：**动画剪辑里的权重轨道**（口型 / 表情，
  优先级最高）和 **`Entity.morph_weights`**（Inspector 的 Morph 滑条、脚本
  `set_morph`）。权重是**逐下标**的，名字来自 glTF 的 `targetNames`（Blender 导出的
  `eyeBlinkLeft` 之类），下标顺序与 Inspector 里列出的顺序一致。变形在 CPU 侧做
  （`xgame-render/src/morph.rs`）：只有**权重变化时**才重算并重写该图元的顶点缓冲，
  静止的表情零开销；带 morph 的图元顶点缓冲多一个 `COPY_DST` 用途，其余图元一位不变。
  Inspector 的 **Morph 区块**逐目标一个滑块（模型没有 morph 时整段不出现）。
- **蒙皮**：每帧把当前姿势的关节矩阵（关节全局 × 逆绑定矩阵）上传到骨骼存储缓冲，
  顶点着色器按顶点的 `joints / weights` 加权合成模型空间坐标。**阴影 pass 一样蒙皮**，
  否则跑动的角色会投出一具绑定姿态的静止影子。每个动画实体占一个定长槽位
  （256 骨），多个角色同时播互不干扰。
- **编辑器**：Inspector 的 **Animation 区块**直接选剪辑、调速度、勾 Loop / Autoplay，
  或「转为状态机」/ 调机器参数、看当前状态；「+ Additive」一键加叠加层（选剪辑、
  拖权重）；「+ Add Layer」往层栈加一层 —— 每层一行（名字 / 模式 / 剪辑 / 权重）
  加一行遮罩（chips + 「+ 骨骼」下拉，骨骼名来自模型节点）；暂停状态下改配置也立刻
  看到新姿势（动画配置 + 机器参数 + 叠加层 + 层栈全进了渲染指纹）；视口有剪辑在播时
  帧策略自动保持连续出帧，停播后回到"没动就不画"的静态门控。

v1 的边界（ROADMAP 阶段七有完整清单）：**单剪辑 + 换剪辑淡入 + 阈值状态机 +
单层叠加混合 + 多层混合栈与骨骼遮罩 + 形变目标** —— 遮罩是二值的（权重整体缩放，
边界不羽化）、没有分层状态机 / 根运动 / 逐条转移时长（统一 `fade_time`）、换本层剪辑
是硬切；负速度不做倒放（停在第 0 帧）。每个模型取第一张蒙皮，超过 256 骨的骨架会在
加载时告警并截断；glTF 匿名节点进不了遮罩名单（只能靠有名字的祖先进）。形变目标
只取剪辑里的**第一条**权重轨道（多网格角色分头/身的资产没覆盖），且只收位置与法线
增量（tangent 增量没有消费者）。

## 音频

**音量总线**：声音按用途分三组（`sfx` 音效 / `bgm` 背景音乐 / `voice` 对白），
一组一个旋钮；主音量在所有组之上再乘一次。组件（Inspector 的 Audio 区块）与
脚本（`play_sound_bus` / `set_bus_volume` / `bus_volume`）都能选组 /
调组音量，`set_bus_volume` **立即作用于该组已在响**的声音（设置菜单的
"音乐音量"滑条就是这个语义）。默认音量全 1.0（= 不改变声音）。


给实体挂上 `audio`，引擎（`xgame-audio`，基于 [kira](https://github.com/tesselode/kira)）
就会解码并播放它；勾上 `spatial` 就是带距离衰减的 3D 声：

```ron
(
    name: "Spinner",
    // ……
    audio: Some((
        clip: "Resources/Audio/Beep.wav",  // 项目相对路径（wav / ogg / mp3）
        volume: 0.35,       // 线性音量：1.0 = 原音量
        pitch: 1.0,         // 播放速率（同时改变音高）
        looping: true,      // 循环（环境音）；false = 播一次
        autoplay: true,     // 进场景即播；false = 等脚本 play_sound 触发
        spatial: true,      // 3D 定位：随实体移动 + 距离衰减
        min_distance: 2.0,  // 满音量的最近距离
        max_distance: 24.0, // 衰减到听不见的最远距离
    )),
)
```

**怎么工作**：

- **脚本只产生意图**：`play_sound` 系列往命令束里塞一条命令，`World` 帧末统一
  结算给引擎 —— 脚本层（`xgame-script`）不依赖 kira，编辑态 / 无头测试不接
  音频设备也能跑全部逻辑。
- **监听者跟随相机**：编辑器 Play 每帧把相机位置与朝向推给听者（up 恒 +Y、
  right = forward × up）；脚本可用 `set_listener` 覆盖（顶视角 / 固定机位玩法）。
- **`autoplay` 首播一次**：靠实体标记表去重；组件被摘除或实体被删除 →
  对应的声音自动停掉（重挂组件会重新响）。
- **无声降级**：设备打不开（无音频服务 / CI 容器）时引擎进入无声模式 ——
  照常受理播放、计数、回收，只是不出声。**所有音频回归测试都跑在这条路径上**，
  用真 WAV 走完整解码链，不用 mock。
- **帧末同步**：音频每**渲染帧**同步一次（不跟物理固定步同频 —— 人耳对 16ms 的
  声源位置更新完全不敏感），插在 `update` 派发之后、输入帧末清理之前。

脚本侧 API（`play_sound` / `play_sound_at` / `stop_sound` / `stop_all_sounds` /
`set_listener` / `set_master_volume`）见 [Docs/脚本API.md](Docs/脚本API.md) 4.11；
混音总线分组与流式长音频有意延后（见 PLAN.md 二十二 的取舍清单）。
示例：SampleScene 的 Spinner 是空间化循环蜂鸣，Jumper 起跳播一次性音效。

## 游戏 UI（屏幕空间）

HUD / 菜单 / 血条这类界面走**实体 + 锚点**：挂 `ui` 组件的实体就是 UI 元素，
父子关系决定绘制顺序，位置由锚点决定（`transform` 对 UI 元素不参与布局）。

**在编辑器里做**：Hierarchy 底部 `+ Add` → `UI Panel / UI Label / UI Button / UI Image`
（或右键 → Create UI），选中后在 Inspector 的 **UI** 区块调锚点 / 样式 / 文本 / 图片。

**锚点模型**（Unity RectTransform 的最小子集）：

```text
rect.min = 父矩形.min + anchor_min × 父矩形.size + offset_min
rect.max = 父矩形.min + anchor_max × 父矩形.size + offset_max
```

| 想要的效果 | anchor min / max | offset min / max |
|---|---|---|
| 贴左上角固定尺寸 | `(0,0)` / `(0,0)` | `(x, y)` / `(x+w, y+h)` |
| 居中固定尺寸 | `(0.5,0.5)` / `(0.5,0.5)` | `(-w/2,-h/2)` / `(w/2,h/2)` |
| 四边拉伸（全屏底图） | `(0,0)` / `(1,1)` | `(0,0)` / `(0,0)` |
| 底部通栏 | `(0,1)` / `(1,1)` | `(0,-h)` / `(0,0)` |

Inspector 的 **Preset** 下拉可以直接套这几种（保持当前尺寸）。

**脚本**：23 个 `ui_*` API，完整清单见 [Docs/脚本API.md](Docs/脚本API.md) 4.12。

```lua
function update(ctx)
    if ui_clicked(ctx, self_id(ctx)) then
        ui_set_text(ctx, self_id(ctx), "已点击")
    end
    ui_set_size(ctx, fill_id, 4.0 + 300.0 * pulse, 14.0)   -- 血条
end
```

**颜色**：场景文件里存**线性 RGBA**（与材质 / 灯光一致），
脚本 API 收 **sRGB 0..1**（美术直觉），Inspector 的取色器显示 sRGB。
三者由引擎在边界处转换，别在中途自己再转一次。

**性能**：布局 + 排版 + 合批 900 个元素 / 3190 个字形约 **0.75 ms/帧**；
矩形与字形共用图集，整屏 UI 通常只有 **1 次 draw call**（图片会打断批次）。
场景里没有 UI 元素时整段跳过。

**已知限制**：没有裁剪（子元素溢出父矩形会画到外面）、pivot / 旋转缩放动画、
九宫格、输入框、手柄 / 键盘导航、设计分辨率缩放（Canvas Scaler）。

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
   **Revert** 用资产把当前实例覆盖回去（**保留它自己的位置**，也**保留实例里每个
   实体的 ID** —— 还原是还原内容，身份不换，场景 diff 才干净；资产里新增的节点
   拿新 ID、资产里删掉的节点从场景消失）。

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

### 运行时实例化：`spawn_prefab`

预制体不只能由作者在编辑器里摆 —— 脚本可以随时实例化它，这是子弹 / 敌人 /
掉落物 / 技能特效的唯一入口：

```lua
-- 世界位置（覆盖预制体根自己写的位置，子节点相对位置原样保留）
local e = spawn_prefab(ctx, "Prefabs/Enemy.prefab.ron", 0.0, 1.0, 0.0)
if e ~= nil then set_entity_pos(ctx, e, 4.0, 1.0, 0.0) end

-- 挂在玩家手上的子弹（坐标是局部偏移，之后跟着玩家走）
spawn_prefab_under(ctx, "Prefabs/Bullet.prefab.ron", self_id(ctx), 0.0, 1.5, 0.0)
```

- 整棵子树一次进场景：层级 / 变换 / 物理 / 粒子 / 音频 / 相机 / UI 全带上，
  **每个新实体的刚体由运行时自动补建**，脚本侧不需要额外调用。
- 返回的 ID **立刻可用**（与 `spawn_entity` 同款语义）；失败返回 `nil` 并留一条
  日志（宿主没接装载器 / 文件读不到 / 预制体是空，三种分开说）。
- 用法与参数细节见 [`Docs/脚本API.md`](Docs/脚本API.md) §4.3，取舍见
  `Docs/决策记录.md` §二十七。

## 资产与导入（GUID + `.meta`）

**问题**：场景里如果存的是 `model: "Resources/Box.glb"`，资产一改名 / 一挪目录就断链，
只能等 `--check` 事后发现再手改。引用格式是数据格式，越晚改越贵 —— 所以资产的引用
一律用**稳定身份**。

**做法**（细节见 `Docs/决策记录.md` §十四）：

- 每个资产旁边一份 `<文件名>.meta`（RON 纯文本）：

  ```ron
  (guid: "43b41a2008432f77ca7fbca93854f890", kind: Model, import: (model_scale: 2.0))
  ```

- 场景 / 预制体里存 `guid://43b4…`，**内存里仍然是路径** —— 渲染 / 物理 / 音频 / 脚本
  完全不知道 GUID 的存在。转换只发生在 `ProjectHandle` 的加载 / 保存边界。
- 改名、挪目录：把资产**连同 `.meta`** 一起搬即可，引用自动跟随。
  （`Fox.gltf` 这类带外部 `.bin` 的模型，`.bin` 要一起搬 —— 它不算独立资产。）
- 老项目升级：`--migrate-assets <项目>`。它只替换引用字面量，**不动注释与排版**；
  也可以直接打开 + 保存一次（保存时会自动编码成 GUID）。

**资源浏览器（Assets 面板，与 Prefabs 共页签）**：

- 目录树浏览 `Resources/` / `Scripts/` / `Prefabs/`，按类型着色图标、可搜索；
- 选中看详情：路径、GUID、导入设置；`.mat` 的详情区还有「编辑材质…」和「Assign」——
  前者打开浮动编辑器改颜色 / 金属度 / 粗糙度 / albedo（草稿 + Apply / Revert）；
- **双击**放入场景（落在相机前方）、**拖到视口**放在鼠标射线打到的地方（射线 × y=0 地面）；
  材质拖到实体上 = 指派给那个实体（双击则指派给当前选中实体）；
- 右上角 ＋ = 新建材质、⟳ = 手动重新扫描（给新拷进来的资产补 `.meta`）。外部拷入 /
  修改资产通常不用点：编辑器每秒自动检测一次（见上文「资产 / 脚本自动热重载」）。

**`--check` 现在会报**：悬空 GUID、缺 `.meta`、孤儿 `.meta`、坏 `.meta`、GUID 冲突、
缺失的模型 / 脚本 / 音频 / 预制体 / 材质，以及材质里缺失或解析失败的 albedo 贴图。
它**严格只读** —— 不会顺手生成 `.meta`（生成是编辑器打开项目 / 保存场景时的事）。

**导入设置已被消费**：`.meta` 里的 `model_scale` 烘焙进顶点 / 节点 / 动画轨道 /
逆绑定矩阵，`texture_srgb` / `texture_nearest` 进纹理采样。改完设置后编辑器会
自动丢缓存重载。`audio_streaming` 仍是数据面（流式播放未落地）。

## 脚本（Lua）

实体可以挂 `.lua` 脚本，引擎在固定步长 / 每帧 / 销毁时回调：

```lua
-- Scripts/Jumper.lua
local JUMP_SPEED = 6.5

function update(ctx)
    if not action_pressed(ctx, "Jump") then return end
    local me = self_id(ctx)
    local v = velocity(ctx, me)
    set_velocity(ctx, me, v.x, JUMP_SPEED, v.z)   -- 保留水平速度，只跳
end
```

- 脚本按**项目相对路径**挂在实体上（`Entity::scripts`），所以一份脚本能挂到
  几十个实体，改一次逻辑全部生效
- 四个钩子全部可选：`start` / `update` / `fixed_update` / `on_destroy`
- Play 模式下改完盘上的 `.lua` 会自动重编译（1 秒内），不必退出重进；工具栏
  **⟳ Scripts** 仍可手动触发一次重载
- 脚本出错**不会**拖垮引擎：编译错误、运行时错误、死循环超限、内存超限都只记一条
  日志，这一帧该钩子不生效，游戏继续跑
- 两层沙箱管住失控脚本：**指令预算**挡"算不完"（死循环 20 万条指令内被掐断），
  **内存上限**（128 MiB/VM）挡"存不下"（不断往表里塞东西）。没有内存上限时，
  一个失控脚本的结局是操作系统 OOM 杀进程 —— 编辑器里没保存的场景跟着一起没
- 完整的函数清单、坐标约定、以及两条语义说明
  （顶层 `local` 是脚本级共享、每实体状态要用 `state_*`）见
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

### 手柄与触摸

手柄与键盘**共存**（多对一），插上就能用，不用删键位：

```ron
InputMap(
    actions: {
        "Jump": [Key(Space), PadButton(South)],
        // 扳机当按钮：越过阈值算按下
        "Fire": [Mouse(Left), PadAxis(axis: RightTrigger, threshold: 0.4)],
    },
    axes: {
        // 摇杆走 analog：连续值 + 死区重标定（推到底 = 恰好 1.0）
        "MoveX": (
            positive: [Key(KeyD)],
            negative: [Key(KeyA)],
            analog: [(axis: LeftX, scale: 1.0, deadzone: 0.15)],
        ),
    },
)
```

- 按键名是**物理位置**：`south` / `east` / `west` / `north`（下 / 右 / 左 / 上），
  外加 `left_bumper` / `right_bumper` / `left_trigger` / `right_trigger` / `start` /
  `select` / `left_stick` / `right_stick` / `dpad_*`。Xbox / PlayStation / Switch 的
  字母差异由后端翻译，游戏逻辑里不该写"A 键"。
- 轴的 `+y` 是"上 / 前"（与 `W` 同向）；扳机是 `0..1`。
- 脚本里也能直查：`pad_button_down(ctx, "south")` / `pad_axis(ctx, "left_x")` /
  `pad_connected(ctx)`。
- `input.ron` 里**一条手柄绑定都没有**时，加载后会补上一套默认绑定（Jump = 南键、
  Fire = 右扳机、左摇杆 = 移动、右摇杆 = 视角）；作者写过一条就不再补。
- **触摸**：多点触点可用（`touch_count(ctx)` / `touch(ctx, id)`），并且**第一根手指
  同时驱动鼠标** —— 于是 `ui_*` 那套按钮在触摸屏上直接可点。

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

### 角色控制器

给实体加一行 `controller` 就是 Unity 式的角色（**实体原点是脚底**）：

```ron
controller: Some((radius: 0.3, height: 1.8, step_offset: 0.35, slope_limit: 45.0)),
```

```lua
function fixed_update(ctx)
    character_simple_move(ctx, axis(ctx, "MoveX") * 5.0, 0.0, axis(ctx, "MoveZ") * 5.0)
    if action_pressed(ctx, "Jump") then character_jump(ctx, 5.0) end
end
```

- 撞墙滑行、**自动上台阶**（0.35m）、45° 坡度限制、**陡坡滑落**、贴地吸附、
  跳跃与落地钳制；`character_is_grounded` / `character_ground_normal` 可查状态
- 碰撞事件进两轨脚本：`on_collision_enter(ctx, other)` / `on_collision_exit(ctx, other)`，
  **双方各收一次**，持续接触不刷屏（角色的接触面由控制器自己上报 —— 胶囊停在
  skin 距离外，物理引擎不会发事件）
- 编辑器 Inspector 有 **Character** 区块；与 `physics` 正交（角色实体的刚体由运行时托管）

### 粒子

给实体加一行 `particle` 就是一组发射器（**CPU 模拟 + billboard 渲染**）：

```ron
particle: Some((
    rate: 60.0,                     // 颗/秒
    max_particles: 512,             // 硬顶（满了丢欠账）
    lifetime: (0.6, 1.2),           // 寿命区间（秒）
    speed: (1.0, 2.2),
    direction: (0.0, 1.0, 0.0),     // 锥轴
    spread: 18.0,                   // 锥半角（度）
    shape: Sphere(radius: 0.15),    // Point / Sphere / Box
    gravity: (0.0, -1.2, 0.0),      // 粒子自己的重力，与物理世界无关
    start_color: (1.0, 0.55, 0.12, 0.9),
    end_color: (0.35, 0.05, 0.0, 0.0),
    blend: Additive,                // Alpha / Additive
    seed: 7,                        // 同种子 = 逐位可复现
)),
```

- **进 Play 才喷**：编辑态不模拟（Play 就是那条编辑器内模拟回路），场景存盘
  只存参数、不存粒子状态
- 确定性模拟：同种子 + 同 dt 序列 ⇒ 逐位相同的粒子流 —— 无头截图与回归测试
  的地基；每发射器一把独立 RNG，两个 `seed: 0` 的实体也喷得不一样
- 出生点在实体世界变换下、出生后活在**世界空间**（实体移动不拖着粒子走）；
  实体删除 ⇒ 粒子立即消失
- Inspector 有 **Particle** 区块（Fire / Smoke / Sparks 预设）；脚本侧
  `particle_burst` / `set_particle_emitting` / `set_particle_rate` 控制运行期行为
- 两种混合都走实例化 billboard（一次 draw 画完同混合的全部粒子）：深度测试开、
  深度写关 —— 粒子被几何遮住、彼此不互相遮挡

```lua
function on_collision_enter(ctx, other)
    particle_burst(ctx, self_id(ctx), 30)  -- 撞一下炸一团火花
end
```

### 相机

给实体加一行 `camera` 就是一台相机（**相机是普通实体**，能被父节点带着走）：

```ron
camera: Some((fov: 60.0, near: 0.1, far: 1000.0, active: true)),
```

- **本地方向 +Z 是镜头朝向**（与 glTF 的模型正面、`forward()` 一致）。宽高比
  不在组件里 —— 它属于视口（编辑器视口 / 游戏窗口 / 截图尺寸各不相同）
- **主相机只有一个入口**：`active` 优先，全都没勾就退回场景里第一台，
  一台都没有则宿主退回各自的兜底机位（老场景零影响）
- 三个宿主读同一份解析：独立游戏、编辑器 **Play 态视口**（Unity 的 Game 视图语义，
  此时自由相机让位、HUD 角标变成 `GameCam`）、无头 `--screenshot`（报告里
  `screenshot.camera.mode` 说明是 `explicit` / `scene` / `auto` 哪一种）
- Inspector 有 **Camera** 区块（FOV / Near / Far / Active）；脚本侧
  `look_at` / `look_at_entity` / `active_camera` / `set_active_camera` /
  `camera_fov` / `set_camera_fov`
- 样例：`projects/SampleScene` 的 "Main Camera" 由 `Scripts/CameraFollow.lua`
  每帧摆到狐狸前侧上方并看向它 —— 不做"绕到背后"是因为场景正中有箱塔会挡视线

```lua
function update(ctx)
    local t = entity_id(ctx, "Fox")
    local p = entity_pos(ctx, t)
    set_pos(ctx, p.x + 4.2, p.y + 2.8, p.z + 3.6)
    look_at(ctx, p.x, p.y + 1.0, p.z)   -- 只改朝向，不改位置
end
```

## 运行时（编辑器和游戏共用的那一段）

`xgame-runtime::World` 是唯一编排者，编辑器按下 ▶ Play 与打包出去的游戏跑的是
**同一段** `update`：

```text
clock.tick(dt)
  ├─ dispatch_start                 新实体跑一次 start
  └─ 固定步长循环（与渲染帧率解耦）
       ├─ dispatch_fixed_update → 补/清刚体 → 推运动学体 → physics.step → 回写动态体
  ├─ dispatch_update                每渲染帧一次
  ├─ particles.step(dt)             CPU 粒子推进（脚本本帧写的 burst 当帧炸出）
  ├─ sync_audio → flush_audio       帧末音频结算（空间声推位置 / 命令交给引擎）
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
| 引擎日志 | 控制台队列有界（1024 条，丢最旧并在标题栏显示「+N 略」）；写盘是 `write_all` 不做 fsync（进程崩溃够用，掉电不承诺）；关键词过滤按字节匹配、每帧只转一次小写，逐条 `to_lowercase()` 会变成 500 次分配/帧 |
| 编辑器空闲 | **空闲时不做渲染重绘**；唯一定时唤醒是自动保存与 1 Hz 内容扫描（只查 metadata、不读文件内容，也不触发视口重画）。Windows 在指针静止时会反复投递同一坐标的 `WM_MOUSEMOVE`，而 egui 把"本帧有输入"当作"要立刻再画一帧"，于是编辑器会一直 60fps 空转、拖窗口时跟模态循环抢消息线程。现在位置未变的 `PointerMoved` 直接丢掉（语义 no-op）；**原始位移 `Event::MouseMoved` 整类丢弃**（光电鼠标传感器噪声以 ±1~2px 灌进来会续命帧调度宽限期，编辑器无人消费原始 delta） |
| 编辑器帧调度 | `frame_policy.rs` 集中决策「要不要连续出帧」：**Hybrid**（默认，`XGAME_FRAME_POLICY` 可切 `always`/`reactive`）—— 交互中 / 刚输入过 220ms 内满速出帧，其余停帧。拖拽阶段与永远满速同手感（~60fps、p99≈35ms），纯事件驱动的旧实现拖拽中有 30% 死时间（操作发涩的根源）。全工程只此一处主动 `request_repaint` |
| 编辑器缩放 | 拖边框时**节流重建**离屏目标（≤10Hz），尺寸一稳定立刻按最终尺寸重建。实测一次拖拽重建次数 43 → 1 |
| 渲染实例化 | 同图元（网格指针相同）合并成一次实例化 draw，逐实例数据走 storage buffer；2000 实体同网格场景 **2001 批 → 2 批**、CPU 2225 → 344 µs/帧（-84%） |
| 视锥剔除 | 实体级世界 AABB × 六个归一化视锥平面；相机外但影子会进画面的投影物只跑阴影 pass；2000 实体窄视场未合批场景 **2000 → 868 批**、CPU 3295 → 1146 µs/帧（-65%） |
| 存档 | 读档按 `EntityId` **一次建索引**再匹配（`Scene::find_entity` 是线性查找，逐条调就是 O(n²)：8000 实体 50 ms → 1.7 ms）；写盘走 `fsx` 原子写含 fsync，2000 实体 1–12 ms —— 手动存档量级，**不适合每帧调** |
| 游戏 UI | 布局 + 文本排版 + 合批是**每帧恒定**成本（不像 3D 有指纹门控可跳过）：900 元素 / 3190 字形 **753 µs/帧**（60fps 预算 4.5%）。矩形与字形共用一条管线 + 同一张图集，**整屏 1 次 draw call**；没有 UI 实体时一次 `any` 扫描就整段跳过 |

> 渲染侧的两条纪律：剔除**开 / 关画面逐字节一致**（宁可白画不能黑屏），合批的批次
> 顺序 = 首次遇到顺序（同机截图逐字节确定的前提）。细节见 `PLAN.md` 二十八 / 三十。

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

**2026-10-29 追加：`rapier3d` 打开 `simd-stable`**（i7-10700，同机背靠背各跑 3 次取中位）：

| 场景（2000 实体 / 300 帧） | 关 SIMD | 开 SIMD | 变化 |
|---|---|---|---|
| 纯视觉（无物理） | 49.0 µs/帧 | 49.9 µs/帧 | 无（不参与物理，属噪声） |
| 全静态体 | 124.0 µs/帧 | 126.0 µs/帧 | 无（噪声） |
| 稀疏动态 ×64 | 815.2 µs/帧 | **518.5 µs/帧** | **-36%** |
| 密集接触 ×1000 | 14704.5 µs/帧 | **13753.0 µs/帧** | **-6.5%** |

三组数据是**全部 3 次都更低**（不是靠一次好运），判据见 `PLAN.md` 八十七。
代价：浮点求值顺序变了 → 物理轨迹与旧版本有微小数值差异；**逐位确定性门禁不受影响**
（它比的是同一个二进制跑两遍）。

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

`xgame-mcp` 是本引擎的 **MCP server** —— 把"读说明书、读脚本契约、读项目、改场景、
动实体、跑检查、无头实跑"暴露成 10 个标准 MCP 工具，Cursor / Claude 等任何 MCP
客户端接上就能**不用鼠标直接操纵引擎**做游戏开发。无窗口、无 GPU，stdio 起服务
（按行 JSON-RPC）。

```json
{ "mcpServers": { "xgame": { "command": "D:\\...\\dist\\xgame-mcp.exe" } } }
```

| 工具 | 干什么 |
|---|---|
| `schema_read` | 读场景格式的机器可读说明书（36 个落盘类型 / 字段 / 默认值 / 示例 / 资产引用；与 `--export-schema` 同源，不需要项目） |
| `script_api_read` | 读脚本 API 的机器可读清单（178 个函数 / 签名 / 参数 / 返回值 / 6 个钩子 / 约定；与 `--export-script-api` 同源，不需要项目） |
| `project_info` | 读清单概要：入口场景、窗口/重力/步长、场景/脚本/预制体清单 |
| `scene_read` | 读场景的完整 RON 文本（与文件同构，可原样写回） |
| `scene_write` | 整体替换场景；**先解析后落盘**，坏数据一个字节不写 |
| `entity_upsert` | 按名字创建/部分更新实体（变换、模型、脚本、父子） |
| `entity_remove` | 按名字删除实体（重名歧义拒绝；子节点提升不连坐） |
| `project_check` | 无头验证（与 `--check` 同一份逻辑），返回 JSON 报告 |
| `asset_list` | 读项目资产树（路径 / 类型 / GUID / 导入参数 / 资产库诊断） |
| `asset_import` | 给单个资产建 `.meta` 并（可选）写导入参数；部分更新、越界钳制、幂等 |
| `asset_move` | 移动 / 改名资产，`.meta` 跟着走、**GUID 不变**（`guid://` 引用天然不断链） |
| `run_headless` | 无头跑 N 帧（与 `--headless` 同一实现），返回统计报告 + 可选场景快照 + 可选截图（MCP 图片块） |

改完先 `project_check` 自证没改坏、再 `run_headless` 看真的跑得动 —— 这是智能体的
「改 → 查 → 验」闭环。完整协议面、参数语义与已知边界（编辑器并发写、清单只读）见
[`Docs/MCP.md`](Docs/MCP.md)，无头运行的报告字段见 [`Docs/无头运行.md`](Docs/无头运行.md)。

## 依赖方向

不许反向依赖（看见 `render` 依赖 `runtime` 就是设计错了）：

```
xgame-core     核心类型：Color / Config / Transform / Scene / Entity / GameClock / AssetId
    ↑
xgame-math     glam 重导出 + projection 辅助
    ↑
xgame-render   wgpu 管线、glTF、PostFx（HDR / ACES / Bloom / FXAA / TAA）、天空 IBL、实例化与剔除
xgame-physics  rapier3d 刚体、碰撞体、物理世界、射线、调试绘制
    ↑
xgame-input    键鼠状态 + 动作映射（与 winit 解耦；落盘走 core::fsx）
xgame-script   Lua 宿主 + GameCtx 方法层（不依赖渲染 / 编辑器）
xgame-audio    kira 封装（设备 / 空间轨道 / 解码缓存 / silent 降级）
    ↑
xgame-runtime  World：场景 / 物理 / 输入 / 脚本 / 音频编排成一条帧管线
    ↑
xgame          门面 crate：prelude + Lifecycle / Inspect + 插件边界（export_plugin!）
    ↑
xgame-editor   egui 面板、SceneRenderer（ScenePipeline wrapper）、GameRuntime（Play 宿主）
xgame-game     独立游戏二进制（与编辑器跑同一段 World::update）
xgame-mcp      MCP server（stdio 按行 JSON-RPC；只依赖 core，无窗口无 GPU）
```

三个宿主（编辑器 / 独立游戏 / MCP）都只是入口，运行时逻辑只在 `xgame-runtime` 一处。
