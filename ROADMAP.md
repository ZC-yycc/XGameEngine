# XGameEngine 开发计划

> Rust 实现的类 Unity 游戏引擎。**本文只记阶段与状态** —— 为什么这样做看 `PLAN.md`，
> 能力差距与优先级看 [`Docs/差距分析.md`](Docs/差距分析.md)，不可推翻的约束看
> [`Docs/决策记录.md`](Docs/决策记录.md)。项目结构、运行命令、各功能怎么用统一在
> [`README.md`](README.md)，本文不再复制。

---

## 整体路线图

### 2026-10-30 三模块：阴影逐级剔除 / 手柄输入 / 正交投影（P1 #15、P2 #28 关闭）

| 项目 | 状态 |
|---|---|
| 阴影逐级剔除 | 每实体算**级联成员位**，批次内实例按 `(批, 成员位)` 分段，阴影 pass 每级只提交成员位带自己的段。实测（`shadow_cost` 新基准，2000 方块 / 4 级）：阴影像素实例 **8000 → 1131（-85.9%）**、场景段 GPU **165.3 → 128.9 µs（-22.0%）**；代价 CPU +40 µs（≈20 ns/实体）。见 PLAN 九十三 / 决策记录 §七十七 |
| 手柄 + 触摸（P1 #15 关闭） | **手柄**：语义按键（物理位置）/ 扳机当按钮 / 摇杆模拟轴（死区重标定）/ `GamepadHub`（`gilrs`，feature 门禁，编辑器与独立游戏各一份）/ 老项目自动补默认绑定。**触摸**：多点触点四相位 + **首点驱动鼠标**（`ui_*` 直接可点）+ winit `WindowEvent::Touch` 接线。脚本 5 个 API（总数 173 → **178**）。**未验证真机**（本机无设备）。见 PLAN 九十三 / 决策记录 §七十八 |
| 正交投影 | 编辑器视口相机 `5` 键切正交（命令面板同款条目）：可见高度 = `ortho_height`（滚轮指数缩放）、裁剪 ±1000、F 聚焦改高度、拾取零改动（平行射线）。`[` / `]` 补速度快捷键。`--ui-ortho` 截图开关。**仍缺**：视口锁定。见 PLAN 九十三 / 决策记录 §七十九 |
| 验证 | 全量 **1140 passed / 0 failed / 15 ignored**（-j 2 + `--test-threads=2`），clippy 0 告警；`--ui-shot` 1280 / 1440 / 1920 三档 + 正交截图；四示例项目 `--check`=0 |


### 2026-10-29 质量冲刺：文本排版缓存 + UI 裁剪 + 命令面板 + SSAO（P2 #18 / #24 关闭）

| 项目 | 状态 |
|---|---|
| M1 文本排版缓存 | `xgame-ui/text.rs`：键 = 内容 + face + 字号量化 + 换行宽度（**对齐不进键**），命中后逐字节校验；1024 条按 tick 淘汰。`Painter::build` 图集翻倍时重跑合成修 UV。实测 `ui_cost` **759.4 → 264.6 µs/帧（-65%）** |
| M2 UI 裁剪 | `UiElement.clip_children`（只裁后代、层级交集继承、GPU scissor 矩形、命中测试同步约束、false 不落盘）；绘制批按 clip 切分。截图对照验证 + `ui_cost` 复测 **274.5 µs/帧**（+3.7% 于 M1，对基线 -64%） |
| M3 命令面板 | Ctrl+Shift+P / F1：模糊搜索 + ↑↓/Enter/Esc，**38 条命令**（全部是既有动作入口的转发）；`--ui-command` 截图开关（不聚焦保逐字节确定），1280/1440/1920 三档自检 |
| M4 SSAO | 半分辨率深度 SSAO（`render.ssao` 节，默认关、不落盘）：AO 生成 + 可分离模糊 + 乘回场景色；**确定性噪声**（像素哈希 + 固定种子 kernel），`strength=0` 与关着逐位一致。实测四趟 **+116.0 µs/帧**（1280×720 / RTX 2060） |
| 验证 | 新增 24 条测试 + 1 条基准（`ssao_cost`，默认 ignore）；全量 **1112 passed / 0 failed / 14 ignored**，clippy 0 告警；见 PLAN 九十二 / 决策记录 §七十三~§七十六 |
| 仍缺 | SSAO：G-buffer 法线（当前深度梯度重建）；裁剪：圆角裁剪 / 九宫格 / 输入框；命令面板：快捷键自定义 |


### 2026-10-29 质量档位（P2 #23 关闭）

| 项目 | 状态 |
|---|---|
| 档位 | `project.ron` 的 `quality` 节：`preset: Low/Medium/High` + 显式覆盖（`msaa` / `shadow_resolution` / `lod_scale`）。全空不落盘 = 今天的行为 |
| 参数 | 低 1×/1024/0.6、中 4×/2048/1.0（= 今天）、高 4×/4096/1.25；采样数**只有 1×/4×**（2×/8× 非 wgpu 保证档，实测 2× 直接管线创建失败） |
| 语义 | 只管"机器向"三颗螺丝，不碰曝光 / 色调映射 / 天空 / Bloom / FXAA / TAA（画面风格归作者）；灯写了 `shadow_resolution` 仍以灯为准；LOD 阈值 = `render.lod_bias × quality.lod_scale` |
| 接线 | `QualityConfig::resolve` 纯函数，编辑器 / 游戏 / 无头截图三宿主同源；MSAA 启动期定（运行期换档要重建管线，不做） |
| 实测 | `quality_cost`（2000 方块 + 4 级 CSM）：场景段 GPU 低 **102.2** / 中 **156.1** / 高 **223.2** µs（三档各 8 轮区间不重叠） |
| 验证 | 红灯（`resolve` 忽略档位）实证；新增 7 条测试；全量 **1088 passed / 13 ignored**，clippy 0 告警；见 PLAN 第九十一节 |
| 仍缺 | 运行期换档（要重建管线）；编辑器档位下拉；阴影距离进档位 |


### 2026-10-29 音频总线 + 默认主音量修复（P2 #29 的分组音量那一半）

| 项目 | 状态 |
|---|---|
| 总线 | `AudioBus`（`Sfx` 默认 / `Bgm` / `Voice`，固定枚举）+ `AudioMix`（主 × 组）；有效音量 = 声源 × 总线（唯一一份乘法），主音量在 kira 主轨上 |
| 行为 | `set_bus_volume` **立即作用于该组已在响**的声音（设置菜单滑条语义）；总线音量变了才逐句柄重设音量（给句柄动画留位置）；未知名字告警并忽略，不悄悄归 Sfx |
| 落盘 | `AudioSource.bus` 缺省 `Sfx` 不落盘（老场景逐字节不变）；Inspector 有 Bus 下拉 |
| 脚本 | 4 个新 API（`play_sound_bus` / `play_sound_at_bus` / `set_bus_volume` / `bus_volume`）；总数 169 → **173**、类别仍 18；`Docs/脚本API.md` + 6 处计数断言同步 |
| 缺陷修复 | **默认主音量 0 = 全局静音**（`AudioPort` 派生 `Default` 的 `f32` 零默认）→ `AudioMix::default()` = 全 1.0，红灯钉住 |
| 验证 | 红灯两处（默认混音退回 0 / 有效音量不乘总线）实证；新增 6 条测试；全量 **1081 passed / 12 ignored**，clippy 0 告警；见 PLAN 第九十节 |
| 仍缺 | 流式长音频、遮挡低通 / 混响（有意延后）；淡入淡出 |


### 2026-10-29 LOD：距离分级 + 绘制距离（P2 #16 的 LOD 那一半）

| 项目 | 状态 |
|---|---|
| 数据 | `Entity.lod`（`LodGroup` / `LodLevel`）：逐级模型 + `max_distance`；`levels` 非空时 `model` 被忽略（第 0 级 = 最近一级）；超出全部有限级 = 不画（绘制距离），最后一级写 0 = 无限远 |
| 判定 | `core::lod` 唯一一份：距离 = 相机到实体**轴心**；"最小覆盖者"与书写顺序无关；`Project.render.lod_bias` 乘阈值（`>1` 推远 / `<1` 拉近 / `<= 0` 关掉 LOD） |
| 渲染 | `DrawBatch::lod_level` 记级号（不同级是不同网格）；距离剔除的实体**不投影**；`FrameStats` 增 `distance_culled_entities` / `low_lod_instances` |
| 资产 / 检查 | LOD 各级模型进 GUID 管线（改名不断链）；`--check` 逐级点名（`(lod[i])`）；schema 增 `LodGroup` / `LodLevel`（38 个类型） |
| 编辑器 | Inspector LOD 区块（逐级模型 + 阈值 + 增删）；逐级预加载；拾取 / 聚焦 / 动画查询走第 0 级 |
| 性能 | 2000 实体（1345 在视锥内）：场景段 GPU **685.9 → 101.1 µs（-85.3%）**，顶点量 1,650,000 → 81,942（**-95.0%**）；CPU 段噪声级。基准 `lod_cost`（默认 ignore） |
| 验证 | 红灯两处（不按距离分级 / 批次不记级号）实证；新增 12 条测试；全量 **1075 passed / 12 ignored**，clippy 0 告警；见 PLAN 第八十九节 |
| 仍缺 | 遮挡剔除（P2 #16 的另一半）；LOD 淡入淡出（会破坏静态帧指纹）；自动减面 |


### 2026-10-29 存档系统 + 运行时读路径门禁 + 读档去 O(n²)

| 项目 | 状态 |
|---|---|
| 存档（P2 #32） | `core::savegame`：`Saves/<slot>.sav`（RON、纯文本、可 diff）、原子写、槽名过路径规则、写前体检（版本 / 有限数 / 单位四元数）、实体按 ID 排序 ⇒ 同状态逐字节相同；runtime `capture_save` / `apply_save` / `save_game` / `load_game` + `SaveVars`；脚本 9 个 API（`save_game` / `load_game` / `has_save` / `save_slots` / `save_set` / `save_get` / `save_remove` / `entity_save_set` / `entity_save_get`） |
| 存档语义 | 存局部变换 + 刚体速度 + 全局 / 每实体变量；**不存** `state_*`、时钟、运行时生成物；读档**替换**不合并、对不上的实体跳过并计数、存档场景不同先切场景；实体变量跟实体走（切场景 / 销毁 / 读档后只留当前场景存在的 ID） |
| 读路径门禁（194 残留） | `core::resolve_read_path`：脚本与音频的读取先过 `normalize_project_relative`，`../..` / 盘符 / `a:b` / `CON` 在**读取处**就拒；`--check` 之外不跑检查的宿主同样挡住 |
| `search_paths` 语义 | 定死为"读的兜底根"（有序、条目自身也要合规、不合规运行时跳过且 `--check` 报进 `unsafe_paths`、只影响读不影响写）；`World` 给 `LuaHost` 与 `AudioEngine` 注入同一份列表 |
| 性能 | 读档实体查找从 O(n²) 改一次建索引：**8000 实体 50.0 ms → 1.7 ms（-96.5%）**、2000 实体 3.4 ms → 0.32 ms（-91%）。新基准 `savegame_cost`（200/2000/8000 三档）；`update_throughput` 无回归（50.5 / 127.4 / 517.9 / 13170 µs） |
| 顺带量清 | 新增 `ui_layout_cost`：1001 元素 HUD 的**运行时侧**（建树 36.2 + 解矩形 11.0 + 命中测试）合计 83.1 µs/帧；文本排版与合批在 render 侧，本轮未量。物理仍是最大项 |
| 验证 | 路径门禁 4 条先红后绿（退回 `root.join` 实跑）；角色状态复位先红后绿；存档 18 条语义测试（core 8 + runtime 10，含**不动点**：capture→存→改乱→读→capture 逐字段一致）+ 读路径 6 条；全量 **1063 passed / 11 ignored**，clippy 0 告警；见 PLAN 第八十八节 |
| 边界 | 不存生成物 / `state_*` / 时钟；不自动存档、无存档点触发器；不做跨版本迁移；写盘含 fsync（2000 实体 1–12 ms，受杀毒影响） |
| 仍缺 | 存档回放 / 生成物复原（需要实体原型注册表）；存档 UI（编辑器里的"存档"面板）；自动存档与存档点 |

### 2026-10-29 物理打开 `simd-stable`（先量后改，唯一的真瓶颈）

| 项目 | 状态 |
|---|---|
| 先量 | 四档基准各跑 3 次取中位：纯视觉 49.0 / 全静态 124.0 / 稀疏动态 815.2 / 密集接触 14704.5 µs/帧；渲染剔除另测只占 40–80 µs（不是瓶颈）→ **唯一真瓶颈是 rapier 求解器** |
| 改动 | `rapier3d` 加 `features = ["simd-stable"]`（一行，不碰引擎代码） |
| 结果 | 稀疏动态 **815.2 → 518.5 µs/帧（-36%）**、密集接触 **14704.5 → 13753.0（-6.5%）**；纯视觉 / 全静态无变化（不参与物理，说明没有副作用）。两组都是 3 次**全部更低**、区间不重叠 |
| 代价 | 浮点求值顺序变了 → 跨版本物理轨迹有微小差异（同版本逐位确定性门禁不受影响） |
| 刻意不做 | 编排层"五次全实体扫描"不优化（24 ns/实体、两万 FPS 上限；换索引 = 引入一类"索引过期"新 bug）；`parallel` 特性不启用（打乱体间顺序，与逐位确定性冲突）；见决策记录 §六十七 |
| 验证 | 全量 **1034 passed / 0 failed / 9 ignored**，clippy 0 告警；release 与 dist 刷新 + 冒烟；见 PLAN 第八十七节 |

### 2026-10-29 写读稳定性门禁 + 无改动不写盘

| 项目 | 状态 |
|---|---|
| 门禁 | `crates/xgame-core/tests/scene_roundtrip.rs`（5 条，进 CI）：40 个随机场景 / 预制体覆盖 `Entity` 全部字段（名字带引号 / 反斜杠 / 换行 / emoji）；坏数据（重复 ID / 悬空父 / 自指 / 成环 / 越界 UI·粒子·morph）必须装得进来且夹取**幂等**；四个示例工程打开→存→存第二轮逐字节不变 |
| 不变量 | `save → load → save` **从第二轮起**逐字节不变；`l1 == l2` 逐字段相等 |
| 顺带修 | `write_scene` / `write_prefab` 在待写文本与盘上逐字节相同时**不写盘** —— mtime 是热更与热重载的判据，白刷一次等于凭空触发一次重载（红灯实证：关掉这行 → mtime 差 410 µs） |
| 查清 | **保存会吃掉注释**（示例工程一次少 7 行教程注释）：重新序列化的必然，想保注释只有 `--migrate-assets` 的词法路径，已写进决策记录 §六十六，不是 bug |
| 验证 | clippy 0 告警；全量 **1034 passed / 0 failed / 9 ignored**（31 套件）；门禁自身 13 s；见 PLAN 第八十六节 |
| 边界 | 门禁不覆盖渲染 / 物理状态的持久化（没有存档系统）；注释保留刻意不做 |

### 2026-10-29 路径纪律：一条规则、两个边界、一套诊断（194 关闭）

| 项目 | 状态 |
|---|---|
| 规则统一 | 场景 / 资产 / 预制体 / 清单内容目录共用 `core::project::normalize_project_relative`：越界（绝对 / 盘符 / `..`）与 Windows 不认的写法（冒号 / 组件结尾空格与点 / 控制字符 / 保留设备名 `CON`·`NUL`·`COM1`…）一律拒绝；**删掉构建器里第四份手写规则** |
| 两个边界 | `load_scene` / `write_scene` / `load_prefab` / `write_prefab` 在**碰盘之前**校验；`write_scene` 报 `ProjectError::InvalidPath`（与 `Write` 分开） |
| 新增诊断 | `--check` 的 `unsafe_paths`：九类路径引用（model / material / albedo / script / audio / ui image / prefab）+ 清单内容目录，**算问题**（退出码 2）且进 `reference_issues()`；越界时不再重复报"文件缺失"；原因标签纯 ASCII |
| 真实战果 | 改造前 `write_scene("Scenes/Level:1.ron")` 返回 `Ok` —— 内容进了 NTFS **备用数据流**：`list_scenes` 看不见它、git 存不下流内容，保存却报成功 |
| 验证 | 5 条旧实现红灯 + 1 条编辑器红灯；全量 **1028 passed / 0 failed / 9 ignored**；四示例项目 `--check` 全 0；三组 CLI 负对照（越界引用 exit 2 / 清单目录 exit 2 / 入口场景 exit 1）；1280 与 1920 `--ui-shot`；见 PLAN 第八十五节 |
| 仍缺 | 运行时对脚本 / 音频引用越界读取的校验（`--check` 已点名）；`search_paths` 语义未定，未纳入 |

### 2026-10-28 语义写前校验 + 欧拉约定统一（194 部分）

| 项目 | 状态 |
|---|---|
| 旋转约定 | 三处各写一份欧拉换算（Inspector / 脚本 `YXZ`、MCP **`XYZ`**）；**单轴恰好同解**所以测试一直绿，混轴 `(30,45,60)` 姿态差 22.7°。现只有 `core::{euler_deg_to_quat, quat_to_euler_deg}` 一处，顺序固定 `YXZ` |
| 语义闸门 | `validate::scene_defects` 覆盖七类：重复 ID / 悬空父 / 自指父 / 层级环 / 非有限变换 / 非有限光源 / 非单位四元数。MCP `scene_write` 非空即拒绝且不落盘；`--check` 补三类新诊断并进退出码 2 |
| 刻意不做 | 编辑器保存 / 自动保存**不拦**（拒绝保存 = 用户改不动也救不回）；不自动修复（会悄悄改写作者数据）；预制体文件不体检 |
| 真实战果 | `SampleScene` 的 `Crate.C` 是手写近似四元数 `(0, 0.3, 0, 0.95)`，`|q|² = 0.9925` → 木箱一直小 0.38%，旧检查全都看不见；已归一化 |
| 验证 | 红绿四条实跑 + 真实 CLI 正负对照（注入层级环退出 2 并点名）；四示例项目 `--check` 退出 0；全量 **1024 passed / 0 failed / 9 ignored**；见 PLAN 第八十四节 |
| 场景路径 | MCP 的 `scene` 参数不再自己写松检查（原来放行 `./a.ron` / `a.prefab.ron` / `a.RON`），改成补后缀后走 `core::normalize_scene_relative`，与编辑器 / `--check` / 构建器同一条规则 |
| 仍缺 | 194 的资产路径规则（更严：冒号 / 结尾空格与点 / Windows 非法字符）未与场景合并 |

### 2026-10-28 视口导航：F 聚焦与数字键对轴（P2 #28 部分）

| 项目 | 状态 |
|---|---|
| F 聚焦 | 选中子树 / 全场景的世界 AABB 并集 → 包围球装框；**只动位置不改朝向**；半径下限 0.25、边距 1.15；没可见体给 warning 不静默 |
| 数字键 | `1`/`3`/`7` = 前/右/顶，`Ctrl` 取反；只转朝向不动位置（先对轴再 F 是常用组合）；**不是正交投影**（管线只有透视） |
| 防 NaN | `PITCH_LIMIT = π/2 − 0.01`：正对轴时 `forward × Y` 退化 → `pan_right` 算出 NaN 毁掉相机位置 |
| 快捷键防护 | 导航键在算 view_proj **之前**处理（同帧生效）；视口/层级快捷键先过 `wants_keyboard_input`（Search 框里打 f 不再飞相机） |
| 测试 | 7 条新增（fit_distance ×3 / focus_on / 轴视角退化 / 端到端按键 ×2），**994 passed / 0 failed / 9 ignored** |
| 仍缺 | 真·正交投影相机、视口锁定、速度快捷键（滚轮 + Preferences 已可调） |

### 2026-10-28 编辑器日志落盘 + 控制台增强（27 关闭 / 26 部分）

| 项目 | 状态 |
|---|---|
| 问题 | `dist` 的 exe 双击就能跑、用户没有终端，而引擎所有 `log::warn!` 只写 stderr → 出问题时我们手上**没有现场** |
| 三去处 | 一个 `log::Log` 后端：stderr（`RUST_LOG` 语义不变）+ 控制台有界队列（1024 条）+ `%APPDATA%\XGameEngine\logs\Editor.log`（追加写、4 MiB 轮转、只留一份 `.1`） |
| 收信策略 | 自家 `info` 与所有人 `warn/error` 收；第三方 `info` 与所有 `debug/trace` 不收（否则一次 `RUST_LOG=trace` 能写满盘）。**落档的网比 stderr 宽** |
| 面板 | 等级开关（Info/Warn/Error/OK）+ 关键词过滤（零分配匹配）；筛选只影响显示、`Clear` 才清；队列溢出计数；「Log 目录」一键打开 |
| 排版教训 | 右侧一栏**不能**用默认 `Layout::right_to_left`（`main_wrap` 默认 true，1280 实测折行）→ 固定一列 + 关 `main_wrap` |
| 验证 | 新增 12 条测试（轮转 / 队列上界 / 策略表 / 筛选 / 落盘降级）；全量 **987 passed / 0 failed / 9 ignored**；三档 `--ui-shot` 逐像素稳定；实跑 `--check`、`--headless`、`--build` 均生成日志且退出 0；见 PLAN 第八十二节 |
| 边界 | 强杀时最后一行可能丢（不 fsync）；轮转只留一份、不做审计级保留；点击溯源（跳到出错资产/脚本行）仍未做 |

### 2026-10-28 深链销毁与卸载期 spawn（R11/121 的两条剩余，补覆盖）

| 项目 | 状态 |
|---|---|
| 契约一 | 8 环深链里每环**恰好**一次 `on_destroy`、顺序不乱（丢一环 = 状态表不释放；重复 = 逻辑跑两遍；乱序 = `state_*` 对不上） |
| 契约二 | `unload_scene` 期间 spawn 的实体只属于**正在被拆掉的旧世界**，不许漏进下一场景 |
| 验证 | 两条均为**补覆盖**（行为本来就对，无红灯）；卸载测试先断言钩子真跑过（`dying:spawn`）避免假绿；全量 **975 passed / 0 failed / 9 ignored**；见 PLAN 第八十一节 |
| 边界 | 真·异常退出（进程被杀 / 断电）仍未验证 |

### 2026-10-28 脚本内存上限（124 的记忆配额那一半）

| 项目 | 状态 |
|---|---|
| 契约 | 每 VM 128 MiB 内存上限（`lua.set_memory_limit`）。沙箱两半：指令预算挡"算不完"，内存上限挡"存不下" |
| 为什么 | 没有上限时，失控脚本的结局是**操作系统 OOM 杀进程**，编辑器里未保存的场景一起没；脚本是用户内容，不能靠作者自律 |
| 行为 | 超限 → `MemoryError` 翻成「脚本内存超限（上限 N MiB）」；**只毁掉那一次调用**，下一帧照常跑；设不上限只 `log::warn!` |
| 验证 | 注释掉上限（旧行为）两条红灯实跑（分配直接过去、无任何错误）；恢复后绿；全量 **973 passed / 0 failed / 9 ignored**；见 PLAN 第八十节 |
| 边界 | 配额是整 VM 一档（不区分哪个脚本吃多了）；单脚本配额、宿主调用成本、项目级可调未做 |

### 2026-10-28 打包必须验自包含（R01/R02 的"清单外依赖"）

| 项目 | 状态 |
|---|---|
| 契约 | 构建只复制清单里的内容目录；**发布前必须验包自包含** —— 对包与源码工程各跑 `check_project`，取引用类问题的**差集**，非空即失败（判据 = 构建有没有引入新问题，早就存在的坏引用不挡构建） |
| 补的缺口 | ① `build.scenes` 里每关的父目录自动进包（原来只带入入口场景的父目录）；② 清单外部引用（预制体/脚本在自定义目录）由 `audit_package` 兜住 |
| 报错 | `guid://…` 翻回源码工程路径（`explain_guids`），否则作者拿着 GUID 无从下手；自检在 `publish` 之前，失败时上一版发布不动 |
| 验证 | 两条旧行为红灯 + 一条防过拦护栏；真实 CLI 正负对照（`--check`=0 / `--build` 退出 1 并点名路径）；构建产物直接跑 60 帧无头 ok；全量 **971 passed / 0 failed / 9 ignored**；见 PLAN 第七十九节 |
| 边界 | 包内真实窗口 + 键鼠输入的玩家端到端、并发发布、断电事务仍未关闭 |

### 2026-10-28 预制体 Revert 保留实例 ID（R07/103 的 ID 那一半）

| 项目 | 状态 |
|---|---|
| 契约 | Revert **保留实例里已有实体的 ID**（按 `source_id` 认回）：资产新增→新 ID，资产删掉→实体消失。删了重建 ≠ 还原 |
| 入口 | `Prefab::instantiate_with(source, id_for)` 决定"ID 从哪来"；`Scene::revert_prefab` 返回**新实例根索引**（索引会变，不许用旧索引） |
| 顺带关掉 | 空资产**不动手**（旧路径先删实例再发现没东西可放）；手改场景撞 ID 时末尾 `ensure_unique_ids()` 自愈 |
| 宿主职责 | 编辑器只负责保住用户亲手摆的东西：根的世界变换 + 父节点。还原是还原**内容** |
| 验证 | 编辑器侧先红后绿（老路径 `left: [7,8]` vs `right: [4,5]`）；core 补三条；全量 **968 passed / 0 failed / 9 ignored**；见 PLAN 第七十八节 |
| 边界 | 嵌套预制体与**字段级覆盖**仍未做；当前是整树内容覆盖，不是 Unity 的"只还原被覆盖字段" |

### 2026-10-28 销毁钩子里的结构性改动（R11/121 的一半）

| 项目 | 状态 |
|---|---|
| 契约 | 钩子里 `spawn` / `destroy` 对 **Scene 立即生效**，但命令**下一帧**才结算（建刚体、派发 on_destroy）—— 别假设"spawn 完就有刚体" |
| 新覆盖 | `on_destroy` 里 spawn 落地一次且字段生效；链式销毁里第二个实体也收到 `on_destroy` 并能读到自己销毁前的状态 |
| 能力边界 | `spawn_entity` **不带脚本**，Lua 侧无"挂脚本"API；分裂带行为的实体要走 `spawn_prefab` |
| 验证 | 两条均为**补覆盖**（行为本来就对，无红灯）；全量 **964 passed / 0 failed / 9 ignored**；见 PLAN 第七十七节 |
| 边界 | `unload_scene` 期间的 spawn、真·异常退出（进程被杀）仍未覆盖 |

### 2026-10-28 确定性门禁（R10/084 的一半）

| 项目 | 状态 |
|---|---|
| 契约 | 同初始场景 + 同 dt，两次运行轨迹必须**逐位相同**（`f32::to_bits` 比对，不用容差）；场景必须顺序敏感（叠放接触） |
| 原理 | `HashMap` 的 `RandomState` 每实例不同种子 → 同进程跑两遍就能暴露"按哈希顺序决定先后"的隐患 |
| 查到的隐患 | `sync_bodies` 清理段按 `body_of_entity.keys()` 收待删列表 = 哈希顺序；已改为按 `id.raw()` 排序。**当前 rapier 对删体顺序不敏感**（加排序前后都绿），属于去掉未受契约保护的顺序依赖，不是已修的缺陷 |
| 三宿主 | `update_with_scene` 只是 swap 场景后调同一个 `world.update(dt)`，headless 同款 → "跑同一段逻辑"是结构保证 |
| 验证 | 两条测试连跑 5 次不 flaky；全量 **962 passed / 0 failed / 9 ignored**；见 PLAN 第七十六节 |
| 边界 | 只覆盖单进程两次运行一致；跨机器浮点、`wrap` 模式、rapier 并行求解未覆盖；R10 的父层级对照仍未做 |

### 2026-10-28 脚本错误限流（124 的一刀）

| 项目 | 状态 |
|---|---|
| 限流 | `LuaHost` 按 `(实体, 脚本, 钩子)` 记连续报错，只在 2 的幂次推日志（1/2/4/8…），第 2 次起文案带「已连续报错 N 次」 |
| 清零 | 同一钩子成功一次即清零；`invalidate*` / 重置 Play / 换场景都清空（否则修好后的第一条错误会被静默） |
| 不做 | **不**自动禁用脚本（会悄悄改变游戏行为）；表超 1024 条时清理已删实体的条目 |
| 验证 | 旧实现 100 帧报 100 条实跑红；配套钉住「成功清零」`[1,1,0,1]` 与「文案带次数」；全量 **960 passed / 0 failed / 9 ignored**；见 PLAN 第七十五节 |
| 边界 | 只限流引擎捕获的 Lua 错误；脚本自己 `log(ctx, …)` 不受节流；124 的内存配额与宿主调用成本仍未做 |

### 2026-10-28 稳态零分配门禁

| 项目 | 状态 |
|---|---|
| 门禁 | `crates/xgame-runtime/tests/alloc_steady_state.rs`：全局分配器记账，断言预热后每帧 0 次分配；**不 ignore**，进 CI |
| 实测 | `entities+physics`（200 实体）与 `ui`（12 按钮 + 布局命中）都是 **0 次 / 0 字节**；dev 与 release 均成立 |
| 负对照 | 在 `update` 里加一次 `format!` → 报「60 次 / 600 字节，每帧 1.0 次」，红 |
| 验证 | 全量 check/clippy 通过，**957 passed / 0 failed / 9 ignored**；见 PLAN 第七十四节 |
| 边界 | 只覆盖无脚本的稳态路径；脚本宿主 / 粒子 / 动画的分配未纳入（要各自量过再加） |

### 2026-10-28 预制体 ID 的跨文件诊断（101 关闭）

| 项目 | 状态 |
|---|---|
| 作用域规则 | ID 只在**单个文件内**唯一；跨文件唯一的是 `(source, source_id)`。场景重复 ID 可在加载边界静默修，预制体的**不许**（会让所有实例的 `source_id` 失配） |
| 新诊断 | `--check` 报 `duplicate prefab id:`（预制体内重复 ID，读盘原文）与 `dangling source:`（`source_id` 悬空，按场景+预制体聚合）；两者都进 `has_issues()` ⇒ 退出码 2 |
| 验证 | 2 条旧行为红灯实跑（`2 failed`，其余 14 条 check 测试全绿）；真实 CLI 正负对照（`9999` → 报、改 `1` → 消失）；全量 check/clippy 通过，**956 passed / 0 failed / 9 ignored**；见 PLAN 第七十三节 |
| 边界 | 预制体坏 RON 本轮静默跳过（内容级诊断不在范围）；G0 其余条目仍未整体关闭 |

### 2026-10-28 三个 G0 数值 / 交互边界

| 项目 | 状态 |
|---|---|
| 物理质量 | 极小球体（半径 1e-20）体积下溢时不再变成 0 kg：密度不可表达则退回刚体附加质量，总质量恒为声明值 |
| 实体 ID | `u64::MAX` 读盘即拒（否则顶死计数器 → 两个实体共用 ID）；`new()` 改 CAS 跳过 0 |
| UI 指针 | `UiPointer.pos` 与按键解耦，"在视口外松手"当帧收尾（此前 `Release` 事件丢失、捕获残留到指针回视口）；新增 `World::ui_pressed()`；隐藏父节点不进命中、脚本改的矩形迟一帧生效，各自有测试钉住（**067 关闭**） |
| 验证 | 3 条旧实现红灯实跑 + 1 条错序红灯实跑；全量 check/clippy 通过，**954 passed / 0 failed / 9 ignored**；见 PLAN 第七十二节 |
| 边界 | 101 的"跨文件诊断"仍未做（预制体 `source_id` 冲突规则未定）；G0 不整体关闭 |

### 2026-10-28 蒙皮链路收尾：骨槽超限 + 真实 glTF 验收（R03 / 031 部分关闭）

| 项目 | 状态 |
|---|---|
| 骨槽超限 | `clamp_skin_influences`（顶点权重清零）+ Skin 截断补单位阵；红灯实证后全绿 |
| 多关节混合 | 4 骨任意权重合成测试补覆盖，GPU vs CPU 逐像素对照 |
| 真实 glTF 验收 | Fox 24 骨 Walk 三时刻逐像素对照，修复纹理共享 bug 后 `exact=0`——蒙皮端到端确认正确 |
| 验证 | **999 passed / 0 failed / 9 ignored**；clippy 0 警告；见 PLAN 第八十四节 |
| 边界 | 任意骨骼位移 bounds 未专项验收 |

### 2026-10-28 CUBICSPLINE 三次样条采样（R04 / 032）

| 项目 | 状态 |
|---|---|
| 动画保真 | `CUBICSPLINE` 不再降级：每帧入切线 / 值 / 出切线完整保留，采样走 Hermite（切线按"每秒"乘段长、旋转结果归一化）；`Track` 的 `frame_start` / `value_start` / `frame_len` 成为唯一布局入口 |
| 验证 | 3 条旧实现红灯实跑（中点 0.5 应为 0.75、2 秒段 0.5 应为 1.0、morph 只剩 6 个值）；全量 check/clippy 通过，**945 passed / 0 failed / 9 ignored**；新增采样成本基准（84 轨道样条 0.80× 线性）；见 PLAN 第七十一节 |
| 边界 | morph 多网格仍只取第一条权重轨道（v1 边界，与插值无关）；根运动与动画 Phase B 未做；G0 不因此关闭 |

### 2026-10-27 G0 打包拒绝旧引擎播放器

| 项目 | 状态 |
|---|---|
| 播放器新鲜度 | 新增源码指纹 `xgame-src-v1:…`（不含 profile，`#[used]` 锚点保住），`builder::pick_player` 跳过源码不一致的候选；全不一致则拒绝并给出重编命令 |
| 验证 | 真实二进制三组对照（同批通过 / 改指纹拒绝 / 跳过旧的用新的并留提醒）；全量 check/clippy/test 全绿 **942 passed / 0 failed / 8 ignored**，release 与 dist 同批刷新、插件 DLL 同批刷新后无头冒烟零 error；见 PLAN 第六十六节 |
| 边界 | 只比源码指纹不做签名；参与指纹的源码一改，既有播放器即过期（需重编，`一键构建.cmd` 满足）；完整发布端到端仍未接 hosted CI；G0 未关闭 |

### 2026-10-27 G0 写盘实现统一与遗留暂存回收

| 项目 | 状态 |
|---|---|
| 写盘单实现 | `xgame_core::fsx` 收敛 core / editor / input 三份副本；新增超龄暂存回收（命名即判据、600 秒阈值）与 POSIX 父目录 fsync |
| 验证 | 旧实现红灯（超龄暂存不回收）；workspace check/clippy/test 全绿 **942 passed / 0 failed / 8 ignored**，release 与 dist 三 EXE 刷新，SampleScene / BehaviorsDemo 检查与无头截图通过、插件指纹负对照有效；见 PLAN 第六十五节 |
| 边界 | 多进程同目标保存仍最后完成者胜出；Windows 无目录 fsync，断电目录持久性未验；旧固定 `.tmp` 与打包临时目录不回收；G0 未关闭 |

### 2026-10-27 G0 资产引用迁移词法保护

| 项目 | 状态 |
|---|---|
| 迁移改写 | 只替换资产字段中的 RON 字符串；注释、同名普通字段与嵌套注释保留原字节 |
| 回归 | 旧实现误改注释与名称已先红；迁移 4 项定向测试通过，完整验证见 PLAN 第七十节 |

### 2026-10-27 G0 最近项目列表保存

| 项目 | 状态 |
|---|---|
| `recent_projects.ron` | 共用偏好配置的独占暂存和同步替换，锁定目标时旧文件不变，残留固定暂存不阻断 |
| 回归 | 旧实现锁定目标测试先红，新实现最近项目 4 项及偏好 12 项、全工作区 check/clippy/test、release 和 SampleScene 冒烟通过；见 PLAN 第六十九节 |
| 边界 | 断电目录持久性尚未验证，G0 未整体关闭 |

### 2026-10-27 G0 编辑器偏好保存

| 项目 | 状态 |
|---|---|
| `editor.ron` | 目标被占用时保留原字节并允许重试；旧固定暂存残留不阻断保存 |
| 回归 | 旧实现锁定目标测试实跑红灯，新实现 12 项偏好测试、全工作区 check/clippy/test、release 与 SampleScene 冒烟通过；格式门禁存在历史未格式化文件，见 PLAN 第六十八节 |
| 边界 | 断电目录持久性尚未验证，G0 未整体关闭 |

### 2026-10-27 G0 资产引用迁移写盘

| 项目 | 状态 |
|---|---|
| 场景 / 预制体 / 材质迁移 | 字面量替换后复用同目录独占暂存与替换；失败保留旧字节，报告逐文件错误并允许重试 |
| 验证与发布 | 回归旧实现红、新实现绿；全工作区 check/clippy/test 通过，release 构建和 dist 三 EXE 刷新；SampleScene 检查及 30 帧无头退出 0，见 PLAN 第六十七节 |
| 边界 | 多文件整体事务、历史暂存回收与断电目录持久化未完成 |

### 2026-10-27 G0 输入映射保存

| 项目 | 状态 |
|---|---|
| `input.ron` | `InputMap::save` 已改同目录独占暂存、同步后替换；目标锁定时旧文件不变，旧 `.tmp` 残留不阻断 |
| 验证 | 旧实现回归实跑失败；输入模块 14 项、workspace check/clippy 通过，完整记录见 PLAN 第六十六节 |
| 边界 | 暂存回收与断电目录持久化尚未验证，G0 未关闭 |

### 2026-10-27 G0 资产元数据保存

| 项目 | 状态 |
|---|---|
| `.meta` 新建 / 更新 | 已改为同目录暂存后发布；新建拒绝覆盖并发者身份，更新失败保留旧文件和索引 |
| 验证 | 旧实现更新失败注入实跑红灯；core 测试 263 项通过，完整验收见 PLAN 第六十五节 |
| 边界 | `input.ron`、迁移写入、目录断电持久化及暂存回收待做，G0 未关闭 |

### 2026-10-27 G0 主保存残留暂存文件

| 项目 | 状态 |
|---|---|
| 原子写暂存 | 已改为同目录独立暂存名（进程号 + 原子序号）；旧版残留 `.tmp` 文件或目录不再阻断项目清单、场景、预制体和材质保存，残留原字节保留供排查 |
| 回归 | 旧实现下 `stale_staging_file_does_not_block_main_saves` 已实跑失败；新实现 core 261 项及 workspace 全量测试通过，check/clippy 通过；完整验收见 PLAN 第六十四节 |
| 边界 | 不自动回收旧暂存文件；其他写入入口及断电目录持久化尚未覆盖，G0 仍未关闭 |

### G3 跨平台构建（计划中，未实施）

| 目标 | 状态 |
|---|---|
| Linux 桌面原生构建与独立包验收 | 待做；优先于其他新平台，目标环境实测后再接 CI |
| macOS 桌面原生构建与独立包验收 | 待做；需 macOS 主机实测，再接 CI |
| Web 构建 | 待评估；先确定浏览器端资产、插件、音频与脚本边界 |
| 当前支持证据 | 仅 Windows/MSVC 完成构建和真实运行验收；详细顺序与完成口径见 [差距分析](Docs/差距分析.md#2026-10-27-跨平台构建计划待实施)及 PLAN 第六十三节 |

### 2026-10-27 真实发布包输入 / 切关 / 插件验收

| 项目 | 状态 |
|---|---|
| 本地发布验收 | 真实 --build → 搬移独立包 → 无参数播放器；三关自定义动作/轴、插件旋转、两次切关及退出销毁断言通过；见 PLAN 第六十二节 |
| 回归与发布 | 全量 check/clippy 通过，917 passed / 0 failed / 7 ignored；release workspace 与 dist 三 EXE 刷新完成 |
| 负对照 | 错绑输入、缺插件各按预期失败；未发现新引擎缺陷，仅增加验收脚本 |
| 边界 | Windows 消息注入与 GPU 窗口，不覆盖物理键盘/焦点/IME；现有 hosted CI 未接桌面门禁，G0 不整体关闭 |

### 2026-10-27 插件注册前兼容检查

| 项目 | 状态 |
|---|---|
| G0 插件门禁 | 已实现版本化 C 描述符与构建指纹，缺失/不匹配在原入口前拒绝；红绿证据见 PLAN 第六十一节 |
| 全量验证 | check/clippy 通过；27 套件 917 passed / 0 failed / 7 ignored |
| 发布与冒烟 | release workspace 成功；dist 三 EXE 与 Demo DLL 同批刷新并核对哈希，旧 DLL 已备份；Demo 120 帧真实 GPU 截图与 SampleScene check 均退出 0 |
| 边界 | 仅当前 workspace 构建单元的保守检查，非完整 Rust ABI 证明；下游 flags/feature、OS 加载时初始化不覆盖；G0 未整体关闭 |

### 2026-10-27 发布目录扫描隔离（历史）

| 项目 | 状态 |
|---|---|
| Build 污染源项目资产库 | 已修复；新增 2 项回归，旧实现先红后绿；设计与证据见 PLAN 第六十节及决策记录 §十四 |
| 全量验证 | check/clippy 通过；27 套件 914 passed / 0 failed / 7 ignored |
| 发布 | release workspace 成功；dist 三 EXE 刷新并核对哈希；保留旧 Build 及真实重打包后，原项目和包 check 均退出 0 |
| G0 边界 | 完整包输入/切关/插件及 ABI 指纹仍待验 |

### 2026-10-27 双角色 GPU 验收（历史）

| 项目 | 状态 |
|---|---|
| G0 R03/031 | 新增 2/5 骨双角色四姿态与槽位重排 GPU 回归，主画面/阴影对照通过；历史短槽行为两项红灯，见 PLAN 第五十九节 |
| 本轮全量 | check/clippy 通过；27 套件 912 passed / 0 failed / 7 ignored |
| 发布 | release workspace 成功；dist 三 EXE 刷新并核对哈希；120 帧 GPU 与新包目录检查/运行通过 |
| 新发现与边界 | 原项目 --check 因 Build 重复资产返回 2；完整包输入/切关/插件及 ABI 指纹仍待验，G0 未关闭 |

### 2026-10-27 morph 与交付复验

| 项目 | 状态 |
|---|---|
| G0 029 morph 包围盒 | 已修；新增 3 条回归，两项真实导入/GPU 回归先红后绿，见 PLAN 第五十八节 |
| 本轮全量验证 | check/clippy 通过；test 910 passed / 0 failed / 7 ignored |
| 发布程序 | release workspace 通过；dist 三个程序已刷新并校验哈希；SampleScene 检查、120 帧 GPU 截图与真实打包通过 |
| G0 整体验收 | 仍未关闭；双角色 GPU、包输入/切关、插件 ABI 与其他剩余项沿用正式差距分析 |

### 2026-10-27 追加状态（上一轮）

| 项目 | 状态 |
|---|---|
| R01—R12 与额外 G0 审计 | 已逐项登记于 [差距分析](Docs/差距分析.md)，G0 未全部关闭 |
| 配置/材质主保存、Lua 预算/ctx、结构派发、零视口 UI | 已修改；6 项旧实现红灯已实跑，见 PLAN 第五十七节 |
| 最终全量验证与 dist | check/clippy/test 曾通过；补录因文件访问失败，release 被磁盘/权限环境阻塞，dist 未刷新；见 PLAN 第五十七节 |
| 后续阶段 | G1—G4 保留，不将 200 项能力矩阵标为完成 |

### 2026-10-26 本轮状态（历史）

正式排序已采纳 **G0 优先于 G1—G4**，以 [差距分析](Docs/差距分析.md) 为唯一入口；新深度报告保持审计快照。下方既有“完成”仅表示相应历史切片完成，不表示生产成熟或 G0 关闭。

| 项目 | 状态 |
|---|---|
| R01—R12 首轮代码修复 | 已改；契约与限制见 [决策记录 §四十](Docs/决策记录.md#四十g0-首轮修复契约与验收边界2026-10-26) |
| G0 整体验收 | 未全部关闭；ABI 兼容拒绝门禁、复杂生命周期、GPU 动态验证及其他主写入等仍待处理 |
| BehaviorsDemo 旧插件 | 已备份并刷新为同批 release DLL；原项目 300 帧 headless 截图复验退出 0；ABI 门禁仍待做 |
| 全量验证 | check / clippy `-D warnings` 通过；test 901 passed / 0 failed / 7 ignored |
| dist 与示例冒烟 | dist 已刷新；SampleScene / CharacterDemo / MyGame headless 截图通过 |
| 独立发布包 | 真实窗口启动、退出通过；完整输入/切关/插件 CI 门禁仍待补 |
| R01/R02 旧实现红灯 | 已补跑最小旧行为失败，Edit 恢复后两个定向测试均通过；证据见 PLAN 第五十六节 |

BehaviorsDemo 旧插件刷新复验与追加的 R01/R02 红绿定向测试均已实跑，其余验证引用本轮既有结果，未重新运行。实施记录见 [PLAN 第五十六节](PLAN.md#五十六g0-首轮修复与正式优先级采纳2026-10-26)。

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
| **P0 资产管线** | ✅ 完成 | 资产 GUID + `.meta` + 资产库诊断 + `--migrate-assets` + 资源浏览器（浏览 / 拖入场景）+ MCP `asset_list`；导入参数消费见「材质资产」；见 PLAN.md 三十二、决策记录 §十四 |
| **双轨 P0–P3** | ✅ 完成 | Rust 组件（UE 插件模式）+ Lua 脚本（Unity 模式 + 热更）双轨；P2.5 基准判定后 P3 删 Rhai，脚本层收敛单一 Lua 轨。决策 D1–D11 见[计划文档](Docs/双轨语言架构计划.md)，实施记录见 PLAN.md 十五–二十一 |
| **七** | 🟨 进行中 | 动画系统（主要切片 ✅：骨骼蒙皮 + 单剪辑 + 交叉淡入 + 阈值状态机 + 叠加 + 多层混合栈与骨骼遮罩 + **形变目标 morph target**；Phase B（分层状态机 / 根运动 / 逐条转移时长）待做） |
| **八** | ✅ 完成 | 音频系统（kira：`AudioSource` + 3D 空间音频 + 监听者跟随 + 静音降级 + 脚本 API，见 PLAN.md 二十二） |
| **Agent-Native ①②③** | ✅ 完成 | MCP 工具面（12 工具，含 `asset_list` / `asset_import` / `asset_move` / `schema_read` / `script_api_read`）+ 无头运行 `--headless` / `--screenshot` + 场景格式说明书 `--export-schema` + 脚本 API 清单 `--export-script-api`；见 [MCP.md](Docs/MCP.md) / [无头运行.md](Docs/无头运行.md) / PLAN.md 五十、五十一 |
| **CI / 自动化** | ✅ 完成 | GitHub Actions（Windows + stable + clippy + rust-cache）：`check` / `clippy` / `test` + 示例项目 `--check` / `--headless` + 两份 JSON 导出；无 GPU 的 runner 上渲染测试自动 skip；见 PLAN.md 五十二、决策记录 §三十四 |
| **渲染 PBR** | ✅ 完成 | Cook-Torrance + 逐实体材质覆盖 + 无头截图（PLAN.md 二十四） |
| **渲染 HDR** | ✅ 完成 | HDR 中间标 + ACES + 曝光（二十五）、Bloom（二十六） |
| **渲染天空** | ✅ 完成 | 程序化天空盒 + 球谐漫反射 + 预过滤镜面 + BRDF LUT（二十七） |
| **渲染批处理** | ✅ 完成 | GPU 实例化合批（二十八）+ 视锥剔除与阴影补投（三十）+ FXAA（三十一） |
| **渲染 CSM** | ✅ 完成 | 4 级级联阴影：视线方向线性深度切分 + 每级紧致正交拟合 + 纹素吸附；4 层深度数组逐层 pass（PLAN.md 四十一、决策记录 §二十三） |
| **渲染 TAA** | ✅ 完成 | 时域抗锯齿：Halton 亚像素抖动（只叠在相机上）+ 重投影历史 + 3×3 邻域钳制；两张 HDR 历史乒乓、默认关（PLAN.md 四十七、决策记录 §二十九） |
| **材质资产** | ✅ 完成 | `.mat` 共享材质（颜色 / 金属度 / 粗糙度 / albedo）+ 项目材质库 + 资产/覆盖两层 + 编辑/指派 + 4 个脚本 API + 导入参数接线（PLAN.md 三十五、决策记录 §十七） |
| **九** | 🟨 进行中 | 后处理 + 阴影：上述渲染片 + **TAA** 已落地；**点光阴影、SSAO、LOD 待做** |
| **游戏 UI** | ✅ 完成（最小集） | 屏幕空间 UI：锚点布局 + 文本（中文 / 粗体 / 换行 / 对齐）+ 圆角面板 / 图片 / 按钮 + 指针交互（悬停 / 按下 / 点击）+ 23 个 `ui_*` 脚本 API + Inspector 区块；见 PLAN.md 三十四、决策记录 §十六 |
| ~~**十**~~ | ✅ 完成 | 脚本系统 + 热重载（已并入六·五与双轨 P0–P3） |
| **粒子系统** | ✅ 完成（CPU 版） | `ParticleEmitter` 组件 + `xgame-particle` 确定性 CPU 模拟 + billboard 实例化 pass（Alpha / Additive）+ Inspector 区块 + 4 个脚本 API；万级以上需上 GPU 模拟（分层已留好）；见 PLAN.md 三十六 |
| **编辑器 Profiler** | ✅ 完成 | 帧时间滚动窗口（600 帧 / 60fps 下 10 秒）+ 主循环分段耗时 + 绘制 / 剔除 / 粒子读数；与 Output Log 同页签组；见 PLAN.md 三十七 |
| **相机实体** | ✅ 完成 | `Entity::camera`（FOV / near / far / active）+ 单机位解析单入口 + 独立游戏 / 编辑器 Play / 无头截图三宿主同源 + 6 个脚本 API（`look_at` 等）；见 PLAN.md 三十八、决策记录 §二十 |
| **编辑器偏好** | ✅ 完成 | 相机速度 / 鼠标灵敏度 / 网格开关 + 完整停靠布局存进 `editor.ron`；磁盘布局校验不过回退出厂布局；见 PLAN.md 三十九、决策记录 §二十一 |
| **自动保存 / 崩溃恢复** | ✅ 完成 | 每 60 秒（可调 / 可关）把脏场景写进 `.autosave/` 副本（原子写、不改未保存状态）；崩溃后重开项目弹「恢复 / 丢弃 / 稍后」；见 PLAN.md 四十二、决策记录 §二十四 |
| **运行期场景切换** | ✅ 完成 | `load_scene` / `current_scene` 两个脚本 API：切换在**帧末**唯一时机应用（先装载、成功才换）；物理 / 粒子 / 音频 autoplay / UI 交互 / `state_*` 一并重置，脚本全局变量保留；编辑器 Play / 独立游戏 / 无头运行三宿主同源；见 PLAN.md 四十三、决策记录 §二十五 |
| **构建设置（关卡顺序）** | ✅ 完成 | `project.ron` 加 `build` 节：版本号 + **有序关卡清单**（顺序即关卡索引）；脚本 `scene_count` / `scene_index` / `load_scene_at` 让"进下一关"不再硬编码路径；`--check` 校验清单、构建器打包前逐关试装、新增 `--build` CLI；见 PLAN.md 四十四、决策记录 §二十六 |
| **运行时预制体实例化** | ✅ 完成 | 脚本 `spawn_prefab` / `spawn_prefab_under`：整棵子树进场景（层级 / 变换 / 物理 / 粒子 / 音频 / 相机 / UI 全带上）、每个新实体贴实例来源、刚体由运行时补建；位置语义"设置不叠加"（世界位置 / 局部偏移两种）；失败三种各自一条日志并返回 `nil`；见 PLAN.md 四十五、决策记录 §二十七 |
| **时间缩放 / 暂停** | ✅ 完成 | `set_time_scale` / `time_scale` 两个脚本 API（Unity `Time.timeScale` 语义）：`0` = 暂停模拟、`0.3` = 慢动作、`2.0` = 二倍速。缩放**只在 `GameClock::tick` 一处消费**，所以物理固定步 / 脚本 `delta`+`time` / 粒子 / 动画钟同帧同源；音频与 UI 不受影响（暂停菜单必须还能点）；编辑器 Play 的预览动画同步跟手；无头报告加 `time_scale` 让 CI 能断言"真的暂停了"；见 PLAN.md 四十六、决策记录 §二十八 |
| **资产 / 脚本热重载** | ✅ 完成 | 1 Hz 轮询内容目录（只看 metadata）：Play 中改 `.lua` 自动重编译、编辑态刷新清单；资产 / `.meta` 变更刷新资产库并丢渲染缓存；预制体刷新清单。**场景不自动覆盖**、脏材质草稿保留、`project.ron` / 已实例化预制体 / 已解码音频不重载；只读会话不启用。见 PLAN.md 四十九、决策记录 §三十一 |
| **层与碰撞矩阵** | ✅ 完成 | 32 层进 `project.ron` 的 `layers` 节（只存覆盖，默认全通 = 老工程逐位一致）；`Entity.layer` + Inspector 层下拉 + 设置面板 32×32 矩阵（对称写、两侧相与）；rapier `InteractionGroups` 建体 / 运行时可改组，角色控制器认自己的层；射线掩码只看成员位（Unity `layerMask` 语义）；脚本 `layer` / `set_layer` / `raycast_masked`；`--check` / MCP 报 `invalid_layers`。见 PLAN.md 五十四、决策记录 §三十六 |

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
      导入设置、材质新建 / 编辑 / 指派；双击 / 拖入视口放入场景 —— P0 资产管线）
- [x] Profiler 面板（帧时间滚动窗口 + 主循环分段 + 绘制读数；`--ui-profiler`
      截图开关 —— 编辑器产品化第一件，见 PLAN.md 三十七）
- [x] 自动保存 + 崩溃恢复（`.autosave/` 副本，间隔可调可关；恢复框三选一 ——
      见 PLAN.md 四十二）
- [x] Preferences + 面板布局持久化（`editor.ron`：相机速度 / 灵敏度 / 网格 +
      完整停靠布局；磁盘布局先校验，坏布局回退出厂；`--ui-preferences` /
      `--ui-restore-prefs` 截图开关 —— 见 PLAN.md 三十九）

## 额外里程碑

| 里程碑 | 状态 | 内容 |
|---|---|---|
| 共享渲染管线 | ✅ 完成 | 从 SceneRenderer 提取 ScenePipeline 到 xgame-render |
| 独立游戏二进制 | ✅ 完成 | xgame-game crate，原生命令行运行 |
| 引擎 / 项目分离 | ✅ 完成 | `project.ron` + `ProjectHandle`；编辑器项目枢纽（新建 / 打开 / 最近项目） |
| 一键构建 | ✅ 完成 | 发布构建脚本（双击 `一键构建.cmd` → `dist/`）；编辑器 Build 按钮把项目打包成独立游戏 |

---

## 商业引擎对齐（状态速览）

下表保留历史功能切片状态；完整能力边界与当前 G0—G4 排序见
[`Docs/差距分析.md`](Docs/差距分析.md)，这里只留状态。

| 能力 | 状态 | 说明 |
|---|---|---|
| PBR 渲染 | ✅ 完成 | Cook-Torrance + 共享材质资产 + 逐实体材质覆盖（此前 Blinn-Phong） |
| 纹理 / 多光源 / Alpha | ✅ 完成 | 贴图采样 + 单帧 8 盏 + glTF `alphaMode` 半透明排序 |
| 内置基础图元 | ✅ 完成 | Cube / Sphere / Plane / Cylinder（`builtin://`） |
| 场景层级 | ✅ 完成 | `Entity::parent` + `Scene::hierarchy()`；拖拽保世界变换、防环 |
| Undo / 复制粘贴 | ✅ 完成 | 快照式撤销（拖拽一步）；整棵子树复制粘贴 |
| 预制体（Prefab） | ✅ 完成 | 资产化 + 实例来源标记 + Apply / Revert |
| 资产引用稳定性（GUID） | ✅ 完成 | 磁盘存 `guid://` + `.meta`（GUID 永不变）、内存存路径；改名 / 挪目录不断链；`--check` 报悬空 GUID / 缺 meta / 冲突 |
| 资源浏览器 | ✅ 完成 | Assets 面板：目录树 / 类型图标 / 搜索 / 详情 / GUID / 导入设置；材质新建与编辑；双击与拖拽放入场景 |
| 材质资产 | ✅ 完成 | `.mat`（颜色 / 金属度 / 粗糙度 / albedo）+ 项目材质库 + 资产/覆盖两层 + 脚本 API；贴图槽目前只有 albedo |
| 导入设置消费 | ✅ 完成 | `model_scale` 烘焙进顶点 / 节点 / 动画 / 逆绑定，`texture_srgb` / `texture_nearest` 进纹理采样；`audio_streaming` 仍待流式播放 |
| 多场景管理 | ✅ 完成 | Scenes 菜单列出 `Scenes/*.ron`：切换 / 新建空场景 / 按当前场景保存 / 显式设启动场景；切场景重置撤销与跨场景临时状态；`--check` 覆盖全部场景；**运行期 `load_scene`**（帧末切换，PLAN 四十三）+ **`build.scenes` 有序清单与关卡索引**（PLAN 四十四）。仍无多开 / 平台预设 / 图标 |
| 角色控制器 | ✅ 完成 | 胶囊控制器 + 自动上台阶 + 坡面 / 陡坡 + 碰撞事件进脚本（见阶段六·六） |
| 相机实体 | ✅ 完成（最小集） | `Entity::camera` + 单机位解析（active 优先）+ 三宿主同源（独立游戏 / 编辑器 Play / 无头截图）+ 6 个脚本 API；缺正交投影 / 多机位分屏 |
| 多相机 / 分屏 | ⬜ 待做 | 单帧只渲染一台主相机（不做 Unity 那种多相机叠画 / split screen） |
| 游戏 UI | ✅ 完成（最小集） | Canvas 等价物 = 实体 + 锚点；文本 / 图片 / 按钮 / 面板；`ui_*` 23 API；缺裁剪、九宫格、输入框、手柄导航 |
| 输入系统 | ✅ 完成 | `InputMap`（`input.ron`）+ 编辑器 / 游戏共用；无手柄 / 触摸 |
| 项目设置 | ✅ 完成 | 项目名 / 入口场景 / 窗口 / 重力 / 步长 / 画面（曝光 / ACES / Bloom / FXAA / TAA），草稿 + Apply；单入口 `from_project`。内容目录只在 RON 里手改 |
| 未保存保护 / Save As | ✅ 完成 | 脏标记 + 双路径确认 + 标题星号 + 快捷键 |
| 无头检查 `--check` | ✅ 完成 | 退出码 0/1/2，CI 与智能体的可机读入口；已接入 GitHub Actions（PLAN 五十二） |
| 无头运行 `--headless` | ✅ 完成 | 固定 dt 逐位可复现；`--screenshot` 离屏出图 |
| 阴影 / 天空 / 后处理 | 🟨 部分 | 4 级 CSM + MSAA + HDR/ACES + Bloom + 天空 IBL + FXAA + **TAA** 已落地；SSAO / 点光阴影待做 |
| 实例化 / 剔除 | 🟨 部分 | GPU 实例化 + 视锥剔除 + 阴影补投已落地；LOD / 遮挡剔除待做 |
| Gizmo 吸附 / Rect 工具 | ⬜ 待做 | `gizmo_snap` 字段已在但未接线；Rect 工具无实现 |
| 编辑器交互稳定性 | ✅ 完成 | 射线 × 世界 AABB 拾取；轴拖拽参考线锚定；选中索引每帧收敛 |
| 编辑器 Profiler | ✅ 完成 | 帧间隔 / 1% low / 主循环分段（运行时 · 界面+视口）+ draw / 剔除 / 粒子读数；600 帧滚动窗口，Pause / Clear；见 PLAN.md 三十七 |
| 编辑器偏好 / 布局 | ✅ 完成 | 相机速度 / 灵敏度 / 网格 + 停靠布局持久化（`editor.ron`）；坏布局校验回退出厂；见 PLAN.md 三十九、决策记录 §二十一 |
| 自动保存 / 崩溃恢复 | ✅ 完成 | 间隔可调（默认 60 秒，`0` = 关）的 `.autosave/` 副本 + 原子写；崩溃后重开项目三选一（恢复 / 丢弃 / 稍后）；恢复后仍是未保存状态；见 PLAN.md 四十二、决策记录 §二十四 |
| 每帧开销 / 可扩展性 | ✅ 完成 | 2000 实体纯视觉 13 µs/帧；渲染 2225 → 344 µs（实例化）、3295 → 1146 µs（剔除）。剩余瓶颈是 rapier 求解器 |
| 崩溃面 / 无界增长 | ✅ 完成 | 未知 body 只警告；控制台上限 1000 条；撤销栈 100 步；缓存有界 |
| 窗口 / 交互延迟 | ✅ 完成 | 空闲不重绘；1 Hz 内容扫描只查 metadata（不读文件内容、不触发视口重画）；拖边框节流重建（43 → 1 次） |

---

## Agent-Native：AI 智能体深度操控引擎（①②③ 已落地）

**目标**：智能体能**深度、精确、可验证地**操作引擎 —— 读写项目、运行检查、拿到可机读
结果，而不是"截图点按钮"。**架构判决式：后续每个架构决策都要过一问 ——「智能体没有
鼠标时，这条路走得通吗？」**

已具备的地基（详见[差距分析](Docs/差距分析.md)第九节）：

- **数据即文本** —— 场景 / 预制体 / 清单 / 输入映射全 RON，可直接读写与 diff；
- **程序化验证** —— `--check`（退出码 0/1/2）；
- **程序化实跑** —— `--headless`（固定 dt 逐位可复现）；
- **程序化观察** —— `--screenshot` 离屏 PNG（同机逐字节一致），MCP 以图片块回传；
- **程序化配置** —— 重力 / 步长 / 渲染参数都在 `project.ron`；
- **程序化契约** —— `--export-schema` / MCP `schema_read`：36 个落盘类型、字段、默认值、
  示例、资产引用规则，确定性 JSON；`--export-script-api` / MCP `script_api_read`：
  160 个脚本函数、签名、参数、返回值、6 个生命周期钩子与全局约定；
- **单一运行时** —— 三宿主复用 World；输入、渲染历史、listener 与发布包仍需分别验证；
- **MCP 工具面** —— 12 个工具（读说明书 / 读脚本契约 / 读 / 改 / 查 / 看效果 / 看资产 /
  改导入参数 / 整理目录）；
- **安全网** —— 本轮 901 passed / 7 ignored；GitHub Actions 在 push / PR 自动跑。先证红仍是要求，R01/R02 已事后补齐最小旧行为失败证据并恢复转绿（见 PLAN 第五十六节）。

进度：① MCP server ✅ → ② 无头运行 + 截图 ✅ → ③ 场景 schema 导出 + 脚本 API
清单导出 ✅（`--export-schema` + `schema_read` / `--export-script-api` +
`script_api_read`）→ ④ 资产 GUID 落地 ✅，`asset_list` ✅ → `asset_import` ✅（单个资产
建 `.meta` + 部分更新导入参数）与 `asset_move` ✅（`.meta` 跟着走、GUID 不变，`guid://`
引用天然不断链）。资产侧「看见 → 导入 → 整理 → 自证」闭环齐了。

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
| 编辑器接入 | ✅ | Inspector 的 Scripts / Physics 区块；脚本自动热重载（改盘上 `.lua`，Play 中 1 秒内重编译）/ 工具栏手动 `⟳ Scripts`；Console 收脚本日志；Play 时 `input_bridge` 灌键鼠 |
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
- [x] 动画片段加载（glTF → `AnimClip` / `Track`；LINEAR / STEP / **CUBICSPLINE Hermite 样条**运行时插值 —— 切线完整保留、按每秒乘段长、旋转归一化，见 PLAN.md 七十一 / 决策记录 §五十一）
- [x] 关键帧插值（线性 / 阶梯 / 三次样条三档；旋转 slerp、样条结果归一化；循环取模、非循环钳位）
- [x] `Animator` 组件（clip / speed / looping / autoplay / fade_time，随场景落盘）
- [x] 播放时钟 `AnimationDriver`（编辑器墙钟、游戏按帧 dt；换剪辑归零）
- [x] 编辑器 Animation 区块（剪辑下拉 / Speed / Fade / Loop / Autoplay / 摘除；指纹与帧策略）
- [x] 换剪辑交叉淡入（`fade_time` 默认 0.25s；淡入中连切两刀重起淡入）
- [x] 阈值状态机（参数表 + 转移表（AtLeast ≥ / Below <）+ 滞回；坏配置安全退回单剪辑；脚本 `set_anim_param`）
- [x] 叠加（additive）混合 v1（增量 = 叠加剪辑当前帧 − 第 0 帧；独立时钟；权重 0 逐位一致）
- [x] 多层混合栈与骨骼遮罩 v1（每层 name / mode / clip / weight / speed / looping / mask；按数组顺序合成；遮罩 = 骨骼子树根名单；`compute_bones_composed` 唯一路径）
- [x] 脚本直接控制：基础剪辑 / 速度 / 循环 / 淡入 + 叠加层与层栈剪辑（6 个 API；状态机优先，见 PLAN.md 四十八）
- [x] 形变目标（morph target / blend shape）：glTF `targets[]` + `targetNames` 解析、
      增量按导入缩放烘焙、权重来源"剪辑权重轨道 > 实体 `morph_weights` > 模型默认"、
      CPU 变形 + 顶点缓冲按需重写、Inspector Morph 滑条、脚本 `morph` / `set_morph`
      （见 PLAN.md 五十五、决策记录 §三十七）
- [ ] 分层状态机（Phase B）；逐条转移时长、根运动

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
- [x] 级联阴影贴图（CSM，4 级；视线方向线性深度切分 + 每级紧致拟合 + 纹素吸附；
      4 层深度数组逐层 pass —— PLAN.md 四十一、决策记录 §二十三）
- [ ] 点光阴影（6 面立方体贴图）
- [x] ToneMapping（ACES）+ HDR 渲染路径 + 曝光（场景 pass → `Rgba16Float` → 曝光 + ACES → LDR；`project.ron` 的 `render` 节三宿主同源；`Off` = 旧管线出口）
- [x] Bloom 泛光（软膝 + 三级 mip + 可分离高斯；阈值在曝光后；**默认关**，整段跳过）
- [x] 程序化天空盒 + 环境光照（IBL）（解析天空一个真相源 + 球谐漫反射 + GGX 预过滤 + BRDF LUT；**默认关**）
- [x] GPU 实例化（同图元合批 + 逐实例 storage buffer；2000 实体 2001 → 2 批、CPU -84%）
- [x] 视锥剔除 + 阴影补投（实体级 AABB × 六平面；相机外投影物仅跑阴影 pass；蒙皮 50% 余量；开/关逐字节一致）
- [x] FXAA 抗锯齿（3.11 质量路径、显示空间 luma、合成后第二趟 pass、惰性 LDR scratch、**默认关**；1× MSAA 硬台阶 -44%）
- [x] TAA 时域抗锯齿（Halton(2,3) 亚像素抖动只叠相机、重投影补上一帧抖动、3×3 邻域钳制、
      两张 HDR 历史乒乓、**默认关**且开着时 FXAA 让位；1× MSAA 12 帧硬台阶 672 → 541、
      中间调 0 → 675 —— PLAN.md 四十七、决策记录 §二十九。**逐物体运动向量仍未做**，
      快速横向移动靠邻域钳制兜）
- [ ] SSAO（HDR 链已就位）
- [ ] LOD（距离分级 / 网格切替）
- [ ] RenderGraph（等出现带依赖分叉的 pass 再抽，见 PLAN.md 二十五）

---

## 项目结构与运行命令

已合并到 [`README.md`](README.md)：crate 清单 / 目录树 / 依赖方向见「引擎与游戏项目是分开的」，
全部运行命令见「快速开始」，每个功能怎么用见 README 对应小节。文档导航也统一放在
README 顶部。