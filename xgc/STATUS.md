# XGC 状态与割接验收

> 这是能力的**现状快照**，不是完成声明。XGC 仍明显低于 XGameEngine 全量能力，
> 当前不能割接。

## 指标（第 105 轮）

| 项 | 值 |
|---|---|
| Rust 文件 | 104（最大 588 行；软上限 600） |
| 测试 | 176（`cargo test --workspace --offline`） |
| 内建组件 schema | 27 类 / 31479 字节，两次导出逐字节相同 |
| 默认系统 | 16 个 |
| 门禁 | fmt / clippy `-D warnings` / xtask check 全绿 |
| CLI 冒烟 | `check` 27 组件 OK；`run 60` frames=60 entities=3 revision=3 hash=`f4bc029f5f995b91` |

## 已具备

| 子系统 | 现状 |
|---|---|
| ECS / World | archetype 列式存储、持久 `EntityId` + generation 句柄、bundle spawn、快照 / 恢复、状态哈希 |
| 事务 | `Command` plan / apply / revert、单 revision、日志 / 重放、Agent 撤销 |
| Agent 协议 | describe（组件 / 系统 / 渲染能力）、diagnostics、query / get / digest / stats / validate、load / save、snapshot / restore、history / replay、plan / apply / revert / undo |
| 确定性 | 固定步 schedule、trace / verify、schema 字节级确定性、状态哈希 |
| 渲染图 | 资源表、pass 读写、确定性拓扑、环 / 多写者 / 未声明资源检测 |
| wgpu 渲染 | 相机 MVP、深度、单 pass、顶点 / 索引缓存、按模型实例化、DrawList、RGBA8 回读 |
| UI | 布局 / 父相对 / 裁剪 / 命中、输入桥、Tab + 方向键焦点、背景 / 焦点环、5x7 文本换行、PNG/PPM 纹理、stretch/preserve/cover/nine-slice、scissor 裁剪 |
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
| UI | 70% | 布局 / 交互 / 焦点 / 文本 / 图片 / 九宫格 / 圆角 / scissor 已通 |
| 迁移工具 | 55% | 组件映射全，但端到端项目 / 资源 / 脚本未验收 |
| 动画 | 55% | 剪辑 + 骨架 + CPU 蒙皮 + 渲染接入；缺 GPU 蒙皮、状态机、分层 |
| 音频 | 40% | 解码 / 混音 / 空间化；缺设备输出与流式 |
| 渲染 | 35% | 基础 wgpu + 实例化 + UI；缺 PBR / 阴影 / 后处理 / 粒子 / 蒙皮绘制 |
| 物理 | 25% | 自研简化碰撞；缺完整刚体、关节、连续碰撞、坡面滑动 |
| 资产 / 打包 | 15% | 扫描与迁移；缺 cooked asset、依赖清单、播放器打包 |
| Lua 脚本 | 50% | 宿主 + 字段 / 实体 / 组件增删 + 命令结算 + 预算 / 内存 / 热重载 + `run-scripts`；缺完整引擎 API 与正式 System 调度 |
| 编辑器 | 60% | eframe 窗口 + 层级 / Inspector 编辑 + 增删 + Play + wgpu 视口 + Undo/Redo + demo 场景 + 选中高亮；缺 Gizmo 与相机控制 |
| 割接验收 | 20% | 无旧 / 新对照、无性能门禁、未推送 |

**整体能力对等约 40%，割接就绪约 20–25%**。核心运行时本身接近可用，
差距集中在脚本世界 API、编辑器、渲染高级特性与端到端验收。

## 与 XGameEngine 的主要差距

| 差距 | 影响 | 优先级 |
|---|---|---|
| 骨骼 / 蒙皮动画 | CPU 蒙皮与 GLB skin 数据已通；缺 GPU 蒙皮管线与骨骼层级导入 | P0 |
| Lua 脚本运行时 | `script.lua` 目前只是数据，没有脚本宿主 / API / 生命周期 | P0 |
| 编辑器 | 无场景编辑 UI、Inspector、层级面板、撤销栈界面 | P0 |
| 物理完整性 | 只有简化角色 / AABB；无完整刚体、关节、连续碰撞、坡面滑动 | P1 |
| 音频设备输出 | 只有 WAV / 软件混音，没有音频设备与实时流 | P1 |
| 跨系统并行 | `SystemAccess` 与批次计划已有，但执行仍是顺序；未做世界分片 | P1 |
| 资产烹饪 / 打包 | 无 cooked asset、依赖清单、播放器打包 | P1 |
| 渲染高级特性 | 无 PBR / 阴影 / 后处理 / 骨骼蒙皮 / 粒子绘制 | P1 |
| UI 字体 | 仅 ASCII 5x7 位图；无字体资产 / 中文 | P2 |
| 网络 / 存档 | 无多人、无游戏存档层 | P2 |

## 割接验收门槛（未满足）

1. 旧项目 `SceneV2` + 资源 + 脚本可端到端迁移并在 XGC 中运行。
2. 骨骼 / 蒙皮、Lua 脚本、编辑器三项 P0 能力可用。
3. 旧 / 新同一场景的 trace、状态哈希、视觉截图有对照验收。
4. 性能基准达到约定预算，且热路径分配有门禁。
5. 推送与 CI 全绿；当前本地提交因无 GitHub 凭据**未推送**。

## 下一步（按顺序）

1. 骨骼 / 蒙皮：GPU 蒙皮管线、骨骼层级导入、动画驱动骨骼。
2. Lua 宿主：脚本生命周期、API 表、热重载、沙箱。
3. 编辑器最小闭环：层级 / Inspector / 保存 / Play。
4. 跨系统并行：世界分片与访问不相交批次执行。
5. 资产烹饪与端到端迁移验收。
