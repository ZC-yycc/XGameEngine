# XGC 状态与割接验收

> 这是能力的**现状快照**，不是完成声明。XGC 仍明显低于 XGameEngine 全量能力，
> 当前不能割接。

## 指标（第 131 轮）

| 项 | 值 |
|---|---|
| Rust 文件 | 156（最大 599 行；软上限 600） |
| 测试 | 222（`cargo test --workspace --offline`） |
| 内建组件 schema | 30 类 / 34668 字节，两次导出逐字节相同 |
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
| wgpu 渲染 | 相机 MVP、深度、单 pass、顶点 / 索引缓存、按模型实例化、DrawList、线性 / sRGB RGBA8 回读 |
| UI | 布局 / 父相对 / 裁剪 / 命中、输入桥、Tab + 方向键焦点、背景 / 焦点环、5x7 文本换行、PNG/PPM 纹理、stretch/preserve/cover/nine-slice、scissor 裁剪、正式帧末 sRGB alpha 合成 |
| 动画 | RON 剪辑、平移 / 旋转 slerp / 缩放采样、目录加载、写回 Transform、Skeleton / Bone / Pose、GLB JOINTS / WEIGHTS / 逆绑定、CPU 蒙皮、DrawList 自动读取祖先骨架姿态并按实体变形上传 |
| 物理 | 重力 / 速度积分 / 地面 / 跳跃、角色台阶 / 高墙 / 球体阻挡、斜坡地面、AABB 分离 |
| 音频 | 空间化增益、总线、WAV 解码、软件混音、WAV 写出 |
| Lua | 安全宿主：执行 / 调用 / 字段与实体 API / 指令预算 / 内存上限 / 生命周期；`ScriptRuntime` 驱动脚本实体并热重载；`Simulation::step_with` 可插脚本帧 |
| 资产 / 迁移 | `.meta` 扫描、旧 SceneV2 全组件映射（含 UI / Particle / Rust 插件）、目录批量迁移 |
| CLI | schema / check / bench / export / import / run / serve / migrate / assets / clips / audio / mix / render-graph / gpu-probe / mesh-probe / render-frame |

## 进度评估（主观加权，非指标）

| 子系统 | 估计完成度 | 依据 |
|---|---:|---|
| 核心 ECS / 确定性 / 事务 / Agent | 80% | 主体可用；缺零分配门禁与跨系统并行 |
| UI | 72% | 布局 / 交互 / 焦点 / 文本 / 图片 / 九宫格 / 圆角 / scissor + 正式帧背景 / 文本 alpha 合成已通 |
| 迁移工具 | 90% | 场景 / 脚本 / 预制体 / 统计对照全通；修复 GlobalTransform 空字段与旧相机 +Z 约定，真实 showcase 1280×720 可渲染出场景；缺旧新逐帧视觉对照 |
| 动画 | 97% | 根 / 关节轨道 + 骨架 + CPU / GPU 蒙皮 + 状态机 + 叠加层（实体 / 关节遮罩）+ 转场淡入；缺根关节混合权重细调 |
| 音频 | 60% | 空间化增益、总线、WAV 解码、软件混音、WAV 写出、cpal 默认输出设备播放（重采样 / 队列）、`xgc play`；缺流式资产 / 3D 声像 |
| 渲染 | 48% | 真实 builtin 几何 + 法线派生方向光面着色 + 天空清屏色 + 实例化 + sRGB HUD 合成；缺 PBR / 网格纹理 / 阴影 / 后处理 / 粒子绘制 |
| 物理 | 25% | 自研简化碰撞；缺完整刚体、关节、连续碰撞、坡面滑动 |
| 资产 / 打包 | 70% | 引用裁剪 + 增量烹饪（哈希未变跳过写入）+ manifest 聚合签名；JSON / XGCB / 图形播放器；缺密码学签名 / 跨机器增量 |
| Lua 脚本 | 80% | 旧 `update(ctx)` + 移动 / 旋转 / 查询 / UI / `action_pressed`；`InputMap` 解析 input.ron 动作与轴；缺窗口键盘事件接线与音频脚本 API |
| 编辑器 | 88% | 层级 / Inspector / Play / 视口 / Undo / Gizmo / 吸附 + 预制体实例化 / Revert / 位置覆写 + Play Scripts（脚本 + input.ron 动作） |
| 割接验收 | 43% | 统计对照一致；正式帧已合成 HUD 并重算层级世界变换，固定参考图 mean_abs 降到 30.50；缺同 tick / trace 对照、并行调度与推送 |

**整体能力对等约 56%，割接就绪约 43%**。核心运行时、迁移、动画、编辑器和
cooked 播放器已可用；差距集中在渲染高级特性、正式调度、逐帧验收与推送。

## 迁移 / 运行证据

- `projects/XGame/Scenes/showcase.ron` → 24 实体 / 51 组件 / 0 缺失父节点。
- `run-world` 60 帧通过；`render-frame` 1280×720 输出 14 个场景 draw / 5 个场景 draw call / 7 个 UI draw。
- 旧 / 新 headless 统计逐项一致：24 实体 / 10 刚体 / 10 碰撞体 / 2 发射器 / 1 监听者。
- `render-frame` 先运行唯一 `HierarchySystem` 重算父子世界变换，再按目标尺寸运行 `UiLayoutSystem`；层级修正改变 20712/921600 像素（2.25%），固定参考图 mean_abs 从 31.23 降到 30.50、max 为 255。该 A/B 仍尚非同 tick。
- GUID 资产（Box.glb、Fox.gltf + Fox.bin）可解析；character 脚本 / Pillar 预制体可迁移。
- cooked 包：引用裁剪 7 文件、XGCB 二进制、manifest 哈希 / 签名校验、`xgc-player` headless 60 帧。
- 自动化测试：222 条；正式帧回归覆盖 sRGB alpha HUD，并钉住渲染前父子 `GlobalTransform` 合成。

## 与 XGameEngine 的主要差距

| 差距 | 影响 | 优先级 |
|---|---|---|
| 渲染高级特性 | 基础几何 / 面着色 / 天空 / HUD 已通；缺 PBR、网格纹理、阴影、后处理、粒子 | P0 |
| 正式 System 调度 | `SystemAccess` / `batches()` 已有，执行仍顺序；脚本尚未成为正式 System | P0 |
| 视觉 / trace 验收 | 旧新统计一致，`render-frame` 已重算层级世界变换；仍无逐帧 trace / 同 tick 截图 | P0 |
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

1. 渲染：HUD / 层级世界变换已进入正式帧；下一步统一同 tick，再做 PBR / 网格纹理 / 阴影。
2. 正式调度：把脚本与默认系统纳入 `Schedule` 的访问声明批次。
3. 逐帧 trace / 截图对照与性能门禁。
4. 密码学签名、跨机器增量烹饪。
5. 推送与 CI 全绿。
