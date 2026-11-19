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
`GroundCollisionSystem` / `AabbCollisionSystem` / `AnimationSystem` /
`UiLayoutSystem` / `UiInteractionSystem` / `UiFocusSystem` / `GravitySystem` /
`HierarchySystem`（默认调度 16 个系统；角色的三轴位移全由 `RapierPhysicsSystem` 的角色阶段推进：
水平 collide-and-slide + 竖直重力 / 地面探测 / 贴地吸附 / 台阶，`CharacterSystem` / `CharacterStepSystem` /
`CharacterBlockSystem` 都已删除） /
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
静态 / GPU 蒙皮同路）、场景光与环境光（`WorldData.environment`，`环境光 + 最多 8 盏方向光 / 点光 × N·L`（方向光无衰减、点光按距离衰减））、
`Rgba16Float` 线性场景中间目标（几何 / 天空 / 粒子先合成，帧末全屏 pass 里先把线性场景乘上曝光（`render.exposure`，缺省 1.0）、叠可选的三级光晕再做唯一一次 ACES，之后可选一趟显示空间 FXAA，UI 最后叠加）、画面曝光（`render.exposure` 的 `exposure`：ACES 之前的线性光乘子，缺省 1.0，非有限 / 非正落回缺省、夹进 0.05..20.0；光晕阈值也按曝光后判定）、三级 HDR 光晕链（`render.bloom` 的 `intensity` / `threshold` / `knee`，旧端 `postfx.rs` 同口径：亮部提取 + ½ / ¼ / ⅛ 逐级降采样 + 每级横纵 9 抽头二项核共 9 个 pass，`Rgba16Float` 乒乓，默认 `intensity = 0` 整条链不跑且画面逐位不变）、可选 PCSS 软阴影与级间混合（`render.shadow` 的 `softness` / `cascade_blend`，默认全 0 = 关；关着走 3×3 PCF + 硬级间切换）、多光源照明（方向光 / 点光最多 8 盏：方向光无衰减、点光按 `fade = 1 - smoothstep(0.8, 1.0, dist/range)` 与 `1/(1 + 0.09d + 0.032d²)` 衰减、范围外整盏跳过；帧 uniform 里就是 8 槽光源表，`intensity <= 0` 与第 9 盏起都不进表，只有第一盏投射阴影的方向光带 CSM 标记）、点光立方体阴影（第一盏 `casts_shadow` 且 `range > 0` 的点光开六面 1024² 深度图，接收端参考深度按主轴分量算、3×3 `textureSampleCompareLevel`；`point_shadow_draw_calls` 是「六面 × 投射体」的诊断口径）、可选 TAA 时间抗锯齿（`render.taa` 的 `enabled` / `history_weight`，默认关；抖动只叠相机、历史按深度反投影重投影并邻域钳制，开着时 FXAA 让位）、可选 SSAO 环境光遮蔽（`render.ssao` 的 `enabled` / `strength` / `radius` / `bias`，默认关；半分辨率 16 抽样 + [1,4,6,4,1]/16 可分离模糊 + 线性上采样乘算，排在 TAA 之后、光晕之前）、显示空间 FXAA（`render.fxaa` 的 `enabled`，默认关；tone-map 之后、UI 之前一趟，平坦区逐字节不变，硬边缘按 FXAA 3.11 质量路径柔化）、天空盒 pass（解析天空渐变 + 太阳盘，远平面 + `LessEqual`
深度测试，几何照常遮挡）、天空漫反射 IBL（解析天空 → L2 球谐 E/π，参数取项目级 `project.ron`
的 `render.sky`）、方向光阴影（4 级 CSM：λ=0.7 practical split + 逐级紧致正交拟合 + 纹素吸附，2048² 深度数组按视线深度选层 + 3×3 PCF；静态网格与**蒙皮网格**都投射：蒙皮那支在深度 pass 里用同一份 `skin_vertex.wgsl` 变形，palette 与主场景同一份 `palette_bytes`）、环境反射（天空 SH 反射方向 × Lazarov 解析 BRDF，逐材质 `f0 = mix(0.04, 基色, metallic)`、
`(1-metallic)` 漫反射、粗糙度夹 0.04）、网格基色贴图（glTF `TEXCOORD_0` + `baseColorTexture`，
外部 uri / GLB 内嵌 PNG → sRGB 采样）、直接光 Cook-Torrance PBR（GGX + Smith + Schlick、
`(1-F)` 漫反射扣减与 π 校准，与旧端逐式一致）、GPU 蒙皮与静态实例共享唯一的基色贴图 / PBR / CSM 阴影接收片元实现、确定性粒子 billboard（CPU SplitMix64 模拟，软圆覆盖，Alpha / Additive，Alpha 后向前排序，深测开 / 深写关，并在线性 HDR 中混合）、截图相机优先级（场景相机 → 顺光自动取景）、`MeshCache` 模型路径 /
`guid://` 解析、`Renderer` 帧入口（DrawList → HDR 几何 / 天空 / 粒子 → 可选 TAA 解析 → 可选 SSAO → 可选三级光晕 → ACES → 可选 FXAA → UI）、
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

PCSS 软阴影与级间混合切片把阴影接收端对齐到旧端同一份合同：`render.shadow` 的 `softness > 0` 时走 PCSS（8 抽头固定泊松盘求遮挡物平均深度，`penumbra = clamp((depth - blocker) * softness * 12, texel, 24 * texel)`，再 12 抽头 `textureSampleCompareLevel`），`cascade_blend > 0` 时在切分带里把下一级按 `t` 混进来；同一个旋钮还驱动 CPU 侧 `cascade_shadow_matrices` 的 `overlap`（把非 0 级拟合框近端多铺 `overlap × 本级跨度`，否则混合带里的片元会落在下一级图外，被「图外不混」守卫退回硬切换）。采样盘是固定常量表（截图逐字节确定），深度数组的 `textureLoad` 用 4 参且下标夹进图内。帧 uniform 592 → 608 字节（多一个 `shadow_soft` vec4）。默认全 0 = 关：走原 3×3 PCF + 硬级间切换，第 150 帧 PPM 与 `run 60` 哈希都不变。回归 4 条 GPU + 3 条消毒单测 + 2 条级联重叠 + 1 条经 `load_render` 入口的阴影解析：缺省逐字节等价且 `passes` 不增、非法值落回硬阴影、`softness = 6` 的中间灰阶从 24 个像素涨到 20086 个、`cascade_blend = 0.05` 经 CPU 重叠拟合改动阴影采样相位、改动面远小于整帧、多铺后每级 `view_range` 近端严格更靠近相机、负数 / NaN / 无穷的重叠量与 0 逐位一致。红证据：着色器 `frame.shadow_soft.x > 0.0` 改成 `> 999.0`（「软阴影必须真的改变画面」）、`set_shadow_settings` 的 `cascade_blend` 恒 0（「级间混合必须接进 CPU 重叠拟合，否则分带仍是硬切换」）、重叠量改成 0（「第 1 级的近端必须更靠近相机」）。**未闭环**：本机这组机位下混合分支改不动像素（两级影子在带内几乎同值），GPU 回归钉的是旋钮接线与重叠拟合，混合比例靠与旧端逐行同式的移植；没做稳态计时。

点光立方体阴影切片补上多光源的最后一环：`crates/xgc-render/src/point_shadow.rs` 按旧端 `pick_point_shadow_slot` 同判据挑**第一盏** `casts_shadow` 且 `range > 0` 的点光（只在前 8 槽里找），`far = range.max(0.05)`、`near = (far * 0.005).max(0.02)`，六面投影 = `flip_u · perspective(90°) · look_at`（up 照 WebGPU 立方体贴图映射表挑、u 轴镜像），参考深度按主轴分量 `max(|dx|,|dy|,|dz|)` 算 `(far/(far-near))·(1 - near/axis)`（用欧氏距离会让斜面整片变黑）。`crates/xgc-render/src/shadow.rs` 复用方向光那支深度管线与实例缓冲：每帧六面各开一次 depth pass（清 1.0），蒙皮投射体也进这六面，`RenderStats::point_shadow_draw_calls` = 六面 × 投射体数；没有点光阴影时绑 1×1 立方体假图、着色器 `params.z = 0` 整段跳过。接收端 `point_shadow_factor` 逐行对齐旧端（范围外 / 贴脸返回 1.0、偏置随 N·L 放大、3×3 `textureSampleCompareLevel`）。帧 uniform 976 → **1008 字节**（+32 B：灯位置（w = 远平面）+ 参数；六面矩阵不进主帧 uniform，只喂阴影 pass 每面 64 B 的帧缓冲），`FrameLighting` / 绑定布局同时拆进 `crates/xgc-render/src/frame_uniform.rs`、`renderer.rs` 的目标准备拆进 `renderer/shadow_prep.rs::ensure_frame_targets`（守 600 行软上限）。证据世界没有点光：第 150 帧 PPM 与 `run 60` 哈希都不变（`4A47275D…`、14/7/8、hash `bb3fdc99df20aa31`、`f4bc029f5f995b91`）。回归 5 条 GPU + 7 条矩阵单测；红证据：去掉点光的 `shadow_params[2]` 标记（「点光必须在地面上投出可见阴影：0 个像素变暗」）、参考深度写成常数 1.0（「点光阴影必须是局部的，不该把整帧压黑：20736 / 20736」）、`pick_light` 去掉 `casts_shadow` 判据（「关掉 casts_shadow 不该跑立方体阴影」与「第 9 盏灯不在 8 槽光源表里，不该为它跑立方体阴影」）。**未闭环**：没有逐面掩码剔除（六面各自画全部投射体，像素正确但多画几遍）、分辨率固定 1024² 不消费灯的 `shadow_resolution`、点光阴影没有软阴影参数、每帧六面各新建一张 64 B 帧 uniform 与绑定组、窗口宿主仍不读 `project.ron`、没做稳态计时。

多光源照明切片把主场景从单方向光改成旧端同款的 8 槽光源表：`crates/xgc-render/src/lights.rs` 按原顺序打包 `is_active()` 的方向光与点光（方向光存传播方向 `w = 0`、点光存世界坐标 `w = 1`，`shadow_params.y` 带点光半径），`intensity <= 0` 与超过 8 盏的一律丢弃，只有第一盏投射阴影的方向光打 CSM 标记（`primary_shadow_light` 与打包表同判据，主阴影光排在第 9 槽之后就不拟合 CSM）。`scene_shading.wgsl` 的片元按盏循环：方向光 `normalize(-dir)` 无衰减；点光 `fade = 1 - smoothstep(0.8, 1.0, dist/range)`、`atten = fade / (1 + 0.09d + 0.032d²)`、范围外或半径非有限整盏 `continue`、`n_dot_l <= 0` 也 `continue`，`radiance = 光色 × 强度 × 衰减 × π` 后进同一套 Cook-Torrance。帧 uniform 608 → **976 字节**，并删掉与 `lights[0]` 重复的 `sun_dir` / `sun_color`（CPU 侧 `sun_*` 只剩天空太阳盘与自动取景用）。证据世界只有一盏方向光，所以第 150 帧 PPM 与 `run 60` 哈希都不变（`4A47275D…`、14/7/8、hash `bb3fdc99df20aa31`、`f4bc029f5f995b91`）。回归 6 条 GPU + 7 条打包单测；红证据是临时把点光衰减行改成 `atten = fade`（「点光必须随距离衰减：近 [239,…] / 远 [240,…]」）、把盏数夹成 1（「逐盏累加必须等于同能量的单盏」与「方向光与点光必须叠加」）、把 `pos_type.w < 0.5` 改成 `< 1.5`（点光被当方向光，三条点光用例全红）、把第 9 盏的丢弃改成绕回槽位 0（「第 9 盏灯必须被丢掉：8 盏 [235,…] / 9 盏 [243,…]」）。**未闭环**：点光当时只照明、还不投影（立方体阴影已由下一刀补上，见上）；没有光照剔除（每片元 8 盏都循环）；窗口宿主仍不读 `project.ron`；没做稳态计时。
曝光切片把画面亮度从常量变成项目级旋钮：`crates/xgc-render/src/exposure.rs` 是唯一的消毒口径（旧端 `RenderConfig` 同式：非有限 / 非正落回 1.0，合法值夹进 0.05..20.0），`project.ron` 的 `render.exposure` 由 `xgc-cli/src/project_render.rs` 读出后灌进 `Renderer::set_exposure`。乘子出现在两处且必须一致：tone-map 在 ACES 之前乘曝光（`tone_map.wgsl` 的 `ToneParams` 从 16 字节扩到 32 字节，带 3 个对齐填充），光晕的亮部提取也乘同一个值（`bloom.wgsl` 的 `fs_bright`；光晕阈值是按曝光后的亮度定义的）—— 只给一处乘，"多亮算亮"就与实际画面分叉。缺省 1.0 时第 150 帧 PPM 与 `run 60` 哈希都不变（证据项目显式写 `exposure: 1.0`，与缺省走同一条乘法，逐位相同）。回归 5 条 GPU + 3 条单元：显式 1.0 与不设曝光逐字节相同、曝光 2.0 的中心像素对上 CPU ACES 镜像（差 ≤1）、0 / 负 / NaN 落回缺省、清屏 0.4 在曝光 1.0 时不过阈值不发光而曝光 3.0 时抬亮、改曝光后光晕链按新曝光重建并与新建渲染器逐字节相同；红证据是临时把 tone-map 的 `* tone.exposure` 改成 `* 1.0`（「曝光加倍必须变亮：62 -> 62」）、把亮部提取的 `* params.exposure` 改成 `* 1.0`（「曝光把亮度抬过阈值后必须有光晕：205 -> 205」）、把 `chain.exposure` 从链缓存键里删掉（「改曝光后光晕链必须按新曝光重建」）。**未闭环**：光照 / 粒子仍在曝光前的线性域里算（与旧端同序）；没做稳态计时。

刚体表增量修补切片把"增删实体就整表重建"也收掉了：`ensure_built` 第一次建表仍按 ID 排序整份插入（岛屿构建顺序必须确定），之后按 `BodySignature` 的 ID 序列做差量 —— 只重建"新增 / 换代 / 构建参数变了"的条目、删掉消失的条目，没变化的刚体保留原 handle（整表重建会把 handle 全重排、在飞速度与接触缓存全丢）。真的动过表才 `query.update`，计数分成 `rebuild_count()`（整表重建，稳态 1）与 `body_patch_count()`（增量修补，稳态 0）。刚体表这一块拆到 `crates/xgc-sim/src/rapier_world/bodies.rs` 守住 600 行软上限。回归三条：中途新增远处刚体只修补一次、已下落刚体继续下落、新刚体也被推进；`despawn` 同样只修补一次且静态地面不被误删；同一世界跑两遍、一遍中途加远处刚体，未受影响刚体的位置 / 朝向 / 线速度都逐位相同。红证据：增量分支退回整份 `rebuild` 后会被 `rebuild` 的空表断言拦下（临时去掉它再跑仍是 5 条红）（「新增实体不该整表重建（否则所有刚体重排、在飞速度清零）」「中途新增远处刚体不该改变已有刚体的轨迹（增量修补不许整表重排）」等）。第 150 帧 PPM 与 `run 60` 哈希都不变（证据世界刚体集合从头到尾没变）。**未闭环**：修补计数只按次数累计、不区分一次修补里动了多少条目；新增刚体复用空闲 handle 使"全量建表"与"逐个补入"的编号不同（接触链上还缺大规模场景实测）。
刚体建体签名切片让运行期改 `physics.body` 字段不再被静默忽略：`rapier_world.rs` 的建体签名从「按 ID 排序的 `(EntityId, EntityKey)`」扩成 `BodySignature`（`kind` / `shape` 的 FNV-1a + `half_extents` / `radius` / `half_height` / `friction` / `restitution` / `mass` / `gravity_scale` / `is_sensor` 的位模式）；位置 / 旋转 / 缩放都不进签名 —— 前两者动态体每帧都在动，缩放沿用旧端「建体时读一次」的取舍、且脚本常拿缩放做脉冲动画（算进签名会变成动画期间每帧重建并清速度：第一版实测就把 frame-150 世界哈希与 PPM 改掉了，去掉后恢复并补了 `animated_scale_does_not_rebuild_the_body_table`）。`RapierWorld::rebuild_count()` 给出整表重建的累计次数（稳态恒为 1）。回归 `rapier_schedule` 五条：把动态立方体改成 `static` 后必须停止下落、动态体连跑 20 帧不许重建但改 `friction` 恰好重建一次、缩放动画 19 帧内不许重建；表驱动逐个改 10 个构建参数、每个都恰好重建一次；`kind` 拼成 `"Dynamic"` 时按统一判据仍当动态体回写（判据已提到 `xgc-world::physics_components` 的 `BodyKind` / `PhysicsBody::kind()`，`rapier_world`、`GravitySystem`、`GroundCollisionSystem`、`AabbCollisionSystem` 共用；此前建体把未知值当 Dynamic、回写按精确字符串匹配，拼错会静默冻在半空）；把 `BodySignature` 的 16 个槽全填 0（含 `kind` / `shape` 两个哈希槽；只清 12 个数值槽只有摩擦那条会红）时实跑红「改成 static 后必须停止下落（参数改了要重建）：y 4.8313904 -> 4.3919845」与「改摩擦必须重建一次：left: 1, right: 2」。第 150 帧 PPM 与 `run 60` 哈希都不变（证据世界的刚体参数逐帧稳定），**未闭环**：增删当时仍整表重建（已由下一轮增量修补切片补上，见上）、`Transform` 平移 / 旋转不进签名（运行期传送静态刚体不会跟着动，与旧端同取舍）。

陡坡沿坡面下滑切片补上物理侧最后一条“陡坡只当墙”的差距：`character_kcc` 新增 `SLIDE_BAND_DEGREES = 10.0` 与 `slip_limit(def) = (slope_limit + 10).min(89)`（旧端 `min_slope_slide_angle` 同式：`slope_limit` 以内能爬、`slope_limit..+10°` 是迟滞带、再陡才滑），角色阶段因此分三档 —— 坡度超过 `slip_limit` 且起点着地时**不清竖直速度**（清了就永远“站”在坡面上，重力进不来），累加的 `vy` 经坡面投影变成沿坡切向的下滑，终局也不清下滑速度；“陡坡当竖直墙”的分支加了 `!slipping` 门禁（正在下滑时把竖直位移清零等于又站住了）。迟滞带内的坡（45°..55°）爬不上去也不主动下滑，避免临界角上每帧抖；顶着坡面走（`grounded_at_start` 在 y=0 平面上）仍走“当墙”分支，上一轮的 creep 用例不变。回归 `cargo test -p xgc-sim --test character_ground` 新增 2 条：60° 坡上无输入时 30 帧内已经开始下滑、150 帧后脚底回到 y=0 附近；50° 坡 120 帧内 `x` / `y` 变化都 < 2 cm。红的证据：把 `slipping` 恒为 `false`（等价于“着地就清 `vy`”）时「30 帧后必须已经开始沿坡下滑：x 1 -> 1」；把 `slip_cos` 换成 `limit_cos`（忽略迟滞带）时「迟滞带内的坡不该下滑：x 2 -> -7.541752」。第 150 帧 PPM SHA-256 不变（`4A47275D…48A0927`、14/7/8、hash 不变，证据世界没有角色与坡面实体），`run 60` 哈希仍 `f4bc029f5f995b91`。**未闭环**：滑到平地后水平速度会保留（角色没有摩擦 / 阻尼，位置由脚本每帧重设速度兜底，与旧端同一取舍）；y=0 仍是解析平面；下滑没有终端速度；没有关节、没有 CCD、同帧角色互见旧位姿；未做稳态计时。

坡面碰撞体切片让 `physics.slope` 在 rapier 里也有一份几何（此前它只是 ECS 的解析高度函数）：`crates/xgc-sim/src/rapier_world/slopes.rs` 按 origin / size / rise 建**静态楔形薄壳**——cuboid 半长 `hypot(size.x, rise) / 2`、厚 `SLOPE_THICKNESS = 0.1`、半深 `size.y / 2`，刚体绕 Z 轴转 `atan2(rise, size.x)`，中心沿法线从坡面中点下移半厚，于是**上表面与 `Slope::height_at` 重合**；另挂 `physics.body` 的实体让位（显式刚体优先），参数非有限的坡不建体。建体签名带 `EntityKey` 代际与全部几何位模式，变了才重建，探针 / 收集表跨帧复用。新增只读 `RapierWorld::ground_under(x, z, from_y)` 向下射线探地，既是回归的对表工具，也是下一刀自实现 `probe_ground` 的底子。回归 `cargo test -p xgc-sim --test slope_collider` 六条：60° 陡坡挡住了 KCC 角色（没有碰撞体时角色穿到 x≈5，实跑 `x=4.9999976`）、四个采样点的高度与解析坡面差 < 1e-3、改组件 `rise` 后碰撞体重建（不重建时高度停在旧值）、带 `physics.body` 的坡面不再自动建体、尺寸为 0 与 `rise=NaN` 的坡被跳过、下坡方向同样对齐；把 `step` 里的 `ensure_slopes` 注释掉时基础三条全红，三条守卫用例各自也有变异红（去跳过 / 放宽尺寸判据 / 给 `atan2` 的 `rise` 取绝对值）。独立审查还指出 `RapierPhysicsSystem` 的访问声明漏了 `Slope::ID`、`valid()` 没挡 `half_length` 溢出成 `inf`，两处已在本切片内修掉。第 150 帧逐字节不变（`hash=bb3fdc99df20aa31`、14/7/8、PPM SHA-256 `4A47275D…48A0927`）：证据世界没有 `physics.slope` 实体。**未闭环**：角色竖直当时仍由 ECS 解析、陡坡当时仍"挡住"而非沿坡面下滑（已由随后的竖直 / 陡坡下滑两刀补齐，见上）；薄壳法线朝上、坡下不与其它几何互剪；坡面不进 `AabbCollisionSystem`，动态刚体与坡面的接触只走 rapier。

角色竖直 / 地面 / 台阶切片把最后两块 ECS 竖直逻辑也搬进 rapier：`crates/xgc-sim/src/character.rs`（`CharacterStepSystem` 的 AABB 台阶抬升）与 `systems.rs` 的 `CharacterSystem`（贴地 / 重力 / 陡坡推出）同片删除，默认调度 18 → 16，角色三轴位移全在 `rapier_world/characters.rs` 的角色阶段完成。`character_kcc::probe_ground` 从胶囊中心向下打 `cast_ray_and_get_normal`，间隙改成法向距离 `gap = (toi - half_height) * n.y - radius`（原来的「竖直落距乘余弦再减中心高」在斜面上差 `core·(n.y-1)`，站坡的角色会被误判陷地、每帧被抬）；y=0 仍是解析兜底地面（与射线命中取 gap 更小者），因为地面还没有碰撞体。竖直按旧端 `character_simple_move` 同口径：腾空 `vy += GRAVITY_Y * dt`、着地清负速度，位移与水平速度一起进 `move_shape`。台阶用旧端 `try_step_up` 的三段扫掠（升 `step_offset + rest` → 前进 `horizontal + radius + 0.02` → 落 `step_height + 0.05`），门禁是「本帧有接触」而不是「水平位移被截短」——小半径胶囊撞立面时 `move_shape` 仍能挤出七八成位移，按比例判会漏台阶；采纳前再探一次落点，`normal.y < cos(slope_limit)` 算爬坡不算上台阶。陡坡改成按竖直墙处理：events 收集接触法线，`normal.y < cos(slope_limit)` 时清零本帧位移的竖直分量、消掉朝墙里的水平分量，并跳过贴地吸附（否则吸附把 KCC 沿坡抬起的 2 cm 抵消，变成每帧水平蚕食 12 mm、胶囊慢慢嵌进坡面）。这条规则**只在起点着地且没有向上意图时**生效（`grounded_at_start && desired.y <= 0`）：`move_shape` 的接触列表里也混着普通竖直墙与天花板（`n.y ≈ 0` 同样小于 `limit_cos`），无条件清竖直位移会把起跳吞掉（原地空跳、松开墙才突然弹起），也会把下落挂在墙上（`vy` 每帧累加、松手时整段放行）。两条回归钉住：`a_character_jumps_while_pressed_against_a_wall` / `a_falling_character_keeps_falling_while_pressed_against_a_wall`；去掉门禁时实跑红「峰值 y=0.24」与「y=2.98365」。回归 `cargo test -p xgc-sim --test character_kcc --test character_ground`：坡面上的**静息脚底高度**比解析坡面高 `(radius + rest_gap)·(sec θ − 1)`（30° 约 5.0 cm、45° 约 13.3 cm、平地 0）：胶囊沿法线保持 `rest_gap` 的间隙时脚底必然抬这么高，不是误差；旧端同样保留这段间隙（它的间隙公式少算半高投影，实际悬空约 14 cm）。回归按这个公式钉（容差 2 mm），上坡过程中断言间隙落在 `[-5 cm, +8 cm]`。贴地吸附（⑤）原先没有回归：现在补 `a_character_walking_down_a_slope_stays_on_the_ground`（30° 下坡、2 m/s、90 帧逐帧断言脚底停在静息高度且 `grounded` 为真）；把吸附分支改成 `if false && …` 时实跑红「第 1 帧下坡时不该离地：x=1.0333333，y=9.472464」。新增 9 条竖直 / 地面（落地 / 跳跃弧线 / 站地不沉 / 站坡不滑 / 走上 30° 坡 / 走下 30° 坡 / 走出坡沿落地 / 落到箱顶 / 确定性）+ 保留 14 条水平 KCC（含顶着墙起跳 / 贴墙下落两条防回归），21 条旧 ECS 用例删除。红的证据：`GRAVITY_Y` 改 0 → 4 条红（「落地后 y=3.0000002」「没站上箱顶：y=2.0000002」「跳跃后没落回地面：y=16.666649」「走出坡面后没掉回地面：y=5.8109937」）；`try_step_up` 直接 `return None` → 2 条红（「角色没走上台阶：x=-1.3000003」「小半径角色没走上台阶：x=-1.0833338」）；陡坡当墙补上之前 creep 用例红「第 43 帧脚底越过了陡坡立面：x=-0.08836889」。第 150 帧逐字节不变（`hash=bb3fdc99df20aa31`、14/7/8、`4A47275D…48A0927`），`run 60` 哈希仍 `f4bc029f5f995b91`。**未闭环**：y=0 仍是解析平面；陡坡的沿坡面下滑与迟滞带已由下一轮补上（见上）；没有关节、没有 CCD、同帧角色互见旧位姿、没有终端速度；未做稳态计时。

角色重力切片给 `CharacterSystem` 加 `pub gravity: [f32; 3]`（缺省 -9.81）：脚底高于地面时按`vy += gravity.y * dt` 累加、`y += vy * dt`，落地时把向下速度清零。此前纯角色没有重力（`GravitySystem` 不在默认调度里，`PhysicsSystem` / `GroundCollisionSystem` 只处理带 `physics.body` 的实体），落差大于 `snap_to_ground` 的角色会悬在半空；现在悬空 3 m 会落到 y=0、跳跃会走完整条抛物线、走出坡面跨度会掉回地面。竖直积分由 `CharacterSystem` 独占：`VelocitySystem` 对挂了 `physics.character` 的实体整份跳过（原先它给角色积分竖直，加上这里就会一帧写两遍 `y`，落下 2× 速度、跳起高度翻倍），`a_character_falls_once_per_frame_under_the_default_schedule` 按半隐式欧拉解析解钉住单次积分（改回双写时实跑红：`y=2.7275007` 对单次积分的 `2.850125`）。只积分 `y`（水平仍归 KCC），落地仍走 ECS 的解析地面 / 坡面。回归 `cargo test -p xgc-sim --test character_gravity` 6 条（含默认调度下竖直只积分一次）；把积分行改成 `+= 0.0` 时实跑三条变红（「落地后 y=3」「落地后 y=16.666647」「掉下来后 y=5.773503」）。第 150 帧逐字节不变（`hash=bb3fdc99df20aa31`、14/7/8、PPM SHA-256 `4A47275D…48A0927`）：证据世界没有 `physics.character` 实体。

可选 SSAO 环境光遮蔽切片在 TAA 之后、光晕之前乘一层遮蔽（`render.ssao` 的 `enabled` / `strength` / `radius` / `bias`，默认关）：`crates/xgc-render/src/ssao.rs` 照旧端 `ssao.rs` 的合同，用固定种子 LCG 生成 16 抽头半球核（纯函数、无随机数 ⇒ 截图逐帧可复现），在**半分辨率** `R8Unorm` 上按深度反投影求世界点、深度梯度取法线、像素哈希旋转采样盘、`smoothstep` 累计遮蔽，再做 [1,4,6,4,1]/16 可分离模糊并线性上采样乘回源色（四趟 pass 一次提交）；`SsaoParams` 448 字节，矩阵退化（含非有限元素，旧端 `determinant().is_finite()` 守卫同式）时强度写 0 ⇒ AO 全 1、画面逐位不变；`view_proj` 用未抖动矩阵。默认关闭（或 `strength = 0`）：不建 AO 纹理、不跑 pass、`passes` 不增，第 150 帧 PPM SHA-256 与上一版相同（`4A47275D…48A0927`），`run 60` 哈希不变；7 条 GPU 回归 + 4 条纯函数单测钉住（含「接触区必须局部变暗」「强度 0 等价关掉」「换尺寸重建 / 关掉释放 / 释放后重开」「448 字节布局」「消毒口径」「非有限矩阵必须判退化」等变异实跑红）。**未闭环**：半分辨率 AO 在薄几何边缘会留一圈淡边（旧端同款）；只有遮蔽没有间接光；窗口宿主仍不读 `project.ron`；未做稳态计时（证据场景是关闭态）。

可选 TAA 时间抗锯齿切片在 tone-map 之前插入一趟时间重投影（`render.taa` 的 `enabled` / `history_weight`，默认关）：`crates/xgc-render/src/taa.rs` 照旧端 `taa.rs` 的合同做 8 相 Halton(2,3) 抖动（纯函数、无随机数 ⇒ 截图逐帧可复现）、像素→裁剪与像素→纹理两套换算（后者 y 取反）、`jitter_projection` 只平移裁剪空间 x/y；`taa.wgsl` 用当前深度反投世界、用上一帧未抖动视图投影重投影到上一帧屏幕并补上一帧抖动，采样历史后按 3×3 邻域 `lo/hi` 钳制再 `mix`（缺省历史权重 0.9 ⇒ 当前帧只进 10%）；天空按视线方向推到 1e4 再重投影（只保留旋转视差）；首帧 / 相机背后 / 越界直通当前帧。历史是两张 `Rgba16Float` 乒乓，尺寸变化重建、关掉即释放；抖动只叠场景矩阵，阴影拟合仍用未抖动矩阵。深度纹理补了 `Sampled` 用途（否则绑定时报 usage 校验错），TAA 开着时 FXAA 自动让位。默认关闭：不建历史、不跑 pass、相机不抖动，第 150 帧 PPM SHA-256 与上一版相同（`4A47275D…48A0927`），`run 60` 哈希不变；8 条 GPU 回归 + 7 条抖动与布局单测 + 1 条接线单测钉住（含「历史权重 0 与 0.9 的第二帧必须不同」「重置历史后首帧直通」「换尺寸必须重建历史」「开着 TAA 时 FXAA 必须让位」「开启 TAA 必须多一趟解析」等变异实跑红）；`reset_taa_history()` 供相机瞬移 / 切场景帧调用。**未闭环**：历史只有颜色、没有深度 / 速度缓冲（旧端同款），快速平移的高光仍可能拖影；解析与 tone-map 两次提交；窗口宿主仍不读 `project.ron`；未做稳态计时（证据场景是关闭态）。

显示空间 FXAA 切片在 tone-map 之后、UI 之前加一趟可选抗锯齿（`render.fxaa` 的 `enabled`，默认关）：`crates/xgc-render/src/fxaa.rs` 按输出**同格式**缓存一张 LDR 暂存纹理，`fxaa.wgsl` 逐行照旧端 `postfx.rs` 的 `FXAA_WGSL` —— FXAA 3.11「质量」路径 5 抽头、亚像素混合 0.75、方向收缩 1/8（下界 1/128）、跨度上限 8 texel、边缘阈值 1/12 与 1/6（沿用旧端取值，比 stock 严一档）、Rec.601 luma、全部 `textureSampleLevel(..., 0.0)`、sRGB 目标手动 encode / decode 往返；平坦区（对比度低于阈值且亚像素项为 0）直接返回中心采样，所以没有边缘的地方逐字节不变。UI 画在 FXAA 之后，HUD 的锐利边不会被当走样柔化（旧端顺序相同）。默认关闭：不建暂存、不跑 pass、`passes` 不增，第 150 帧 PPM SHA-256 与上一版相同（`4A47275D…48A0927`），对旧端仍 32.22% / 0.61 / 194。六条 GPU 回归钉住关闭态逐字节等价、硬边缘被柔化且只动边缘 ±2 内（平坦角点逐字节不变）、纯色清屏不变、尺寸 / 输出格式变化重建暂存且同尺寸两次确定、换尺寸 / 格式后与新建渲染器逐字节相同、sRGB 目标的混合发生在显示空间（最亮值 ≤208，线性空间会到 215）、光晕与 FXAA 叠加时 12 趟 pass；红的证据两条 —— 合成改成原样回传时「开着 FXAA 必须至少改变一个边缘像素（否则着色器没跑）」，把平坦区早退改成 `raw_m + 0.01` 时「FXAA 只许动边缘附近，却有 3836 个远离边缘的像素被改了」；把 scratch 复用判据改成只看「有没有」时「换尺寸 / 格式后必须与新建渲染器逐字节相同」；把 sRGB 分支改成永假时「sRGB 目标的边缘混合必须发生在显示空间（线性空间会亮到 215）：215」。**未闭环**：TAA 让位已由下一刀补上（TAA 开着时 FXAA 强制关）；开启时每帧多一趟全屏 pass、一个绑定组与多一次 submit（tone-map 与 FXAA 分两次提交），视图每次现建；未做稳态计时（证据场景是关闭态）。
角色控制器缺省对齐切片把 `physics.character` 的缺省值与旧端 `xgame_core::CharacterController` 逐项对齐（`1.8 / 0.3 / 0.35 / 45 / 0.02 / 0.3`），并把消毒与几何派生量收到组件上：`CharacterController::sanitized()` / `half_height()` / `center_height()` 是唯一真相源，`character_kcc` 只做委托。割接前两个引擎读同一批场景，"没写这些字段"的角色必须走出同样的轨迹；schema 导出的缺省值也随之变成与旧端一致（30 个类型 35709 → 35710 字节），显式写了字段的场景不受影响。回归 `cargo test -p xgc-world` 6 条：缺省逐项相等、NaN / ±inf 落缺省、越界钳制（按旧端区间字面值）、派生量、清洗幂等，以及 schema 缺省值与 `Default` 逐字段相等（编辑器按 schema 建组件、运行期工厂走 `Default`，只改一处会静默分叉）。区间常量之间的咬合改用编译期 `const _: ()` 断言。迁移端 `OldCharacter` 的六个字段也补了 `#[serde(default = ...)]`：不给缺省时手写旧 RON 在迁移入口直接报 `Unexpected missing field `step_offset``。红的证据：缺省改回 `0.3 / 0.01 / 0.2` 时 `defaults_match_the_old_engine` 报 `left: 0.3, right: 0.35`；`finite_or` 改成直接返回原值时 NaN 漏到 `clamp` 上，两条用例变红。同批复核角色行为回归：缺省 `skin_width` 变 0.02 后撞墙用例不再能断言"水平速度精确为 0"（KCC 的 `normal_nudge_factor` 每帧推离墙面，反向分量留成约 -0.002），改成断言 `|vx| < 0.05`；证据世界没有 `physics.character` 实体，第 150 帧 PPM SHA-256 不变。**未闭环**：`jump_velocity` / `jump` 还没有同样的消毒；依赖旧缺省的既有 XGC 场景需要重新核一遍（本仓证据世界没有角色，`projects/XGame` 的角色由迁移生成、字段显式写出）。

角色水平 KCC 切片把 `physics.character` 的水平位移交给 rapier：`crates/xgc-sim/src/character_kcc.rs` 是胶囊 / KCC 参数的唯一真相源（消毒与缺省在 `crates/xgc-world/src/character_components.rs` 的 `CharacterController::sanitized()`）（`half_height`、`center_height`、`rest_gap = skin_width × 1.1`、`character_capsule`、`build_character_controller`），`RapierWorld` 为角色建运行时托管的 kinematic-position-based 胶囊刚体并按 `EntityId.raw()` 排序（确定性），每帧在 `pipeline.step` 之后跑一次 `move_shape`，只回写 `Transform.position` 的 x / z 与水平 `Velocity`（y 与朝向仍归 ECS）。胶囊比脚底高 `rest_gap` 是刻意的：rapier 0.22 的 `check_and_fix_penetrations` 是空函数，起始间隙 ≤ `offset` 会放行整段位移（穿墙），而 ECS 把脚底压在地面高度、间隙为 0。竖直仍走 ECS 的原因同样来自上游：KCC 在胶囊上 `grounded` / `snap_to_ground` / `autostep` / `is_sliding_down_slope` 全不可用（旧端同记录）。参数逐项对齐旧端（`offset = Absolute(skin_width)`、`autostep: None`、`max_slope_climb_angle = clamp(slope_limit, 0, 89)°`、`min_slope_slide_angle = (slope_limit + 10).min(89)°`、`snap_to_ground: None`、`normal_nudge_factor = 1e-4`）。同片删掉 `CharacterBlockSystem`（AABB 最近轴推出：斜墙顶住、高速穿薄墙），默认调度 19 → 18；`VelocitySystem` 对角色整份跳过（水平归 KCC、竖直归 `CharacterSystem`）。回归先证会红：`VelocitySystem` 改回三轴积分 → 三条变红（只积竖直 / 刚体优先 / 撞墙停住）；`move_horizontal` 返回零向量 → 撞墙与斜墙滑动两条变红。斜墙用例用 20°：45° 时从 +X 撞到的是盒角不是面。证据世界没有 `physics.character` 实体，第 150 帧 PPM SHA-256 不变。审查揪出并修掉两处：①角色托管的签名带上 `EntityKey` 的 index + generation（以及 `skin_width` / `slope_limit` 的位模式）—— `Replace` / 组件增删 / 快照恢复只换代不换 ID，只看 ID 时签名不变就提前返回，角色水平位移静默冻结、旧胶囊还作为幽灵碰撞体留着（用例 `a_character_keeps_moving_after_its_entity_key_changes`，签名退回只看 ID 时实跑变红）；②默认调度把 `CharacterSystem` 挪到 `RapierPhysicsSystem` 之后，它是移动后修正，跑在 KCC 之前时坡面没有 rapier 碰撞体、每帧先挤进 `v·dt` 再被拉回（用例 `a_character_does_not_creep_into_a_steep_slope_under_the_default_schedule`，顺序挪回去时实跑变红）。为守住 600 行软上限，角色托管拆到 `crates/xgc-sim/src/rapier_world/characters.rs`；球体碰撞体也补了 `character_is_blocked_by_a_ball_collider`。复审又揪出两处并已修掉：③台阶抬升原先排在 `CharacterSystem` 之前，抬起的 y 被贴地吸附吸回地面，0.05~0.2 m 的低台阶在默认调度下根本站不上去 —— 现在它排在 `CharacterSystem` 之后、抬起的 y 留到帧末（`character_steps_onto_a_low_box_under_the_default_schedule`，把顺序挪回去时报 `y=0`）；④刚体表 `ensure_built` 原来只按 `physics.body` 数量判是否重建，`Replace` / 组件增删 / 快照恢复换代后 `BodyLink.key` 过期、动态体的 Transform / Velocity 回写静默冻结 —— 签名改成按 ID 排序的 `(EntityId, EntityKey)`（探针缓冲跨帧复用），`a_dynamic_body_keeps_moving_after_its_entity_key_changes` 钉住（退回数量判据时报 `y=5.8603435` 两帧不动）。

第三轮复审又补一处：台阶判据要按 KCC 的停位间隙外扩 —— KCC 会停在距立面 `offset = skin_width` 处（penetration 修复是空实现），胶囊因此永远碰不到台阶面，撞住那一帧速度已被回写成 0、预测位移也归零，小半径（0.05 m）或低速度（1 m/s）下角色卡在 `offset` 外面一次都抬不起来（`a_small_capsule_still_steps_onto_a_low_box`，不外扩时报 `x=-1.0601`、y=0）；现在判据按 `rest_gap + 1 mm` 外扩，代价是贴着低矮实体也会被抬上去，这正是「压住台阶就上」的语义。同一轮还收了角色表的每帧分配（签名探针与收集表跨帧复用，与刚体表探针同口径），以及「没有 `Transform` 的角色不进签名」（否则之后补上 `Transform` 时签名不变、永远不建胶囊）。

角色坡限 / 贴地切片让 `physics.character` 的两个死字段生效：`CharacterSystem` 把坡度超过 `slope_limit` 的上升 `physics.slope` 按墙挡住——`Slope::base_x_at` 算出该高度处的坡面落点，把角色推到离自己更近的一侧（以坡底 `base - radius` 与坡背 `origin.x + size.x + radius` 的中点为界，并清掉朝墙的水平速度）；陡坡判据按胶囊半径取前进方向的前沿 / 后沿高度（用角色中心点的话，胶囊中心还在坡外时前缘已经压在坡上，角色会先挤进一个半径再被推回）。落点夹进坡面自己的 X 跨度，否则坡面架在高台上时会外推出坡面之外几米的"隐形墙"（夹取只限制墙位置，坡架高台时角色即使在坡体下方、只要 X 在跨度内仍会被挡——没有真碰撞体，刻意简化）。y=0 仍是兜底地面（与坡面高度取大值），下坡一律当地面；下行离地不超过 `snap_to_ground` 时贴回，且只在 `vy <= 0` 时吸，起跳不受影响。回归先证会红 —— 还原旧实现后「60° 坡挡住角色」（`x=1`）与「下行吸附」（`y=2.9867516`）实跑变红；本轮修的两处推法缺陷也单独验红（只按坡底推 + 不夹跨度时 `x=-0.3` / `x=-3.186836`，本该 10.3 / -0.3）。另补守卫与新用例：多帧积分真的走上 / 走下 30° 坡、上限取坡面自身倾角时正好可走（把 `<=` 改成 `<` 即红）、陷在坡体高处时去最近一侧（改回按坡面中点选边即红 `x=10.3`）、`snap_to_ground = 0` 不吸附、下坡任意角度当地面、`slope_limit = NaN` 落回缺省。第四轮复审又补一处：`physics.body` 与 `physics.character` 同时挂的实体只归刚体表 / 脚本负责——`character_kcc::owned_by_kcc` 是唯一判据（有 `CharacterController`、没有 `PhysicsBody`、有 `Transform` 才算 KCC 托管），`CharacterSystem` 与 `CharacterStepSystem` 都先按 `PhysicsBody` 跳过。不跳过的话，新顺序里 rapier 先回写同实体的 `Transform`、这两个 ECS 系统再写一遍，下一帧又被刚体覆盖，站在台阶边 / 坡边时脚下就是逐帧抖动；`a_character_with_a_rigid_body_is_left_alone_by_the_character_systems` 钉住（去掉跳过时实跑报「带刚体的角色不该被台阶系统抬起来：y=0.19999999」与「带刚体的角色不该被坡面系统推走：x=9.7」）。`VelocitySystem` 对凡是挂了 `CharacterController` 的实体一个分量都不积分，给刚体组合积分只会和刚体回写打架，位置完全由刚体表 / 脚本负责。第 150 帧 PPM SHA-256 不变 —— 证据世界里**没有** `physics.character` / `physics.slope` 实体，这条路径对第 150 帧本来就不会被调用。**未闭环**：水平位移已走 rapier 的 `KinematicCharacterController`（见上），但竖直 / 地面 / 台阶仍由 ECS 解析；没有关节、没有 CCD；陡坡这里是挡住而不是像旧端那样沿坡面下滑（旧端还带 `slope_limit + 10°` 的迟滞带）；`GravitySystem` 不在默认调度里、`PhysicsSystem` / `GroundCollisionSystem` 只处理 `physics.body`，所以纯角色没有重力：≤ `snap_to_ground` 的落差能靠贴地跟住（`vy == 0` 也走这支），更大的落差不会下落（**已由角色重力切片补上**：`CharacterSystem.gravity` 缺省 -9.81，腾空累加、落地清速）；`ground` 也只由坡面与 y=0 组成、不含台阶顶面。

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
重力、`CharacterJumpSystem` 消费起跳请求、`VelocitySystem` 给实体积分 `position += linear * dt`
（带 `CharacterController` 的角色整份跳过——水平 collide-and-slide 与竖直重力 / 射线探地 / 贴地吸附 /
台阶三段扫掠都在 `RapierPhysicsSystem` 的角色阶段，`CharacterSystem` / `CharacterStepSystem` 已删除）、
`GroundCollisionSystem` 地面夹持、`AabbCollisionSystem` 两两分离动态刚体
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

`cargo test --workspace --offline` 全绿（454 条），`clippy -D warnings` / `fmt --check` 通过；
`xgc-xtask check` 全绿（233 个 Rust 文件，最大 599 行）；30 个内建类型的 schema 为 35710 字节，
两次导出逐字节相同（`Value` 带 `kind` 标签，`quat` / `vec4` 不再歧义）。

**主要缺口**：跨系统并行执行（批次计划已就绪，世界所有权拆分未做）、完整刚体、
后处理之外的画质项（PCSS / 点光立方体阴影 / TAA / SSAO / FXAA 都已可选；主场景与阴影 pass 的 palette / model 都已并进 dynamic offset 竞技场；级联正交盒按相机视锥切片拟合与旧端同式，视锥外投射体本就按同一拟合框处理，不算缺口）、
物理的角色仍缺关节、连续碰撞（CCD）、同一帧内多角色互见旧位姿（`move_shape` 读的 `query` 只在 `pipeline.step` 时刷新，角色间阻挡与遍历顺序有关）、UI 的旧字体对齐（align/valign 只能按 kind 推断）、
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
