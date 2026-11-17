# XGC 割接验收（未完成）

> 结论：**XGameEngine → XGame Core 的完整割接尚未达成。**
> 本文件只记录可复现证据与未满足项；能力状态见 [STATUS.md](STATUS.md)。

## 门槛矩阵

| 门槛 | 状态 | 证据 / 缺口 |
|---|---|---|
| 场景 + 资源 + 脚本 + 预制体端到端迁移 | 通过 | showcase 24 实体；character 2 脚本；Pillar 3 实体；`verify-migration` 一致 |
| 迁移世界可运行 | 通过 | `run-world` / `run-cooked` / `xgc-player` 60 帧同哈希 |
| 旧 / 新 headless 统计一致 | 通过 | 24 / 10 / 10 / 2 / 1（`compare-headless`） |
| 视觉可渲染 | 通过（基础） | `render-frame` 第 150 帧 hash `bb3fdc99df20aa31`，1280×720，14 个场景 draw / 7 call（场景 5 + 两种粒子混合）/ 8 个 UI draw；截图相机优先级与旧端一致（`mode: "scene"`）；场景光 / PBR / 4 级 CSM / RGBA16F 场景 / 线性粒子合成 / 唯一全局 ACES / 曝光后曲线前三级光晕（默认关）/ tone 后 UI 已贯通 |
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

# 粒子：确定性 / 发射率 / 清洗与硬上限（CPU）+ Alpha / Additive billboard（GPU）
cargo test -p xgc-sim particles --offline
cargo test -p xgc-render --test particles --offline

# GPU 蒙皮：静态 PBR/贴图逐像素同值、关节带贴图移动、>256 CPU 回退、256 槽 palette、阴影接收
cargo test -p xgc-render --test skinned_pbr --offline

# HDR：resize / 输出格式 / alpha，粒子线性合成，tone 后 UI
cargo test -p xgc-render --test hdr --test particles --test frame_ui --offline

# 光晕：关闭态逐位等价 / 开态对旧端公式对表 / 暗场景不发光 / resize 重建链（GPU）
cargo test -p xgc-render --test bloom --offline

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
> 蒙皮 draw 单次提交切片把一帧里所有 GPU 蒙皮 draw 收进同一批：一个 frame binding、一个 pass、一次 submit（palette 仍逐角色），此前每条 draw 各建 frame bind / encoder / submit。左右双角色差分回归钉住批处理不串 palette（右侧换姿态时左侧按列像素逐字节不变）；第 150 帧仍逐字节相同，冷启动数因并发构建不可比，未做稳态计时。
> 姿态感知包围盒切片让 `scene_bounds` 改用 `posed_bounds`（会动的顶点：按每根真有 palette 项的关节变换整网格 AABB 后求并，覆盖变形顶点凸包且只需 O(顶点 + 关节) 次变换；不动的顶点：缺蒙皮属性 / 权重全零 / 关节越界时按顶点并进盒子，因为它们在 CPU 与 shader 里都留在绑定姿态），自动取景与级联远距离拟合都跟着当前姿态；集成回归在旧实现下停在绑定盒 `[-0.8..0.8]`、自动取景 **0** 个可见像素，现在盒子跟到 +4、画面里能看到网格。已知近似：整网格 AABB 比旧端加载期按关节预计算的顶点范围盒更松，骨架场景的自动取景会略远。第 150 帧仍逐字节相同（证据世界无骨架），对旧端 32.22% / 0.61 / 194；交错 A/B 中位数 752.62 → 765.10 ms（+1.7%，区间重叠且该场景无 pose item，未观察到可归因回归）。级联正交盒与旧端同为“按相机视锥切片拟合 + 逐级剔除”，视锥外投射体本来就走同一拟合框，不需要另做扩展覆盖。
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
> 仍缺：PCSS / 点光阴影、TAA / SSAO / FXAA 等其余后处理、
> 逐角色 palette 合并（每角色 16 KiB palette 仍各自一份）、物理的角色控制器 / 关节 / 连续碰撞 / 坡面滑动、
> UI 面板底色微差（align/valign 只能按 kind 推断）。旧端本机仍报告项目 Rust 插件构建指纹不匹配并跳过，XGC 也尚未迁移旧 Animator
> 状态机与完整动画素材。
> 当前 `ScriptRuntime::run_frame` 不返回错误，脚本诊断仍只写 stderr；正式调度切片需要把失败传播纳入宿主契约。旧端 `particle.burst` 是 `serde(skip)` 的一次性脚本运行时请求（不是场景迁移字段），XGC 的脚本请求桥尚未接入。

## 建议顺序

1. 割接视觉：RGBA16F 场景、线性粒子合成、唯一全局 ACES、曝光后曲线前三级光晕、tone 后 UI、蒙皮网格投射 CSM 阴影、256 槽 GPU palette、姿态感知包围盒、蒙皮 draw 单次提交已贯通；下一步做逐角色 palette 合并，再回物理角色控制器 / 关节。
2. 正式调度：脚本与系统统一访问声明。
3. 旧新逐帧 trace + 截图像素门禁。
4. 密码学签名 / 跨机器增量。
5. 推送 + CI。
