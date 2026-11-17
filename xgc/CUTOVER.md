# XGC 割接验收（未完成）

> 结论：**XGameEngine → XGame Core 的完整割接尚未达成。**
> 本文件只记录可复现证据与未满足项；能力状态见 [STATUS.md](STATUS.md)。

## 门槛矩阵

| 门槛 | 状态 | 证据 / 缺口 |
|---|---|---|
| 场景 + 资源 + 脚本 + 预制体端到端迁移 | 通过 | showcase 24 实体；character 2 脚本；Pillar 3 实体；`verify-migration` 一致 |
| 迁移世界可运行 | 通过 | `run-world` / `run-cooked` / `xgc-player` 60 帧同哈希 |
| 旧 / 新 headless 统计一致 | 通过 | 24 / 10 / 10 / 2 / 1（`compare-headless`） |
| 视觉可渲染 | 通过（基础） | `render-frame` 第 150 帧 hash `94f44ec6bd500707`，1280×720，14 个场景 draw / 5 call / 7 个 UI draw；截图相机优先级与旧端一致（`mode: "scene"`）；场景光 / 环境光 / `.mat` 资产乘子 / ACES / 天空漫反射 IBL 已贯通 |
| 视觉对等 | **未通过** | 旧 / 新同为第 150 帧、同用场景相机，仍有 594588/921600 像素不同（64.52%），mean_abs **1.81**、max 201；天空 / 地面 / 材质 / 物体落点与翻滚姿态已基本一致，剩余最大差异是 UI 面板（5x7 位图 vs 旧字体） |
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
> `模型基色因子 → .mat 资产 → 实体覆盖`（全是乘子），片元与清屏色都过同一条 ACES 曲线。
> 旧 / 新整帧 mean_abs 依次为 28.65 → 23.79 → 21.24 → 15.97 → 15.27 → 16.34 → 19.65 → 19.27 → 18.78 →
> 1.81（差异像素 64.52%）。倒数第三步是**脚本 `look_at` 朝向修正**：旧实现把俯仰塞进欧拉角 Z 分量，
> 每帧把相机转成「yaw 对、俯仰 0、滚转 45°」，整帧取景与天空方向全偏；修正后天空 / 地面几乎逐像素一致。
> 然后**网格基色贴图**让狐狸从白模变回橙狐；最后把刚体换成与旧引擎**同版本同参数**的 rapier3d 0.22，
> 右侧棕箱区域平均差 18.56 → 4.44（翻滚姿态第一次对上）。
> 中间两步（阴影、动态刚体补零速度）让均值反向，但都是旧引擎本来就有的行为：影子位置取决于各自
> 的仿真位置，而简化的 AABB 求解器没有旋转 / 静-动碰撞，落点与堆叠仍不同。最后一步
> **环境反射**（天空 SH 反射方向 × Lazarov 解析 BRDF）把物体表面抬亮，随后把 **metallic / roughness
> 搬进实例数据**（96B）做逐材质反射：棕色 / 红色表面与旧图的通道差已缩小到个位数，差异像素 98.07%。
> 随后补 **接触清速度**：贴地 / 堆叠的盒子不再每帧带着累积速度抖动（3 条物理回归钉住稳定度）；
> 第 150 帧像素与上一步逐像素相同（物体早已落定），世界哈希因速度归零而变化。
> 第 150 帧世界哈希变为
> `94f44ec6bd500707`：`render.material` 新增 `asset` 字段参与状态哈希，不是仿真变化（`verify-migration` 一致）。
> 仍缺：UI 字体 / 面板（5x7 位图 vs 旧字体，占剩余差异约 19%）、直射光 GGX 高光、CSM / PCSS / 点光阴影、
> GPU 蒙皮快路径的光照 / 贴图、物理的角色控制器 / 关节 / 连续碰撞 / 坡面滑动。旧端本机仍报告项目 Rust 插件构建指纹不匹配并跳过，XGC 也尚未迁移旧 Animator
> 状态机与完整动画素材。
> 当前 `ScriptRuntime::run_frame` 不返回错误，脚本诊断仍只写 stderr；正式调度切片需要把失败传播纳入宿主契约。

## 建议顺序

1. 割接视觉：rapier 物理已通（mean_abs 2.88 → 1.81）；下一步补 UI 字体 / 面板（最大剩余差异），再做直射光 GGX 高光 / CSM / 软阴影与 GPU 蒙皮快路径。
2. 正式调度：脚本与系统统一访问声明。
3. 旧新逐帧 trace + 截图像素门禁。
4. 密码学签名 / 跨机器增量。
5. 推送 + CI。
