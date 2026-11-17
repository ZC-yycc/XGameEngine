# XGC 状态与割接验收

> 这是能力的**现状快照**，不是完成声明。XGC 仍明显低于 XGameEngine 全量能力，
> 当前不能割接。

## 指标（第 140 轮）

| 项 | 值 |
|---|---|
| Rust 文件 | 176（最大 596 行；软上限 600） |
| 测试 | 267（`cargo test --workspace --offline`） |
| 内建组件 schema | 30 类 / 34950 字节，两次导出逐字节相同 |
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
| wgpu 渲染 | 相机 MVP、深度、单 pass、顶点 / 索引缓存、按模型实例化、DrawList、颜色合成顺序 `模型基色因子 → .mat 资产 → 实体覆盖 → 选择高亮`（静态 / GPU 蒙皮同路）、场景光（环境光 + 主方向光，面法线朝向相机）、ACES 色调映射（含清屏色）、天空盒 pass（解析天空渐变 + 太阳盘，远平面 + `LessEqual` 深度测试）、天空漫反射 IBL（解析天空 → L2 球谐 E/π）、方向光阴影（单级正交拟合 + 2048² 深度图 + 3×3 PCF）、截图相机优先级（场景相机 → 顺光自动取景）、线性 / sRGB RGBA8 回读 |
| UI | 布局 / 父相对 / 裁剪 / 命中、输入桥、Tab + 方向键焦点、背景 / 焦点环、5x7 文本换行、PNG/PPM 纹理、stretch/preserve/cover/nine-slice、scissor 裁剪、正式帧末 sRGB alpha 合成 |
| 动画 | RON 剪辑、平移 / 旋转 slerp / 缩放采样、目录加载、写回 Transform、Skeleton / Bone / Pose、GLB JOINTS / WEIGHTS / 逆绑定、CPU 蒙皮、DrawList 自动读取祖先骨架姿态并按实体变形上传 |
| 物理 | 重力 / 速度积分 / 地面 / 跳跃、角色台阶 / 高墙 / 球体阻挡、斜坡地面、AABB 分离 |
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
| 渲染 | 69% | 真实 builtin 几何 + glTF 基色因子 + `.mat` 资产乘子 + 场景光 + 天空盒画面 + 天空漫反射 IBL + 环境反射（SH 反射方向 + Lazarov 解析 BRDF，介电体近似）+ 方向光阴影 + ACES + 实例化 + sRGB HUD 合成 + 截图取景优先级；缺逐材质 metallic/roughness 的真实 split-sum / CSM / PCSS / 点光阴影 / 网格纹理 / 后处理 / 粒子绘制 |
| 物理 | 25% | 自研简化碰撞；缺完整刚体、关节、连续碰撞、坡面滑动 |
| 资产 / 打包 | 70% | 引用裁剪 + 增量烹饪（哈希未变跳过写入）+ manifest 聚合签名；JSON / XGCB / 图形播放器；缺密码学签名 / 跨机器增量 |
| Lua 脚本 | 80% | 旧 `update(ctx)` + 移动 / 旋转 / 查询 / UI / `action_pressed`；`InputMap` 解析 input.ron 动作与轴；缺窗口键盘事件接线与音频脚本 API |
| 编辑器 | 88% | 层级 / Inspector / Play / 视口 / Undo / Gizmo / 吸附 + 预制体实例化 / Revert / 位置覆写 + Play Scripts（脚本 + input.ron 动作） |
| 割接验收 | 55% | 统计对照一致；旧 / 新截图同用场景相机、同一第 150 帧；环境反射接入后 mean_abs 19.65 → **19.27**，差异像素 99.30% → 98.08%；刚体求解器仍缺旋转 / 静-动碰撞（落点与堆叠偏差是主因）；仍缺纹理 / 逐帧 trace、并行调度与推送 |

**整体能力对等约 63%，割接就绪约 55%**。核心运行时、迁移、动画、编辑器和
cooked 播放器已可用；差距集中在渲染高级特性、正式调度、逐帧验收与推送。

## 迁移 / 运行证据

- `projects/XGame/Scenes/showcase.ron` → 24 实体 / 51 组件 / 0 缺失父节点。
- `run-world` 60 帧通过；`render-frame` 第 150 帧 hash `94f44ec6bd500707`（`render.material` 新增 `asset` 字段参与状态哈希，迁移逐字段仍然一致），1280×720 输出 14 个场景 draw / 5 个场景 draw call / 7 个 UI draw。
- 旧 / 新 headless 统计逐项一致：24 实体 / 10 刚体 / 10 碰撞体 / 2 发射器 / 1 监听者。
- **旧端截图相机已确认为 `mode: "scene"`**（重跑旧 `xgame-editor` 读到的报告），两次旧运行图像逐像素相同（0/921600）；XGC 的 `render-frame` 现在按同一优先级取景：有场景相机就用场景相机，只有缺相机时才退到顺光自动取景。
- 场景级光源与环境光已迁移进 `WorldData.environment`（showcase：`Sun` 方向 (0.45,-1,0.35)、强度 1.4、色 (1.0,0.97,0.92)，`ambient` (0.09,0.1,0.13)）；`verify-migration` 逐光源字段对照，缺省环境不落盘。
- `render-frame` 缺省 0 帧；显式帧数复用 `Simulation::step_with` + `ScriptRuntime::run_frame`，最后重算层级与目标尺寸 UI。**环境反射**（天空 SH 在反射方向求值 × Lazarov 解析 BRDF，介电体 f0=0.04、粗糙度 0.5）接入后第 150 帧旧 / 新差异 903883/921600（98.08%）、mean_abs **19.27**、max 215（上一步 19.65）：棕色 / 蓝色表面被天光抬亮，与旧图 PBR 的镜面环境项方向一致。仍是近似——逐材质 metallic / roughness 与预过滤立方体贴图留给完整 PBR 切片；刚体求解器（旋转 / 静-动碰撞 / 堆叠）仍是画面差距主因。为守住 600 行预算把实例化 WGSL 抽到 `instanced.wgsl`（`include_str!`）。
- 天空参数按项目级读取：`project.ron` 的 `render.sky`（`enabled/intensity/zenith/horizon/ground/sun_size/sun_intensity`），缺省 = 天空关（画面与接入前一致）；CPU 侧用 1024 点斐波那契球把解析天空投影成 L2 球谐（E/π 口径），与常量 `ambient` 同标定直接相加。
- 无 `animation.state` 时 `set_anim_param` 现在安全 no-op，不再让同帧其它脚本命令整批失败；状态机迁移仍是独立差距。
- GUID 资产（Box.glb、Fox.gltf + Fox.bin）可解析；glTF 首图元 `baseColorFactor` 在静态实例与 GPU 蒙皮路径均参与最终颜色，且选择高亮仍能替换最终颜色；character 脚本 / Pillar 预制体可迁移。
- cooked 包：引用裁剪 7 文件、XGCB 二进制（v2 带场景环境）、manifest 哈希 / 签名校验、`xgc-player` headless 60 帧。
- 自动化测试：251 条；正式帧回归覆盖 sRGB alpha HUD、帧数契约、固定步推进与最终父子 `GlobalTransform` 合成；基色因子、场景光、`.mat` 乘子与 ACES 均有 GPU 像素门禁（期望值由 CPU 镜像现算），环境往返 / 迁移 / 验收 / 取景拟合均有 CPU 回归。

## 与 XGameEngine 的主要差距

| 差距 | 影响 | 优先级 |
|---|---|---|
| 渲染高级特性 | 基础几何 / glTF 基色因子 / `.mat` 资产乘子 / 场景光 + 环境光 / 天空盒画面 + 漫反射 IBL / 方向光阴影 / ACES / 面着色 / HUD 已通；缺 CSM / PCSS / 点光阴影 / 镜面 IBL / metallic-roughness BRDF / 网格纹理 / 后处理 / 粒子 | P0 |
| 正式 System 调度 | `SystemAccess` / `batches()` 已有，执行仍顺序；脚本尚未成为正式 System | P0 |
| 视觉 / trace 验收 | 第 150 帧同 tick 截图已可复现；阴影接入后**仿真状态差异成了画面差距的主因**（物体位置不同 → 影子位置不同）；仍无逐帧 trace / 状态哈希对照 | P0 |
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

1. 仿真与 PBR：环境反射已补上（近似）；下一步补**刚体求解器**（旋转 / 静-动碰撞 / 堆叠稳定，画面差距主因），然后把 metallic / roughness 搬进实例数据做真实 split-sum 与 GGX，再做 CSM / 软阴影 / 网格纹理。
2. 正式调度：把脚本与默认系统纳入 `Schedule` 的访问声明批次。
3. 逐帧 trace / 截图对照与性能门禁。
4. 密码学签名、跨机器增量烹饪。
5. 推送与 CI 全绿。
