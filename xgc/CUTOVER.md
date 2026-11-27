# XGC 割接验收（未完成）

> 结论：**XGameEngine → XGame Core 的完整割接尚未达成。**
> 本文件只记录可复现证据与未满足项；能力状态见 [STATUS.md](STATUS.md)。

## 门槛矩阵

| 门槛 | 状态 | 证据 / 缺口 |
|---|---|---|
| 场景 + 资源 + 脚本 + 预制体端到端迁移 | 通过 | showcase 24 实体；character 2 脚本；Pillar 3 实体；`verify-migration` 一致 |
| 迁移世界可运行 | 通过 | `run-world` / `run-cooked` / `xgc-player` 60 帧同哈希 |
| 旧 / 新 headless 统计一致 | 通过 | 24 / 10 / 10 / 2 / 1（`compare-headless`） |
| 视觉可渲染 | 通过（基础） | `render-frame` 第 150 帧 hash `6f8c32afeba3b59a`，1280×720，14 个场景 draw / 7 call（场景 5 + 两种粒子混合）/ 8 个 UI draw；截图相机优先级与旧端一致（`mode: "scene"`）；场景光 / PBR / 4 级 CSM / 场景 pass 4× MSAA（颜色与深度同采样、1× 等同关闭、收尾统一 resolve）/ RGBA16F 场景 / 线性粒子合成 / 唯一全局 ACES / 曝光旋钮（`render.exposure`，缺省 1.0）/ 曝光后曲线前三级光晕（默认关） / 多光源照明（方向光 / 点光最多 8 盏 + 点光衰减） / 点光立方体阴影（6 面深度图） / 可选 PCSS 软阴影 + 级间混合（`render.shadow`，默认关）/ tone 后可选 FXAA（默认关）/ 预过滤环境 IBL（GGX 5 级立方体 + 256 样本 BRDF 积分表，split-sum）/ tone 后 UI 已贯通 |
| 视觉对等 | **通过** | 第 211 轮把判定口径工具化：`xgc parity` 把结构性差异（单通道 ≥8）拆成「轮廓 1 px 带内的 MSAA 覆盖差」与「内部着色 / 结构差」——轮廓判据 = 中心像素与 8 邻域任一分量差 > `PARITY_EDGE_THRESHOLD = 40`（与结构性 ≥8 是两个独立口径），两张图各判一次取并集；门槛 = 内部结构性 ≤ 0.01% 且总结构性 ≤ 0.1%（`mean_abs` / `max` 只记录不判定）。状态对齐帧实测：第 1 帧结构性 275（**0.030%**）= 轮廓 266 + 内部 9（**0.0010%**）、`mean_abs` 0.01、`max` 38；第 150 帧 701（**0.076%**）= 轮廓 671 + 内部 30（**0.0033%**）、`mean_abs` 0.02、`max` 125，逐帧通过。第 200 / 300 帧端到端结构性 1184（0.128%）/ 2382（0.258%）、内部 75（0.0081%）/ 223（0.0242%）——集中在动态物（红方块 25 / 72、狐狸 5 / 59、蓝 Spinner 5 / 33 像素）的软阴影边缘与面着色，属动态体状态漂移而非渲染器差，由 `xgc gate` 端到端口径守（当前实测见 [脚本位置读回](#脚本位置读回)，参考环境指纹语义见 [验收说明](acceptance/README.md)）。旧端参考必须在验收机重新生成；判定口径与复现命令见本文件「可复现命令」。 |
| 逐帧 trace / 状态哈希对照 | **通过** | Lua 相机同钩子读取最新位置后，`acceptance/trace.txt` 从 `frame=0 hash=b649435388a25b18` 到第 150 帧 `b8123d45447bb10b`。首个变化在第 2 帧，见 [脚本位置读回](#脚本位置读回)。核对 12 份迁移产物与采样快照的实体 ID、组件值和世界修订号，以及 151 个检查点的帧号、实体数和修订号。层级系统改为按实体 ID 写回，六个采样帧分别重跑的完整快照逐字节一致。 `xgc trace <世界.json> [帧数] [资产根]` 逐帧打印 `frame / entities / revision / hash`（无资产根时与 `run-world` 同一条无脚本步进路径，带资产根时走 `ScriptedRunner`、与 `run-scripts` 同一条路径；第 0 帧是装载后状态；坏脚本 / 语法错 / 不可用资产根都会打印「第 N 帧运行失败」并退出码 2）；集成回归 `crates/xgc-cli/tests/trace.rs`：与进程内重放逐步对表、两次运行逐字节相同、不同世界第 0 帧即不同、带资产根时第 1 帧起与无脚本路径分叉且汇总行与 `run-scripts` 末行逐字一致；旧引擎没有逐帧 trace 流，但 `--headless --dump-scene` 可导出任意帧数终态 RON，`xgc run-scripts <世界> <资产根> <帧数> <输出.json>` 写同形世界 JSON（第 4 参数），`xgc migrate` 把旧 dump 转世界 JSON 后逐实体 / 组件对表（showcase 1/2/5/10/30/60/100/150 帧）：Spinner 旋转全等、物理恒差 g·dt²≈2.725 mm 且落地收敛、相机差 4e-6；帧首建体修复后第 1–6 帧与旧端逐位一致；glTF 动画剪辑 + 状态机迁移后 Fox 第 150 帧停在 Idle/Survey；宿主阶段夹到固定步与动画之间后第 300 帧当帧进入 Run（脚本参数当帧生效），additive 叠加层后 Fox 第 150 帧仍停 Idle/Survey（叠加 Run 权重 0.1368）；本轮（layers 组件字段变化后）渲染侧哈希：无脚本 `4a5cf2c54331a972`、带脚本 `b6a3103f8331a972`（layers 状态随层机器 / 脚本当帧推进，属预期）；`run 60` 仍 `f4bc029f5f995b91`；割接门禁命令 `xgc gate <世界.json> <资产根> <参考目录> [总差异%] [结构性%] [--strict-total]` 把两类判定合成一条：`xgc/acceptance/trace.txt` 逐帧对表（showcase 150 帧 = 151 个 checkpoint，参考必须从 0 连续；忽略 `revision`，它是内部计数器）+ `xgc/acceptance/frame-<N>.png` 截图分桶判定（结构性（单通道 ≥8）始终按 0.5% 硬判；总差异缺省 2% 只在参考目录带同环境 `env.json`、显式阈值或 `--strict-total` 时硬判，否则仅诊断；本机当前实测见 [脚本位置读回](#脚本位置读回)）；退出码 0 = 全过 / 1 = 有未通过项 / 2 = 用法、IO、GPU 或空参考目录错误；CI 只跑 trace 子集（不碰 GPU） |
| 正式 System 调度（计划即执行顺序） | **通过** | `Schedule::plan()` 取代全批贪心的 `batches()`：按注册顺序**连续**分批（同批两两无访问冲突）+ 宿主阶段（插在第一个 `runs_after_host()` 系统之前；默认调度在固定步与动画之间），`run` / `step_with` 都只走计划，摊平顺序 == 注册顺序；`Simulation` 构造时固化计划（稳态分配不回归）；`xgc plan` 输出确定性 JSON（16 系统 / 12 批 + 宿主阶段；宿主阶段不恒最后，默认调度夹在固定步与动画之间；两次输出逐字节相同）。证据：`run 60` 仍 `f4bc029f5f995b91`；showcase 150 步无脚本 trace、带资产根 trace 与 `run-scripts` 末行在「宿主阶段插到固定步与动画之间」切片后分别为 `4a5cf2c54331a972` / `b6a3103f8331a972` / `b6a3103f8331a972`（layers 状态随层机器 / 脚本当帧推进，属预期）；回归 `crates/xgc-sim/tests/schedule_plan.rs`（8 条）+ `crates/xgc-cli/tests/plan.rs`（4 条） |
| 跨系统并行执行 | **通过** | 执行计划（连续批次 + 访问声明 + `xgc plan`）与所有权拆分（`World::extract_writes` / `WriteSet` / `restore_writes`）之上，第 210 轮落**分区执行**：批内 `System::supports_parted()` 的系统组成并行子集（`animation_apply` / `animation_layers` / `skeleton_pose` / `audio_mixer` 已转），其余按注册顺序串行；批内两两无访问冲突 ⇒ 先串后并的 `state_hash` / `revision` 与纯串行逐位一致（1 / 2 / 8 线程同场景 parity 已进门禁）。默认仍 1 线程（每帧新建线程破坏零分配门禁），用 `Schedule::set_parallel_threads(n)` 或 CLI `xgc run-world <世界> [帧数] [线程数]` / `xgc trace <世界> [帧数] [资产根] [线程数]` 显式开启：`run-world … 60 4` 与默认同哈希、`trace … 4` 逐字节一致。脚本不进 `System`：`ScriptRuntime` 用 mlua 的 `Rc` / `RefCell`、mlua 0.10 无 send feature ⇒ `!Send`，且 xtask 依赖门禁不允许 `xgc-script → xgc-sim`。剩余优化：常驻线程池（让默认启动并行）、`HierarchySystem` 的 `set_field` revision 语义 |
| 性能基准 + 热路径分配门禁 | **通过当前预算** | [分配回归](crates/xgc-sim/tests/steady_state.rs) 的 210 实体场景预热后测 60 帧。借用字段计算状态哈希使分配从 14,598 次 / 311,344 字节降到 1,398 次 / 191,344 字节，复用分批下标和 AABB 刚体缓冲后降到 438 次 / 45,424 字节，当前门禁为 720 次。257 刚体持续接触场景的混合串行批从 660 次分配降到零。Rapier 建体与运动学同步复用父链缓冲，八层移动父链每 60 帧从 300 次 / 34,560 字节降到 180 次 / 5,760 字节，每帧射线核对碰撞体位姿。名字与刚体场景连续 60 次 `state_hash` 零分配且哈希仍为 `cda5ee4b28a9f49e`，1 万实体复用槽位后的乱序哈希也通过零分配门禁。1,026 个 HUD 元素的父子布局预热后测 60 帧，从 2,580 次 / 35,399,280 字节降到零，同机三次 Release 从 32.63–33.58 ms 降至 7.50–7.71 ms，每帧核对锚点、裁剪和可见性。[嵌套字段回归](crates/xgc-sim/tests/hash_allocation.rs) 各用 128 个实体连续计算 60 次哈希，`animation.layers`、`animation.state`、`render.lod` 的分配从 1,021,440、560,640、99,840 次分别降到零，三个原哈希逐位一致。同机 Release 耗时分别从 92.32 降至 15.43 ms、42.05 降至 4.78 ms、7.39 降至 1.31 ms。整帧仍需零分配验收。CI 的 `test` job 另记录 Release 吞吐。 |
| 推送 / CI | **迁移与检查已推送** | 基线重生成、项目迁移与原生检查已合入远端 `master`（`35befa9`、`78853be`、`c1dbd6b`、`d8b5828`）。[XGC CI 工作流](../.github/workflows/xgc.yml) 在 push / pull_request 上跑门禁与 CPU 测试，GPU 集成测试在带显卡的验收机执行。历史成功记录见运行 `45245793893`。 |

## M0 基础计量

在 i7-10700、Windows 10 19045、平衡电源方案和 rustc 1.98.1 MSVC 上运行单线程 Release。三个计时用例各预热两轮，再各测九次。基础固定步预热八帧，再测九组 60 帧。预算取自[框架设计 §13.3](../Docs/引擎框架设计.md#133-微基准)。

| 项目 | 中位数（最小–最大） | 预算 |
|---|---:|---|
| [百万字段读](crates/xgc-world/tests/m0_bench.rs)，逐行累加 `Transform.position[0]` | 1.9001 ms（1.8678–2.0918） | ≤ 1 ms，未达标 |
| [十万实体层级传播](crates/xgc-sim/tests/m0_bench.rs)，深度 16，每次移动根并核对全量结果 | 55.6609 ms（50.9258–64.4275） | ≤ 2 ms，未达标 |
| [全量 schema 序列化](crates/xgc-xtask/tests/m0_bench.rs)，30 个类型、80,271 字节 | 0.4032 ms（0.3218–0.4597） | ≤ 50 ms，通过 |
| [基础固定步](crates/xgc-sim/tests/steady_state.rs)，256 实体且末端子节点的 x 到达 549 | 九组均为 0 次 / 0 字节 | 预热后零分配，通过 |

在 `xgc/` 运行 `cargo test --locked --offline --release -p xgc-world -p xgc-sim -p xgc-xtask --test m0_bench -- --ignored --nocapture --test-threads=1`。基础循环用 `cargo test --locked --offline --release -p xgc-sim --test steady_state -- foundation_world_loop_has_no_allocations --exact --nocapture --test-threads=1`。计时先用 `--no-run` 完成构建，再在独占窗口串行跑测试产物。

## S2 物理场基线

用 i7-10700、32 GiB RAM、Windows 10、Rust 1.98.1 MSVC 测量 Release ThinLTO 的默认单线程固定步。每档预热 18 步。组场与首步另计，状态哈希在计时外计算。[基准代码](crates/xgc-sim/tests/physics_yard.rs)

| 动态 / 静态 | 样本 | 组场 / 首步 ms | p50 / p95 / p99 ms | 采样期间分配 | 末态哈希 |
|---:|---:|---:|---:|---:|---|
| 500 / 2,000 | 120 | 1.790 / 16.243 | 8.917 / 16.838 / 19.706 | 1,887 次 / 3,165,280 B | `e497270e9a458961` |
| 5,000 / 20,000 | 8 | 10.918 / 851.222 | 664.439 / 796.961 / 796.961 | 192 次 / 2,096,640 B | `3fc9b5c7e91574d5` |
| 10,000 / 40,000 | 6 | 76.609 / 5,679.561 | 4,479.241 / 5,263.490 / 5,263.490 | 156 次 / 3,145,344 B | `774298565b695ca5` |

中档尚未达到 6.7 ms 的 CPU 预算。单独跑中档的 AABB 与 Rapier 各 8 步，中位耗时分别为 1,597.062 ms 和 94.529 ms。中高档短样本的 p99 取最大值。复测低中档的分配计数和哈希一致，耗时随机器负载波动。GPU、磁盘加载与峰值内存仍待测。[S2 验收口径](../Docs/引擎框架设计.md#131-旗舰基准场景xg-bench)

AABB 加入空间树后，仍按实体 ID 求解，分离改变位置时刷新后续候选。碰撞密集时每个左体最多查询四次空间树，随后按 ID 扫描余下实体。旧版与新版的 1,570 条位置、速度位模式和有向事件逐字节相同，showcase 的 151 个检查点一致。[候选对回归](crates/xgc-sim/tests/aabb_candidates.rs)

同机相邻运行的 Release 对照中，低档 120 步的 p50 / p95 / p99 从 7.891 / 11.059 / 12.652 ms 降至 4.631 / 6.817 / 6.990 ms，中档 8 步从 577.655 / 709.270 / 709.270 ms 降至 63.963 / 76.487 / 76.487 ms。中档单独运行 AABB 的 p50 从 518.138 ms 降至 11.191 ms，预热后两版均为零分配。整步中档仍有 192 次 / 2,096,640 B 分配。Rapier 单独运行的 p50 为 30.258 ms，中档仍高于 CPU 预算。

在 `xgc/` 运行 `cargo test --locked --offline -j 2 --release -p xgc-sim --test physics_yard -- --ignored --nocapture --test-threads=1`。可按测试名选择档位，或用 `XGC_S2_SAMPLES` 指定样本数。

## 项目迁移的实体 ID

从 `xgc/` 先运行 `cargo run -q -p xgc-cli --offline -- migrate ../projects/XGame ../tmp/xgc-project --dry-run` 查看计划，去掉 `--dry-run` 后迁移至新目录。两种模式共用预检与报告，包含实际生成的组件数、项目资产引用数、实体 ID 引用数，以及按源文件和旧实体 ID 定位的待移植项。项目清单的内容根和有序 `search_paths` 共用复制清单，兜底资产及其伴生文件保留源项目的相对目录。GUID 冲突在预检时报出两条路径。XGame 实测生成 230 个组件、26 处资产引用和 16 处实体引用，组件场景中的 Spinner 与 Bobber 仍需移植运行逻辑。

单场景、批量和整项目迁移都按原顺序保存 `script.lua.paths`，场景与预制体使用同一投影，保留路径原文、GUID 和脚本文件字节。每个实体的挂载列表计为一个组件。运行时每个挂载各有一个 Lua VM，重排后保留路径相同的 VM。每个固定步在各钩子内按挂载顺序调用，摘除挂载时调用 `on_destroy(ctx)`，命令并入同一批结算。[迁移回归](crates/xgc-migrate/tests/project_migrate/scripts.rs) · [执行回归](crates/xgc-script/tests/mounts.rs)

项目先为场景分配 ID，再为预制体分配 ID。两组都按源文件路径和文件内旧 ID 排序。父节点和场景中的预制体成员来源 ID 随之重写，关卡清单保留原顺序。

预制体输出为 `.prefab.json`，伴生 `.meta` 随文件改名并保留 GUID。场景成员沿用 GUID，未登记 GUID 的成员使用目标项目相对路径。编辑器用所选成员的来源加载资产并 Revert。已有成员保留实体 ID，根节点保留放置变换与外部父节点，子节点恢复资产局部变换并重算世界变换。

编辑器的 Apply 将现有成员的组件值写回预制体，保留资产来源 ID、根变换和 GUID。写前核对场景与资产的磁盘原文，资产通过暂存文件一次替换，场景撤销历史仍归当前会话。实体或组件结构变化、脚本路径与插件注册键变化会被拒绝。跨文件结构性 Apply 与并发写入的锁仍待实现。[预制体验收回归](crates/xgc-editor/tests/prefab.rs)

输出根目录的 `entity-id-map.json` 与场景、预制体一起发布。`files` 中每项以 `source` 和 `target` 标识走过迁移的文件，`ids` 的键和值都是十进制字符串。例如 `{"source":"Scenes/a.ron","target":"scenes/a.json","ids":{"42":"2"}}` 表示该文件的旧实体 `42` 迁为 `2`。每个世界新增实体的初始分配位置都高于本次迁出的场景和预制体实体。单场景迁移保留旧 ID，供下文的逐帧对表使用。

## 迁移 Lua 接口审计

干跑和正式项目迁移都会输出 `lua-api` 段。新版名单来自生产 Lua 挂载环境，旧版名单固定自旧引擎的 `--export-script-api` 目录。182 个旧全局函数中，68 个同名引擎绑定已注册，112 个无对应函数，`error` 和 `warn` 同名但解析为 Lua 标准函数。报告按分类逐名列出缺项，并记录钩子、物理、碰撞、生成、变换及动画的结算时序。`projects/XGame` 的八个脚本使用 31 个不同的旧接口名，实测均有引擎绑定。逐名注册审计衡量可见性，具体语义由运行回归验收。[旧版名单](crates/xgc-migrate/src/legacy_lua_api.txt) · [运行时探测](crates/xgc-script/src/api.rs) · [迁移回归](crates/xgc-cli/tests/migrate_project.rs)

真实项目回归核对两种迁移的审计段相同、干跑未写目标目录、迁移后脚本和原文件逐字节一致。首行项目汇总和退出码沿用原协议。

## 项目只读检查

`xgc check <旧项目目录>` 检查迁移源。迁移后运行 `xgc check <输出目录>` 或 `xgc check <输出目录>/project.xgc`，检查清单里的场景、原生预制体、父子关系、跨文件实体 ID、预制体来源、GUID 与项目相对路径引用，以及清单登记的输入映射。检查过程只读文件，并按解析后的路径对引用的 Lua 脚本去重、编译。目录存在原生清单时始终采用原生格式。

退出码 `0` 表示检查通过，`1` 表示清单或入口场景无法读取，`2` 表示找到内容问题。随仓库提供的 XGame 项目迁移后报告 4 个场景、35 个场景实体、1 个预制体、8 个脚本引用、18 个已登记资产、0 个问题。Rust 插件组件当前不会执行，检查以提示报告。

## 用编辑器与播放器打开迁移项目

从 `xgc/` 运行 `cargo run -p xgc-editor --offline -- ../tmp/xgc-project` 打开入口场景。在层级面板的 `Scenes` 中切换关卡。`Play` 用项目脚本按 60 Hz 运行编辑世界的副本，`Pause` 后可 `Step` 一次或 `Resume`，`Stop` 返回原编辑世界、选择及历史。播放器可用 `cargo run -p xgc-player --offline -- ../tmp/xgc-project/project.xgc --scene character` 打开角色场景，加入帧数可无头运行。两个宿主都支持项目目录和清单文件，视口与脚本从项目根解析 GUID。两个窗口按同一 60 Hz 时钟推进，每次最多补 4 步。窗口输入使用清单的 `input` 路径，未登记时不绑定动作，登记的文件读取或解析失败时报告错误。

编辑器、播放器和 CLI 共用脚本步进、根运动和粒子结算。迁移后的角色场景在 `run-scripts`、`run-cooked`、原生及 cooked 播放器中各运行 120 帧，终态均为 6 个实体、哈希 `c123b7e8fd701709`。脚本出错时宿主报告脚本名及帧号。showcase 的 151 个轨迹检查点与四张同机严格截图通过。

用迁移后的 showcase 在原生编辑器的编辑态、Play 态分别抓取了 1280×720、1440×720、1920×720 截图。三档均显示 GUID 引用的狐狸模型，工具栏和场景路径均在窗口内，中文可读。Play 的血条和按钮在同一画面内，编辑器与播放器共用 1280×720 的 HUD 逻辑坐标，并按显示矩形换算指针。输入回归把两次重绘之间的按下与释放送进 Play，在随后补跑的三步中 Lua 只收到一次点击，Stop 后编辑世界保留原值。指针按绘制顺序捕获最上层控件，拖出视口仍保留按下状态，在视口外松开、失焦或暂停后单步都会清除捕获。回到原控件松开只触发一次点击。[Play 回归](crates/xgc-editor/src/app/hud_tests.rs)

Lua 的 `action_down`、`action_pressed` 和 `action_released` 分别读取按住状态、按下边沿和松开边沿。两个窗口共用固定步消费边界，短按即使在两步之间开始并结束，按下和松开也各送达一次。组合键按主键事件的修饰键匹配，同一动作的多个绑定各自贡献边沿。编辑器暂停清除待消费边沿，窗口失焦还会清空按住状态。[Lua 输入回归](crates/xgc-host/tests/input.rs) · [动作映射回归](crates/xgc-sim/tests/input.rs)

HUD 图片按项目相对路径或 GUID 从同一资产目录读取。GPU 回归核对了两个引用形式的像素、跨帧缓存和缺失图片行为。图片读取只访问项目根及 GUID 目录。[图片回归](crates/xgc-render/tests/ui_image_fill.rs)

## 打包依赖

`xgc pack <世界.json> <项目根> <输出目录>` 从组件 schema 识别的资产引用出发，递归保留预制体、材质、glTF 外部缓冲和贴图及对应的 `.meta`。依赖成环时每个文件只处理一次。模型、脚本和音频的引用先查项目根，再按清单中的 `search_paths` 顺序查找。体检、宿主和打包共用该解析规则。打包保留实际命中的文件位置，`cook.json` 第 2 版签名包含搜索顺序及输入映射，旧包须重新生成。回归将源项目移开后运行 cooked 世界，验证脚本仍按原顺序命中、终态哈希一致、登记的输入映射可用。另以带外部缓冲与贴图的狐狸模型验证依赖可独立加载，并对比重复打包的清单。[搜索根回归](crates/xgc-cli/tests/cooked_search_paths.rs) · [依赖回归](crates/xgc-cli/tests/native_project_check.rs)

`pack` 在目标同级目录组包，回读 JSON 与 XGCB 世界并校验清单及文件哈希，然后持有父目录的长期锁发布。写入失败保留可运行的上一版，下一次打包会先根据目录和事务标记恢复上次中断。回归覆盖并发发布、三个中断边界和无归属目录保护。恢复验收使用预置目录状态，适用范围是本地文件系统上的协作发布。[发布回归](crates/xgc-cli/tests/cooked_publish.rs)

## 项目画面设置

迁移将旧项目的 `render` 和 `quality` 写进 `project.xgc`，组包时纳入 `cook.json` 签名。编辑器、播放器和 CLI 共用画面配置，覆盖曝光、色调映射、天空、光晕、FXAA、阴影软化、TAA、SSAO、LOD 偏置和画质档。MSAA 按设备能力选择 1× 或 4×。灯光显式设置的阴影分辨率优先，阴影采样与级联拟合使用实际附件尺寸。[画面设置](crates/xgc-base/src/render_settings.rs) · [GPU 曝光回归](crates/xgc-render/tests/exposure.rs)

在 RTX 2060 Vulkan 610.88 上，同一场景以旧项目配置、迁移后的原生项目和 cooked 包各渲染一张 640×360 图，三张逐字节一致。showcase 的 151 个轨迹检查点及四张同机严格门禁图通过。四张图的总差异为 0.49%、1.07%、1.35%、1.54%，结构差异为 0.03%、0.08%、0.13%、0.26%。

## Agent 查询与重放

`world.query` 按持久实体 ID 升序返回结果。继续翻页时将上一页的 `next_cursor` 填入 `cursor` 并沿用筛选条件。写事务或更换世界后旧游标失效。查询先筛选 ID，只为当前页读取投影字段或完整实体。投影查询可指定 `format: "columns"`，返回按 `select` 排列的类型目录和数组行，首列为十进制字符串 ID。缺失字段为 `null`，组件中的实体引用也用十进制字符串。MCP 的 `world_query` 以紧凑 JSON 返回列式结果。[查询与 MCP 回归](crates/xgc-agent/tests/query.rs)

同机 Release 在 12,000 实体中筛选并查询 1,000 行、每行 4 个字段，预热 4 轮后采样 40 轮。JSON-RPC 行式响应的 p50、p95、p99 由 7.993、9.268、9.425 ms 降至 3.832、3.941、4.019 ms。列式响应为 1.808、1.854、1.889 ms，MCP 列式 p99 为 2.354 ms。200 行的 MCP 正文由 124,457 字节降至 12,855 字节，距约 10 KB 的目标仍有差距，省略缺省值尚待实现。100 操作事务的计划与提交合计 p50、p95、p99 为 1.159、1.192、1.227 ms。回归检查了每次事务 revision 加一、全部修改生效与日志重放哈希一致。计时包含协议解析和响应序列化，进程间传输延迟另计。[Release 测量](crates/xgc-agent/tests/query.rs)

`edit.undo` 与 `edit.revert` 成功后会记录逆向事务。回归覆盖误删、恢复、后续编辑与重放，逐项比对世界状态、哈希及 revision。[协议回归](crates/xgc-agent/tests/protocol.rs)

`edit.apply` 与 MCP 的 `edit_apply` 支持可选 `base_revision` 和 `idempotency_key`。首次提交的版本冲突会保留计划。提交成功后，同一会话使用原计划和键重试，可取回最初的 `tx_id`、`revision` 和 `inverse`，实体和日志只变更一次。成功加载或恢复世界后重新开始记账。[提交回归](crates/xgc-agent/tests/edit_apply.rs) · [MCP 回归](crates/xgc-agent/tests/mcp.rs)

`world.restore` 和 `world.load` 先构造完整世界，成功后才重置编辑基线。恢复失败时，现有快照、查询游标、待应用计划和撤销历史可以继续使用。即使恢复后的 revision 与原世界相同，成功恢复也会使旧游标失效。[恢复回归](crates/xgc-agent/tests/restore.rs)

`sim.run` 与 MCP 的 `sim_run` 从编辑世界的副本预览最多 300 个固定步，返回第 0 帧起的哈希和最多 16 个实体字段的采样值。指定原生项目目录或清单时走共享脚本宿主，从项目解析 GUID 与搜索根。省略时只执行模拟系统。预览成功或失败后，编辑快照、查询游标、待应用计划和事务历史仍可继续使用。[预览回归](crates/xgc-agent/tests/sim_run.rs) · [MCP 进程回归](crates/xgc-cli/tests/mcp_sim_run.rs)

传入 `record: true` 可取得 `trace_id`。`sim.replay` 与 MCP 的 `sim_replay` 从该记录的世界和固定步长重跑，项目资产按回放时的文件重新读取。每条连接保留最近一次成功记录。回放在首个哈希分歧处停止，返回帧号、期望和实测哈希以及有变化的采样字段。实际 MCP 进程回归把一段已跑过的 Lua 改为从第 2 帧起偏移位置，回放报告该帧的 X 坐标由 2 变为 11。第 3 帧故意抛错，回放到第 2 帧已结束。编辑快照逐字段一致，JSON-RPC 回归另验证编辑世界继续修改后仍能重放已记录的起点。[MCP 进程回归](crates/xgc-cli/tests/mcp_sim_run.rs) · [记录生命周期回归](crates/xgc-agent/tests/sim_run.rs)

`world.explain` 与 MCP 的 `world_explain` 共用渲染器的模型抽取判据。给定相机实体或眼点时返回选中的模型，或报告缺少组件、全局变换、模型引用以及超出有限 LOD 距离等排除原因。结果覆盖资产加载前的抽取阶段。[诊断回归](crates/xgc-agent/tests/explain.rs)

`script.check` 及 MCP 的 `script_check` 接受内联 Lua 源码，`xgc script-check <脚本.lua>` 读取单文件。这两个入口和项目体检都调用运行时的 Lua 5.4 文本编译器，语法错误为 `SCRIPT.COMPILE`，包含可确定的脚本名与行号。检查不执行脚本。 [脚本检查回归](crates/xgc-agent/tests/script_check.rs)

Lua VM 默认使用 128 MiB 内存上限。每次钩子调用有约 20 万条指令额度，调用内协程共用额度。超限结束当前调用，后续钩子仍可使用同一 VM。Lua 文件由宿主装载，运行时动态编译只接受文本。回归覆盖会恢复的协程、受保护调用、内存诊断和下一次调用恢复。引入沙箱限制时，两项回归分别核对了 showcase 的 151 个脚本轨迹检查点和角色场景的第 120 帧哈希。[沙箱回归](crates/xgc-script/tests/sandbox.rs)

在含 4,096 个背景实体的脚本回归中，Lua 每帧读取其他实体的名称和局部位置，再写回自身位置。帧视图直接读组件并复用实体行，保留按持久 ID 的同名首占规则。预热后测 60 帧，同机 Release 三轮均从 12,294,840 次分配、1,937,328,660 字节降至 1,500 次、106,080 字节。耗时从 1.624–1.703 秒降至 32.28–59.14 毫秒。数值覆盖 Lua 调用和命令结算，showcase 的 151 个轨迹检查点逐字节一致。[脚本帧分配回归](crates/xgc-script/tests/view_allocations.rs)

Lua 的 `print`、`warn`、`log(ctx, ...)` 与 `xgc.log` 统一写入 stderr。CLI 的逐帧轨迹和 Agent 的 stdio 响应可直接按各自协议解析。[日志与轨迹回归](crates/xgc-cli/tests/trace.rs)

脚本重载先在每个挂载的 VM 内初始化候选环境。全部候选通过后才切换活动代码，保留 `state_*` 状态并清除新版已删掉的钩子。候选的实体、UI、音频写入在初始化后撤回。回归覆盖同实体多个挂载的整批替换、语法或顶层错误后继续调用旧代码，以及下一步读取原持久状态。showcase 的 151 个检查点和角色场景第 120 帧完整快照保持逐字节一致。[重载回归](crates/xgc-script/tests/mounts.rs)

编辑器 Play 每秒比较已装入 Lua VM 的源文件修改时间和大小，暂停期间仍会检查。变更批次在固定步和工具栏单步之前切换，改坏或删除脚本时旧模块继续运行，修改后的下一批会重试。回归覆盖 GUID 挂载、删除与重建、同批失败和无改动挂载的局部变量保留，并验证 Stop 后编辑态及文件原文。[编辑器回归](crates/xgc-editor/src/app/reload_tests.rs) · [批次回归](crates/xgc-script/tests/mounts.rs)

## 脚本物理固定步

Lua 的 `set_velocity` 对动态和运动学刚体在当前钩子内可用 `velocity` 读回。脚本批次提交后，Rapier 在下一固定步解算前按调用顺序消费指令一次，随后速度由解算推进。这个边界让重力继续累积，并让没有 `core.velocity` 的刚体也能响应。静态刚体保持固定，非刚体可继续写入 `core.velocity`。重载预检会撤回候选指令。没有 `core.velocity` 的刚体在下一钩子的 `velocity` 读回，以及运动学刚体的跨步速度读回，仍待与旧端对表。

在 `f77c354` 上，真实 Lua 发出一次 6.5 m/s 的向上速度命令，下一步 Rapier 回写 −0.4905001 m/s。修复后回写 6.3365 m/s，三轴实际位移符合四子步积分，后续一步继续受重力。碰撞钩子回归验证进入时的最后一条速度指令推进无速度组件的刚体，退出时的指令使其停止。运动学回归从物理查询确认碰撞体位移。[共享宿主回归](crates/xgc-host/tests/physics_scripts.rs) · [运动学回归](crates/xgc-sim/src/rapier_world.rs) · [批次重载回归](crates/xgc-script/tests/mounts.rs)

同机 Release 预热后各测 60 帧。210 实体混合场景的 438 次分配、45,424 字节和八层父链运动学场景的 180 次分配、5,760 字节前后相同。1,026 控件的 HUD 布局仍为零。脚本帧视图的 4,096 背景实体用例测得 1,261 次、75,517 字节。工作区全目标检查、严格 Clippy、xtask 和八个受影响包的 487 项测试通过，另有 6 项测试忽略。两次 schema 导出与修改前逐字节相同。showcase 的 151 个检查点在串行和四线程下均与参考一致，同机四张严格截图通过。角色场景四个入口在第 120 步得到 6 个实体和 `c123b7e8fd701709`，完整快照与修改前一致。[仿真分配回归](crates/xgc-sim/tests/steady_state.rs) · [帧视图分配回归](crates/xgc-script/tests/view_allocations.rs)

## 脚本位置读回

Lua 在同一钩子中调用 `set_pos` 或用 `xgc.set` 写 `core.transform:position` 后，`pos`、`entity_pos` 和 `look_at` 使用最近一次写入的位置。世界字段仍由固定步里的脚本批次结算。热重载候选初始化也可即时读回，初始化结束时还原位置。当前接口仍以本地位置为准，带父节点的世界坐标是 Lua 对等的开放项。[朝向回归](crates/xgc-script/tests/legacy_look_at.rs) · [重载回归](crates/xgc-script/tests/mounts.rs)

showcase 的第 1、150、200、300 帧对照完整快照，仅主相机的本地与全局旋转可能变化。对同一个钩子计算出的视线方向，第 300 帧的单位向量误差由 `2.28933e-4` 降到 `4.13598e-8`。第 0、1 帧哈希保持原值，串行与四线程的 151 个检查点逐字节一致。[当前轨迹](acceptance/trace.txt)

RTX 2060 / Vulkan / 610.88 同机复用旧引擎四张参考图，严格总差异为 0.49%、0.51%、0.78%、0.65%，结构性差异为 0.03%、0.02%、0.06%、0.06%。角色场景四个入口在第 120 步仍为 6 实体和 `c123b7e8fd701709`，完整快照逐字节一致。Release 帧视图在 4,096 背景实体下预热后测 60 帧，前后均为 1,261 次分配、75,517 字节。工作区全目标检查、严格 Clippy、xtask 与六个包的 233 项测试通过，另有 1 项忽略。两次 schema 导出与改前逐字节相同。[帧视图分配回归](crates/xgc-script/tests/view_allocations.rs)

## 脚本音频与混音

`play_sound`、带位置和总线的播放、停止、监听者及音量控制通过共享宿主在固定步结束时结算。片段按项目搜索顺序或 GUID 解析。同一会话内复用解码后的 WAV，现有声部随总线音量和监听位置更新。48 kHz 单声道 PCM 使用调用方缓冲，按源采样率及 pitch 采样。测试核对了命令先后、现有声部的音量变化、跨块循环及采样值。44.1 kHz 的 `Beep.wav` 经 CLI 同采样率混音，前后 WAV 逐字节一致。[宿主混音回归](crates/xgc-host/tests/audio.rs) · [采样回归](crates/xgc-sim/tests/audio.rs)

编辑器 Play 和独立播放器按固定步向同一设备输出层提交 PCM。每个窗口在首次有声时打开一条流，重采样相位跨块延续，队列最多保留约 100 毫秒。暂停、停止和脚本错误清除旧采样。测试覆盖追帧时三个音频块按序提交、缓冲耗尽时补静音及恢复后从新采样开始。本机默认设备已打开 48 kHz 输入流并提交 PCM。[Play 回归](crates/xgc-editor/src/play.rs) · [设备缓冲回归](crates/xgc-audio/src/pcm.rs)

场景中启用 `autoplay` 且登记片段的 `audio.source` 在存续期间启动一次。空间声部跟随实体的世界位置，源组件移除时只停掉该实体的声部。回归用 Lua 移动父节点、移除和重挂音源，并核对同片段的未绑定声部不受影响。播放生命周期不会新增实体或改写组件。[音源生命周期回归](crates/xgc-host/tests/audio.rs)

## glTF 动画插值

glTF 的三次样条 TRS 轨道保留入切线、关键值和出切线。姿势和根运动共用 Hermite 采样，切线按段长缩放。旋转在采样后归一化，退化时用关键帧间的球面插值。回归覆盖两秒段、循环根运动、退化旋转，以及错误输出组数和越界访问器。[glTF 动画回归](crates/xgc-render/tests/animations.rs)

## 结构化反射与文件版本

持久实体 ID 保留 `u64::MAX` 作为分配游标耗尽标记，`u64::MAX - 1` 仍可使用。JSON、XGCB、世界恢复和 Spawn/Replace 在接收实体时共用这一校验。回归覆盖非法输入保留原快照和实体句柄，以及 Agent 采样、编辑器新建和 Lua 生成实体的错误反馈。[世界边界回归](crates/xgc-world/tests/entity_ids.rs) · [Lua 边界回归](crates/xgc-script/tests/entity_ids.rs)

Inspector 的 `animation.animator → events` 可以增删事件，逐项编辑时间和名称，并撤销修改。Agent 使用 `animation.animator:events[0].time` 查询，在 `edit.plan` 的组件字段中写 `events[0].time`。路径下标从 0 起。项目 Lua 的 `xgc.set` 也使用零基路径改写叶字段。

`render.lod → levels` 同样可以增删和逐项编辑。模型与最大距离分别是 `levels[0].model` 和 `levels[0].max_distance`，距离单位为米，零或负值代表无限距离。项目检查会报告具体层级的模型引用，打包会保留仅由 LOD 引用的相对路径和 GUID 资产。运行时按实体世界轴心到相机的距离选级，超出全部有限距离时主绘制、方向光和点光阴影一起跳过。取景和根运动使用首个可绘制模型。CLI、原生编辑器与播放器使用同一项目 LOD 偏置与画质档缩放。GPU 回归确认超距与移除实体逐像素相同，showcase 的 151 帧轨迹与同机四张严格截图通过。

递归字段支持字符串键映射与带载荷的枚举。`settings["main.gain"].Scaled.factor` 先取映射中已有的键，再取当前 `Scaled` 分支的载荷。键按 JSON 字符串规则转义。Inspector 用增删条目或切换分支替换容器整值，切换时使用该分支的缺省载荷。Agent 通过同一字段路径读取和更新，事务可撤销。

基础动画状态机直接保存机器定义与运行参数。Inspector 和 Agent 可编辑 `machine.states[0].clip`、`machine.transitions[0].when.AtLeast.value` 与 `parameters["speed"]`。转移来源可选任意状态或具名状态，条件支持无条件、达到阈值和低于阈值。模拟直接读取类型化机器，Lua 的 `set_anim_param` 同帧写入累积在同一张参数表。

`animation.layers → layers` 是有序混合层列表。每层可以按路径编辑模式、遮罩、参数和机器，例如 `layers[0].machine.Enabled.states[0].clip` 与 `layers[0].parameters["survey"]`。基础和分层状态机共享字段定义。模拟和渲染直接使用类型化的层栈，Lua 的按名 setter 与 `xgc.set` 按调用顺序结算。

生成式 Inspector 从反射 schema 读取数值边界、单位、步长、分组和只读标记。动画事件时间以秒显示，方向键步长为 0.05 秒，普通拖动按每逻辑像素 0.05 秒换算并由 egui 智能取整，输入最低为 0。基础和分层动画的转场运行状态显示在 `Blend State`，可查看但不能通过面板逐项写入。CLI 的 `schema` 和 Agent 的 `engine.describe` 输出可选的 `step`、`group`、`read_only`。只读标记描述界面行为，现有命令、运行时及存储仍使用原字段和校验。

`render.lod` 从 Rust 声明派生字段描述、读取和写入。`#[derive(Component)]` 从组件的 `Default` 取得缺省值，`#[derive(ReflectValue)]` 描述列表里的具名结构体，列表和字符串键映射复用元素的投影。字段声明旁可设置单位、边界、步长、分组、只读、说明与资产用途，组件字段可用 `skip_hash` 标记不计入回放哈希的作者数据。项目检查、迁移报告和打包共用 `asset` 字段元数据遍历引用，包括 LOD 列表里的模型引用。生成代码继续使用已有的 `FieldDescriptor`、`Value` 和 `Component` 接口，LOD 的字段名、顺序和缺省值保持原契约。

世界、预制体与事务日志使用 JSON 包络 v6，cooked 世界使用 XGCB v7。仓库内当前原生世界样例已按 `script.lua.paths` 一次性重写。旧版迁移输出须从源 RON 重新生成到新目录，cooked 包须重新打包。手工修改过的旧原生输出需要保留原件，供重新应用修改时核对。

## 可复现命令

在验收机从仓库根目录运行 [截图参考重生成流程](acceptance/README.md#在验收机重生成截图)。脚本从旧引擎的实际截图报告提取适配器指纹，用仓库轨迹和严格截图阈值校验完整参考集。下列命令用于单项对照。

```sh
# 迁移 + 数据对照
cargo run -q -p xgc-cli --offline -- migrate ..\projects\XGame\Scenes\showcase.ron ..\tmp\showcase.json
cargo run -q -p xgc-cli --offline -- verify-migration ..\projects\XGame\Scenes\showcase.ron ..\tmp\showcase.json

# 统计对照（旧 headless JSON 由旧 xgame-editor 生成）
xgame-editor --headless --frames 60 projects/XGame > old.json
cargo run -q -p xgc-cli --offline -- compare-headless old.json ..\tmp\showcase.json

# 逐帧 trace（XGC 侧）：每帧状态哈希，可直接 diff 两次运行；
# 口径是「已推进 N 步」的原始世界，不含 render-frame 收尾的层级 / UI 重算
# （showcase 150 步末行 hash `69188393f5f33383`、带资产根 `adfe898315f33383` ≠ render-frame 的 `cfce1746f5f33383`：render-frame 收尾会再跑层级 / UI 重算）
cargo run -q -p xgc-cli --offline -- trace ..\tmp\showcase.json 150
# 带资产根时走脚本路径（与 run-scripts 同一条逐帧路径，脚本帧也逐帧打印）
cargo run -q -p xgc-cli --offline -- trace ..\tmp\showcase.json 150 ..\projects\XGame
# 线程数显式开启批内并行（默认 1 = 串行）：结果哈希必须与默认一致
cargo run -q -p xgc-cli --offline -- trace ..\tmp\showcase.json 150 ..\projects\XGame 4
cargo run -q -p xgc-cli --offline -- run-world ..\tmp\showcase.json 60 4

# 视觉对等判定（拆分轮廓带与内部；状态对齐帧按严格口径，其余帧由 gate 端到端口径守）
cargo run -q -p xgc-cli --offline -- parity acceptance\frame-1.png ..\tmp\par-1.ppm
cargo run -q -p xgc-cli --offline -- parity acceptance\frame-150.png ..\tmp\par-150.ppm

# 割接门禁（逐帧 trace 对表 + 截图阈值判定；参考集在 xgc/acceptance/）
cargo run -q -p xgc-cli --offline -- gate ..\tmp\showcase.json ..\projects\XGame acceptance

# 执行计划（确定性 JSON：系统访问声明 + 连续批次 + 宿主阶段）
cargo run -q -p xgc-cli --offline -- plan

# 跨端终态对表：旧端没有逐帧 trace 流，用固定帧数 checkpoint 逼近；
# 旧端 --dump-scene 出任意帧数终态 RON，XGC run-scripts 第 4 参数出同形世界 JSON，再 migrate 后逐字段对比
cd ..
cargo run -q -p xgame-editor --offline -- --headless --frames 150 --dump-scene ..\tmp\old-150.ron projects\XGame
cd xgc
cargo run -q -p xgc-cli --offline -- run-scripts ..\tmp\showcase.json ..\projects\XGame 150 ..\tmp\xgc-150.json
cargo run -q -p xgc-cli --offline -- migrate ..\tmp\old-150.ron ..\tmp\old-150.migrated.json
python ..\tmp\compare_states.py ..\tmp\old-150.migrated.json ..\tmp\xgc-150.json

# 视觉指标：先在根 workspace 生成旧引擎第 150 帧，再回 xgc 生成同帧
# 注意：旧端参考截图必须在验收机重新生成。仓库里检入的 `tmp/old-150.png` 来自另一台
# GPU / 驱动环境（±1 舍入整体偏移，15.8% 像素不同），直接对比会得到 18.43% 的假差异；
# 本机重渲染的 `tmp/old-150-now.png` 才是正确基线（材质因子后 2.39%、内建图元后 2.38%、帧首建体后 2.16%）。
cd ..
cargo run -q -p xgame-editor --offline -- --headless --frames 150 --screenshot tmp\old-150-now.png projects\XGame
cd xgc
cargo run -q -p xgc-cli --offline -- render-frame ..\tmp\showcase.json ..\tmp\new-150.ppm ..\projects\XGame 1280 720 150
cargo run -q -p xgc-cli --offline -- image-diff ..\tmp\old-150-now.png ..\tmp\new-150.ppm 0 report

# 材质垂直切片：真实 Box + 合成蒙皮 GLB 都必须消费 baseColorFactor
cargo test -p xgc-render --test material_factor --offline

# 模型材质因子：实体没有 render.material 时用 glTF 自带 metallic / roughness（spec 缺省 1.0）
cargo test -p xgc-render --test glb --test mesh --offline

# 物理首帧口径：刚体表必须在速度积分之前建好（旧端 sync_bodies() 同序）
cargo test -p xgc-sim --test rapier_first_frame --offline

# glTF 动画剪辑：模型自带剪辑列表（确定性）+ 解析 / 采样 / 迁移回归
cargo run -q -p xgc-cli --offline -- clips ..\projects\XGame\Resources\Fox.gltf
cargo test -p xgc-render --test animations --offline
cargo test -p xgc-cli --test clips --offline
cargo test -p xgc-migrate --test animation --offline

# 内建图元与旧端逐顶点一致：cube 24 顶点带面法线 / UV、sphere(0.5, 24, 32) 平滑法线、cylinder 32 段、plane +Y 法线
cargo test -p xgc-render --test mesh --offline

# 方向光阴影：开关两帧对比（暗像素数量 + 只会变暗）（GPU）
cargo test -p xgc-render --test shadows --offline

# CI 本地等价（`.github/workflows/xgc.yml`，windows-latest、无 GPU）
# check job = fmt --check / clippy -D warnings / xtask check / CLI 冒烟三连（check、run 60、plan）
# test job = 12 个不碰 wgpu 设备的包 + 14 个 CPU-only render 测试目标 + release 吞吐基线（只打印不设阈值）
# 本机等价命令就是 workflow 里两个 job 的 `run:` 行，逐条实跑即 dry run

# 蒙皮投射阴影：GPU palette / 257 关节 CPU 回退 / 阴影跟随当前姿态 / 与静态网格逐像素等价
cargo test -p xgc-render --test skinned_shadow --offline

# 角色三轴位移都在 rapier 角色阶段：水平 collide-and-slide + 竖直重力 / 射线探地 / 贴地吸附 / 台阶三段扫掠
# 陡坡：超过 `slope_limit + 10°` 迟滞带沿坡面下滑（迟滞带内站得住），顶着坡走仍按竖直墙挡
cargo test -p xgc-sim --test character_kcc --test character_ground --offline
# 刚体表：改 `PhysicsBody` 参数会重建、稳态帧不重建
cargo test -p xgc-sim --test rapier_schedule --offline

# 角色水平 KCC（纯 CPU）：胶囊 collide-and-slide、撞墙停住、斜墙沿墙滑、球体阻挡、刚体优先托管、
# `Replace` 换代后仍能移动、默认调度下不挤进陡坡、确定性
cargo test -p xgc-sim --test character_kcc --offline

# 角色控制器缺省值与旧端逐项对齐、消毒收口在组件上（组件单元回归）
cargo test -p xgc-world character_components --offline


# 坡面碰撞体（纯 CPU）：60° 陡坡挡住角色 / 上表面与解析坡面逐点对齐 / 改坡面参数后重建 / 带 physics.body 的坡让位 / 尺寸退化与非有限参数被跳过 / 下坡同样对齐
cargo test -p xgc-sim --test slope_collider --offline

# 粒子：确定性 / 发射率 / 清洗与硬上限（CPU）+ Alpha / Additive billboard（GPU）
cargo test -p xgc-sim particles --offline
cargo test -p xgc-render --test particles --offline

# GPU 蒙皮：静态 PBR/贴图逐像素同值、关节带贴图移动、>256 CPU 回退、256 槽 palette、阴影接收、竞技场跨帧复用
cargo test -p xgc-render --test skinned_pbr --offline

# HDR：resize / 输出格式 / alpha，粒子线性合成，tone 后 UI
cargo test -p xgc-render --test hdr --test particles --test frame_ui --offline

# 光晕：关闭态逐位等价 / 开态对旧端公式对表 / 暗场景不发光 / resize 重建链（GPU）
cargo test -p xgc-render --test bloom --offline
# 曝光：缺省 1.0 逐字节不变 / 加倍变亮并对表 ACES / 光晕阈值按曝光后判定 / 改曝光重建光晕链
cargo test -p xgc-render --test exposure --offline
# 软阴影：缺省逐字节不变且不多跑 pass / 非法值落回硬阴影 / softness 加宽过渡带 / cascade_blend 经 CPU 重叠拟合改动阴影采样相位（本机机位下 GPU 混合分支不执行）
cargo test -p xgc-render --test shadow_soft --offline
# 多光源照明：方向光 / 点光叠加、点光衰减与 8 槽上限
# 点光立方体阴影：六面深度图、局部影子、范围外 / 超预算不投影
cargo test -p xgc-render --test point_shadows --offline

# 显示空间 FXAA：关闭态逐字节不变 / 硬边缘被柔化且只动边缘 ±2 内 / 纯色清屏不变 / 尺寸与输出格式重建 / sRGB 目标混合在显示空间 / 光晕叠加 12 趟（GPU）
cargo test -p xgc-render --test fxaa --offline
cargo test -p xgc-render --test taa --offline

# 天空盒画面：解析天空逐像素对表 + 几何遮挡（GPU）
cargo test -p xgc-render --test sky_box --offline

# 天空漫反射 IBL：SH 镜像对表 + 天空开关（GPU）
cargo test -p xgc-render --test sky_ibl --offline

# 材质资产：解析 + 乘子语义 + 悬空引用降级（GPU）
cargo test -p xgc-render --test material_asset --offline

# 场景光 / 环境光：迁移 + 验收 + 取景（无需 GPU）
cargo test -p xgc-world --test environment -p xgc-migrate --test migrate --test verify --offline
cargo test -p xgc-render --test shot --offline

# cooked 播放
cargo run -q -p xgc-cli --offline -- pack ..\tmp\showcase.json ..\projects\XGame ..\tmp\cooked
cargo run -q -p xgc-player --offline -- ..\tmp\cooked 60
```

> 本切片的旧实现红线是：真实 `Box.glb` 声明 `baseColorFactor=[0.8,0,0,1]`，中心像素却为
> `[255,255,255,255]`；修复后静态 / GPU 蒙皮均消费模型因子，showcase 左侧三层 Box 恢复红色。
> 旧端截图相机已实测为 `mode: "scene"`（重跑旧 `xgame-editor` 读报告，两次运行图像逐像素相同），
> 因此 XGC `render-frame` 的优先级是"有场景相机就用场景相机，缺相机才顺光自动取景"；
> 顺光自动取景（包围盒拟合 + 场景主方向光推视轴）保留为兜底，参数与旧端逐项一致。
> 天空按项目级配置读取（`project.ron` 的 `render.sky`），CPU 把解析天空投影成 L2 球谐
> （E/π，1024 点斐波那契球，无随机数）后与 `ambient` 相加，片元侧逐项对表。
> 场景级光源 / 环境光已迁移进 `WorldData.environment`；颜色合成顺序对齐旧引擎
> `模型基色因子 → .mat 资产 → 实体覆盖`（全是乘子），线性片元与清屏色先进入 HDR 场景，帧末统一过 ACES。
> GPU 蒙皮现在只保留顶点变形入口，片元合同与静态实例共同拼接唯一的 `scene_shading.wgsl`：基色因子 / metallic / roughness、sRGB 基色贴图、ambient / 天空 SH、Cook-Torrance 与 4 级 CSM 阴影接收全部同源；ACES 只有 `tone_map.wgsl` 一份。四条 GPU 回归分别钉住 identity 与静态逐字节相同、关节平移带着非均匀贴图移动、超出 palette 容量的 CPU 回退不丢 UV / 贴图（本刀起门槛 257 关节）、静态投射体能让 GPU 蒙皮表面变暗。该切片第 150 帧 PPM 与上一版 SHA-256 相同；release `render-frame` 新进程冷启动中位数由 730.44 ms 变为 806.42 ms（+10.4%，含新增 shader / pipeline 编译；当前 GPU 蒙皮还按实体单独建 uniform / bind / encoder 并提交，不代表稳态帧耗时）。
> HDR 切片让几何 / 天空 / 粒子写线性 `Rgba16Float`，Alpha / Additive 先合成、全屏三角形再执行一次 ACES，UI 最后叠加；第 150 帧 PPM SHA-256 `4A47275DD54A4AA2780353B6B37E6BD2999F9D16BEDA20197973BA6DB48A0927`，世界 hash 与 14/7/8 draw 统计不变。相对上一版 XGC 的差异是 6.19% / mean_abs 0.04 / max 13，相对旧端改善为 32.22% / **0.61** / 194。冷启动 release 四跑中位数 818.57 → 741.66 ms（-9.4%），只判定未见回归；该测量包含启动 / 设备 / pipeline 编译，不代表稳态帧耗时。
> 阴影画质切片按旧端 `ShadowConfig` / `scene_pipeline.rs` 合同接上**可选 PCSS 软阴影 + 级间混合**（`render.shadow` 的 `softness` / `cascade_blend`，默认全 0 = 关、关着走 3×3 PCF + 硬级间切换且逐位不变）：`softness > 0` 时 8 抽头固定泊松盘找遮挡物平均深度、`penumbra = clamp((depth - blocker) * softness * 12, texel, 24 * texel)`、再 12 抽头 `textureSampleCompareLevel`；`cascade_blend > 0` 时切分带里按 `t` 混下一级，同一个旋钮让 `cascade_shadow_matrices` 把非 0 级拟合框近端多铺 `overlap × 本级跨度`（否则带内片元落在下一级图外、被「图外不混」守卫退回硬切换）。帧 uniform 592 → 608 字节；第 150 帧 PPM SHA-256 不变（`4A47275D…`、14/7/8、hash `bb3fdc99df20aa31`）、`run 60` 哈希不变。4 条 GPU 回归 + 3 条消毒单测 + 2 条级联重叠回归 + 1 条经 `load_render` 入口的阴影解析回归钉住缺省逐字节等价且 `passes` 不增、非法值落回硬阴影、`softness = 6` 的中间灰阶 24 → 20086 个像素、`cascade_blend = 0.05` 经 CPU 重叠拟合改动阴影采样相位、多铺后 `view_range` 近端严格更靠近相机；红证据：着色器 `frame.shadow_soft.x > 0.0` 改 `> 999.0`（「软阴影必须真的改变画面」）、旋钮的 `cascade_blend` 恒 0（「级间混合必须接进 CPU 重叠拟合，否则分带仍是硬切换」）、重叠量改 0（「第 1 级的近端必须更靠近相机」）。**未闭环**：本机机位下混合分支本身改不动像素，GPU 回归钉的是旋钮接线与重叠拟合；点光阴影仍未做；没做稳态计时。
> 点光立方体阴影切片补上多光源的最后一环：`crates/xgc-render/src/point_shadow.rs` 按旧端 `pick_point_shadow_slot` 同判据挑**第一盏** `casts_shadow` 且 `range > 0` 的点光（只在前 8 槽里找），`far = range.max(0.05)`、`near = (far * 0.005).max(0.02)`，六面投影 = `flip_u · perspective(90°) · look_at`（up 照 WebGPU 立方体贴图映射表挑、u 轴镜像），参考深度按主轴分量 `max(|dx|,|dy|,|dz|)` 算 `(far/(far-near))·(1 - near/axis)`（用欧氏距离会让斜面整片变黑）。`crates/xgc-render/src/shadow.rs` 复用方向光那支深度管线与实例缓冲：每帧六面各开一次 depth pass（清 1.0），蒙皮投射体也进这六面，`RenderStats::point_shadow_draw_calls` = 六面 × 投射体数；没有点光阴影时绑 1×1 立方体假图、着色器 `params.z = 0` 整段跳过。接收端 `point_shadow_factor` 逐行对齐旧端（范围外 / 贴脸返回 1.0、偏置随 N·L 放大、3×3 `textureSampleCompareLevel`）。帧 uniform 976 → **1008 字节**（+32 B：灯位置（w = 远平面）+ 参数；六面矩阵不进主帧 uniform，只喂阴影 pass 每面 64 B 的帧缓冲），`FrameLighting` / 绑定布局同时拆进 `crates/xgc-render/src/frame_uniform.rs`、`renderer.rs` 的目标准备拆进 `renderer/shadow_prep.rs::ensure_frame_targets`（守 600 行软上限）。证据世界没有点光：第 150 帧 PPM 与 `run 60` 哈希都不变（`4A47275D…`、14/7/8、hash `bb3fdc99df20aa31`、`f4bc029f5f995b91`）。回归 5 条 GPU + 7 条矩阵单测；红证据：去掉点光的 `shadow_params[2]` 标记（「点光必须在地面上投出可见阴影：0 个像素变暗」）、参考深度写成常数 1.0（「点光阴影必须是局部的，不该把整帧压黑：20736 / 20736」）、`pick_light` 去掉 `casts_shadow` 判据（「关掉 casts_shadow 不该跑立方体阴影」与「第 9 盏灯不在 8 槽光源表里，不该为它跑立方体阴影」）。**未闭环**：没有逐面掩码剔除（六面各自画全部投射体，像素正确但多画几遍）、分辨率固定 1024² 不消费灯的 `shadow_resolution`、点光阴影没有软阴影参数、每帧六面各新建一张 64 B 帧 uniform 与绑定组、窗口宿主仍不读 `project.ron`、没做稳态计时。
>
> 多光源照明切片把主场景从单方向光换成旧端同款的 8 槽光源表：`crates/xgc-render/src/lights.rs` 按原顺序打包 `is_active()` 的方向光与点光（方向光存传播方向 `w = 0`、点光存世界坐标 `w = 1`、`shadow_params.y` 是点光半径），`intensity <= 0` 与第 9 盏起一律丢弃，只有第一盏投射阴影的方向光打 CSM 标记（阴影图只有一份，标记错了别的方向光会采样到不属于自己的图；主阴影光与打包表走同一个 `take(MAX_LIGHTS)` 判据，排在第 9 槽之后就不拟合 CSM）。`scene_shading.wgsl` 的片元按盏循环：方向光 `normalize(-dir)` 无衰减；点光 `fade = 1 - smoothstep(0.8, 1.0, dist/range)`、`atten = fade / (1 + 0.09d + 0.032d²)`、范围外 / 半径非有限整盏跳过，`radiance = 光色 × 强度 × 衰减 × π` 后进同一套 Cook-Torrance。帧 uniform 608 → 976 字节（+16 B 盏数 vec4、+8×48 B 灯表、−32 B 两个与 `lights[0]` 重复的 `sun_dir` / `sun_color`）。证据世界只有一盏方向光：第 150 帧 PPM SHA-256 不变（`4A47275D…`、14/7/8、hash `bb3fdc99df20aa31`）、`run 60` 哈希不变。6 条 GPU 回归（点光照亮 / 范围外与非有限半径整盏跳过且逐字节不变 / 距离衰减 / 两盏同位置强度 1 等于一盏强度 2 / 8 盏上限丢弃第 9 盏 / 方向光与点光叠加）+ 7 条打包单测；红证据：衰减行改 `atten = fade`（「点光必须随距离衰减：近 [239,…] / 远 [240,…]」）、盏数夹 1（「逐盏累加必须等于同能量的单盏」与「方向光与点光必须叠加」）、`pos_type.w < 0.5` 改 `< 1.5`（点光被当方向光）、第 9 盏绕回槽位 0（「第 9 盏灯必须被丢掉：8 盏 [235,…] / 9 盏 [243,…]」）。**未闭环**：点光当时只照明、还不投影（立方体阴影已由下一刀补上，见上）；没有光照剔除（每片元 8 盏都循环）；窗口宿主仍不读 `project.ron`；没做稳态计时。
> 曝光切片把 `render.exposure` 接上（旧端 `RenderConfig::sanitized` 同式消毒：非有限 / 非正落回 1.0，合法值夹进 0.05..20.0；曝光 0 是"全黑画面"不是关闭态），项目级配置经 `xgc-cli/src/project_render.rs` 读进 `Renderer::set_exposure`。乘子在 tone-map（`tone_map.wgsl` 的 `ToneParams` 扩到 32 字节，ACES 之前）与光晕亮部提取（`bloom.wgsl` 的 `fs_bright`，阈值按曝光后的亮度判）两处且必须一致。缺省 1.0 时第 150 帧 PPM SHA-256 不变（`4A47275D…`、14/7/8、hash `bb3fdc99df20aa31`）、`run 60` 哈希不变。5 条 GPU 回归 + 3 条单元回归钉住缺省逐字节等价、加倍变亮并对表 ACES、非法值落回缺省、光晕阈值读曝光后的值、改曝光重建光晕链；红证据：tone-map 的 `* tone.exposure` 改成 `* 1.0`（「曝光加倍必须变亮：62 -> 62」）、亮部提取的 `* params.exposure` 改成 `* 1.0`（「曝光把亮度抬过阈值后必须有光晕：205 -> 205」）、`chain.exposure` 移出链缓存键（「改曝光后光晕链必须按新曝光重建」）。**未闭环**：光照 / 粒子仍在曝光前的线性域里算；没做稳态计时。
> 光晕切片按旧端 `postfx.rs` 合同接上**曝光后、唯一 ACES 前**的三级 Bloom（`render.bloom` 的 `intensity` / `threshold` / `knee`，软膝式按最大通道取亮度，½ / ¼ / ⅛ 逐级降采样后每级横纵各一次 9 抽头二项核，共 9 个 pass，权重 0.5 / 0.3 / 0.2 在 tone-map 同一趟 pass 里叠回线性场景**再**过 ACES —— 过完曲线的高光没有余量）。默认 `intensity = 0` 是关闭态：整条链不跑、`passes` 不增，第 150 帧 PPM SHA-256 与上一版相同（`4A47275D…`、自比 0/921600），对旧端仍 32.22% / 0.61 / 194。七条 GPU 回归 + 三条单元回归（参数容错、链随源尺寸重建并释放、关闭态换尺寸重建占位绑定）钉住关闭态逐字节等价、开态抬亮量与旧端公式对表（差 ≤2）、低于阈值的暗场景即使 `intensity = 4` 也不发光、resize 重建链且同尺寸两次逐字节确定、亮几何的光晕向外扩散；把合成行乘 `intensity` 临时改成乘 `0.0` 时后三条实跑变红，证明回归真的走到合成。曝光已在本轮接上（`render.exposure` 同时进 tone-map 与光晕亮部提取，见上）。以下代价仍成立：开启时每帧仍跑 9 个 pass 并重建亮部提取 / tone-map 各一个绑定组（链内 8 个建链时一次建好）；关闭态 tone-map 绑定组按场景纹理缓存、链在关掉时整份释放；没做稳态计时。
> 蒙皮投射切片让**蒙皮网格进阴影批次**（旧端同一个 `vs_shadow` 按 skin slot 分支，XGC 用 `shadow_skin` 的 palette 管线 + 同一份 `skin_vertex.wgsl` 变形公式）：≤256 关节走 GPU palette，>256 / palette 不完整走 CPU 变形后的顶点，四级级联复用每帧一次的绑定，depth bias 与静态投射体共用 constant 2 / slope 2。四条 GPU 回归在旧实现下全红（变暗 0 像素），现在各 176/9216 像素变暗、阴影质心随姿态平移、单位 palette 与静态网格阴影逐像素相同。证据世界 `showcase-particles.json` 不含骨架组件，所以第 150 帧与 HDR 切片逐字节相同（SHA-256 不变、自比 0 像素），对旧端仍 32.22% / 0.61 / 194；冷启动 757.02 → 734.43 ms（-3.0%，只判定未见回归）。**未闭环**：CSM 拟合仍按绑定姿态包围盒，大位移关节可能落到级联外；CPU 回退多算一次变形；palette 在主 / 阴影 pass 各建一次。
> 256 槽 palette 切片把 `MAX_JOINTS` 从 64 抬到 **256**（旧端 `BONES_PER_SLOT` 同容量），palette uniform 4 KiB → 16 KiB（仍在 wgpu 默认 64 KiB 绑定上限内）；65~256 关节的网格不再静默掉到 CPU。新回归把权重压在 90 号槽并平移它：上限临时改回 64 时该网格 `gpu_skin_meshes` 0 ≠ 1 变红，现在留在 GPU 且表面随 90 号槽移动；257 关节仍确定性回退（原 65 关节用例改口径）。证据世界没有骨架，第 150 帧仍逐字节相同（32.22% / 0.61 / 194），冷启动 734.43 → 743.15 ms（+1.2%，与改前区间重叠且该场景无蒙皮 draw，未观察到可归因回归）；另按设备 `max_uniform_buffer_binding_size` 判定是否走 GPU（downlevel GL 恰好 16 KiB）。
> 蒙皮 draw 单次提交切片把一帧里所有 GPU 蒙皮 draw 收进同一批：一个 frame binding、一个 pass、一次 submit（palette 仍逐角色；下一轮竞技场已并进常驻缓冲），此前每条 draw 各建 frame bind / encoder / submit。左右双角色差分回归钉住批处理不串 palette（右侧换姿态时左侧按列像素逐字节不变）；第 150 帧仍逐字节相同，冷启动数因并发构建不可比，未做稳态计时。
> 蒙皮 uniform 竞技场切片把逐角色的 palette（16 KiB）与 model（96 B）并进常驻缓冲：`crates/xgc-render/src/skinned/arena.rs` 的 `UniformArena` 按设备 `min_uniform_buffer_offset_alignment` 算槽距（palette 16 KiB 仍是 16 KiB、model 96 B 补到 256 B），layout 声明 `has_dynamic_offset: true` 后每条 draw 只换偏移，容量按 2 的幂扩容（只增不减）、暂存区跨帧复用；`queue.write_buffer` 与提交同队列有序，所以重写本帧缓冲一定排在上一帧提交之后。`RenderStats::skin_uniform_buffers` 是可观测口径（只统主场景，不含阴影 pass）：首批 2 张缓冲，同尺寸第二帧 **0**（旧实现每帧每角色 2 张，随角色数线性涨）。两条单元回归钉住槽距对齐与跨帧复用，集成回归 `repeated_frames_reuse_the_skin_uniform_arenas` 断言 2 → 0（把 `upload` 开头临时插 `self.slots = 0;` 时实跑变红：“同尺寸第二帧必须复用竞技场、不再建缓冲：2”）；既有左右角色差分用例仍逐字节通过（串 palette 会立刻红）。证据画面没有骨架实体、竞技场不建缓冲，第 150 帧 PPM SHA-256 与上一版相同（`4A47275D…48A0927`，对旧端仍 32.22% / 0.61 / 194）。**未做稳态计时**（冷启动单帧含 pipeline 编译，不能归因），真正的稳态测量仍要常驻 profiler。**未闭环**：阴影 pass 的蒙皮 palette / model 仍逐投射体建缓冲，需要把 `ShadowTarget::render` 从 `&self` 改 `&mut self`。
> 阴影 pass 的蒙皮竞技场切片把逐投射体的 palette（16 KiB）与 model（64 B）也并进常驻缓冲：`crates/xgc-render/src/shadow_skin.rs` 的 `SkinnedShadow` 持有两张 `UniformArena`，`SkinnedBinding` 只记槽号，4 级级联只换 dynamic offset；`ShadowTarget::render` 改成 `&mut self` 并返回本帧新建缓冲数，逐帧准备拆到 `crates/xgc-render/src/renderer/shadow_prep.rs`（行数预算）。`RenderStats::shadow_skin_uniform_buffers` 是口径：首批 2 张、同尺寸第二帧 **0**（回归 `repeated_frames_reuse_the_shadow_skin_arenas`；把 `upload` 开头临时插 `self.slots = 0;` 时实跑变红）。既有 4 条阴影回归仍全绿，另加一条两投射体用例 `two_skinned_casters_keep_their_own_shadow_palettes`：两个投射体沿屏幕纵轴分开（z=∓2.4）、姿态反向平移，断言两侧阴影行重心相距超过两个窗口宽、且同屏时各自留在原位（把每级 draw 的偏移强制成槽 0 时实跑变红：「窗口内阴影像素太少，无法定位：0」），钉住"每个 caster 用自己那格 palette"。第 150 帧 PPM SHA-256 与上一版相同（`4A47275D…48A0927`，证据世界无骨架）。**未做稳态计时**（证据场景没有蒙皮 draw，冷启动单帧含 pipeline 编译）。
> 角色坡限 / 贴地切片让 `physics.character` 里两个原本没人读的字段生效：`CharacterSystem` 把坡度超过 `slope_limit` 的上升 `physics.slope` 按墙挡住——`Slope::base_x_at` 算该高度处的坡面落点，推到离角色更近的一侧（坡底或坡背，清掉朝墙的水平速度），落点夹进坡面 X 跨度以免坡架高台时出现坡面外的隐形墙；y=0 仍是兜底地面、下坡一律当地面，下行离地不超过 `snap_to_ground` 时贴回且只在 `vy <= 0` 时吸（否则 `grounded` 会抖）。旧实现把任何坡面都当地面。回归先证会红：还原旧实现后「60° 坡挡住角色」（停在 `x=1`）与「下行吸附」（`y=2.9867516` 不下落）实跑变红；修推法时另做一次 —— 只按坡底推 + 不夹跨度时 `character_stays_outside_the_high_end_of_a_steep_slope` 报 `x=-0.3`（本该 10.3）、`a_raised_slope_does_not_push_from_outside_its_own_span` 报 `x=-3.186836`（本该 -0.3）。另加多帧积分走上 / 走下坡、上限取坡面自身倾角（`<=` 改 `<` 即红 `y=0`）、陷在坡体高处去最近一侧（改回按坡面中点选边即红 `x=10.3`）、`snap_to_ground = 0`、下坡、`slope_limit = NaN` 等守卫；把 `snap` 临时置 0 时走下坡用例红 `y=4.618803`。第 150 帧 PPM SHA-256 `4A47275D…48A0927` 不变（证据世界没有 `physics.character` / `physics.slope` 实体）。**未闭环**：水平位移已走 rapier 的 `KinematicCharacterController`（见上），但竖直 / 地面 / 台阶仍由 ECS 解析（无 `probe_ground` / `try_step_up` / `snap_to_ground`）；没有关节、没有 CCD；陡坡是挡住而非沿坡面下滑，也没有旧端的 `slope_limit + 10°` 迟滞带；`GravitySystem` 不在默认调度里、纯角色也没有重力：≤ `snap_to_ground` 的落差靠贴地跟住（`vy == 0` 也走这支），更大的落差不会下落（**已由角色重力切片补上**）。
> 角色控制器缺省对齐切片把 `physics.character` 的缺省值与旧端 `xgame_core::CharacterController` 逐项对齐（`height 1.8` / `radius 0.3` / `step_offset 0.35` / `slope_limit 45` / `skin_width 0.02` / `snap_to_ground 0.3`），消毒与几何派生量收到组件上：`CharacterController::sanitized()` / `half_height()` / `center_height()` 是唯一真相源，`character_kcc` 的六个入口全部改成委托（原来每个字段各抄一份清洗规则，漂移后 schema 缺省值 / KCC 参数 / 台阶判据会各说各话）。schema 的缺省值随之变化（30 个类型 35709 → 35710 字节），显式写了这些字段的场景不受影响、没写的按旧端缺省解释。新增 6 条组件单元回归（缺省逐项相等 / NaN 与 ±inf 落缺省 / 越界钳制按旧端区间字面值 / 派生量 / 清洗幂等 / schema 缺省与 `Default` 一致）；区间常量互相咬合的关系（`2·RADIUS_MAX + 1e-3 <= HEIGHT_MAX` 等）改成编译期 `const _: ()` 断言；迁移端 `OldCharacter` 六字段补 `#[serde(default = ...)]`（手写旧 RON 省略字段时迁移入口原来报 `Unexpected missing field `step_offset``，新用例 `maps_character_with_old_defaults_when_fields_are_omitted` 钉住）；红的证据：缺省改回旧的 `0.3 / 0.01 / 0.2` 时 `defaults_match_the_old_engine` 报 `left: 0.3, right: 0.35`；把 `finite_or` 改成直接返回原值时两条用例因 NaN 漏到 `clamp` 上变红。角色行为回归同批复核：缺省 `skin_width` 0.01→0.02 让 KCC 停位间隙变大，撞墙用例从"水平速度精确为 0"改成 `|vx| < 0.05`（`normal_nudge_factor` 每帧推离墙面，反向分量留成约 -0.002），并补"贴墙后 30~60 帧位置漂移 < 0.02"与"不再保留朝墙推进分量"。证据世界没有 `physics.character` 实体，第 150 帧 PPM SHA-256 不变（`4A47275D…48A0927`、14/7/8 与 hash 不变）。**未闭环**：`jump_velocity` / `jump` 还没有同样的消毒；依赖旧缺省的既有 XGC 场景需要重新核一遍。

> 角色水平 KCC 切片让 `physics.character` 的水平位移走 rapier：`crates/xgc-sim/src/character_kcc.rs` 是胶囊 / 参数 / 消毒的唯一真相源（`half_height`、`center_height`、`rest_gap = skin_width × 1.1`、`character_capsule`、`build_character_controller`），`RapierWorld` 为角色建运行时托管的 kinematic-position-based 胶囊并按 `EntityId.raw()` 排序（确定性），每帧在 `pipeline.step` 之后跑一次 `move_shape`，只回写 `Transform.position` 的 x / z 与水平 `Velocity`（y 与朝向仍归 ECS）。胶囊比脚底高 `rest_gap` 是刻意的：rapier 0.22 的 `check_and_fix_penetrations` 是空函数、`stop_at_penetration = false`，起始间隙 ≤ `offset` 会放行整段位移（穿墙），而 ECS 恰好把脚底压在地面高度、间隙为 0；抬到 `rest_gap` 才落在安全区。竖直继续走 ECS 也来自上游事实：KCC 在胶囊上 `grounded` / `snap_to_ground` / `autostep` / `is_sliding_down_slope` 全不可用（旧端 `crates/xgame-physics/src/character.rs` 同记录），旧引擎本来就只用 `move_shape` 做滑动。参数逐项对齐旧端（`offset = Absolute(skin_width)`、`autostep: None`、`max_slope_climb_angle = clamp(slope_limit, 0, 89)°`、`min_slope_slide_angle = (slope_limit + 10).min(89)°`、`snap_to_ground: None`、`normal_nudge_factor = 1e-4`）。同片删掉 `CharacterBlockSystem`（AABB 最近轴推出：斜墙顶住、高速穿薄墙），默认调度 19 → 18；`VelocitySystem` 对角色整份跳过（水平归 KCC、竖直归 `CharacterSystem`），`physics.body` 与 `physics.character` 同时在的实体由刚体托管（一个实体只给一套：dynamic 由刚体回写位置，kinematic 仍要脚本每帧推位姿）。回归先证会红：`VelocitySystem` 改回三轴积分 → `velocity_system_only_integrates_the_character_vertically` / `a_character_with_a_rigid_body_is_owned_by_the_body` / `character_stops_at_a_wall_instead_of_passing_through` 三条变红；`move_horizontal` 返回零向量 → 撞墙与斜墙滑动两条变红（说明沿墙滑真是 KCC 给的切向位移）。斜墙用例用 20°：从 +X 撞 45° 盒角时看到的是棱角不是面，实测 z 只挪 -0.006。证据世界没有 `physics.character` 实体、角色阶段空转，第 150 帧 PPM SHA-256 与上一版相同（`4A47275D…48A0927`、14/7/8 与 hash 不变）。审查揪出的两处已在本片修掉，各自有先红证据：①角色托管的签名除半径 / 半高外还带 `EntityKey` 的 index + generation 与 `skin_width` / `slope_limit` 的位模式 —— `Replace` / 组件增删 / 快照恢复只换代不换 ID，只看 ID 时签名不变就提前返回，角色 `key` 停在过期代际、水平位移静默冻结且旧胶囊留作幽灵碰撞体（`a_character_keeps_moving_after_its_entity_key_changes`，签名退回只看 ID 时报 `x=0.50000006`）；②默认调度把 `CharacterSystem`（陡坡推出 / 贴地吸附）挪到 `RapierPhysicsSystem` 之后 —— 坡面没有 rapier 碰撞体，修正跑在 KCC 之前就会让角色每帧先挤进 `v·dt` 再被拉回，脚本每帧重设速度时是 30 Hz 来回跳（`a_character_does_not_creep_into_a_steep_slope_under_the_default_schedule`，顺序挪回去时报 `x=-0.25000077`），陡坡判据同时从角色中心点改成按前进方向的胶囊前沿 / 后沿取高度。为守住 600 行软上限，角色托管拆到 `crates/xgc-sim/src/rapier_world/characters.rs`（`RapierWorld` 的子模块，直接读写同一套刚体表）。复审补修两处：③台阶抬升改到 `CharacterSystem` **之后**（原顺序下抬起的 y 被 `snap_to_ground` 每帧吸回地面，0.05~0.2 m 的低台阶默认调度下上不去，`character_steps_onto_a_low_box_under_the_default_schedule` 挪回顺序时报 `y=0`）；④刚体表建体签名从"只比 `physics.body` 数量"改成按 ID 排序的 `(EntityId, EntityKey)` —— `Replace` / 快照恢复只换代不换 ID，只看数量时 `BodyLink.key` 过期、动态体回写静默冻结（`a_dynamic_body_keeps_moving_after_its_entity_key_changes`，退回数量判据时报 `y=5.8603435` 不动）。⑤台阶判据要按 KCC 的停位间隙外扩 —— KCC 会停在距立面 `offset = skin_width` 处（penetration 修复是空实现），胶囊因此永远碰不到台阶面，撞住那一帧速度已被回写成 0、预测位移也归零，小半径（0.05 m）或低速度（1 m/s）下角色卡在 `offset` 外面一次都抬不起来（`a_small_capsule_still_steps_onto_a_low_box`，不外扩时报 `x=-1.0601`、y=0）；现在判据按 `rest_gap + 1 mm` 外扩，代价是贴着低矮实体也会被抬上去，这正是「压住台阶就上」的语义。同一轮还收了角色表的每帧分配（签名探针与收集表跨帧复用，与刚体表探针同口径），以及「没有 `Transform` 的角色不进签名」（否则之后补上 `Transform` 时签名不变、永远不建胶囊）。第四轮复审又补一处：`physics.body` 与 `physics.character` 同时挂的实体只归刚体表 / 脚本负责——`character_kcc::owned_by_kcc` 是唯一判据（有 `CharacterController`、没有 `PhysicsBody`、有 `Transform` 才算 KCC 托管），`CharacterSystem` 与 `CharacterStepSystem` 都先按 `PhysicsBody` 跳过。不跳过的话，新顺序里 rapier 先回写同实体的 `Transform`、这两个 ECS 系统再写一遍，下一帧又被刚体覆盖，站在台阶边 / 坡边时脚下就是逐帧抖动；`a_character_with_a_rigid_body_is_left_alone_by_the_character_systems` 钉住（去掉跳过时实跑报「带刚体的角色不该被台阶系统抬起来：y=0.19999999」与「带刚体的角色不该被坡面系统推走：x=9.7」）。`VelocitySystem` 对凡是挂了 `CharacterController` 的实体一个分量都不积分：给这种实体积分只会和刚体回写打架，位置完全由刚体表 / 脚本负责。**未闭环**：竖直 / 地面 / 台阶仍在 ECS（`probe_ground` / 抬离地面 / `try_step_up` / `snap_to_ground` 未移植）；`physics.slope` 已有一份 rapier 静态楔形碰撞体（见上），但角色竖直仍在 ECS，所以陡坡仍挡住而非沿坡面下滑、迟滞带不生效；没有关节、没有 CCD；未做稳态计时（证据场景没有角色，冷启动不能归因）。

> 热路径分配门禁切片让稳态帧分配从 1697 次/帧降到 230 次/帧（`crates/xgc-sim/tests/steady_state.rs` 的线程本地计数分配器：200 walker + 10 动态刚体、8 帧预热、60 帧 101838 → 13818 次），数值型组件世界的 `World::state_hash()` 60 次调用零分配、整帧预算 = 60 × (实体数 + 64)；`crates/xgc-world/src/hash.rs` 里每帧现建的实体行号 `Vec`、逐实体组件 ID 排序、逐字段 `field_names()` 改成 `Archetype::hash_layout`（`OnceLock`）与 `thread_local` scratch，哈希输出逐位不变（`run 60` `f4bc029f5f995b91`、第 150 帧 PPM SHA-256 相同）；变异回每实体现建时「数值型组件零分配」与「稳态帧预算」两条实跑红（72000 次 / 114018 次；变异值比改前基线更大，因为每实体现建比旧路径还多一条布局 `Vec`）。release 吞吐 0.057 ms/帧（210 实体，600 帧 33.98 ms）。**未闭环**：字符串字段经 `Value` 反射读取每实体仍克隆一次；门禁目前只统计调用线程的分配（默认调度是单线程），并行 System 执行落地时要汇总各 worker 线程的计数；没有 CI release 基线。
>
> 可选 SSAO 环境光遮蔽切片照旧端 `ssao.rs` 在 TAA 之后、光晕之前乘一层遮蔽（`render.ssao` 的 `enabled` / `strength` / `radius` / `bias`，默认关）：固定种子 LCG 的 16 抽头半球核（纯函数、无随机数）、半分辨率 `R8Unorm` 上深度反投影 + 深度梯度法线 + 像素哈希旋转 + `smoothstep` 累计遮蔽、[1,4,6,4,1]/16 可分离模糊、线性上采样乘回源色（四趟 pass 一次提交），`SsaoParams` 448 字节，退化矩阵（含非有限元素）把强度写 0（画面逐位不变），`view_proj` 用未抖动矩阵。默认关闭：不建 AO 纹理、不跑 pass、`passes` 不增，第 150 帧 PPM SHA-256 与上一版相同（`4A47275D…48A0927`、自比 0/921600）；7 条 GPU 回归 + 4 条纯函数单测，变异「乘算改成直接回传源色」「去掉解析 pass 计数」「`is_enabled` 漏掉 `strength > 0`」「消毒不落默认」「反投影守卫漏掉 `is_finite`」「关掉不释放目标」全都实跑红。**未闭环**：半分辨率 AO 在薄几何边缘留淡边（旧端同款）；只有遮蔽没有间接光；窗口宿主仍不读 `project.ron`；未做稳态计时。
> 可选 TAA 时间抗锯齿切片照旧端 `taa.rs` 在 tone-map 之前插入一趟时间重投影（`render.taa` 的 `enabled` / `history_weight`，默认关）：8 相 Halton(2,3) 抖动是帧号纯函数（截图逐帧可复现）、像素→裁剪与像素→纹理两套换算（后者 y 取反）、`jitter_projection` 只平移裁剪空间 x/y；`taa.wgsl` 用当前深度反投世界、用上一帧未抖动视图投影重投影并补上一帧抖动，历史按 3×3 邻域 `lo/hi` 钳制后 `mix`（缺省权重 0.9），天空按视线方向推 1e4（只保留旋转视差），首帧 / 相机背后 / 越界直通；历史两张 `Rgba16Float` 乒乓、换尺寸重建、关掉即释放；抖动只叠场景矩阵（阴影拟合仍用未抖动矩阵），深度纹理补 `Sampled` 用途，TAA 开着时 FXAA 自动让位。默认关闭：不建历史、不跑 pass、相机不抖动，第 150 帧 PPM SHA-256 与上一版相同（`4A47275D…48A0927`、自比 0/921600）；8 条 GPU 回归 + 7 条抖动与布局单测 + 1 条接线单测，变异「去掉抖动」「合成忽略 blend」「重置不清重投影状态」「两处尺寸判据都只看有没有」「FXAA 不让位」「不记解析 pass」全都实跑红。**未闭环**：历史只有颜色没有深度 / 速度（旧端同款），快速平移的高光仍可能拖影；解析与 tone-map 两次提交；窗口宿主仍不读 `project.ron`；未做稳态计时。
>
> 跨帧视觉对照 + 脚本 UI 颜色修正切片先把余差按帧拆开：旧编辑器 `--headless --frames N --screenshot` 的 N = 1 / 50 / 100 / 150 与 XGC 同帧渲染比，差异 **32.33% / 36.23% / 32.38% / 32.22%**、结构性 **2.64% / 3.88% / 1.86% / 1.82%** ⇒ 第 1 帧就存在、不是动画漂移；再定位到血条（脚本颜色）、狐狸轮廓 / 阴影、右侧紫色物体三块。血条是色彩空间口径错：HudDemo 脚本给 sRGB，旧引擎在脚本入口 `color_srgb()` 转线性，XGC 的 `ui_set_bg_color` 原来原样写进 `UiElement.background`、渲染端又当线性编码到 sRGB 目标，等于编码两次（旧 `(220,83,71)` vs 修前 `(239,154,144)`）。`crates/xgc-script/src/legacy.rs` 加唯一入口 `srgb_to_linear()`（同旧端阈值 0.04045、alpha 不转），`ui_components.rs` 文档写明组件颜色是线性值；修后血条回到 `(220,83,71)`，第 150 帧对旧端 **32.22% → 32.16%**、mean_abs **0.61 → 0.58**、结构性 16801 → **16280（1.77%）**。脚本现在写进世界的是线性值，基线随之变更：世界 hash **`d050bd357f20aa31`**、PPM SHA-256 **`f6e623b8c4d04beb9cc0ae0d9d23ff6a07ddbbe72afc5684143a899e41186ee3`**（本文档历史段落里的 `bb3fdc99df20aa31` / `4A47275D…` 都是修正前基线）。回归 `legacy_ui_color_arguments_are_srgb` 钉住落地值（含 0.04045 阈值处取值与 alpha 不转；容差 1e-5 抓的是「漏换算 / alpha 被误转」，抓不到阈值处两支的微小差值），把入口换算改回原样即实跑红。**未闭环**：旧端还有 `ui_set_text_color` / `ui_set_image_tint`（没有边框色 setter，`ui_set_fill` 是填充比例）尚未移植，将来必须走同一个入口；组件缺省 `border_color` 仍是旧端 sRGB 数值（70/74/84 的编码值）没有换算成线性，`border_width` 缺省 0 所以当前不可见，等专门对齐旧端 UI 缺省值的一刀再改；彩色表面高光与狐狸轮廓仍是结构性差异来源。

> 视觉余差归因切片给 `xgc image-diff` 加了第四个参数 `report`：除了原有总览，还打印「余差归因」——按最大 RGB 通道差把像素分成 相同 / ±1 / 2-3 / 4-7 / ≥8 五桶、三通道平均绝对差、以及 16×9 块里差异最大的 5 块（`crates/xgc-render/src/diff.rs` 的 `diff_breakdown`；`spans` 把宽度 / 高度切成首尾相接的段，除不尽时余数平均分给前几段，保证不丢像素）。对第 150 帧旧 `tmp/old-150.png` vs 新 PPM 实跑：**624650 像素逐字节相同、244107 像素（26.5%）只差 1 LSB、2-3 差 22541、4-7 差 13501、结构性（≥8）16801 = 1.82%**；通道平均绝对差 R 0.72 / G 0.84 / B 0.86（注意总览的 mean_abs 是 RGBA 四通道均值、这里的 R/G/B 是三通道均值，口径不同（四通道 vs 三通道），不能当作同一指标直接比较）；最差块集中在第 4 行（row=4 col=12 99.94%、col=9 97.84%、row=5 col=13 93.06%），推断集中在地面上的阴影边界带。⇒ 32.22% 的差异里绝大多数是 8 位编码舍入（推断是同一线性值经不同的 tone-map / sRGB 往返路径差 1 个 LSB），真正的结构性差异只有 1.82%（推断集中在阴影边界与 UI）。七条纯函数回归（分桶边界（含差值 3 / 7 / 8 的桶上下边界）、平手按行列升序、最多 5 块上限、块网格与最差块排序、窄轴切段不丢像素、尺寸不同报错、只改 alpha 不算差异）+ 三条 CLI 集成回归（真实 PPM 的分桶与通道均值 / 不加 `report` 时输出不变 / 未知第四参数退出码 2）；把 ±1 桶临时算进结构性桶时 `buckets_split_rounding_from_structural_differences` 实跑红（`left: 0 right: 1`）；把 `2..=3` 收紧成 `2..=2`、`4..=7` 收紧成 `4..=6` 各自让桶用例红；把最差块上限从 5 放宽到 6 让 `worst_blocks_cap_at_five` 红；把 alpha 也计入差异让 `alpha_only_differences_are_not_counted` 红（另三条连坐）；把切段余数丢掉时 `spans_keep_every_pixel_when_the_axis_does_not_divide_evenly` 实跑红。**未闭环**：工具只归因不修改，余差本身尚未收窄；旧端仍没有逐帧截图序列，单帧归因不能定位动画差异。

> FXAA 切片按旧端 `postfx.rs` 合同在 tone-map 之后、UI 之前加一趟可选抗锯齿（`render.fxaa` 的 `enabled`，默认关）：`crates/xgc-render/src/fxaa.rs` 按输出同格式缓存 LDR 暂存纹理，`fxaa.wgsl` 逐行照旧端 `FXAA_WGSL`（FXAA 3.11 质量路径 5 抽头、亚像素 0.75、方向收缩 1/8 下界 1/128、跨度上限 8 texel、边缘阈值 1/12 与 1/6、Rec.601 luma、`textureSampleLevel(..., 0.0)`、sRGB 手动 encode / decode），平坦区直接返回中心采样以保持逐字节不变。UI 画在 FXAA 之后（HUD 锐利边不被柔化，旧端顺序相同）。默认关闭：不建暂存、不跑 pass、`passes` 不增，第 150 帧 PPM SHA-256 与上一版相同（`4A47275D…48A0927`、自比 0/921600），对旧端仍 32.22% / 0.61 / 194（`project.ron` 没有 `render.fxaa` 节）。六条 GPU 回归钉住关闭态逐字节等价、硬边缘被柔化且只动边缘 ±2 内、纯色清屏不变、尺寸 / 输出格式变化重建暂存且同尺寸两次确定、换尺寸 / 格式后与新建渲染器逐字节相同、sRGB 目标的混合发生在显示空间（最亮值 ≤208，线性空间会到 215）、光晕与 FXAA 叠加时 12 趟 pass；红的证据两条 —— 合成改成原样回传时「开着 FXAA 必须至少改变一个边缘像素（否则着色器没跑）」，把平坦区早退改成 `raw_m + 0.01` 时「FXAA 只许动边缘附近，却有 3836 个远离边缘的像素被改了」；把 scratch 复用判据改成只看「有没有」时「换尺寸 / 格式后必须与新建渲染器逐字节相同」；把 sRGB 分支改成永假时「sRGB 目标的边缘混合必须发生在显示空间（线性空间会亮到 215）：215」。**未闭环**：TAA 让位已由下一刀补上（TAA 开着时 FXAA 强制关）；开启时每帧多一趟全屏 pass、一个绑定组与多一次 submit（tone-map 与 FXAA 分两次提交），视图每次现建；未做稳态计时（证据场景是关闭态，冷启动含管线编译不能归因）。
> 姿态感知包围盒切片让 `scene_bounds` 改用 `posed_bounds`（会动的顶点：按每根真有 palette 项的关节变换整网格 AABB 后求并，覆盖变形顶点凸包且只需 O(顶点 + 关节) 次变换；不动的顶点：缺蒙皮属性 / 权重全零 / 关节越界时按顶点并进盒子，因为它们在 CPU 与 shader 里都留在绑定姿态），自动取景与级联远距离拟合都跟着当前姿态；集成回归在旧实现下停在绑定盒 `[-0.8..0.8]`、自动取景 **0** 个可见像素，现在盒子跟到 +4、画面里能看到网格。已知近似：整网格 AABB 比旧端加载期按关节预计算的顶点范围盒更松，骨架场景的自动取景会略远。第 150 帧仍逐字节相同（证据世界无骨架），对旧端 32.22% / 0.61 / 194；交错 A/B 中位数 752.62 → 765.10 ms（+1.7%，区间重叠且该场景无 pose item，未观察到可归因回归）。级联正交盒与旧端同为“按相机视锥切片拟合 + 逐级剔除”，视锥外投射体本来就走同一拟合框，不需要另做扩展覆盖。
>
> 角色重力切片给 `CharacterSystem` 加 `pub gravity: [f32; 3]`（缺省 -9.81，只有 `y` 被消费）：腾空时`vy += gravity.y * dt` 并积分 `y`，落地时清掉向下速度；`GravitySystem` 不在默认调度、`PhysicsSystem` /`GroundCollisionSystem` 只处理 `physics.body`，所以纯角色此前根本没有重力（落差大于 `snap_to_ground` 就悬空）。回归 `cargo test -p xgc-sim --test character_gravity`：悬空 3 m 落地、跳跃弧线落回、站在地面不被拉下、走出坡面跨度后掉回 y=0、站在坡面上不被拽下、默认调度下竖直只积分一次；把积分行改成 `+= 0.0` 时后三条里的三条实跑变红（「落地后 y=3」「落地后 y=16.666647」「掉下来后 y=5.773503」）。竖直积分由 `CharacterSystem` 独占：`VelocitySystem` 原先对角色只积分竖直（水平已交给 KCC），两边同时积分会让默认调度一帧写两遍 `y`，自由落体看起来重力翻倍、跳跃高度翻倍；现在它按 `physics.character` 整份跳过角色，`a_character_falls_once_per_frame_under_the_default_schedule` 按解析解 `y = y0 - g·dt²·n(n+1)/2` 钉住单次积分（改回双写时实跑红：「默认调度把竖直积分了两遍：y=2.7275007，单次积分应为 2.850125」）。第 150 帧 PPM 与上一版逐字节相同（证据世界没有 `physics.character` 实体），`run 60` 哈希仍 `f4bc029f5f995b91`。未闭环：没有终端速度、只积分 y、地面仍是解析坡面 + y=0（不是 rapier 射线），台阶顶面不在 `ground` 里。

> 刚体表增量修补切片把"增删实体就整表重建"收掉：`ensure_built` 第一次仍按 ID 排序整份建表（岛屿顺序必须确定），之后按 `BodySignature` 的 ID 序列做差量——只重建新增 / 换代 / 构建参数变了的条目，删掉消失的条目，未变化的刚体保留原 handle（整表重建会重排所有 handle、丢掉在飞速度与接触缓存）。真的动过表才 `query.update(&colliders)`；`rebuild_count()` 与 `body_patch_count()` 是两个诊断口径（稳态 1 / 0）。刚体表整块拆到 `crates/xgc-sim/src/rapier_world/bodies.rs` 守住 600 行。回归三条：中途新增远处刚体只修补一次且已有刚体继续下落、`despawn` 只修补一次且静态地面不被误删、同一世界"加 / 不加"远处刚体时未受影响刚体的位置 / 朝向 / 线速度都逐位相同。红证据：增量分支退回整份 `rebuild` 后会被 `rebuild` 的空表断言拦下（临时去掉它再跑仍是 5 条红）。第 150 帧与 `run 60` 哈希不变（证据世界刚体集合不变）。未闭环：修补计数只按次数累计、不区分一次修补里动了多少条目；新增刚体复用空闲 handle 使编号与全量建表不同（接触链上缺大规模实测）。
> 刚体建体签名切片让运行期改 `physics.body` 字段不再被静默忽略：`rapier_world.rs` 的建体签名从「按 ID 排序的 `(EntityId, EntityKey)`」扩成 `BodySignature`（`kind` / `shape` 的 FNV-1a + `half_extents` / `radius` / `half_height` / `friction` / `restitution` / `mass` / `gravity_scale` / `is_sensor` 的位模式），位置 / 旋转 / 缩放不进签名（前两者动态体每帧都在动；缩放沿用旧端「建体时读一次」，也是避免脉冲缩放动画每帧重建 —— 第一版把缩放算进签名就改掉了 frame-150 哈希与 PPM，去掉后恢复）；`RapierWorld::rebuild_count()` 是整表重建次数（稳态恒 1）。回归五条：改 `kind` 后刚体必须停止下落、动态体连跑 20 帧不重建但改 `friction` 恰好重建一次、缩放动画 19 帧不重建、表驱动逐个改 10 个构建参数都必须重建、`kind` 拼错也按动态体回写（判据已提到 `xgc-world::physics_components` 的 `BodyKind` / `PhysicsBody::kind()`，`rapier_world`、重力、地面夹持、AABB 分离共用）；把 16 个槽全填 0（含 `kind` / `shape` 两个哈希槽；只清 12 个数值槽只有摩擦那条会红）时实跑红「改成 static 后必须停止下落（参数改了要重建）：y 4.8313904 -> 4.3919845」与「改摩擦必须重建一次：left: 1, right: 2」。第 150 帧与 `run 60` 哈希不变。未闭环：增删当时仍整表重建（已由下一轮增量修补切片补上，见上）、`Transform` 平移旋转不进签名。
>
> 陡坡沿坡面下滑切片补上物理侧最后一条“陡坡只当墙”的差距：`character_kcc` 新增 `SLIDE_BAND_DEGREES = 10.0` 与 `slip_limit(def) = (slope_limit + 10).min(89)`（旧端 `build_rapier_controller` 的 `min_slope_slide_angle` 同式），角色阶段分三档：坡度超过 `slip_limit` 且起点着地时不再清竖直速度（清了就永远“站”在坡面上），累加的 `vy` 经坡面投影变成沿坡切向的下滑；“陡坡当竖直墙”的分支加 `!slipping` 门禁（下滑时清零竖直位移等于又站住）；迟滞带内的坡（`slope_limit..+10°`）爬不上去也不主动下滑，避免临界角上每帧抖；顶着坡面走（`grounded_at_start` 在 y=0 平面上）仍走“当墙”分支。回归 `cargo test -p xgc-sim --test character_ground` 新增 2 条：60° 坡上无输入 30 帧内已开始下滑、150 帧后脚底回到 y=0 附近；50° 坡 120 帧内 `x` / `y` 变化都 < 2 cm。红的证据：`slipping` 恒为 `false` 时「30 帧后必须已经开始沿坡下滑：x 1 -> 1」；`slip_cos` 换成 `limit_cos` 时「迟滞带内的坡不该下滑：x 2 -> -7.541752」。第 150 帧 PPM SHA-256 不变（`4A47275D…48A0927`、14/7/8、hash 不变，证据世界没有角色 / 坡面实体），`run 60` 哈希仍 `f4bc029f5f995b91`。**未闭环**：滑到平地后水平速度会保留（角色没有摩擦 / 阻尼，位置由脚本每帧重设速度兜底，与旧端同一取舍）；y=0 仍是解析平面；下滑没有终端速度；下滑加速度只由竖直速度累加器给出，切向速度渐近 `g·t/sinθ`（比无摩擦快 `1/sin²θ`，60° 约 1.33×，推演未量化）；“陡坡当墙”里的 `!slipping` 门禁是防御性写法 —— 单纯沿坡下滑时 `move_shape` 不产生接触事件，去掉它 60° 用例仍绿，等有“下滑中同时顶住坡面”的场景再补回归；没有关节、没有 CCD、同帧角色互见旧位姿；未做稳态计时。
>
> 角色竖直 / 地面 / 台阶切片把最后两块 ECS 竖直逻辑搬进 rapier：`crates/xgc-sim/src/character.rs`（`CharacterStepSystem` 的 AABB 台阶抬升）与 `systems.rs` 的 `CharacterSystem`（贴地 / 重力 / 陡坡推出）同片删除，默认调度 18 → 16，三轴位移都在 `rapier_world/characters.rs`。`character_kcc::probe_ground` 用 `cast_ray_and_get_normal`，间隙是法向距离 `gap = (toi - half_height) * n.y - radius`（旧公式「竖直落距乘余弦再减中心高」在斜面上差 `core·(n.y-1)`，站坡的角色会被误判陷地、每帧被抬）；y=0 仍是解析兜底地面。竖直按旧端 `character_simple_move` 同口径（腾空累加 `vy`、着地清零），位移与水平速度一起进 `move_shape`。台阶用旧端 `try_step_up` 的三段扫掠（升 `step_offset + rest` → 前进 `horizontal + radius + 0.02` → 落 `step_height + 0.05`），门禁是「本帧有接触」而不是「水平位移被截短」（小半径胶囊撞立面时 `move_shape` 仍能挤出七八成位移），采纳前再探落点、`normal.y < cos(slope_limit)` 算爬坡不算上台阶。陡坡按竖直墙处理：events 收集接触法线，`normal.y < cos(slope_limit)` 时清零本帧位移的竖直分量、消掉朝墙里的水平分量，并跳过贴地吸附（否则吸附把 KCC 沿坡抬起的 2 cm 抵消，变成每帧水平蚕食 12 mm、胶囊慢慢嵌进坡面）。这条规则**只在起点着地且没有向上意图时**生效（`grounded_at_start && desired.y <= 0`）：`move_shape` 的接触列表里也混着普通竖直墙与天花板（`n.y ≈ 0` 同样小于 `limit_cos`），无条件清竖直位移会把起跳吞掉（原地空跳、松开墙才突然弹起），也会把下落挂在墙上（`vy` 每帧累加、松手时整段放行）。两条回归钉住：`a_character_jumps_while_pressed_against_a_wall` / `a_falling_character_keeps_falling_while_pressed_against_a_wall`；去掉门禁时实跑红「峰值 y=0.24」与「y=2.98365」。坡面上的**静息脚底高度**比解析坡面高 `(radius + rest_gap)·(sec θ − 1)`（30° 约 5.0 cm、45° 约 13.3 cm、平地 0）：胶囊沿法线保持 `rest_gap` 的间隙时脚底必然抬这么高，不是误差；旧端同样保留这段间隙（它的间隙公式少算半高投影，实际悬空约 14 cm）。回归按这个公式钉（容差 2 mm），上坡过程中断言间隙落在 `[-5 cm, +8 cm]`。回归 `cargo test -p xgc-sim --test character_kcc --test character_ground`（9 + 14 条；21 条旧 ECS 竖直 / 坡面用例随实现删除）；红的证据三组：`GRAVITY_Y = 0` 时四条变红（「落地后 y=3.0000002」「没站上箱顶：y=2.0000002」「跳跃后没落回地面：y=16.666649」「走出坡面后没掉回地面：y=5.8109937」）、`try_step_up` 直接返回 None 时两条变红（「角色没走上台阶：x=-1.3000003」「小半径角色没走上台阶：x=-1.0833338」）、贴地吸附（⑤）原先没有回归：现在补 `a_character_walking_down_a_slope_stays_on_the_ground`（30° 下坡、2 m/s、90 帧逐帧断言脚底停在静息高度且 `grounded` 为真）；把吸附分支改成 `if false && …` 时实跑红「第 1 帧下坡时不该离地：x=1.0333333，y=9.472464」。陡坡当墙补上之前 creep 用例红「第 43 帧脚底越过了陡坡立面：x=-0.08836889」；把陡坡门禁退回无条件触发时另两条红「峰值 y=0.24182506」「y=2.98365」。第 150 帧 PPM SHA-256 不变（`4A47275D…48A0927`、hash/14-7-8 不变，证据世界没有角色），`run 60` 哈希仍 `f4bc029f5f995b91`。**未闭环**：y=0 仍是解析平面而非碰撞体；陡坡挡住而非沿坡面下滑、`(slope_limit + 10°)` 迟滞带不生效（已由陡坡下滑切片补齐，见上）；没有关节、没有 CCD、同帧角色互见旧位姿、没有终端速度；未做稳态计时（证据场景没有角色，冷启动含管线编译不能归因）。
> 坡面碰撞体切片让 `physics.slope` 在 rapier 里也有一份几何：`crates/xgc-sim/src/rapier_world/slopes.rs` 按 origin / size / rise 建静态楔形薄壳（cuboid 半长 `hypot(size.x, rise) / 2`、厚 `SLOPE_THICKNESS = 0.1`、半深 `size.y / 2`，绕 Z 轴转 `atan2(rise, size.x)`，中心沿法线从坡面中点下移半厚），所以**上表面与解析 `Slope::height_at` 重合**；另挂 `physics.body` 的实体让位（显式刚体优先），参数非有限的坡不建体。建体签名 = ID + `EntityKey`（index + generation）+ origin / size / rise 位模式，变了才重建（先删旧刚体连带碰撞体、再 `query.update`），探针 / 收集表跨帧复用。新增只读 `RapierWorld::ground_under(x, z, from_y)` 向下射线探地，既是回归的对表工具，也是下一刀自实现 `probe_ground` 的底子。回归 `cargo test -p xgc-sim --test slope_collider` 六条：60° 陡坡把 KCC 角色挡在坡脚（没有碰撞体时穿到 x≈5，实跑 `x=4.9999976`）、四个采样点高度与解析坡面差 < 1e-3、改组件 `rise` 后碰撞体重建、带 `physics.body` 的坡不再自动建体（去跳过即红）、`size` 为 0 与 `rise=NaN` 被跳过（放宽尺寸判据即红）、下坡方向同样对齐（`atan2` 取 `rise` 绝对值即红）。把 `step` 里的 `ensure_slopes` 注释掉时基础三条全红（「x=1 处射线没打到坡面碰撞体」等）。第 150 帧 PPM SHA-256 不变（`4A47275D…48A0927`、证据世界没有 `physics.slope` 实体），`run 60` 哈希仍 `f4bc029f5f995b91`。**未闭环**：角色竖直仍在 ECS，陡坡仍挡住而非沿坡面下滑；薄壳法线朝上、坡下不与其它几何互剪；坡面不进 `AabbCollisionSystem`，动态刚体与坡面的接触只走 rapier。
> 旧 / 新整帧 mean_abs 依次为 28.65 → 23.79 → 21.24 → 15.97 → 15.27 → 16.34 → 19.65 → 19.27 → 18.78 →
> 0.85 → 0.63 → **0.61**（差异像素 32.22%）。前一步是**确定性粒子模拟 + billboard**：按旧端 SplitMix64 / 发射 / 生命周期合同推进，
> Alpha 后向前排序并以预乘方式混合，Additive 单独批次；Campfire.Smoke 的位置 / 尺寸 / 透明度已对齐。再前一轮是
> **4 级 CSM**（λ=0.7 practical split、逐级紧致拟合 + 纹素吸附、深度数组按视线深度选层）。再往前是**UI 粗体字面 + 原生 kerning**（面板区 3.14 → 1.41）
> 与**直接光 Cook-Torrance PBR**：GGX + Smith + Schlick、
> `(1-F)` 漫反射扣减与 π 校准逐式照搬旧端，蓝箱 / 棕箱区域平均差降到 1.45 / 2.12。
> 再往前是**真字体 UI**（`ab_glyph` + 与旧端同一套系统字体 + 字形图集，面板区域 5.19 → 3.14）
> 与**脚本 `look_at` 朝向修正**：旧实现把俯仰塞进欧拉角 Z 分量，
> 每帧把相机转成「yaw 对、俯仰 0、滚转 45°」，整帧取景与天空方向全偏；修正后天空 / 地面几乎逐像素一致。
> 然后**网格基色贴图**让狐狸从白模变回橙狐；最后把刚体换成与旧引擎**同版本同参数**的 rapier3d 0.22，
> 右侧棕箱区域平均差 18.56 → 4.44（翻滚姿态第一次对上）。
> 中间两步（阴影、动态刚体补零速度）让均值反向，但都是旧引擎本来就有的行为：影子位置取决于各自
> 的仿真位置，而简化的 AABB 求解器没有旋转 / 静-动碰撞，落点与堆叠仍不同。更早的
> **环境反射**（天空 SH 反射方向 × Lazarov 解析 BRDF）把物体表面抬亮，随后把 **metallic / roughness
> 搬进实例数据**（96B）做逐材质反射：棕色 / 红色表面与旧图的通道差已缩小到个位数，差异像素 98.07%。
> 随后补 **接触清速度**：贴地 / 堆叠的盒子不再每帧带着累积速度抖动（3 条物理回归钉住稳定度）；
> 第 150 帧像素与上一步逐像素相同（物体早已落定），世界哈希因速度归零而变化。
> 第 150 帧世界哈希变为
> `bb3fdc99df20aa31`：`particle.emitter.config` 的完整旧配置参与状态哈希，粒子运行态不落盘（`verify-migration` 一致）。
> 仍缺：后处理之外的画质项（PCSS / 点光立方体阴影 / TAA / SSAO / FXAA 都已可选；主场景与阴影 pass 的 palette / model 都已并进 dynamic offset 竞技场）、
> 物理的角色仍缺关节、连续碰撞（CCD）、同一帧内多角色互见旧位姿（`move_shape` 读的 `query` 只在 `pipeline.step` 时刷新，角色间阻挡与遍历顺序有关）、
> UI 面板底色微差（align/valign 只能按 kind 推断）。旧端本机仍报告项目 Rust 插件构建指纹不匹配并跳过，XGC 也尚未迁移旧 Animator
> 状态机与完整动画素材。
> 当前 `ScriptRuntime::run_frame` 不返回错误，脚本诊断仍只写 stderr；正式调度切片需要把失败传播纳入宿主契约。旧端 `particle.burst` 是 `serde(skip)` 的一次性脚本运行时请求（不是场景迁移字段），XGC 的脚本请求桥尚未接入。

> **glTF `NORMAL` 顶点法线着色**：旧端顶点着色器输入 `@location(1) normal`，用 `transpose(mat3_inverse(mat3(model)))` 变换、蒙皮时先 `skin · vec4(n,0)`，片元 `normalize(input.world_normal)` 且**不按视线翻号**；XGC 此前完全丢弃 `NORMAL`，片元一律用屏幕导数叉积。切片按旧端同式接上：`crates/xgc-render/src/glb/{attributes.rs,normals.rs}` 解析并校验条数，`Mesh.normals` 为空表示模型没导法线、片元退回导数（旧端加载期生成平面法线，对硬边模型两者面内等价）。红证据：删 NORMAL 装配 → “有 NORMAL 必须逐顶点一条 left: 0 right: 4”与“侧向法线必须让平面几乎不受光”；阈值改 `1e30` → 侧向 / 正向同为 `[248,248,248]`；`normal_matrix` 改成直接截取 → “非均匀缩放下法线必须走逆转置：缩放 8 倍后 [208,208,208] / 预变形参考 [215,215,215]”（光强压到 0.8，避免 ACES 高光饱和把判别力压到 2 个灰阶）；解析处把法线提前归一化 → “NORMAL 必须原样读入（不提前归一）”。另外 4 条回归里还有一条专门钉坏资产：`NORMAL` 不是 float VEC3、或条数与 `POSITION` 不一致，都必须报错而不是静默截断。第 150 帧世界哈希不变 `d050bd357f20aa31`，PPM SHA-256 变为 `872c3aaba5f76a17a0d301998c735a719c66bad78fc74871a7a0f23e3ba118d1`（只有 2 个像素差 1 LSB，证据世界只有 `Box.glb` 带 NORMAL），对旧端仍 32.16% / mean_abs 0.58。CPU 蒙皮回退只重算位置，所以每帧把法线流清零、退回屏幕导数（与切片前一致，不会拿绑定姿态的法线着错光）。当轮未闭环：内建图元仍走导数；缺 `NORMAL` 的模型已在下一轮改成加载期按几何生成面积加权顶点法线（见下一条）。

> **预过滤环境 IBL**：旧端环境镜面是 split-sum —— 天空按 GGX 重要性采样烘成 5 级预过滤立方体（`ENV_MIPS = 5`、基级 128²、每级 96 / 48 / 24 / 16 / 12 样本），再加一张 256 样本 BRDF 积分表（Karis `A / B`），片元 `env_spec(reflect(-v,n), roughness × 4) × (f0 × A + B)`；XGC 此前是 L2 球谐近似反射 + Lazarov 解析 BRDF 近似。切片按旧端同式接上：`crates/xgc-render/src/{env_map.rs,env_prefilter.wgsl}` 负责烘焙，公式与天空盒共用 `sky_common.wgsl` 一份（无绑定，绑定声明各自写）。缓存键 = 天空 uniform（80 字节）的 FNV-1a 哈希：首次 31 趟 pass（30 级面 + 1 张 BRDF 表）、之后同设置 0 趟，`RenderStats::env_map_passes` 是这个口径；BRDF 表只烘一次。关天空的项目不跑任何 pass 也不采样，`passes` 仍为 2、帧逐字节不变。红证据：片元里把 `env_spec` 乘 0 → `换地平线色后金属球中心的反射必须明显不同：暖 [21, 21, 21] / 冷 [21, 21, 21]`；缓存判据改恒 false → `天空设置没变时不该重建环境图`；删掉 `!settings.enabled` 判据 → 关天空也跑 31 趟。审查复核抓到一个真 bug：BRDF LUT 的 Schlick-GGX 要用 IBL 的 `k = r²/2`（LUT 积的是已除掉 `(N·L)(N·V)` 分母的 `G_vis`），首版误用直接光的 `(r+1)²/8`，把掠射平滑金属的 `A` 压低约 57%（0.04 粗糙度、N·V = 0.2 时 0.668 → 0.283）；按旧端改正后与旧端的整帧差异从 18.17% 收到 **3.48%**。第 150 帧世界哈希仍 `d050bd357f20aa31`，PPM SHA-256 为 `6bc10cedf798a1f696d80c614d1db0d8972e86ab08ec09d9f5ea92a8329be582`；与旧端差异 **3.48%（32113 像素）/ mean_abs 0.46 / max 198**，只差 1 LSB 的 10319 个像素占 1.1%（此前 244107 = 26.5%）、结构性（单通道 ≥8）16412 = **1.78%** 集中在几何边缘 / 阴影边界 / 狐狸轮廓，相对上一版 XGC（`xgc/target/evidence/same-tick/normals4-150.ppm`）改 31.46% 像素、mean_abs 0.18、max 27。旧的 `env_brdf_approx` / `env_specular` CPU 镜像随实现删除（镜像必须镜像着色器），两条依赖它的 GPU 用例改成相对判据。未闭环：天空设置一变就整份重烘 31 趟（没有分级失效）；天空关着时仍惰性建一次约 1 MiB 环境图纹理与两条管线（只为固定绑定组布局，不跑 pass）。
> **glTF 缺 `NORMAL` 时按几何生成顶点法线**：旧端 `crates/xgame-render/src/gltf_loader.rs:1335` 的 `compute_face_normals` 在模型没导法线时逐三角形求 `cross(p1 - p0, p2 - p0)`、把面法线累加到三个顶点再逐顶点归一（面积加权平滑法线，不是平直着色）；XGC 此前退回屏幕导数叉积，与旧端在带索引的折角网格上观感不同。切片按旧端同式接上：`glb/normals.rs::face_normals`（非索引图元按 `POSITION` 顺序展开、退化三角形贡献零向量、越界索引跳过、整块无效落 `(0,0,1)`），`glb/attributes.rs` 在缺 `NORMAL` 时调用它。`Mesh.normals` 为空现在只剩「不是 glTF 来源」。回归 4 条纯函数单测 + 1 条折翼几何「生成结果写回显式 `NORMAL` 后逐字节复现同一帧」；红证据：删掉生成分支 → 「没导 NORMAL 时加载期必须补齐每顶点法线 left: 0 right: 4」与「折翼几何没导 NORMAL 时也必须逐顶点生成法线 left: 0 right: 4」。第 150 帧世界哈希仍 `d050bd357f20aa31`、PPM SHA-256 `d1cefdde4ab8ffe8c07b3f7c9d246ddb0fb144e85bb84ff46a3b087bcb363fef`，对旧端仍是 3.48% / mean_abs 0.46 / max 198 / 结构性 1.66%；相对上一版只改 2 个像素（证据世界的 `Fox.gltf` 是非索引几何，生成结果与导数同值）。另各验一次红：把面积权重改成「每面先归一化再累加」后共享边顶点从 `(0, 0.8944, 0.4472)` 变 `(0, 0.7071, 0.7071)`；去掉非索引展开后水平面期望 `(0, -1, 0)` 实得 `(0, 0, 1)`。未闭环：内建图元仍走导数（它们没有 `NORMAL` 流）。

> 场景 pass 4× MSAA 切片把颜色与深度都改成 4 采样（适配器不支持 `MULTISAMPLE_X4` 时退回 1×，1× 等同关闭且逐字节等同切片前），各 pass 画进多采样附件、收尾统一一次 `resolve`；
> `Renderer::set_sample_count` 覆盖档位，换档同时失效 HDR 多采样目标 / 深度 / 管线缓存与 TAA、SSAO 的深度绑定。
> 红证据：`scene_sample_count` 恒 1 → 斜边没有过渡像素；跳过收尾 resolve → 中心与背景都成清屏色；去掉 TAA 的 `set_sample_count` → wgpu 报多采样绑定不匹配。
> 第 150 帧：世界哈希仍 `d050bd357f20aa31`，PPM SHA-256 `8c114a807813adb4e06324a4d8fbd94f9f21a227c43182d607535f4018478cda`；与旧端 30485/921600（**3.31%**）、mean_abs **0.42**、max 198、只差 1 LSB 的 10374 = 1.13%、结构性 15310 = 1.66%；相对上一版只改 0.59% 像素。
> 未闭环：4× 每帧多一张约 14 MiB HDR 多采样纹理、收尾一次全屏 resolve、质量档位旋钮未接 `project.ron`。

## 建议顺序

1. 割接视觉：预过滤环境 IBL（GGX 5 级立方体 + 256 样本 BRDF 积分表，split-sum 取代 SH 近似反射）已接入、天空设置变化才重烘 31 趟、与旧端差异从 32.16% 收到 3.31%（mean_abs 0.58 → 0.42，其中场景 4× MSAA 相对上一版只改 0.59% 像素），再经 glTF 模型材质因子到 2.39%（本机重生成旧端参考，mean_abs 0.36）、再经内建图元逐顶点一致 / 帧首建体 / glTF 动画剪辑 + 状态机 / additive 叠加层到 **2.08% / 0.16 / 0.58%**、再经「根运动」到 **1.64% / 0.09 / 0.36%**（第 200 / 300 帧 2.55% / 2.19%，结构性 0.77% / 0.58%）、再经「layers 分层状态机 / 关节遮罩」到第 200 帧 **2.26% / 0.15 / 结构性 0.54%**（狐狸 bbox 2542 → 478；第 150 / 300 帧逐字节不变），再经「UI 脚本 setter 对齐」到第 150 帧 **1.63% / 0.08 / 结构性 0.35%**、再经「UI 覆盖层 SDF（圆角 AA + 边框）」到 **1.46% / 0.03 / 结构性 0.19%**（第 200 / 300 帧 2.03% / 2.02%，结构性 0.31% / 0.42%；UI 区域结构性归零）、再经「UI 覆盖层 SDF（圆角 AA + 边框）」到 **1.46% / 0.03 / 结构性 0.19%**（第 200 / 300 帧 2.03% / 2.02%，结构性 0.31% / 0.42%；UI 区域结构性归零）；RGBA16F 场景、线性粒子合成、唯一全局 ACES、曝光后曲线前三级光晕、tone 后 UI、蒙皮网格投射 CSM 阴影、256 槽 GPU palette、姿态感知包围盒、蒙皮 draw 单次提交、蒙皮 palette / model 竞技场已贯通；阴影 pass 的蒙皮 palette / model 也已并进同一套竞技场，角色坡面 `slope_limit` 与贴地 `snap_to_ground` 也已生效；角色已有运行时托管的运动学胶囊、水平位移走 rapier `KinematicCharacterController` 的 collide-and-slide、tone-map 后可选的显示空间 FXAA（默认关）、glTF `NORMAL` 顶点法线着色（缺 NORMAL 退回屏幕导数）也已贯通；角色的竖直 / 地面 / 台阶已搬进 KCC、`physics.slope` 也有静态楔形碰撞体、陡坡沿坡面下滑与 `(slope_limit + 10°)` 迟滞带也已生效；画面曝光（`render.exposure`）也已接上（缺省 1.0 逐字节不变），阴影画质（`render.shadow` 的 `softness` / `cascade_blend`，可选 PCSS 软阴影 + 级间混合、默认全 0 = 关）也已接通；多光源照明（方向光 / 点光最多 8 盏 + 点光距离衰减，帧 uniform 扩到 976 字节）也已接通；点光立方体阴影（六面 1024² 深度图、帧 uniform 扩到 1008 字节、`point_shadow_draw_calls` 诊断口径）也已接通；TAA（`render.taa`）也已接通（默认关、关着逐字节不变），SSAO（`render.ssao`，默认关；半分辨率 16 抽样 + 可分离模糊 + 线性上采样乘算）也已接通（关着不建 AO 纹理、帧逐字节不变）。再经「刚体建体改用父链世界变换 + 矩阵分解」到 **0.72% / 0.02 / 结构性 0.13%**（第 200 / 300 帧 1.49% / 1.57%，结构性 0.21% / 0.31%），再经「截图相机裁剪面」到 **1.01% / 0.02 / 结构性 0.08%**（第 200 / 300 帧 1.30% / 1.50%，结构性 0.13% / 0.26%；第 1 帧 0.96% → 0.48%，结构性 0.60% → 0.03%）。本轮「根运动宿主接线」是纯重构：把根运动轨道表提到 `MeshCache::root_motion_library`、CLI / editor / player 三宿主共用（CLI 的 `trace` / `run-scripts` 第 150 帧哈希仍 `b6a3103f8331a972`、`run 60` 仍 `f4bc029f5f995b91`）。
2. 正式调度：脚本与系统统一访问声明。
3. 旧新逐帧 trace（XGC 侧 `xgc trace` 已建、脚本帧也已纳入，缺旧端输出；视觉方面导了 `NORMAL` 的模型已按旧端口径着色）+ 截图像素门禁已建（`xgc gate`，参考在 `xgc/acceptance/`：逐帧 trace 151 个 checkpoint + 4 张截图全过，退出码 0/1/2）；视觉余差归因工具 `image-diff … report` 已建（只差 1 LSB 的 0.72% 像素、结构性 0.08%、与旧端总差异 1.01%），跨帧对照已确认余差是静态的、旧 UI 脚本布局 / 文本 / 样式 setter 与焦点三件套已按旧端语义对齐、UI 覆盖层已换旧端 SDF（圆角 AA + 边框；UI 区域结构性归零、第 150 帧 max 仍 136），余差收窄还剩狐狸位姿残差、木箱与阴影轮廓、红球高光；性能门禁的 XGC 侧热路径分配门禁已建立；CI release 基线已建立（`.github/workflows/xgc.yml` 的 `test` job 跑 `steady_state --ignored`，只记数字）。
4. 密码学签名 / 跨机器增量。
5. 推送 + CI（已通：推送恢复；`.github/workflows/xgc.yml` 运行 45245793893 结论 成功，见矩阵）。
