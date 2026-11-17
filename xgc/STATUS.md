# XGC 状态与割接验收

> 这是能力的**现状快照**，不是完成声明。XGC 仍明显低于 XGameEngine 全量能力，
> 当前不能割接。

## 指标（第 161 轮）

| 项 | 值 |
|---|---|
| Rust 文件 | 206（最大 599 行；软上限 600） |
| 测试 | 333（`cargo test --workspace --offline`） |
| 内建组件 schema | 30 类 / 35709 字节，两次导出逐字节相同 |
| 默认系统 | 18 个 |
| 门禁 | fmt / clippy `-D warnings` / xtask check 全绿 |
| CLI 冒烟 | `check` 30 组件 OK；`run 60` frames=60 entities=3 revision=3 hash=`f4bc029f5f995b91` |

## 已具备

| 子系统 | 现状 |
|---|---|
| ECS / World | archetype 列式存储、持久 `EntityId` + generation 句柄、bundle spawn、快照 / 恢复、状态哈希 |
| 事务 | `Command` plan / apply / revert、单 revision、日志 / 重放、Agent 撤销 |
| Agent 协议 | describe（组件 / 系统 / 渲染能力）、diagnostics、query / get / digest / stats / validate、load / save、snapshot / restore、history / replay、plan / apply / revert / undo |
| 确定性 | 固定步 schedule、trace / verify、schema 字节级确定性、状态哈希 |
| 渲染图 | 资源表、pass 读写、确定性拓扑、环 / 多写者 / 未声明资源检测 |
| wgpu 渲染 | 相机 MVP、深度、顶点 / 索引缓存、按模型实例化、DrawList、颜色合成顺序 `模型基色因子 → .mat 资产 → 实体覆盖 → 选择高亮`；静态 / GPU 蒙皮共享同一份基色贴图、PBR 与 CSM 阴影接收 WGSL；几何 / 天空 / 粒子在线性 `Rgba16Float` 场景中合成后由唯一全屏 WGSL 执行一次 ACES，UI 随后在显示目标上叠加；场景光、天空盒 + 天空漫反射 IBL、4 级 CSM、确定性粒子 billboard（软圆覆盖、Alpha / Additive、Alpha 后向前排序、深测开 / 深写关）、截图相机优先级、线性 / sRGB RGBA8 回读 |
| UI | 布局 / 父相对 / 裁剪 / 命中、输入桥、Tab + 方向键焦点、背景 / 焦点环、系统真字体 + 粗体字面 + kerning / 换行、PNG/PPM 纹理、stretch/preserve/cover/nine-slice、scissor、正式帧末 sRGB alpha 合成 |
| 动画 | RON 剪辑、平移 / 旋转 slerp / 缩放采样、目录加载、写回 Transform、Skeleton / Bone / Pose、GLB JOINTS / WEIGHTS / 逆绑定、≤256 关节 GPU 蒙皮、超限 / 不完整 palette 的 CPU 蒙皮回退，DrawList 自动读取祖先骨架姿态 |
| 物理 | **rapier3d 0.22（与旧引擎同版本 + `simd-stable`）**：静态 / 动态 / 运动学刚体、Auto/Cuboid/Ball/Capsule/Cylinder 碰撞体、密度质量、摩擦 / 弹性 / 传感器、角动量与接触力矩（会翻滚）、每固定帧一次 `step(1/60)`；角色台阶 / 高墙 / 球体阻挡、跳跃、AABB 分离 + 接触清速度保留给角色与斜坡 |
| 音频 | 空间化增益、总线、WAV 解码、软件混音、WAV 写出 |
| Lua | 安全宿主：执行 / 调用 / 字段与实体 API / 指令预算 / 内存上限 / 生命周期；`ScriptRuntime` 驱动脚本实体并热重载；`Simulation::step_with` 可插脚本帧 |
| 资产 / 迁移 | `.meta` 扫描、旧 SceneV2 全组件映射（含 UI / Particle / Rust 插件）、场景级光源 / 环境光迁移与验收、`.mat` 材质资产引用迁移与渲染期解析、动态刚体补零速度（旧引擎速度由物理后端持有）、目录批量迁移 |
| CLI | schema / check / bench / export / import / run / serve / migrate / assets / clips / audio / mix / render-graph / gpu-probe / mesh-probe / render-frame |

## 进度评估（主观加权，非指标）

| 子系统 | 估计完成度 | 依据 |
|---|---:|---|
| 核心 ECS / 确定性 / 事务 / Agent | 80% | 主体可用；缺零分配门禁与跨系统并行 |
| UI | 72% | 布局 / 交互 / 焦点 / 文本 / 图片 / 九宫格 / 圆角 / scissor + 正式帧背景 / 文本 alpha 合成已通 |
| 迁移工具 | 90% | 场景 / 脚本 / 预制体 / 统计对照全通；修复 GlobalTransform 空字段与旧相机 +Z 约定，真实 showcase 1280×720 可渲染出场景；缺旧新逐帧视觉对照 |
| 动画 | 97% | 根 / 关节轨道 + 骨架 + CPU / GPU 蒙皮 + 状态机 + 叠加层（实体 / 关节遮罩）+ 转场淡入；缺根关节混合权重细调 |
| 音频 | 60% | 空间化增益、总线、WAV 解码、软件混音、WAV 写出、cpal 默认输出设备播放（重采样 / 队列）、`xgc play`；缺流式资产 / 3D 声像 |
| 渲染 | 90% | 真实 builtin 几何 + glTF 基色因子 / UV / 基色贴图 + `.mat` 资产乘子 + Cook-Torrance 直接光 + 天空盒 / 漫反射 IBL / 环境反射 + 4 级 CSM（**静态与蒙皮网格都投射**）+ **确定性 CPU 粒子模拟与实例化 billboard**（软圆、Alpha / Additive、后向前排序）+ **GPU 蒙皮共享静态 PBR / 贴图 / 阴影接收**（256 槽 palette）+ **RGBA16F 线性场景 / 粒子混合 / 单次全局 ACES / tone 后 UI** + **曝光后曲线前的三级 HDR Bloom**（默认关）+ **蒙皮 palette / model 竞技场（dynamic offset 合并，主场景与阴影 pass 各一套）** + 真字体 UI + sRGB HUD + 截图取景；缺 PCSS / 点光阴影 / TAA·SSAO·FXAA 等其余后处理（级联正交盒与旧端同为视锥切片拟合 + 逐级剔除，不算缺口） |
| 物理 | 55% | rapier3d 0.22 与旧引擎同一条求解路径（角动量 / 接触力矩 / 堆叠），动态刚体帧状态与旧图可比；缺角色控制器接 rapier、关节 / 连续碰撞 / 坡面滑动、增量增删刚体（当前按数量变化整表重建） |
| 资产 / 打包 | 70% | 引用裁剪 + 增量烹饪（哈希未变跳过写入）+ manifest 聚合签名；JSON / XGCB / 图形播放器；缺密码学签名 / 跨机器增量 |
| Lua 脚本 | 84% | 旧 `update(ctx)` + 移动 / 旋转 / 查询 / UI / `action_pressed`；`look_at` 按 Y-up 对准且无滚转（有回归）；`InputMap` 解析 input.ron 动作与轴；缺窗口键盘事件接线与音频脚本 API |
| 编辑器 | 88% | 层级 / Inspector / Play / 视口 / Undo / Gizmo / 吸附 + 预制体实例化 / Revert / 位置覆写 + Play Scripts（脚本 + input.ron 动作） |
| 割接验收 | **98%** | 统计对照一致；旧 / 新截图同用场景相机、同一第 150 帧；RGBA16F 场景 + 全局 tone-map 后 mean_abs **0.61**、差异像素 **32.22%**、max 194；余差主要是狐狸动画 / 几何边缘与 UI，HDR 架构差异已关闭 |

**整体能力对等约 65%，割接就绪约 58%**。核心运行时、迁移、动画、编辑器和
cooked 播放器已可用；差距集中在渲染高级特性、正式调度、逐帧验收与推送。

## 迁移 / 运行证据

- `projects/XGame/Scenes/showcase.ron` → 24 实体 / 51 组件 / 0 缺失父节点。
- `run-world` 60 帧通过；`render-frame` 第 150 帧 hash `bb3fdc99df20aa31`（`particle.emitter.config` 保存完整旧配置并参与状态哈希，粒子运行态不落盘），1280×720 输出 14 个场景 draw / 7 个 draw call（5 个场景批次 + Alpha / Additive 粒子各 1）/ 8 个 UI draw；迁移逐字段验证仍为“一致”。
- 旧 / 新 headless 统计逐项一致：24 实体 / 10 刚体 / 10 碰撞体 / 2 发射器 / 1 监听者。
- **旧端截图相机已确认为 `mode: "scene"`**（重跑旧 `xgame-editor` 读到的报告），两次旧运行图像逐像素相同（0/921600）；XGC 的 `render-frame` 现在按同一优先级取景：有场景相机就用场景相机，只有缺相机时才退到顺光自动取景。
- 场景级光源与环境光已迁移进 `WorldData.environment`（showcase：`Sun` 方向 (0.45,-1,0.35)、强度 1.4、色 (1.0,0.97,0.92)，`ambient` (0.09,0.1,0.13)）；`verify-migration` 逐光源字段对照，缺省环境不落盘。
- `render-frame` 缺省 0 帧；显式帧数复用 `Simulation::step_with` + `ScriptRuntime::run_frame`，最后重算层级与目标尺寸 UI。**接触清速度**接入后第 150 帧旧 / 新差异 903854/921600（98.07%）、mean_abs **18.78**、max 207 与上一步**逐像素相同**——150 帧时物体早已落定；此前的问题是「贴地后每帧仍带着累积速度抖动」，单帧看不出来但播放会抖。世界哈希变为 `0b6d186abfc5ca5b`：位置不变、存的**速度**从漂移值变成 0，符合预期。

**阴影 pass 的蒙皮竞技场**接入后，逐投射体的 palette（16 KiB）与 model（64 B）不再各建缓冲：`shadow_skin.rs` 的 `SkinnedShadow` 也持有两张 `UniformArena`，layout 声明 `has_dynamic_offset: true` 后每级级联只换偏移；`SkinnedBinding` 只记槽号，不再持有绑定组。为此把逐帧阴影准备从 `renderer.rs` 拆到 `crates/xgc-render/src/renderer/shadow_prep.rs`（行数预算），`ShadowTarget::render` 改成 `&mut self` 并返回本帧新建的缓冲数，落到 `RenderStats::shadow_skin_uniform_buffers`（`skin_uniform_buffers` 仍只统主场景）。回归 `repeated_frames_reuse_the_shadow_skin_arenas` 在同一渲染器上连渲两帧断言 2 → 0；把 `upload` 开头临时插 `self.slots = 0;` 时该用例实跑变红（「同尺寸第二帧必须复用阴影蒙皮竞技场、不再建缓冲」）。既有 4 条阴影回归（GPU palette 投影、257 关节 CPU 回退、阴影随姿态平移、单位 palette 与静态网格逐像素相同）仍全绿；另加一条两投射体用例 `two_skinned_casters_keep_their_own_shadow_palettes`：两个投射体沿屏幕纵轴分开（z=∓2.4）、姿态反向平移，断言两侧阴影行重心相距超过两个窗口宽、且同屏时各自留在原位（把每级 draw 的偏移强制成槽 0 时实跑变红：「窗口内阴影像素太少，无法定位：0」），钉住"每个 caster 用自己那格 palette"。第 150 帧与上一版**逐字节相同**（证据世界无骨架）：hash `bb3fdc99df20aa31`、14 draw / 7 call / 8 UI draw、PPM SHA-256 `4A47275DD54A4AA2780353B6B37E6BD2999F9D16BEDA20197973BA6DB48A0927`，对旧端仍 32.22% / 0.61 / 194。**未做稳态计时**：证据场景没有蒙皮 draw，冷启动单帧含 pipeline 编译，不能归因于本刀。

**蒙皮 uniform 竞技场**接入后，逐角色的 palette（16 KiB）与 model（96 B）不再各建缓冲：`skinned/arena.rs` 的 `UniformArena` 把整批槽位并进一张常驻缓冲，槽距按设备 `min_uniform_buffer_offset_alignment` 向上取整（16 KiB palette 仍是 16 KiB，96 B model 补到 256 B），layout 声明 `has_dynamic_offset: true` 后每条 draw 只换一次偏移；容量按 2 的幂扩容（只增不减，封顶在历史峰值槽数），暂存区跨帧复用，同尺寸的第二帧一张缓冲都不建（`queue.write_buffer` 与提交同队列有序，重写本帧缓冲必排在上一帧提交之后）。`RenderStats::skin_uniform_buffers` 是这个代价的可观测口径：首批 2 张（palette / model 各一张），同尺寸第二帧 **0**（旧实现每帧每个角色 2 张，随角色数线性涨）。两条单元回归钉住槽距对齐（16 KiB 偏移恰 16384、model 偏移是对齐的整数倍且 ≥96 B）与“同尺寸复用 / 涨过容量才扩容”，集成回归 `repeated_frames_reuse_the_skin_uniform_arenas` 在同一个渲染器上连渲两帧断言 2 → 0；把 `upload` 开头临时插 `self.slots = 0;` 时该用例实跑变红（“同尺寸第二帧必须复用竞技场、不再建缓冲：2”）。既有的左右角色差分用例仍逐字节通过（批处理把 palette 串了会立刻红）。第 150 帧与上一版**逐字节相同**（证据世界没有骨架实体，竞技场根本不建缓冲）：hash `bb3fdc99df20aa31`、14 draw / 7 call / 8 UI draw、PPM SHA-256 `4A47275DD54A4AA2780353B6B37E6BD2999F9D16BEDA20197973BA6DB48A0927`，对旧端仍 32.22% / 0.61 / 194。**未做稳态计时**：证据场景没有蒙皮 draw，冷启动单帧含 pipeline 编译，不能归因于本刀；“每帧缓冲数 2N → 0”是结构性收益，真正的稳态测量仍要常驻进程 profiler。当时**未闭环**的阴影半边已在下一轮补齐（见上）。

**画面设置里的三级 HDR Bloom**（旧端 `render.bloom` / `BloomConfig` 同口径）接入后，线性场景在**曝光后、唯一 ACES 前**叠一层亮部光晕：`bloom.wgsl` 逐行照旧端 `postfx.rs` 的软膝式（按最大通道取亮度，`soft = clamp(b - threshold + knee, 0, 2*knee)`、`contribution = max(soft²/(4*knee + ε), b - threshold)`、`weight = contribution / b`），三级 `BLOOM_LEVELS = 3` 先降到 ½ / ¼ / ⅛ 再横纵各一次 9 抽头二项核（70/56/28/8/1 除以 256），每级 2 个 pass 加首轮亮部提取共 9 个 pass；权重 0.5 / 0.3 / 0.2 在 `tone_map.wgsl` 里乘 `intensity` 叠回线性场景**然后才过 ACES** —— 过完曲线的高光已经没有余量，所以光晕必须在曲线前，这也是它必须和 tone-map 同一个 pass 合成的原因（多一趟显示空间 pass 就会把已映射过的值再映射一次）。默认 `intensity = 0` 是关闭态：整条链不跑、`passes` 不增、第 150 帧 PPM 与上一版**逐字节相同**（hash `bb3fdc99df20aa31`、14/7/8、SHA-256 `4A47275DD54A4AA2780353B6B37E6BD2999F9D16BEDA20197973BA6DB48A0927`、自比 0/921600），对旧端仍 32.22% / 0.61 / 194。七条 GPU 回归 + `bloom/tests.rs` 两条单元回归 + `renderer::hdr` 一条释放回归（`sanitized` 容错逐字段、源尺寸变了链确实重建、关掉后 `views()` 回 `None`）钉住：关闭态与不调 `set_bloom` 逐字节等价且 `passes` 不变、开态抬亮量与旧端公式对表（差 ≤2）、低于阈值的暗场景即使 `intensity = 4` 也逐字节不变（软膝式在该亮度贡献恰为 0）、尺寸变化重建链且同尺寸两次逐字节确定（8×8 → 16×16 → 5×3）、亮几何的光晕向外扩散而角落不同步抬亮（中心抬升 ≥ 2× 角落）、开过再关必须回到"从没开过"的逐字节画面且不再跑链。把合成行的 `* bloom.intensity` 临时改成 `* 0.0` 时后三条实跑变红（"开着 218，关着 218"、"实际 218，期望 229"、"中心 0"），证明回归真的走到合成而不是只在测关闭态。`project.ron` 的天空 / 光晕解析从 `xgc-cli/src/render.rs` 搬到 `xgc-cli/src/project_render.rs`（缺节 = 关、缺省逐项等于旧端 `SkyConfig` / `BloomConfig` 的 `default`），顺手守住 600 行软上限。**仍未闭环**：没有暴露 `render.exposure` 旋钮（旧端默认 1.0，光晕提取前先乘曝光；XGC 里曝光恒为 1，两者在默认设置下等价，一旦要支持调曝光就得同时接进 `instanced.rs` 的帧 uniform 与光晕提取）；光晕链的 6 张中间目标（三级 × a / b 乒乓）按尺寸缓存，尺寸不变时不再分配；开启时每帧跑 9 个 pass 并重建亮部提取与 tone-map 各**一个**绑定组（链内 8 个绑定组在建链时一次建好），关闭态还会把 tone-map 绑定组按场景纹理缓存（不随帧重建）；关掉（`intensity = 0`）时按旧端合同把链整份释放、只留管线与采样器。

**蒙皮 draw 单次提交**接入后，一帧里所有 GPU 蒙皮 draw 共用**一个** frame binding、**一个** pass、**一次** submit（旧端也是 palette 逐角色、提交不逐角色）：`SkinnedPipeline::draw_batch` 先把每条 draw 的 palette / model 绑定建好，再一次 encoder + 一次 render pass 画完；此前每条蒙皮 draw 各建一次 frame bind / encoder / submit，多角色场景的提交次数随角色数线性增长。palette 仍逐角色（角色姿态本来就不同），逐角色的 16 KiB palette 上传没有合并（下一轮竞技场已把逐角色缓冲并进常驻缓冲，见下）。新回归 `batched_skinned_draws_keep_their_own_palettes` 用左右两个角色做差分：右侧角色换姿态时左侧按列切出的像素必须**逐字节不变**、右侧必须变化——串 palette 的批处理会立刻红。第 150 帧仍逐字节相同（证据世界没有骨架实体）：hash `bb3fdc99df20aa31`、14/7/8、PPM SHA-256 `4A47275D…`，对旧端仍 32.22% / 0.61 / 194。**未做稳态计时**：证据场景没有蒙皮 draw，冷启动四跑（本机当时有并发构建，818–998 ms）不能归因于本刀；提交次数 N→1 是结构性收益，真正的稳态测量仍要常驻进程 profiler。为守住 600 行软上限，测试夹具（两个 GLTF 构造器）从 `skinned_pbr` / `skinned_shadow` 提到 `tests/skinned_support/` 共用。**仍未闭环**：级联正交盒仍按相机视锥切片拟合（视锥外投射体不保证进深度图）；palette / model uniform 仍逐角色分配（下一轮竞技场已并进常驻缓冲，见下）。

**姿态感知的场景包围盒**接入后，`scene_bounds` 不再用绑定姿态的网格盒子：蒙皮实体改用 `posed_bounds`，两桶求并 —— **会动的顶点**按"真的有 palette 项"的关节各变换一次整网格 AABB（逐顶点结果是这些变换点的凸组合，并集 AABB 必然覆盖凸包；同一关节的盒对整张网格是常量，因此只需 O(顶点 + 关节) 次变换），**不动的顶点**（缺 JOINTS_0 / WEIGHTS_0、权重全零、关节下标越界）按顶点并进盒子——`skin_positions` 与 shader 都让它们留在绑定姿态，漏掉这一桶就会把这类顶点裁出画面（补回归前实跑确认：盒只剩 `[4.5..7.0]`，绑定顶点在 -0.5 之外）。它同时喂给自动取景（`frame_scene`）与级联远距离拟合（`prepare_shadow` 的 `far_d`）。五条新回归：三条单元（混合权重顶点都在盒内、平移 palette 时盒整体跟随、零权重顶点不被漏掉）与两条集成（场景包围盒跟着 +4 的姿态走，旧实现停在 `[-0.8..0.8]`；被姿态移开的网格仍在自动取景画面内，旧实现 **0** 个可见像素）。**已知近似**：这里用"整网格 AABB 按关节变换"，比旧端加载期预计算的"每关节顶点范围盒"更松，带骨架场景的自动取景会比旧端略远；等旧新逐帧截图像素门禁上骨架场景时需要按旧端口径收紧。第 150 帧与上一版逐字节不变（证据世界没有骨架实体）：hash `bb3fdc99df20aa31`、14/7/8、PPM SHA-256 `4A47275D…`，对旧端仍 32.22% / 0.61 / 194。性能用"两个可执行文件交错跑"的同机 A/B（每轮 before / after 各一次，抵消热漂移）：中位数 752.62 → 765.10 ms（+1.7%，区间 720–779 / 721–826 重叠）；该场景没有任何 pose item，新分支对每个 draw 只多一次 `Option` 判空（带骨架时则每帧多一次上面那笔边界计算），判定未见可归因回归。**仍未闭环**：级联正交盒本身还是按相机视锥切片拟合的，视锥外的投射体不保证进深度图（旧端也是 frustum 拟合 + 逐级剔除，但会为相机外投射体留 shadow-only 尾段）；多角色批量提交当时还没做（第 27 轮已补：同一模型的多条蒙皮 draw 合成一次提交）。

**256 关节 GPU palette** 接入后，65~256 关节的蒙皮网格不再静默掉到 CPU 慢路径：`MAX_JOINTS` 64 → 256（与旧引擎 `BONES_PER_SLOT = 256` 同容量），palette uniform 4 KiB → **16 KiB**（仍在 wgpu 默认 `max_uniform_buffer_binding_size` 64 KiB 之内），`skin_vertex.wgsl` 的数组长度与 Rust 侧 `MAX_JOINTS_BYTES` 同步。路由判据仍是"palette 可完整表达"（每根有权重的关节都要有 pose 与逆绑定），257 关节或 palette 不完整照旧确定性回退 CPU，阴影两支路都跟着同一条判据走。新 GPU 回归把权重全压在 90 号槽并平移它：把上限临时改回 64（等价旧 routing）时该网格掉回 CPU（实跑 `gpu_skin_meshes` 0 ≠ 1 变红），现在留在 GPU 且表面随 90 号槽右移；单元测试钉住"256 接受 / 257 拒绝"与 16 KiB palette 字节数，原 65 关节回退用例改为 257 关节、回退像素合同不变。第 150 帧与上一版**逐字节相同**（证据世界没有骨架实体）：hash `bb3fdc99df20aa31`、14 draw / 7 call / 8 UI draw、PPM SHA-256 `4A47275DD54A4AA2780353B6B37E6BD2999F9D16BEDA20197973BA6DB48A0927`，自比 0/921600，对旧端仍为 296950/921600（32.22%）、mean_abs **0.61**、max 194。release 冷启动四跑中位数 734.43 → 743.15 ms（+1.2%，四跑 717–792 ms 与改前 729–789 ms 区间重叠；该场景不含蒙皮 draw，本刀不可能影响它）。另加设备上限判定：downlevel GL 路径的 `max_uniform_buffer_binding_size` 正好等于 16 KiB，更小的设备在路由阶段就退回 CPU，不会等到建 bind group 才失败。代价：每个蒙皮 draw 每帧多传 12 KiB palette（以 24 关节狐狸为例），相对一帧约 15.8 MiB 的 HDR 目标可忽略；多角色批量提交仍是缺口。

**蒙皮网格投射 CSM 阴影**接入后，静态与蒙皮投射体进同一批级联深度 pass：`skin_vertex.wgsl` 成为 GPU 变形公式唯一真相源（主场景与阴影入口共同 include），`palette_bytes` 的 `pose × inverse_bind` 也只算一次给两边。≤256 关节走 palette 管线（顶点流只要位置 / 关节下标 / 权重，`fragment: None`、`Depth32Float`、与静态投射体共用 constant 2 / slope 2 偏置），>256 或 palette 不完整的先 CPU 变形再按静态顶点流画（缓存 key 与主通道同一份）；每帧每 caster 的 palette / model 绑定在级联循环外建一次，四级复用。四条 GPU 像素回归在旧实现下全红（变暗 0 像素），现在 GPU 路径与 257 关节回退各让 176/9216 像素变暗、阴影质心随关节平移移动、单位 palette 与静态网格阴影逐像素相同。证据世界 `showcase-particles.json` 里没有 `animation.skeleton` / `animation.pose` / `animation.animator`（唯一叫 Fox 的实体是静态网格），所以第 150 帧与上一版**逐字节相同**：hash `bb3fdc99df20aa31`、14 draw / 7 call / 8 UI draw、PPM SHA-256 `4A47275DD54A4AA2780353B6B37E6BD2999F9D16BEDA20197973BA6DB48A0927`，自比 0/921600 像素差异，对旧端仍为 296950/921600（32.22%）、mean_abs **0.61**、max 194。release 新进程冷启动四跑中位数 757.02 → 734.43 ms（-3.0%；仍只判定未见回归）。**当时明确未闭环**：`scene_bounds` 仍按绑定姿态拟合级联，大位移关节可能落在拟合外；CPU 回退每帧多算一次变形；GPU palette 在主 / 阴影两个 pass 各建一次 —— 前两项已由姿态感知包围切片与大位移用例补齐，第三项已由「蒙皮 uniform 竞技场」两刀并进常驻缓冲（见上）。

**RGBA16F 场景 + 单次全局 tone-map** 接入后，静态 / GPU 蒙皮、天空和粒子全写线性 HDR；Alpha / Additive 粒子先混合，`tone_map.wgsl` 再对整帧执行唯一一次 ACES，UI 最后在显示目标上叠加。第 150 帧世界 hash 与 14 draw / 7 call / 8 UI draw 不变，PPM SHA-256 为 `4A47275DD54A4AA2780353B6B37E6BD2999F9D16BEDA20197973BA6DB48A0927`；相对上一版 XGC 有 57034/921600（6.19%）像素变化、mean_abs 0.04、max 13，相对旧端改善为 296950/921600（32.22%）、mean_abs **0.61**、max 194。GPU 回归钉住线性 Additive / Alpha 合成、UI 不进 ACES、resize / 输出格式切换和最终 alpha=1。release 新进程冷启动中位数 818.57 → 741.66 ms（-9.4%）；进程启动与 shader / pipeline 编译噪声远大于全屏 pass 的稳态成本，只能判定未见冷启动回归，不能据此声称 HDR 更快。

**GPU 蒙皮共享主场景着色**接入后，≤256 关节且 palette 完整的网格不再因基色贴图退回 CPU：静态与蒙皮顶点入口共同拼接唯一的 `scene_shading.wgsl`，因此逐 draw 基色 / metallic / roughness、sRGB 基色贴图、环境光 / 天空 SH、Cook-Torrance 直接光与 4 级 CSM 阴影接收不再有第二份公式；ACES 后续已从场景片元移到唯一的全屏 `tone_map.wgsl`。GPU identity 与静态画面逐字节相同；关节平移会带着非均匀贴图像素移动；超出 palette 容量的 CPU 回退逐字节保留 UV / 贴图（本刀起门槛是 257 关节，此前为 65）；静态投射体能在 GPU 蒙皮表面留下阴影。第 150 帧仍为 hash `bb3fdc99df20aa31`、14 draw / 7 call / 8 UI draw，输出与粒子切片逐字节相同（PPM SHA-256 `CCBD1EF164D691EF3617D1CDA48BC20D3D3CB15E7277F6EF3C98A5C1913671CB`），旧 / 新差异仍为 309707/921600（33.61%）、mean_abs **0.63**、max 194。冷启动 release `render-frame` 四跑中位数由 **730.44 ms** 变为 **806.42 ms**（+10.4%；每跑都是新进程且只渲一帧，含新增 shader / pipeline 编译，且当时 GPU 蒙皮仍按实体单独建 uniform / bind / encoder 并提交（uniform / bind 已由竞技场切片并进常驻缓冲，encoder 已是单批一次），不能当稳态 GPU 帧耗时）；多角色批量提交与蒙皮投射阴影当时仍是后续缺口（均已在此后切片补齐，256 槽 palette 亦然）。

**确定性粒子模拟 + billboard 渲染**接入后第 150 帧旧 / 新差异 309707/921600（33.61%）、mean_abs **0.63**、max 194（上一步 0.85 / 33.62%）：CPU 侧按旧端契约复现 SplitMix64、发射累加器、duration / looping、球 / 盒 / 点采样、锥角、阻力 / 重力、世界空间生命周期与稳定保留，并在边界按同范围清洗配置 / 设 200k 硬上限、缓存 RON 解析；GPU 侧画相机朝向软圆 billboard，Alpha 后向前排序并用预乘混合，Additive 单独批次（有实际累加像素回归），深度测试开而深度写关闭。完整旧 `ParticleEmitter` 直接强类型反序列化，避开 `ron::Value` 丢失枚举变体名的问题；迁移配置逐字段验证为“一致”。截图中的 Campfire.Smoke 大块灰斑位置 / 大小 / 透明度已对齐；后续 HDR 切片已经拆除当时的单粒子 ACES 近似，Alpha / Additive 现在线性场景中合成后统一 tone-map。

**4 级 CSM** 接入后第 150 帧旧 / 新差异 309856/921600（33.62%）、mean_abs **0.85**、max 194（上一步 0.87 / 33.68%）：把旧端的 `cascade_splits`（practical split，λ=0.7）/ `cascade_shadow_matrices`（逐段紧致正交拟合、纹素吸附防抖）逐行移植，深度图从单张换成 4 层数组、每级一次 pass，片元按 `dot(world - eye, forward)` 选层；`shadow.rs` 拆成 CPU 拟合 `shadow_csm.rs` + GPU 侧，帧 uniform 368 → 592B。阴影边缘/远处软化向旧图靠拢。**顺带定位**：画面左侧那片大块软灰斑（占当时剩余差异约 30%）在放大对照后确认是旧端的**篝火烟雾粒子**（Campfire.Smoke），不是阴影问题；该缺口已由后一轮确定性粒子切片补齐。

**UI 粗体字面 + 原生 kerning** 接入后第 150 帧旧 / 新差异 310384/921600（33.68%）、mean_abs **0.87**、max 194（上一步 0.96 / 33.89%）：旧端的粗体是**真正的粗体字面**（msyhbd.ttc 等候选），不是描边放大；这轮把粗体候选接进 `FontAtlas`（缺粗体时按旧端约定静默退回常规字重），并去掉此前的「合成粗体」（同字二次绘制 +0.6px），同时把 kerning 从「只取正值」改成字体原生值（负字距也生效）。标题 / 按钮 / 副标题的笔画粗细与字距都向旧图靠拢，UI 面板区域平均差 3.14 → 1.41、贡献 18.6% → 9.3%。世界哈希 `3a23384035e76a42` 不变（纯渲染改动）。

**直接光 Cook-Torrance PBR** 接入后第 150 帧旧 / 新差异 312313/921600（33.89%）、mean_abs **0.96**、max 194（上一步 1.65 / 64.26%）：着色器逐式照搬旧端 `scene_pipeline` 的 PBR —— GGX 分布 + Smith 可见性 + Schlick 菲涅尔、漫反射按 `(1-F)` 扣减、`radiance = 光色 × 强度 × π` 的 π 校准；环境光也从「基色 × ambient」改成旧端 `ambient × (albedo·(1-metallic) + F_env)`（平底材质的蓝通道因此有个菲涅尔小底）。棕箱区域 4.44 → 1.45、蓝箱 5.22 → 2.12、狐狸 6.93 → 5.84。CPU 镜新增 `ambient_light` / `fresnel_schlick` / `sky_kd_env`，所有平光测试期望值改由镜像现算。世界哈希 `3a23384035e76a42` 不变（纯渲染改动）。

**UI 真字体**接入后第 150 帧旧 / 新差异 592265/921600（64.26%）、mean_abs **1.65**、max 200（上一步 1.81）：UI 文本从 5x7 位图换成 `ab_glyph` + 与旧端同一套系统字体候选（msyh/Deng/simhei…），CPU 侧 shelf 打包字形图集（RGBA8，rgb=255、alpha=覆盖率），GPU 侧独立 alpha 混合管线一次 draw 画完整段文本；字形落点吸整数像素、图集先排版后上传。`ui.element` 新增 `text_align` / `text_valign` 两个字段（schema 34950 → 35437 字节）：旧场景的 align/valign 是 RON 单元枚举，经 `ron::Value` 转换后变体名丢失，迁移只能按 kind 推断（按钮居中、其余左对齐、垂直居中）。UI 面板区域平均差 5.19 → 3.14、贡献 16.8% → 10.9%。世界哈希 `3a23384035e76a42`（UI 组件多了两个字段，属预期）；`run 60` 的 demo 世界无 UI，哈希保持 `f4bc029f5f995b91`。

**rapier 物理后端**接入后第 150 帧旧 / 新差异 594588/921600（64.52%）、mean_abs **1.81**、max 201（上一步 2.88）：与旧引擎同版本（rapier3d 0.22 + `simd-stable`）、同 `IntegrationParameters::default()`、同重力 `(0,-9.81,0)`、同「每固定帧一次 `step(1/60)`」；静态 / 动态刚体与 Auto 形状按旧端 `body_from_scene` 的密度质量规则建体，动态体每步回写 Transform / Velocity，运动学体每步推送位姿。右侧棕箱区域平均差 18.56 → 4.44、贡献 40.9% → 15.6%，红 / 蓝堆也同步改善——**翻滚姿态第一次和旧图对上**。世界哈希变为 `28ade8bd9fc5ca5b`；`run 60` 的 demo 世界不含 `physics.body`，哈希保持 `f4bc029f5f995b91` 不变。

**网格基色贴图**接入后第 150 帧旧 / 新差异 595546/921600（64.62%）、mean_abs **2.88**、max 201（上一步 3.04）：glTF 的 `TEXCOORD_0` 与 `baseColorTexture` 解析进 `Mesh`（外部 uri / GLB 内嵌两种来源），pipelines 统一绑定 `1×1 白纹理` 兜底，着色器按 `基色因子 × 实体颜色 × 贴图` 取 albedo；狐狸区域平均差 12.69 → 7.07。该轮带贴图蒙皮仍临时走 CPU 蒙皮 + 实例化；后续 GPU 蒙皮共享主场景着色切片已拆掉这条临时分流。

**脚本 `look_at` 朝向修正**后第 150 帧旧 / 新差异 **595547/921600（64.62%）、mean_abs 3.04、max 201**（上一步 mean_abs 18.78、差异 98.07%）。根因：`look_at` 把俯仰塞进欧拉角 Z 分量，每帧把相机转成「yaw 正确、俯仰 0、滚转 45°」——整帧取景与天空方向全偏。修正后天空 / 地面区域几乎逐像素一致（底部 30% 平均差 0.53），剩余差异集中在 UI 面板（5x7 位图 vs 旧字体）与物体区域（狐狸纹理、落点 / 影子）。世界哈希 `67505484bfc5ca5b`（相机旋转变了，属预期）。

**逐材质 metallic / roughness** 接入后第 150 帧旧 / 新差异 903854/921600（98.07%）、mean_abs **18.78**、max 207（上一步 19.27）：实例数据 80B → 96B 带上 `[metallic, roughness]`，着色器按 `f0 = mix(0.04, 基色, metallic)`、漫反射乘 `(1 - metallic)`、粗糙度夹到 `MIN_ROUGHNESS = 0.04` 计算环境反射（与旧引擎 split-sum 同形，env 仍是 SH 近似）。棕色表面 (119,84,56) 对旧 (118,85,62)、红色 (166,13,34) 对旧 (161,13,30)——颜色通道基本对上；剩余差距集中在**几何位置 / 影子（刚体求解器）**与直射光高光、纹理。
- 天空参数按项目级读取：`project.ron` 的 `render.sky`（`enabled/intensity/zenith/horizon/ground/sun_size/sun_intensity`），缺省 = 天空关（画面与接入前一致）；CPU 侧用 1024 点斐波那契球把解析天空投影成 L2 球谐（E/π 口径），与常量 `ambient` 同标定直接相加。
- 无 `animation.state` 时 `set_anim_param` 现在安全 no-op，不再让同帧其它脚本命令整批失败；状态机迁移仍是独立差距。
- GUID 资产（Box.glb、Fox.gltf + Fox.bin）可解析；glTF 首图元 `baseColorFactor` 在静态实例与 GPU 蒙皮路径均参与最终颜色，且选择高亮仍能替换最终颜色；character 脚本 / Pillar 预制体可迁移。
- cooked 包：引用裁剪 7 文件、XGCB 二进制（v2 带场景环境）、manifest 哈希 / 签名校验、`xgc-player` headless 60 帧。
- 自动化测试：333 条；正式帧回归覆盖 sRGB alpha HUD、帧数契约、固定步推进与最终父子 `GlobalTransform` 合成；基色因子、场景光、`.mat` 乘子、HDR 粒子合成、唯一 ACES、三级 Bloom、tone 后 UI、蒙皮网格投射阴影与输出格式 / resize 均有 GPU 像素门禁（期望值由 CPU 镜像现算），256 槽 palette 的路由与字节数、蒙皮 uniform 竞技场（主场景与阴影 pass 两套）的槽距对齐与跨帧复用另有单元 / 集成回归，环境往返 / 迁移 / 验收 / 取景拟合均有 CPU 回归。

## 与 XGameEngine 的主要差距

| 差距 | 影响 | 优先级 |
|---|---|---|
| 渲染高级特性 | 几何 / 材质 / PBR / 基色纹理 / 天空盒 + 漫反射 IBL / 环境反射 / 4 级 CSM（静态 + 蒙皮投射）/ RGBA16F 场景 + 全局 ACES / tone 后 HUD / CPU 粒子 + billboard / GPU 蒙皮主场景着色（256 槽 palette）+ 曝光后曲线前三级 Bloom 已通；缺 PCSS / 点光阴影 / TAA·SSAO·FXAA 等其余后处理（级联正交盒与旧端同为视锥切片拟合 + 逐级剔除，不算缺口） | P0 |
| 正式 System 调度 | `SystemAccess` / `batches()` 已有，执行仍顺序；脚本尚未成为正式 System | P0 |
| 视觉 / trace 验收 | 第 150 帧同 tick 截图可复现，mean_abs 0.61；仍缺旧端逐帧 trace / 状态哈希对照与截图阈值门禁 | P0 |
| 物理完整性 | 简化角色 / AABB；无完整刚体、关节、连续碰撞、坡面滑动 | P1 |
| 跨系统并行 | 未做世界分片 / 并行执行 | P1 |
| 资产签名 | FNV 聚合校验，非密码学签名；无跨机器增量 | P1 |
| 编辑器细节 | 预制体 / Gizmo / Play Scripts 已通；缺旋转缩放快捷键细化、Prefab Apply | P1 |
| 音频 | 设备输出 / 混音已通；缺流式资产 / 3D 声像 | P1 |
| 图形播放器验收 | 窗口代码已编译，未实际开窗截图 | P1 |
| UI 字体 / 网络 | ASCII 5x7；无字体资产 / 中文；无多人 / 存档 | P2 |

## 割接验收门槛

1. **通过**：旧项目场景 + 资源 + 脚本 + 预制体端到端迁移并运行。
2. **通过（基础）**：骨骼 / 蒙皮、Lua、编辑器三项 P0 能力可用。
3. **未通过**：旧 / 新同一场景的 trace、状态哈希、视觉截图逐帧对照。
4. **未通过**：性能基准与热路径分配门禁。
5. **未通过**：推送与 CI；当前所有 commit 因无 GitHub 凭据**未推送**。

## 下一步（按顺序）

1. 割接视觉：RGBA16F 场景、线性粒子合成、唯一全局 ACES、tone 后 UI、蒙皮网格投射 CSM 阴影、256 槽 GPU palette、姿态感知的场景包围盒、蒙皮 draw 单次提交、曝光后、ACES 前的三级 HDR Bloom 已贯通；阴影 pass 的蒙皮 palette / model 也已并进同一套竞技场，下一刀回物理角色控制器 / 关节。
2. 正式调度：把脚本与默认系统纳入 `Schedule` 的访问声明批次。
3. 逐帧 trace / 截图对照与性能门禁。
4. 密码学签名、跨机器增量烹饪。
5. 推送与 CI 全绿。
