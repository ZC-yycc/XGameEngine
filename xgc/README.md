# XGame Core（XGC）

这是 `Docs/引擎框架设计` 提案的**独立实现 workspace**，与根仓库的
`xgame-*` crate 不互相依赖，也不被根 workspace 收录。旧引擎仍是唯一
产品路径；XGC 在 M0–M6 完成并通过割接门禁之前不替换它。

> 现状、差距与割接门槛见 [`STATUS.md`](STATUS.md) 与 [`CUTOVER.md`](CUTOVER.md)。当前**不能割接**。

## 当前阶段

**M0 已完成；M1 前十刀已落地**：`xgc-base`（持久 ID / 句柄 / 诊断）、
`xgc-reflect`（组件注册表 / 确定性 schema / 动态字段读写 / schema 语义校验）、
`xgc-world`（archetype 列式存储、单 / 双组件可变查询、按 archetype 分片的并行
遍历、生成回收、`Command` 事务 plan / apply / revert、结构化 `Spawn` / `Despawn` /
`AddComponent` / `RemoveComponent` + 实体快照撤销、可序列化 `EntityData` / 组件工厂、
`Transform` / `Velocity` / `Name` / `Parent` / `GlobalTransform` +
`render.mesh` / `render.material` / `render.camera` + `script.lua` / `audio.source` +
`physics.body` 十一个内建组件（`xgc_world::builtin_registry` 是 CLI / Agent / xtask
的唯一清单）、实体级 revision、世界快照 / 恢复、事务日志 JSON 往返与重放、状态哈希、
`World::validate_data` 世界体检）、`xgc-store`（版本化 JSON 包络 + 原子写）、
`xgc-sim`（Scenario / Trace / 固定步 `Schedule` / `SystemAccess` 冲突判定 /
`Schedule::batches` / `PhysicsSystem` / `CharacterJumpSystem` / `VelocitySystem` /
`GroundCollisionSystem` / `CharacterSystem` / `CharacterStepSystem` /
`CharacterBlockSystem` / `AabbCollisionSystem` / `AnimationSystem` /
`UiLayoutSystem` / `UiInteractionSystem` / `UiFocusSystem` / `GravitySystem` /
`HierarchySystem` /
`AnimationClip` 平移 / 旋转 slerp / 缩放关键帧采样（边界钳制 / 循环取模）/ RON 剪辑加载 /
`AnimationPlaybackSystem` 把采样结果叠加 `Animator.base_*` 写回 `Transform` /
`SkeletonPoseSystem` 合成关节世界矩阵到 `animation.pose` / GLB `JOINTS_0` + `WEIGHTS_0`
+ 逆绑定矩阵解析 / ≤256 关节 GPU 蒙皮 `pose × inverse_bind`（超限 / 不完整 palette 确定性回退 CPU）/
`AnimationLibrary::load_dir` 扫描目录 /
`AnimationLibrary` 按名索引 /
逐帧 state hash / 回放校验）、`xgc-agent`（JSON-RPC：describe（含 components / sim systems / render 能力清单）/
engine.diagnostics（诊断码目录）/ engine.history（事务日志）/ engine.replay（重放）/ world.snapshot / world.restore / query / get / digest / stats /
validate / load / save /
plan / apply / revert / undo；查询支持字段投影 +
标量过滤 + 稳定分页）、`xgc-migrate`（旧 `SceneV2` RON → `WorldData`：身份 / 名称 / 父子 / Transform +
Mesh / Material / Camera / Lua / Audio / Physics 映射）、`xgc-asset`（扫描 `.meta`，
`guid://` → 路径的只读资产目录）、`xgc-render`（资源描述 / pass / resource 渲染图，确定性拓扑排序 / 环检测 /
单写者 / 未声明与读写用途校验 / `PassExecutor` 执行骨架；wgpu 设备、真实纹理 /
缓冲创建、clear pass、可复用 `TrianglePipeline` + `RenderCache` 管线缓存 + uniform
材质色、`Mesh` 顶点 / 索引缓冲、builtin 网格、最小 GLB 几何 / 蒙皮 / 首图元
`baseColorFactor` 解析、`.mat` 材质资产解析与乘子（`模型因子 → 资产 → 覆盖 → 选择高亮`，
静态 / GPU 蒙皮同路）、场景光与环境光（`WorldData.environment`，`环境光 + 主方向光 × N·L`）、
`Rgba16Float` 线性场景中间目标（几何 / 天空 / 粒子先合成，帧末全屏 pass 里先叠可选的三级光晕再做唯一一次 ACES，UI 后叠加）、三级 HDR 光晕链（`render.bloom` 的 `intensity` / `threshold` / `knee`，旧端 `postfx.rs` 同口径：亮部提取 + ½ / ¼ / ⅛ 逐级降采样 + 每级横纵 9 抽头二项核共 9 个 pass，`Rgba16Float` 乒乓，默认 `intensity = 0` 整条链不跑且画面逐位不变）、天空盒 pass（解析天空渐变 + 太阳盘，远平面 + `LessEqual`
深度测试，几何照常遮挡）、天空漫反射 IBL（解析天空 → L2 球谐 E/π，参数取项目级 `project.ron`
的 `render.sky`）、方向光阴影（4 级 CSM：λ=0.7 practical split + 逐级紧致正交拟合 + 纹素吸附，2048² 深度数组按视线深度选层 + 3×3 PCF；静态网格与**蒙皮网格**都投射：蒙皮那支在深度 pass 里用同一份 `skin_vertex.wgsl` 变形，palette 与主场景同一份 `palette_bytes`）、环境反射（天空 SH 反射方向 × Lazarov 解析 BRDF，逐材质 `f0 = mix(0.04, 基色, metallic)`、
`(1-metallic)` 漫反射、粗糙度夹 0.04）、网格基色贴图（glTF `TEXCOORD_0` + `baseColorTexture`，
外部 uri / GLB 内嵌 PNG → sRGB 采样）、直接光 Cook-Torrance PBR（GGX + Smith + Schlick、
`(1-F)` 漫反射扣减与 π 校准，与旧端逐式一致）、GPU 蒙皮与静态实例共享唯一的基色贴图 / PBR / CSM 阴影接收片元实现、确定性粒子 billboard（CPU SplitMix64 模拟，软圆覆盖，Alpha / Additive，Alpha 后向前排序，深测开 / 深写关，并在线性 HDR 中混合）、截图相机优先级（场景相机 → 顺光自动取景）、`MeshCache` 模型路径 /
`guid://` 解析、`Renderer` 帧入口（DrawList → HDR 几何 / 天空 / 粒子 → 可选三级光晕 → ACES → UI）、
`render.camera` → view-projection + MVP uniform、每个 `GlobalTransform` → 模型矩阵并
乘进 MVP、`depth32float` 深度缓冲 / Less 深度测试、单 render pass 批量 draw、
`GpuMeshCache` 顶点 / 索引缓冲跨帧复用、按模型分组实例化（storage instance 缓冲）、
UI 背景矩形叠加（无深度，focused 元素带强调色外环，真字体文本按元素宽度自动换行 / 对齐
单 draw，PNG / PPM 图片经纹理管线单 draw，`image_mode` 支持 stretch / preserve /
cover / nine_slice，`corner_radius` 圆角由片元 SDF 裁剪，绘制按 `ui.layout.clip` 设置
scissor，正式帧末对背景 / 焦点环 / 文本做 sRGB alpha 合成）、从 `render.mesh` + `render.material` 抽
`DrawList` 并按实体顺序 draw、线性 / sRGB RGBA8 回读）、
`xgc-cli`（`schema` / `check [路径]` /
`bench` / `export` / `import` / `run` / `serve` / `migrate` / `render-graph`）、
`xgc-xtask`（文件预算 / 依赖方向 / schema 确定性门禁）。

实测（i7-10700，release，10 000 个 `(Transform, Velocity)`）：

| 操作 | 时间 |
|---|---|
| spawn 10 000 | 1.89 ms（189 ns/实体） |
| 单组件查询 10 000 | 22.5 µs（2.25 ns/实体） |
| despawn 10 000 | 334 µs |

GPU 蒙皮主场景着色切片用同一 showcase 第 150 帧做了 release 冷启动 A/B：每次启动新 `xgc render-frame` 进程并只渲一帧，切片前四跑 `783.02 / 719.85 / 726.80 / 734.09 ms`（中位数 **730.44 ms**），切片后 `822.76 / 790.08 / 934.70 / 766.49 ms`（中位数 **806.42 ms**，+10.4%）。这个数包含新增共享场景 shader 的第二条 pipeline 编译，且**当时**的 GPU 蒙皮仍按实体创建 uniform / bind group / encoder 并单独提交（此后的单次提交切片已把 encoder / submit 收到一帧一次），**不是稳态 GPU 帧耗时**；像素合同与 frame-150 逐字节确定性负责正确性验收，后续必须另设 pipeline 缓存、多角色批量提交与稳态 profiler 门禁。

HDR 统一 tone-map 切片沿用同一命令同机背靠背冷启动：改前 `809.34 / 832.12 / 827.80 / 744.47 ms`（中位数 **818.57 ms**），改后 `743.46 / 719.81 / 739.85 / 756.30 ms`（中位数 **741.66 ms**，-9.4%）。新增 1080p 约 15.8 MiB 的 `Rgba16Float` 场景目标和一次全屏 pass 后未见冷启动回归；但进程启动、shader/pipeline 编译和驱动抖动远大于该 pass 的稳态成本，**不能据此声称 HDR 更快**，仍需 GPU timestamp / 常驻进程 profiler 门禁。

蒙皮投射阴影切片同样同机背靠背：改前 `807.94 / 742.62 / 764.36 / 749.68 ms`（中位数 **757.02 ms**），改后 `789.24 / 729.28 / 733.15 / 735.72 ms`（中位数 **734.43 ms**，-3.0%）。门禁画面（`showcase-particles.json`）里**没有骨架实体**，所以这一刀只多一条 depth-only pipeline 的编译与每帧一次 caster 解析，冷启动未见回归；蒙皮投射的正确性由 4 条 GPU 像素回归负责（旧实现 0 像素变暗 → 现在 176/9216），不代表多角色稳态开销。

HDR 光晕切片默认关闭，因此不改变任何既有画面的性能与像素：证据世界第 150 帧 PPM 与上一版 SHA-256 相同（`4A47275D…48A0927`、自比 0/921600），关闭态只多读一次 uniform、tone-map 绑定组按场景纹理缓存（不随帧重建）。开启时多 6 张 ½ / ¼ / ⅛ 的 `Rgba16Float` 中间目标（每级 a / b 两块乒乓）与 9 个全屏 pass，每帧只重建亮部提取与 tone-map 各一个绑定组（链内 8 个在建链时一次建好），**未做稳态计时**（当前没有常驻 profiler 门禁，冷启动把 shader 编译与进程启动抖到一起，不能归因），代价量级需按 GPU timestamp 另测。

蒙皮 draw 单次提交切片让一帧所有 GPU 蒙皮 draw 共用**一个** frame binding、**一个** pass、**一次** submit（palette 仍逐角色；下一轮竞技场已把逐角色缓冲并进常驻缓冲）。新回归 `batched_skinned_draws_keep_their_own_palettes` 用左右两个角色做差分：右侧换姿态时左侧按列切出的像素必须逐字节不变。**未做稳态计时**：证据场景没有蒙皮 draw，本机当时的冷启动四跑（818–998 ms，有并发构建）不能归因于本刀，提交次数 N→1 属于结构性收益，真正的稳态测量仍要常驻进程 profiler。

蒙皮 uniform 竞技场切片把逐角色的 palette（16 KiB）与 model（96 B）并进常驻缓冲：`crates/xgc-render/src/skinned/arena.rs` 的 `UniformArena` 按设备 `min_uniform_buffer_offset_alignment` 算槽距（16 KiB palette 仍是 16 KiB，96 B model 补到 256 B），layout 声明 `has_dynamic_offset: true` 后每条 draw 只换偏移，容量按 2 的幂扩容（只增不减，封顶在历史峰值槽数）、暂存区跨帧复用。`RenderStats::skin_uniform_buffers` 给出可观测口径（只统主场景，阴影侧见下）：首批 2 张缓冲，同尺寸第二帧 **0**（旧实现每帧每个角色 2 张；回归 `repeated_frames_reuse_the_skin_uniform_arenas`，把 `upload` 开头临时插 `self.slots = 0;` 时实跑变红）。

阴影 pass 那半边同样并进了竞技场：`shadow_skin.rs` 的 `SkinnedShadow` 持有两张 `UniformArena`，`SkinnedBinding` 只记槽号；`RenderStats::shadow_skin_uniform_buffers` 首批 2 张、同尺寸第二帧 **0**（回归 `repeated_frames_reuse_the_shadow_skin_arenas`，把 `upload` 开头临时插 `self.slots = 0;` 时实跑变红）。两条单元回归钉住槽距对齐与跨帧复用，另加一条两投射体用例 `two_skinned_casters_keep_their_own_shadow_palettes`：两个投射体沿屏幕纵轴分开（z=∓2.4）、姿态反向平移，断言两侧阴影行重心相距超过两个窗口宽、且同屏时各自留在原位（把每级 draw 的偏移强制成槽 0 时实跑变红：「窗口内阴影像素太少，无法定位：0」），钉住"每个 caster 用自己那格 palette"。证据画面没有骨架实体、竞技场根本不建缓冲，所以第 150 帧 PPM SHA-256 与上一版相同（`4A47275D…48A0927`）。**未做稳态计时**：冷启动单帧含 pipeline 编译，不能归因于本刀；“每帧缓冲数 2N → 0”是结构性收益，真正的稳态测量仍要常驻进程 profiler。

角色坡限 / 贴地切片让 `physics.character` 的两个死字段生效：`CharacterSystem` 把坡度超过 `slope_limit` 的上升 `physics.slope` 按墙挡住——`Slope::base_x_at` 算出该高度处的坡面落点，把角色推到离自己更近的一侧（以坡底 `base - radius` 与坡背 `origin.x + size.x + radius` 的中点为界，并清掉朝墙的水平速度）；落点夹进坡面自己的 X 跨度，否则坡面架在高台上时会外推出坡面之外几米的"隐形墙"（夹取只限制墙位置，坡架高台时角色即使在坡体下方、只要 X 在跨度内仍会被挡——没有真碰撞体，刻意简化）。y=0 仍是兜底地面（与坡面高度取大值），下坡一律当地面；下行离地不超过 `snap_to_ground` 时贴回，且只在 `vy <= 0` 时吸，起跳不受影响。回归先证会红 —— 还原旧实现后「60° 坡挡住角色」（`x=1`）与「下行吸附」（`y=2.9867516`）实跑变红；本轮修的两处推法缺陷也单独验红（只按坡底推 + 不夹跨度时 `x=-0.3` / `x=-3.186836`，本该 10.3 / -0.3）。另补守卫与新用例：多帧积分真的走上 / 走下 30° 坡、上限取坡面自身倾角时正好可走（把 `<=` 改成 `<` 即红）、陷在坡体高处时去最近一侧（改回按坡面中点选边即红 `x=10.3`）、`snap_to_ground = 0` 不吸附、下坡任意角度当地面、`slope_limit = NaN` 落回缺省。第 150 帧 PPM SHA-256 不变 —— 证据世界里**没有** `physics.character` / `physics.slope` 实体，这条路径对第 150 帧本来就不会被调用。**未闭环**：仍不走 rapier 的 `KinematicCharacterController`（无胶囊扫掠 / 关节 / 墙面滑动）；陡坡这里是挡住而不是像旧端那样沿坡面下滑（旧端还带 `slope_limit + 10°` 的迟滞带）；`GravitySystem` 不在默认调度里、`PhysicsSystem` / `GroundCollisionSystem` 只处理 `physics.body`，所以纯角色没有重力：≤ `snap_to_ground` 的落差能靠贴地跟住（`vy == 0` 也走这支），更大的落差不会下落；`ground` 也只由坡面与 y=0 组成、不含台阶顶面。

**姿态感知的场景包围盒**切片用"两个可执行文件交错跑"的同机 A/B（每轮 before / after 各一次，抵消热漂移）：改前 `778.8 / 748.7 / 720.4 / 756.6 ms`（中位数 **752.62 ms**），改后 `795.9 / 721.4 / 734.3 / 826.0 ms`（中位数 **765.10 ms**，+1.7%）。两组区间 720–779 / 721–826 重叠，且证据场景没有任何 `pose` item —— 新分支对每个 draw 只多一次 `Option` 判空，因此判定未见可归因回归；带骨架的场景每帧会多一次 O(顶点 + 关节) 的边界计算（整网格 AABB 按关节变换，比旧端加载期预计算的每关节顶点盒松，后续上骨架截图门禁时要收紧）。真正的收益是"被姿态移出去的几何仍被取景 / 级联远距离覆盖"（5 条新回归负责）。

256 槽 palette 切片沿用同一命令：改前 `789.24 / 729.28 / 733.15 / 735.72 ms`（中位数 **734.43 ms**），改后 `792.03 / 747.12 / 717.08 / 739.18 ms`（中位数 **743.15 ms**，+1.2%）。改前区间 729–789 ms、改后 717–792 ms，两者重叠，且该场景**没有任何蒙皮 draw**，本刀不可能影响它，因此只能判定"未观察到可归因回归"。真实代价是可分析的常量项：palette uniform 4 KiB → 16 KiB，每个蒙皮 draw 每帧多传 12 KiB（以 24 关节狐狸为例），相对一帧约 15.8 MiB 的 HDR 目标可忽略。downlevel GL 窗口路径的 `max_uniform_buffer_binding_size` 正好是 16 KiB，所以在路由阶段按设备上限判定是否走 GPU（更小的设备退回 CPU），不靠"应该没事"。

命令事务（debug，100 条 `SetField`）：plan + apply **77.8 µs**，revert **69.6 µs**；
apply / revert 各只推进一个 revision，回滚后状态哈希回到原值。同一计划可组合
`Spawn → AddComponent → SetField`、`AddComponent → SetField`、`RemoveComponent → AddComponent`；
计划内维护预测组件集合，`SetField` 能写到刚添加的组件上，删除后的字段写入会被拒绝。
`Command` 可 JSON 往返：`Spawn { data: EntityData }` 反序列化后由组件工厂重建真实组件列；
销毁回滚恢复**原 EntityId 与全部组件**；组件增删同样保留 EntityId，回滚后
archetype 与 state hash 回到原样。`WorldData` 可把整个世界落盘再恢复，
`state_hash` / 实体 ID / revision 一致；`Journal` 可从初始快照重放并校验 revision。
`xgc run 60` 已跑通固定步帧循环：命令 → 系统（`RapierPhysicsSystem` 用与旧引擎同版本的
rapier3d 0.22 驱动 `physics.body` 实体，`PhysicsSystem` 只给非刚体实体的 dynamic 加
重力、`CharacterJumpSystem` 消费起跳请求、`VelocitySystem` 积分、
`GroundCollisionSystem` 地面夹持、`CharacterSystem` 角色脚底贴合地面 / `physics.slope`（坡度超过 `slope_limit` 的上升坡按墙挡住并推到最近一侧、下行离地不超过 `snap_to_ground` 贴回）、
`CharacterStepSystem` 低矮台阶抬升、`CharacterBlockSystem` 高墙 / 球体推出、`AabbCollisionSystem` 两两分离动态刚体
（接触时清掉沿分离轴的接近速度，落地 / 堆叠不再抖动）
（球体按中心线精确分离）、`AnimatorSystem` 推进动画时钟、`UiLayoutSystem` 算 UI 根 /
父相对矩形、裁剪区并继承可见性，`UiInteractionSystem` 读 `ui.input` 写 hover / press /
click，`UiFocusSystem` 消费 nav / confirm 做 Tab 循环与方向键几何选邻，
`Simulation::set_ui_input` 作为宿主输入桥，`hit_test` 取最上层可交互元素、
`AudioMixerSystem` 按监听者距离 + `audio.mixer` 总线写 `audio.gain`、WAV（PCM16 /
Float32）解码 + 目录素材库、`SoftwareMixer` 软件混音 + WAV 写出、`HierarchySystem`
合成世界变换）→ 快照 / trace。`render-frame` 的可选帧数缺省为 0；显式传入时复用
`Simulation::step_with` + `ScriptRuntime::run_frame` 推进固定 60 Hz 正式帧，每步随后推进同一份
`ParticleSystem`，再按截图尺寸重算 `GlobalTransform` 与 UI 布局；编辑器 Play 与图形播放器复用同一粒子系统并把当帧四边形交给 `Renderer`。`VelocitySystem` 按 archetype 分片并行，1 / 2 / 8
线程 state hash 逐位一致。
层级系统会跳过环里的节点，不让一个坏父链阻塞其它实体；节点 / 存在集 / 结果表
跨帧复用，稳态不再取 `WorldData`。`SystemAccess` 已能判定读写冲突，
`Schedule::batches()` 给出确定性并行批次；跨系统并行执行还差世界所有权拆分。
Agent 协议可完成 `describe（含能力清单）→ plan（含 before 预览）→ apply → history →
replay → query / get / digest / stats / validate → save/load → revert / undo` 闭环；查询 IR 支持
`select: ["core.transform:position"]` 字段投影、`filter` 的 `eq/ne/lt/le/gt/ge`
标量比较（向量用 `axis: x/y/z/w` 取分量）与稳定分页。所有 64 位实体 ID 在 JSON
里是十进制字符串，避免 2^53 精度丢失。`xgc check <world.json>` 会按注册表体检
未知组件 / 字段 / 值类型 / 重复项。`xgc migrate <旧.ron> <新.json>` 已能迁移旧
`SceneV2` 的实体身份 / 名称 / 父子 / `Transform`，并补默认 `GlobalTransform`；
参数也可以是一个目录：`xgc migrate <旧场景目录> <输出目录>` 会批量迁移所有
`.ron`，单场景失败只记入报告、不中断其它场景。Mesh / Material / Camera / Lua /
Audio / Physics / Animator / Character / UI / Particle / Rust 插件 / Lod 已映射到对应
组件；空 LOD 组按旧语义跳过，
报告里计数。真实项目目录 4 个场景、35 个实体、76 个旧组件映射成功，0 父缺失。

`cargo test --workspace --offline` 全绿（346 条），`clippy -D warnings` / `fmt --check` 通过；
`xgc-xtask check` 全绿（207 个 Rust 文件，最大 599 行）；30 个内建类型的 schema 为 35709 字节，
两次导出逐字节相同（`Value` 带 `kind` 标签，`quat` / `vec4` 不再歧义）。

**主要缺口**：跨系统并行执行（批次计划已就绪，世界所有权拆分未做）、完整刚体、
PCSS / 点光阴影、TAA / SSAO / FXAA 等其余后处理（主场景与阴影 pass 的 palette / model 都已并进 dynamic offset 竞技场；级联正交盒按相机视锥切片拟合与旧端同式，视锥外投射体本就按同一拟合框处理，不算缺口）、
物理的角色控制器 / 关节 / 连续碰撞 / 墙面滑动、陡坡沿坡面下滑与 `slope_limit + 10°` 迟滞带（`slope_limit` / `snap_to_ground` 已生效）、UI 的旧字体对齐（align/valign 只能按 kind 推断）、
脚本正式 System 调度与粒子 burst 请求桥，
以及旧新逐帧 trace / 性能验收。

## 命令

```sh
cd xgc
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p xgc-xtask -- check
cargo run -p xgc-cli -- schema
cargo run -p xgc-cli -- check
cargo run -p xgc-cli -- bench
cargo run -p xgc-cli -- export world.json
cargo run -p xgc-cli -- import world.json
cargo run -p xgc-cli -- run 60
cargo run -p xgc-cli -- serve
cargo run -p xgc-cli -- migrate <旧.ron|旧目录> <新.json|新目录>
cargo run -p xgc-cli -- render-graph
cargo run -p xgc-cli -- gpu-probe
cargo run -p xgc-cli -- mesh-probe <glb>
cargo run -p xgc-cli -- assets <项目根>
cargo run -p xgc-cli -- clips <剪辑目录>
cargo run -p xgc-cli -- audio <WAV目录>
cargo run -p xgc-cli -- mix <输入.wav> <输出.wav>
cargo run -p xgc-cli -- render-frame <世界.json> <out.ppm> [资产根] [宽] [高] [帧数]
```

本机 cargo 不在 PATH 时，用 `C:\Users\Administrator\.cargo\bin\cargo.exe`；
网络不可用时追加 `--offline`（依赖已在本地缓存）。

## 纪律

- 一个概念一个类型；不写 `V2` / `legacy` / 兼容别名。
- 组件是纯数据；系统只通过查询 / 命令改世界。
- 10k 实体路径先保证正确，再谈零分配与并行；性能数字必须来自实跑。
- 核心 crate 默认 `#![deny(unsafe_code)]`；需要 unsafe 时单独评审。
- 文档与代码冲突时以 [主文档](../Docs/引擎框架设计.md) 的决策为准。
