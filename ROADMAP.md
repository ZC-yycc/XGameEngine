# XGameEngine 开发计划

> Rust 实现的类 Unity 游戏引擎。**本文只记阶段与状态** —— 为什么这样做看 `PLAN.md`，
> 能力差距与优先级看 [`Docs/差距分析.md`](Docs/差距分析.md)，不可推翻的约束看
> [`Docs/决策记录.md`](Docs/决策记录.md)。项目结构、运行命令、各功能怎么用统一在
> [`README.md`](README.md)，本文不再复制。

---

## 整体路线图

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
- [x] 动画片段加载（glTF → `AnimClip` / `Track`；LINEAR / STEP 运行时插值，CUBICSPLINE 降采样）
- [x] 关键帧插值（位置 / 缩放线性、旋转 slerp；循环取模、非循环钳位）
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