# XGC 割接验收（未完成）

> 结论：**XGameEngine → XGame Core 的完整割接尚未达成。**
> 本文件只记录可复现证据与未满足项；能力状态见 [STATUS.md](STATUS.md)。

## 门槛矩阵

| 门槛 | 状态 | 证据 / 缺口 |
|---|---|---|
| 场景 + 资源 + 脚本 + 预制体端到端迁移 | 通过 | showcase 24 实体；character 2 脚本；Pillar 3 实体；`verify-migration` 一致 |
| 迁移世界可运行 | 通过 | `run-world` / `run-cooked` / `xgc-player` 60 帧同哈希 |
| 旧 / 新 headless 统计一致 | 通过 | 24 / 10 / 10 / 2 / 1（`compare-headless`） |
| 视觉可渲染 | 通过（基础） | `render-frame` 第 150 帧 hash `bb3fdc99df20aa31`，1280×720，14 个场景 draw / 7 call（场景 5 + 两种粒子混合）/ 8 个 UI draw；截图相机优先级与旧端一致（`mode: "scene"`）；场景光 / PBR / 4 级 CSM / RGBA16F 场景 / 线性粒子合成 / 唯一全局 ACES / 曝光后曲线前三级光晕（默认关）/ tone 后可选 FXAA（默认关）/ tone 后 UI 已贯通 |
| 视觉对等 | **未通过** | 旧 / 新同为第 150 帧、同用场景相机，仍有 296950/921600 像素不同（32.22%），mean_abs **0.61**、max 194；HDR + 全局 tone-map 相对上一版 XGC 只改 6.19% 像素、mean_abs 0.04、max 13，余差主要是狐狸动画 / 几何边缘与 UI |
| 逐帧 trace / 状态哈希对照 | **未通过** | XGC 重放自身确定；无旧引擎逐帧 trace |
| 正式 System 调度 / 并行 | **未通过** | `batches()` 计划已有，执行顺序 |
| 性能基准 + 热路径分配门禁 | **未通过** | 无 release CI 门禁 |
| 推送 / CI | **未通过** | 无 GitHub 凭据，全部 commit 仅本地 |

## 可复现命令

```sh
# 迁移 + 数据对照
cargo run -q -p xgc-cli --offline -- migrate ..\projects\XGame\Scenes\showcase.ron ..\tmp\showcase.json
cargo run -q -p xgc-cli --offline -- verify-migration ..\projects\XGame\Scenes\showcase.ron ..\tmp\showcase.json

# 统计对照（旧 headless JSON 由旧 xgame-editor 生成）
xgame-editor --headless --frames 60 projects/XGame > old.json
cargo run -q -p xgc-cli --offline -- compare-headless old.json ..\tmp\showcase.json

# 视觉指标：先在根 workspace 生成旧引擎第 150 帧，再回 xgc 生成同帧
cd ..
cargo run -q -p xgame-editor --offline -- --headless --frames 150 --screenshot tmp\old-150.png projects\XGame
cd xgc
cargo run -q -p xgc-cli --offline -- render-frame ..\tmp\showcase.json ..\tmp\new-150.ppm ..\projects\XGame 1280 720 150
cargo run -q -p xgc-cli --offline -- image-diff ..\tmp\old-150.png ..\tmp\new-150.ppm

# 材质垂直切片：真实 Box + 合成蒙皮 GLB 都必须消费 baseColorFactor
cargo test -p xgc-render --test material_factor --offline

# 方向光阴影：开关两帧对比（暗像素数量 + 只会变暗）（GPU）
cargo test -p xgc-render --test shadows --offline

# 蒙皮投射阴影：GPU palette / 257 关节 CPU 回退 / 阴影跟随当前姿态 / 与静态网格逐像素等价
cargo test -p xgc-render --test skinned_shadow --offline

# 角色坡限 / 贴地吸附：超过 slope_limit 的坡当墙挡住、下行按 snap_to_ground 贴地（CPU）；
# 台阶在 character，坡限 / 吸附语义在 character_slope（拆分前 character.rs 已贴到 600 行上限）
cargo test -p xgc-sim --test character --test character_slope --offline

# 角色水平 KCC（纯 CPU）：胶囊 collide-and-slide、撞墙停住、斜墙沿墙滑、球体阻挡、刚体优先托管、
# `Replace` 换代后仍能移动、默认调度下不挤进陡坡、确定性
cargo test -p xgc-sim --test character_kcc --offline

# 角色重力（纯 CPU）：空中自由落体落地 / 跳跃弧线落回 / 站地不被拉下 / 走出坡面跨度后下落 / 站在坡面上不被拽下 /
# 默认调度下竖直只积分一次（`VelocitySystem` 对角色整份跳过）
cargo test -p xgc-sim --test character_gravity --offline

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

# 显示空间 FXAA：关闭态逐字节不变 / 硬边缘被柔化且只动边缘 ±2 内 / 纯色清屏不变 / 尺寸与输出格式重建 / sRGB 目标混合在显示空间 / 光晕叠加 12 趟（GPU）
cargo test -p xgc-render --test fxaa --offline

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
> 光晕切片按旧端 `postfx.rs` 合同接上**曝光后、唯一 ACES 前**的三级 Bloom（`render.bloom` 的 `intensity` / `threshold` / `knee`，软膝式按最大通道取亮度，½ / ¼ / ⅛ 逐级降采样后每级横纵各一次 9 抽头二项核，共 9 个 pass，权重 0.5 / 0.3 / 0.2 在 tone-map 同一趟 pass 里叠回线性场景**再**过 ACES —— 过完曲线的高光没有余量）。默认 `intensity = 0` 是关闭态：整条链不跑、`passes` 不增，第 150 帧 PPM SHA-256 与上一版相同（`4A47275D…`、自比 0/921600），对旧端仍 32.22% / 0.61 / 194。七条 GPU 回归 + 三条单元回归（参数容错、链随源尺寸重建并释放、关闭态换尺寸重建占位绑定）钉住关闭态逐字节等价、开态抬亮量与旧端公式对表（差 ≤2）、低于阈值的暗场景即使 `intensity = 4` 也不发光、resize 重建链且同尺寸两次逐字节确定、亮几何的光晕向外扩散；把合成行乘 `intensity` 临时改成乘 `0.0` 时后三条实跑变红，证明回归真的走到合成。**未闭环**：无 `render.exposure` 旋钮（旧端默认 1.0，两者在默认设置下等价）；开启时每帧仍跑 9 个 pass 并重建亮部提取 / tone-map 各一个绑定组（链内 8 个建链时一次建好）；关闭态 tone-map 绑定组按场景纹理缓存、链在关掉时整份释放；没做稳态计时。
> 蒙皮投射切片让**蒙皮网格进阴影批次**（旧端同一个 `vs_shadow` 按 skin slot 分支，XGC 用 `shadow_skin` 的 palette 管线 + 同一份 `skin_vertex.wgsl` 变形公式）：≤256 关节走 GPU palette，>256 / palette 不完整走 CPU 变形后的顶点，四级级联复用每帧一次的绑定，depth bias 与静态投射体共用 constant 2 / slope 2。四条 GPU 回归在旧实现下全红（变暗 0 像素），现在各 176/9216 像素变暗、阴影质心随姿态平移、单位 palette 与静态网格阴影逐像素相同。证据世界 `showcase-particles.json` 不含骨架组件，所以第 150 帧与 HDR 切片逐字节相同（SHA-256 不变、自比 0 像素），对旧端仍 32.22% / 0.61 / 194；冷启动 757.02 → 734.43 ms（-3.0%，只判定未见回归）。**未闭环**：CSM 拟合仍按绑定姿态包围盒，大位移关节可能落到级联外；CPU 回退多算一次变形；palette 在主 / 阴影 pass 各建一次。
> 256 槽 palette 切片把 `MAX_JOINTS` 从 64 抬到 **256**（旧端 `BONES_PER_SLOT` 同容量），palette uniform 4 KiB → 16 KiB（仍在 wgpu 默认 64 KiB 绑定上限内）；65~256 关节的网格不再静默掉到 CPU。新回归把权重压在 90 号槽并平移它：上限临时改回 64 时该网格 `gpu_skin_meshes` 0 ≠ 1 变红，现在留在 GPU 且表面随 90 号槽移动；257 关节仍确定性回退（原 65 关节用例改口径）。证据世界没有骨架，第 150 帧仍逐字节相同（32.22% / 0.61 / 194），冷启动 734.43 → 743.15 ms（+1.2%，与改前区间重叠且该场景无蒙皮 draw，未观察到可归因回归）；另按设备 `max_uniform_buffer_binding_size` 判定是否走 GPU（downlevel GL 恰好 16 KiB）。
> 蒙皮 draw 单次提交切片把一帧里所有 GPU 蒙皮 draw 收进同一批：一个 frame binding、一个 pass、一次 submit（palette 仍逐角色；下一轮竞技场已并进常驻缓冲），此前每条 draw 各建 frame bind / encoder / submit。左右双角色差分回归钉住批处理不串 palette（右侧换姿态时左侧按列像素逐字节不变）；第 150 帧仍逐字节相同，冷启动数因并发构建不可比，未做稳态计时。
> 蒙皮 uniform 竞技场切片把逐角色的 palette（16 KiB）与 model（96 B）并进常驻缓冲：`crates/xgc-render/src/skinned/arena.rs` 的 `UniformArena` 按设备 `min_uniform_buffer_offset_alignment` 算槽距（palette 16 KiB 仍是 16 KiB、model 96 B 补到 256 B），layout 声明 `has_dynamic_offset: true` 后每条 draw 只换偏移，容量按 2 的幂扩容（只增不减）、暂存区跨帧复用；`queue.write_buffer` 与提交同队列有序，所以重写本帧缓冲一定排在上一帧提交之后。`RenderStats::skin_uniform_buffers` 是可观测口径（只统主场景，不含阴影 pass）：首批 2 张缓冲，同尺寸第二帧 **0**（旧实现每帧每角色 2 张，随角色数线性涨）。两条单元回归钉住槽距对齐与跨帧复用，集成回归 `repeated_frames_reuse_the_skin_uniform_arenas` 断言 2 → 0（把 `upload` 开头临时插 `self.slots = 0;` 时实跑变红：“同尺寸第二帧必须复用竞技场、不再建缓冲：2”）；既有左右角色差分用例仍逐字节通过（串 palette 会立刻红）。证据画面没有骨架实体、竞技场不建缓冲，第 150 帧 PPM SHA-256 与上一版相同（`4A47275D…48A0927`，对旧端仍 32.22% / 0.61 / 194）。**未做稳态计时**（冷启动单帧含 pipeline 编译，不能归因），真正的稳态测量仍要常驻 profiler。**未闭环**：阴影 pass 的蒙皮 palette / model 仍逐投射体建缓冲，需要把 `ShadowTarget::render` 从 `&self` 改 `&mut self`。
> 阴影 pass 的蒙皮竞技场切片把逐投射体的 palette（16 KiB）与 model（64 B）也并进常驻缓冲：`crates/xgc-render/src/shadow_skin.rs` 的 `SkinnedShadow` 持有两张 `UniformArena`，`SkinnedBinding` 只记槽号，4 级级联只换 dynamic offset；`ShadowTarget::render` 改成 `&mut self` 并返回本帧新建缓冲数，逐帧准备拆到 `crates/xgc-render/src/renderer/shadow_prep.rs`（行数预算）。`RenderStats::shadow_skin_uniform_buffers` 是口径：首批 2 张、同尺寸第二帧 **0**（回归 `repeated_frames_reuse_the_shadow_skin_arenas`；把 `upload` 开头临时插 `self.slots = 0;` 时实跑变红）。既有 4 条阴影回归仍全绿，另加一条两投射体用例 `two_skinned_casters_keep_their_own_shadow_palettes`：两个投射体沿屏幕纵轴分开（z=∓2.4）、姿态反向平移，断言两侧阴影行重心相距超过两个窗口宽、且同屏时各自留在原位（把每级 draw 的偏移强制成槽 0 时实跑变红：「窗口内阴影像素太少，无法定位：0」），钉住"每个 caster 用自己那格 palette"。第 150 帧 PPM SHA-256 与上一版相同（`4A47275D…48A0927`，证据世界无骨架）。**未做稳态计时**（证据场景没有蒙皮 draw，冷启动单帧含 pipeline 编译）。
> 角色坡限 / 贴地切片让 `physics.character` 里两个原本没人读的字段生效：`CharacterSystem` 把坡度超过 `slope_limit` 的上升 `physics.slope` 按墙挡住——`Slope::base_x_at` 算该高度处的坡面落点，推到离角色更近的一侧（坡底或坡背，清掉朝墙的水平速度），落点夹进坡面 X 跨度以免坡架高台时出现坡面外的隐形墙；y=0 仍是兜底地面、下坡一律当地面，下行离地不超过 `snap_to_ground` 时贴回且只在 `vy <= 0` 时吸（否则 `grounded` 会抖）。旧实现把任何坡面都当地面。回归先证会红：还原旧实现后「60° 坡挡住角色」（停在 `x=1`）与「下行吸附」（`y=2.9867516` 不下落）实跑变红；修推法时另做一次 —— 只按坡底推 + 不夹跨度时 `character_stays_outside_the_high_end_of_a_steep_slope` 报 `x=-0.3`（本该 10.3）、`a_raised_slope_does_not_push_from_outside_its_own_span` 报 `x=-3.186836`（本该 -0.3）。另加多帧积分走上 / 走下坡、上限取坡面自身倾角（`<=` 改 `<` 即红 `y=0`）、陷在坡体高处去最近一侧（改回按坡面中点选边即红 `x=10.3`）、`snap_to_ground = 0`、下坡、`slope_limit = NaN` 等守卫；把 `snap` 临时置 0 时走下坡用例红 `y=4.618803`。第 150 帧 PPM SHA-256 `4A47275D…48A0927` 不变（证据世界没有 `physics.character` / `physics.slope` 实体）。**未闭环**：水平位移已走 rapier 的 `KinematicCharacterController`（见上），但竖直 / 地面 / 台阶仍由 ECS 解析（无 `probe_ground` / `try_step_up` / `snap_to_ground`）；没有关节、没有 CCD；陡坡是挡住而非沿坡面下滑，也没有旧端的 `slope_limit + 10°` 迟滞带；`GravitySystem` 不在默认调度里、纯角色也没有重力：≤ `snap_to_ground` 的落差靠贴地跟住（`vy == 0` 也走这支），更大的落差不会下落（**已由角色重力切片补上**）。
> 角色水平 KCC 切片让 `physics.character` 的水平位移走 rapier：`crates/xgc-sim/src/character_kcc.rs` 是胶囊 / 参数 / 消毒的唯一真相源（`half_height`、`center_height`、`rest_gap = skin_width × 1.1`、`character_capsule`、`build_character_controller`），`RapierWorld` 为角色建运行时托管的 kinematic-position-based 胶囊并按 `EntityId.raw()` 排序（确定性），每帧在 `pipeline.step` 之后跑一次 `move_shape`，只回写 `Transform.position` 的 x / z 与水平 `Velocity`（y 与朝向仍归 ECS）。胶囊比脚底高 `rest_gap` 是刻意的：rapier 0.22 的 `check_and_fix_penetrations` 是空函数、`stop_at_penetration = false`，起始间隙 ≤ `offset` 会放行整段位移（穿墙），而 ECS 恰好把脚底压在地面高度、间隙为 0；抬到 `rest_gap` 才落在安全区。竖直继续走 ECS 也来自上游事实：KCC 在胶囊上 `grounded` / `snap_to_ground` / `autostep` / `is_sliding_down_slope` 全不可用（旧端 `crates/xgame-physics/src/character.rs` 同记录），旧引擎本来就只用 `move_shape` 做滑动。参数逐项对齐旧端（`offset = Absolute(skin_width)`、`autostep: None`、`max_slope_climb_angle = clamp(slope_limit, 0, 89)°`、`min_slope_slide_angle = (slope_limit + 10).min(89)°`、`snap_to_ground: None`、`normal_nudge_factor = 1e-4`）。同片删掉 `CharacterBlockSystem`（AABB 最近轴推出：斜墙顶住、高速穿薄墙），默认调度 19 → 18；`VelocitySystem` 对角色整份跳过（水平归 KCC、竖直归 `CharacterSystem`），`physics.body` 与 `physics.character` 同时在的实体由刚体托管（一个实体只给一套：dynamic 由刚体回写位置，kinematic 仍要脚本每帧推位姿）。回归先证会红：`VelocitySystem` 改回三轴积分 → `velocity_system_only_integrates_the_character_vertically` / `a_character_with_a_rigid_body_is_owned_by_the_body` / `character_stops_at_a_wall_instead_of_passing_through` 三条变红；`move_horizontal` 返回零向量 → 撞墙与斜墙滑动两条变红（说明沿墙滑真是 KCC 给的切向位移）。斜墙用例用 20°：从 +X 撞 45° 盒角时看到的是棱角不是面，实测 z 只挪 -0.006。证据世界没有 `physics.character` 实体、角色阶段空转，第 150 帧 PPM SHA-256 与上一版相同（`4A47275D…48A0927`、14/7/8 与 hash 不变）。审查揪出的两处已在本片修掉，各自有先红证据：①角色托管的签名除半径 / 半高外还带 `EntityKey` 的 index + generation 与 `skin_width` / `slope_limit` 的位模式 —— `Replace` / 组件增删 / 快照恢复只换代不换 ID，只看 ID 时签名不变就提前返回，角色 `key` 停在过期代际、水平位移静默冻结且旧胶囊留作幽灵碰撞体（`a_character_keeps_moving_after_its_entity_key_changes`，签名退回只看 ID 时报 `x=0.50000006`）；②默认调度把 `CharacterSystem`（陡坡推出 / 贴地吸附）挪到 `RapierPhysicsSystem` 之后 —— 坡面没有 rapier 碰撞体，修正跑在 KCC 之前就会让角色每帧先挤进 `v·dt` 再被拉回，脚本每帧重设速度时是 30 Hz 来回跳（`a_character_does_not_creep_into_a_steep_slope_under_the_default_schedule`，顺序挪回去时报 `x=-0.25000077`），陡坡判据同时从角色中心点改成按前进方向的胶囊前沿 / 后沿取高度。为守住 600 行软上限，角色托管拆到 `crates/xgc-sim/src/rapier_world/characters.rs`（`RapierWorld` 的子模块，直接读写同一套刚体表）。复审补修两处：③台阶抬升改到 `CharacterSystem` **之后**（原顺序下抬起的 y 被 `snap_to_ground` 每帧吸回地面，0.05~0.2 m 的低台阶默认调度下上不去，`character_steps_onto_a_low_box_under_the_default_schedule` 挪回顺序时报 `y=0`）；④刚体表建体签名从"只比 `physics.body` 数量"改成按 ID 排序的 `(EntityId, EntityKey)` —— `Replace` / 快照恢复只换代不换 ID，只看数量时 `BodyLink.key` 过期、动态体回写静默冻结（`a_dynamic_body_keeps_moving_after_its_entity_key_changes`，退回数量判据时报 `y=5.8603435` 不动）。⑤台阶判据要按 KCC 的停位间隙外扩 —— KCC 会停在距立面 `offset = skin_width` 处（penetration 修复是空实现），胶囊因此永远碰不到台阶面，撞住那一帧速度已被回写成 0、预测位移也归零，小半径（0.05 m）或低速度（1 m/s）下角色卡在 `offset` 外面一次都抬不起来（`a_small_capsule_still_steps_onto_a_low_box`，不外扩时报 `x=-1.0601`、y=0）；现在判据按 `rest_gap + 1 mm` 外扩，代价是贴着低矮实体也会被抬上去，这正是「压住台阶就上」的语义。同一轮还收了角色表的每帧分配（签名探针与收集表跨帧复用，与刚体表探针同口径），以及「没有 `Transform` 的角色不进签名」（否则之后补上 `Transform` 时签名不变、永远不建胶囊）。第四轮复审又补一处：`physics.body` 与 `physics.character` 同时挂的实体只归刚体表 / 脚本负责——`character_kcc::owned_by_kcc` 是唯一判据（有 `CharacterController`、没有 `PhysicsBody`、有 `Transform` 才算 KCC 托管），`CharacterSystem` 与 `CharacterStepSystem` 都先按 `PhysicsBody` 跳过。不跳过的话，新顺序里 rapier 先回写同实体的 `Transform`、这两个 ECS 系统再写一遍，下一帧又被刚体覆盖，站在台阶边 / 坡边时脚下就是逐帧抖动；`a_character_with_a_rigid_body_is_left_alone_by_the_character_systems` 钉住（去掉跳过时实跑报「带刚体的角色不该被台阶系统抬起来：y=0.19999999」与「带刚体的角色不该被坡面系统推走：x=9.7」）。`VelocitySystem` 对凡是挂了 `CharacterController` 的实体一个分量都不积分：给这种实体积分只会和刚体回写打架，位置完全由刚体表 / 脚本负责。**未闭环**：竖直 / 地面 / 台阶仍在 ECS（`probe_ground` / 抬离地面 / `try_step_up` / `snap_to_ground` 未移植）；`physics.slope` 仍是纯 ECS 高度函数、没有碰撞体，所以陡坡仍挡住而非沿坡面下滑、迟滞带不生效；没有关节、没有 CCD；未做稳态计时（证据场景没有角色，冷启动不能归因）。

> FXAA 切片按旧端 `postfx.rs` 合同在 tone-map 之后、UI 之前加一趟可选抗锯齿（`render.fxaa` 的 `enabled`，默认关）：`crates/xgc-render/src/fxaa.rs` 按输出同格式缓存 LDR 暂存纹理，`fxaa.wgsl` 逐行照旧端 `FXAA_WGSL`（FXAA 3.11 质量路径 5 抽头、亚像素 0.75、方向收缩 1/8 下界 1/128、跨度上限 8 texel、边缘阈值 1/12 与 1/6、Rec.601 luma、`textureSampleLevel(..., 0.0)`、sRGB 手动 encode / decode），平坦区直接返回中心采样以保持逐字节不变。UI 画在 FXAA 之后（HUD 锐利边不被柔化，旧端顺序相同）。默认关闭：不建暂存、不跑 pass、`passes` 不增，第 150 帧 PPM SHA-256 与上一版相同（`4A47275D…48A0927`、自比 0/921600），对旧端仍 32.22% / 0.61 / 194（`project.ron` 没有 `render.fxaa` 节）。六条 GPU 回归钉住关闭态逐字节等价、硬边缘被柔化且只动边缘 ±2 内、纯色清屏不变、尺寸 / 输出格式变化重建暂存且同尺寸两次确定、换尺寸 / 格式后与新建渲染器逐字节相同、sRGB 目标的混合发生在显示空间（最亮值 ≤208，线性空间会到 215）、光晕与 FXAA 叠加时 12 趟 pass；红的证据两条 —— 合成改成原样回传时「开着 FXAA 必须至少改变一个边缘像素（否则着色器没跑）」，把平坦区早退改成 `raw_m + 0.01` 时「FXAA 只许动边缘附近，却有 3836 个远离边缘的像素被改了」；把 scratch 复用判据改成只看「有没有」时「换尺寸 / 格式后必须与新建渲染器逐字节相同」；把 sRGB 分支改成永假时「sRGB 目标的边缘混合必须发生在显示空间（线性空间会亮到 215）：215」。**未闭环**：没有 TAA 让位（旧端 TAA 开时自动关 FXAA）；开启时每帧多一趟全屏 pass、一个绑定组与多一次 submit（tone-map 与 FXAA 分两次提交），视图每次现建；未做稳态计时（证据场景是关闭态，冷启动含管线编译不能归因）。
> 姿态感知包围盒切片让 `scene_bounds` 改用 `posed_bounds`（会动的顶点：按每根真有 palette 项的关节变换整网格 AABB 后求并，覆盖变形顶点凸包且只需 O(顶点 + 关节) 次变换；不动的顶点：缺蒙皮属性 / 权重全零 / 关节越界时按顶点并进盒子，因为它们在 CPU 与 shader 里都留在绑定姿态），自动取景与级联远距离拟合都跟着当前姿态；集成回归在旧实现下停在绑定盒 `[-0.8..0.8]`、自动取景 **0** 个可见像素，现在盒子跟到 +4、画面里能看到网格。已知近似：整网格 AABB 比旧端加载期按关节预计算的顶点范围盒更松，骨架场景的自动取景会略远。第 150 帧仍逐字节相同（证据世界无骨架），对旧端 32.22% / 0.61 / 194；交错 A/B 中位数 752.62 → 765.10 ms（+1.7%，区间重叠且该场景无 pose item，未观察到可归因回归）。级联正交盒与旧端同为“按相机视锥切片拟合 + 逐级剔除”，视锥外投射体本来就走同一拟合框，不需要另做扩展覆盖。
>
> 角色重力切片给 `CharacterSystem` 加 `pub gravity: [f32; 3]`（缺省 -9.81，只有 `y` 被消费）：腾空时`vy += gravity.y * dt` 并积分 `y`，落地时清掉向下速度；`GravitySystem` 不在默认调度、`PhysicsSystem` /`GroundCollisionSystem` 只处理 `physics.body`，所以纯角色此前根本没有重力（落差大于 `snap_to_ground` 就悬空）。回归 `cargo test -p xgc-sim --test character_gravity`：悬空 3 m 落地、跳跃弧线落回、站在地面不被拉下、走出坡面跨度后掉回 y=0、站在坡面上不被拽下、默认调度下竖直只积分一次；把积分行改成 `+= 0.0` 时后三条里的三条实跑变红（「落地后 y=3」「落地后 y=16.666647」「掉下来后 y=5.773503」）。竖直积分由 `CharacterSystem` 独占：`VelocitySystem` 原先对角色只积分竖直（水平已交给 KCC），两边同时积分会让默认调度一帧写两遍 `y`，自由落体看起来重力翻倍、跳跃高度翻倍；现在它按 `physics.character` 整份跳过角色，`a_character_falls_once_per_frame_under_the_default_schedule` 按解析解 `y = y0 - g·dt²·n(n+1)/2` 钉住单次积分（改回双写时实跑红：「默认调度把竖直积分了两遍：y=2.7275007，单次积分应为 2.850125」）。第 150 帧 PPM 与上一版逐字节相同（证据世界没有 `physics.character` 实体），`run 60` 哈希仍 `f4bc029f5f995b91`。未闭环：没有终端速度、只积分 y、地面仍是解析坡面 + y=0（不是 rapier 射线），台阶顶面不在 `ground` 里。
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
> 仍缺：PCSS / 点光阴影、TAA / SSAO 等其余后处理（主场景与阴影 pass 的 palette / model 都已并进 dynamic offset 竞技场）、
> 物理的角色竖直 / 地面 / 台阶仍由 ECS 解析（水平已走 rapier KCC、纯角色重力已由 `CharacterSystem` 积分）、关节、连续碰撞（CCD）、陡坡沿坡面下滑与 `slope_limit + 10°` 迟滞带、同一帧内多角色互见旧位姿（`move_shape` 读的 `query` 只在 `pipeline.step` 时刷新，角色间阻挡与遍历顺序有关）、
> UI 面板底色微差（align/valign 只能按 kind 推断）。旧端本机仍报告项目 Rust 插件构建指纹不匹配并跳过，XGC 也尚未迁移旧 Animator
> 状态机与完整动画素材。
> 当前 `ScriptRuntime::run_frame` 不返回错误，脚本诊断仍只写 stderr；正式调度切片需要把失败传播纳入宿主契约。旧端 `particle.burst` 是 `serde(skip)` 的一次性脚本运行时请求（不是场景迁移字段），XGC 的脚本请求桥尚未接入。

## 建议顺序

1. 割接视觉：RGBA16F 场景、线性粒子合成、唯一全局 ACES、曝光后曲线前三级光晕、tone 后 UI、蒙皮网格投射 CSM 阴影、256 槽 GPU palette、姿态感知包围盒、蒙皮 draw 单次提交、蒙皮 palette / model 竞技场已贯通；阴影 pass 的蒙皮 palette / model 也已并进同一套竞技场，角色坡面 `slope_limit` 与贴地 `snap_to_ground` 也已生效；角色已有运行时托管的运动学胶囊、水平位移走 rapier `KinematicCharacterController` 的 collide-and-slide、tone-map 后可选的显示空间 FXAA（默认关）也已贯通；下一步把竖直 / 地面 / 台阶也搬进 KCC，再给 `physics.slope` 建真碰撞体（关节 / 连续碰撞 / 迟滞带随后）。
2. 正式调度：脚本与系统统一访问声明。
3. 旧新逐帧 trace + 截图像素门禁。
4. 密码学签名 / 跨机器增量。
5. 推送 + CI。
