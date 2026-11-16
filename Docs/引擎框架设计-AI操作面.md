# XGame Core·AI 操作面（提案）

> **体裁：设计提案（未实施，实施后归档）。** 本文是《[引擎框架设计](引擎框架设计.md)》的
> AI 分册，只回答 §10–§12：Agent 协议（查询 / 事务 / 仿真 / 诊断 / 视觉）、编辑器、
> 代码库如何让 AI 可维护。
> **不回答：** 数据模型 / 调度 / 渲染（见[运行时](引擎框架设计-运行时.md)）；
> 性能目标 / 路线 / 迁移（见[主文档](引擎框架设计.md)）。
> **真相源：** 协议方法名与 JSON 字段以本文为准；与主文档冲突时以主文档的决策为准。
> **分册导航：** [主文档](引擎框架设计.md) ｜ [运行时](引擎框架设计-运行时.md)

## 10. AI 操作面

这一节是整份设计的重心。判据只有一句：**智能体没有鼠标、没有人类记忆、上下文有限，
它能不能独立完成"看见 → 修改 → 验证 → 回滚"？**

当前引擎的答案是"部分能"：能读 schema / 脚本契约、能整文件写场景、能单实体
upsert、能跑 `--check` 与 300 帧无头。但**没有查询、没有批量事务、没有回滚、
没有 diff、错误只有文本**（当前 15 个 MCP 工具的实际能力边界）。新框架把这条链路补齐，
并且把它做成引擎的**一等接口**，而不是 MCP 层外挂的一组工具。

### 10.1 三个问题

| 问题 | 现状 | 目标 |
|---|---|---|
| **看见** | 整份 RON、`project_check` 文本、10 条错误样本、一张截图 | 分页查询 + 摘要 + `explain` + 状态哈希 + 截图 diff |
| **修改** | 一次一个实体 / 组件 / 资产；整文件重写；无预览 | 一个事务 N 条命令；plan 先看 diff；apply 原子提交 |
| **证明** | 重跑整个项目；读日志；人眼看图 | 定向仿真 + state hash + 自动断言 + golden diff |
| **反悔** | 编辑器内存撤销（100 份整场景）；MCP 无 | `revert(tx_id)`；文件级 / 项目级回滚 |

### 10.2 协议分层

| 层 | 能力 | 例子 | 读写 |
|---|---|---|---|
| **L0 发现** | 引擎是什么、能做什么、限制在哪 | `engine.describe` / `schema` / `diagnostics.explain` | R |
| **L1 查询** | 世界现在是什么样 | `world.query` / `get` / `digest` / `diff` / `explain` | R |
| **L2 编辑** | 改世界，先计划后提交 | `edit.plan` / `apply` / `revert` / `history` | W |
| **L3 仿真** | 跑时间、快照、回放 | `sim.run` / `step` / `snapshot` / `restore` / `replay` / `watch` | W |
| **L4 验证** | 证明改对了 | `project.check` / `test.run` / `script.check` / `render.capture` / `render.diff` | R/W |
| **L5 诊断** | 为什么不对 | `diag.list` / `diag.explain` / `log.query` / `world.explain` | R |
| **L6 会话** | 持续协作 | `session.open` / `subscribe` / `claim` / `close` | W |

L0–L5 是**无状态也成立**的（每次调用可以打开项目、执行、返回）；L6 是**有状态会话**
（低延迟、增量、多轮）。CI 用无状态，Agent / 编辑器用会话；两者执行同一份代码。

> 下面先讲诊断（L5）再讲视觉验证（L4），因为诊断是验证的入口；协议层号不变。

### 10.3 L0：发现（Describe）

`engine.describe` 是 AI 的"说明书"，分节返回，避免一次吐出几 MB：

```json
{
  "protocol": "xgc.agent/1",
  "engine": {"version": "0.1.0", "build": "…", "platform": "windows-x86_64"},
  "determinism": {"scope": "same-binary-same-platform", "fixed_dt": 0.0166667},
  "limits": {
    "entities": 1000000,
    "bones_per_skinned_mesh": 512,
    "lights_clustered": 1024,
    "script_instructions_per_call": 200000,
    "script_memory_mib": 128,
    "max_query_rows": 5000
  },
  "sections": ["components", "commands", "queries", "render", "importers", "tools", "unsupported"],
  "unsupported": ["2d", "netcode", "terrain", "cross_platform_bit_exact"]
}
```

- **能力清单**不是手写文档：组件 / 命令 / 查询 / 导入器 / 渲染特性全部从注册表生成；
  文档与代码不可能漂移。
- **限制**必须显式：AI 最常犯的错是"以为引擎支持"或"以为数值被钳制了"。
  所有会拒绝的边界（骨骼数、灯数、查询行数、脚本预算）都要在 `limits` 里。
- **`unsupported`** 是一等字段：明确说"没有 2D / 网络 / 地形"，比让 AI 猜更省时间。
- 分节拉取：`describe {sections:["components"], cursor}`；大清单分页。

### 10.4 L1：查询（Query）

**为什么不能给整份 RON**：10k 实体的场景文本约数 MB，按 token 估算在数十万量级；
一次对话读不完，也没法增量。查询要能"只拿需要的字段、只要变化的部分、只要一页"。

```json
// world.query
{
  "scene": "scenes/level1.scene.ron",
  "select": ["entity.id", "entity.name", "core.transform.position", "game.health.hp"],
  "filter": {"and": [
    {"has": "game.health"},
    {"lt": ["game.health.hp", 20]},
    {"aabb": [[-10, 0, -10], [10, 4, 10]]}
  ]},
  "order": ["entity.id"],
  "limit": 200,
  "include_defaults": false
}
```

```json
{
  "revision": 42,
  "rows": [
    {"entity.id": "17", "entity.name": "Slime_01",
     "core.transform.position": [2.0, 0.0, -3.0], "game.health.hp": 12.0},
    {"entity.id": "18", "entity.name": "Slime_02",
     "core.transform.position": [-4.0, 0.0, 1.5], "game.health.hp": 8.0}
  ],
  "next_cursor": "eyJpZCI6MTh9"
}
```

设计要点：

- **两种结果形态**：默认行对象（好读）；`format:"columns"` 返回
  `{columns:[{name,type}], rows:[[…]]}`（省 token、适合大结果集）。同一查询计划，
  只换序列化。
- **稳定游标**：游标编码 `(revision, last_entity_id)`，不是偏移量；世界变了游标失效
  并报错，而不是悄悄跳行。
- **省略默认值**：`include_defaults:false` 时只返回非默认字段；schema 知道默认值，
  AI 不需要看一屏 `scale:[1,1,1]`。
- **稳定排序**：默认按 `entity.id`；允许按任意标量字段排序，平手按 ID。
- **64 位 ID 用字符串**：`entity.id` / `parent` / `subject.id` 在 JSON 里是十进制
  字符串（避免 2⁵³ 精度丢失）；`revision` 是数字。
- **只读**：查询永远不产生写锁；在安全点执行，结果绑定 revision。
- **预算**：单次查询有行数 / 字节 / 时间上限；超限返回已取部分 + `next_cursor`，
  不阻塞仿真，也不静默截断（`truncated:true`）。

配套查询：

| 方法 | 返回 | 用途 |
|---|---|---|
| `world.get(entity)` | 单实体全组件 + 非默认值 + 路径 | 看细节 |
| `world.digest(scene)` | `{entity_count, structure_hash, component_hash}` | 廉价判断"变没变" |
| `world.diff(from_rev, to_rev)` | 变更实体 / 字段 / 创建 / 删除 | 增量上下文、审计 |
| `world.explain(entity)` | 可见性 / 物理 / UI 的逐级原因 | 排"为什么没画 / 没碰 / 没响应" |
| `world.find(name / path)` | 唯一实体或歧义诊断 | 人 / AI 寻址 |
| `world.graph(asset / entity)` | 依赖 / 反向引用 | 删除前影响分析 |
| `world.spatial(aabb / sphere / ray)` | 命中实体（排序） | 空间查询 |

`world.explain` 是杀手级能力，示例：

```json
{
  "entity": "17",
  "render": {"visible": false, "stage": "frustum", "reason": "world_aabb_outside",
             "aabb": [[40,0,40],[41,1,41]], "camera": "main"},
  "physics": {"body": "dynamic", "sleeping": true, "colliders": 1, "layer": 3},
  "ui": {"visible": true, "hit_test": false, "reason": "pointer_outside_viewport"}
}
```

### 10.5 L2：编辑（Plan → Apply → Revert）

```json
// edit.plan
{
  "base_revision": 42,
  "ops": [
    {"op": "entity.create", "temp_id": "door", "name": "Door",
     "parent": "3", "components": {"core.transform": {"position": [1,0,2]}}},
    {"op": "component.set", "entity": "door",
     "path": "game.door.locked", "value": true},
    {"op": "prefab.instantiate", "prefab": "guid://0190…", "parent": "3",
     "transform": {"position": [4,0,2]}}
  ]
}
```

```json
// plan 结果
{
  "plan_id": "p-7",
  "valid": true,
  "diagnostics": [],
  "predicted_diff": {
    "entities_created": 2,
    "entities_modified": 0,
    "files": ["scenes/level1.scene.ron"],
    "bytes": 812
  },
  "affected": {"entities": ["tmp:door", "91"], "assets": ["guid://0190…"]}
}
```

```json
// edit.apply
{"plan_id": "p-7", "base_revision": 42, "idempotency_key": "agent-7f3a"}
```

```json
// apply 结果
{"revision": 43, "tx_id": "t-9", "diff": {"created": ["91", "92"], "modified": []}}
```

规则：

- **乐观并发**：`base_revision` 不匹配 → 拒绝，返回当前 revision 与冲突字段；
  不做"最后写入者赢"。
- **幂等**：`idempotency_key` 重复提交返回同一结果，不重复创建实体。
- **临时 ID**：同一计划内 `temp_id` 可被后续 op 引用；apply 时解析成真实 `EntityId`，
  结果里返回映射。
- **原子**：跨文件（场景 + 预制体 + 项目配置）要么全成，要么全不动；失败留下可恢复
  的 journal，不留下半写文件。
- **可逆**：每个 op 生成逆 op + before-image；`revert(tx_id)` 产生新 revision。
- **可审计**：`edit.history` 返回事务列表（作者、时间、op 数、影响实体），
  时间只用于审计，不进仿真。
- **冲突粒度**：事务记录"触碰的实体 / 字段 / 文件"；两个事务触碰集合不相交即可
  并行提交，相交则第二个拒绝并给出冲突清单。

**多 Agent 协作**：`session.claim(entities|scene, ttl)` 申请写租约；未持有租约的
破坏性写被拒绝。租约过期自动释放；这是"两个 AI 同时改同一棵树"的最小防护，
不替代 revision 冲突检测。

### 10.6 L3：仿真（Run / Snapshot / Replay）

```json
// sim.run
{
  "frames": 300,
  "dt": 0.0166667,
  "inputs": [{"frame": 10, "key": "Space", "down": true},
             {"frame": 20, "key": "Space", "down": false}],
  "stop_on": ["script_error", "nan_transform"],
  "snapshot": "after",
  "watch": ["entity.17.core.transform.position", "game.health.hp"]
}
```

```json
{
  "trace_id": "tr-3",
  "frames_run": 300,
  "state_hash": "9f2c…",
  "report": {"ok": true, "frame_us": {"p50": 120, "p99": 340},
             "entities": 1200, "bodies": 64, "script_errors": 0},
  "watch": {"entity.17.core.transform.position": [[0,0,0], [0,0,0.5], [0,0,1.0]]},
  "snapshot": "snap-4",
  "events": [{"frame": 10, "kind": "input", "key": "Space"},
             {"frame": 42, "kind": "script", "level": "error", "code": "SCRIPT.RUNTIME"}]
}
```

- **`sim.step`**：推一帧（交互式调试）；`pause` / `resume` / `time_scale`。
- **`sim.snapshot` / `restore`**：内存命名快照；`sim.save` / `sim.load` 落盘。
- **`sim.replay(trace_id)`**：重放并逐帧比对 state hash；返回首个分歧帧 + 差异摘要。
- **`sim.watch`**：按帧采样字段，返回序列；AI 看趋势不用拉全量状态。
- **`trace`**：每帧记录输入、命令、事件、state hash、脚本错误、物理统计；
  体积可控（默认只记摘要，`full:true` 才记全量）。
- **确定性**：同二进制 + 同平台 + 同输入 → 同 hash；不同线程数也要同 hash
  （见[运行时 §5.2](引擎框架设计-运行时.md)）。

**时间旅行调试**：编辑器拖时间轴 = 恢复快照 + 重放到目标帧；断点 = `stop_on`
条件；这是 AI 与人都能用的同一套机制。

### 10.7 L5：诊断（Diagnostics）

```json
{
  "code": "WORLD.DUP_ENTITY_ID",
  "severity": "error",
  "subject": {"kind": "entity", "id": "18446744073709551615"},
  "span": {"file": "scenes/level1.scene.ron", "line": 42, "column": 3},
  "message": "实体 ID 重复：两个实体使用同一 ID",
  "details": {"duplicates": ["17", "17"]},
  "fix": {"kind": "rename_id", "value": "18"},
  "docs": "xgc://diagnostics/WORLD.DUP_ENTITY_ID"
}
```

- **code 稳定**：命名空间 `AREA.REASON`，只增不改；AI 可以按 code 匹配处理逻辑。
- **subject**：实体 / 资产 / 场景 / 字段 / 命令 / 系统；不是一坨字符串。
- **span**：文件 + 行列，或 JSON Pointer；能点回源码。
- **fix**：可选的机器可应用修复（仍然走 `edit.plan`，不直接改）。
- **分级**：error（阻断）/ warning（可用但有风险）/ info（建议）。
- **列表**：`diag.list {scene, severity, code, limit, cursor}`；`diag.explain(code)`
  返回完整说明与示例。
- **不静默**：拒绝时一定带诊断；不钳制、不猜、不"尽力而为"。
- **日志同构**：`log.query {level, code, entity, since, limit}` 返回与诊断同形的
  结构（level / code / subject / span / message / frame），控制台、CI、AI 读同一份，
  不再从自由文本里正则抠信息。

### 10.8 L4：视觉验证（Capture / Diff）

```json
// render.capture
{
  "camera": {"mode": "named", "name": "main"},
  "width": 1280, "height": 720,
  "frame": 120,
  "view": "beauty",
  "samples": 1
}
```

返回：PNG（MCP image 内容块 / 文件）+ `hash` + 机位 + `render_stats`
（draw calls / culled / overdraw / GPU 时间）。

- **确定性**：固定机位 / 固定帧 / 固定采样序列；同输入两次逐字节相同。
- **`render.diff`**：两张图 + 容差 → 差异图 + 变更包围盒 + 像素计数；AI 不用"看"
  两张图猜哪里变了。
- **`render.golden`**：命名基线；CI 比对；失败输出差异图。
- **`render.explain`**：逐 pass 的输入输出、耗时、资源；排"为什么黑屏 / 为什么没影子"。
- **调试视图**：`view: wireframe|overdraw|normals|clusters|shadow_cascades|lod`。

### 10.9 L6：会话与并发

```text
xgc serve --project . --transport stdio|tcp --mode read|edit|admin
```

- **单写者**：命令队列串行结算；查询在安全点执行；不会有两个线程同时改世界。
- **订阅**：`session.subscribe {topics:["revision","diag","log","frame"]}`，
  服务端推送 JSON-RPC notification；AI 可以"等世界变了再读"，不用轮询。
- **会话状态**：当前项目 / 场景 / revision / 快照 / 租约 / 订阅；断线重连可恢复。
- **无状态模式**：`--transport stdio` 一次性执行（CI / MCP 兼容），每次调用打开项目；
  大项目冷启动慢，但无状态、可并发、可容器化。
- **权限模式**：`read`（只读）/ `edit`（可改场景与资产）/ `admin`（可构建 / 删除 /
  导入）；默认 `read`，破坏性操作要显式升级。TCP 默认只绑 `127.0.0.1`，启动时生成
  一次性 token 文件；stdio 继承父进程权限，不额外开端口。
- **多实例**：同一项目允许多个只读会话；写会话最多一个（文件锁 + revision）；
  多 Agent 用租约分区。

### 10.10 与 MCP 的关系

- **协议是源，MCP 是适配器**：`xgc-agent` 定义方法 / 类型 / 诊断；MCP server
  由协议描述生成 `tools/list` 与 `tools/call`，不手写 15 份 JSON Schema。
- **工具注解**：每个方法带 `readOnlyHint` / `destructiveHint` / `idempotentHint`；
  MCP 客户端能据此决定是否要人工确认。
- **输出契约**：每个方法有 `outputSchema`；错误分两层（协议错误 / 工具错误），
  工具错误体是结构化诊断，不是纯文本。
- **兼容旧工具名**：如果外部已有 Agent 依赖当前 15 个工具名 / 字段，保留一层
  **协议适配**（只做字段映射，不含第二份引擎逻辑）；这是对外契约，不是兼容层。
  新项目建议直接用 `xgc.agent/1`。

### 10.11 Token / 延迟预算

| 任务 | 现状（估算 / 实测） | 目标 | 机制 |
|---|---|---|---|
| 读 10k 实体场景 | 数 MB RON / 数十万 token | 200 行 × 4 字段 ≈ 10 KB / 3k token | 查询 + 分页 + 省略默认值 |
| 按名字找实体 | 读整文件 / 线性扫 | O(1) + 唯一性诊断 | NameIndex |
| 改一个字段 | 整文件重写 / 单实体 upsert | 单字段 patch，~100 B | 字段级命令 |
| 批量 100 处修改 | 100 次调用 / 100 次写盘 | 1 次 plan + 1 次 apply | 事务 |
| 验证改动 | 全项目 `--check` + 300 帧无头 | 定向 sim 60 帧 + hash + capture | 会话 + 快照 |
| 回滚 | 无 / 手动反向改 | `revert(tx_id)` | 逆命令 |
| 排"为什么没画" | 读日志 / 猜 | `world.explain` 一次调用 | 逐级可见性原因 |
| 能力发现 | 读源码 / 文档 | `engine.describe` 一节 | 注册表 |

数字是设计目标，M1 用真实 Agent 任务测（§10.12），不靠估算验收。

### 10.12 Agent 验收任务（M1 出口）

用 10 个可自动判定的任务衡量"AI 可操作性"，不是只看工具数量：

| # | 任务 | 判定 |
|---|---|---|
| 1 | 在场景里创建一个带脚本的旋转方块 | 实体存在、脚本运行、60 帧后旋转角非零 |
| 2 | 找到并修复一个悬空 GUID | `check` 通过、引用指向正确资产 |
| 3 | 给 100 个实体批量改层 | 1 个事务、revision +1、100 个实体层正确 |
| 4 | 让角色走到指定点 | 300 帧后位置误差 ≤ 0.1 |
| 5 | 排查"实体不可见" | `world.explain` 命中正确阶段 |
| 6 | 回滚一次误删 | 实体与组件恢复、revision 前进、hash 回到删除前 |
| 7 | 改材质颜色并截图验证 | 截图中心像素颜色在容差内 |
| 8 | 写一个 Lua 行为并通过 `script.check` | 静态检查 0 error、运行无 error |
| 9 | 把场景从 1000 实体扩到 10000 实体 | 查询 / 编辑 / 运行都在预算内 |
| 10 | 解释一次物理穿透 | `world.explain` / `sim.trace` 给出原因 |

每个任务记录：成功 / 失败、工具调用数、token 数、耗时、是否需要人工干预。
**这组任务是"AI 原生"的验收，不是功能演示。**

---

## 11. 编辑器

### 11.1 编辑器 = 协议客户端

- 编辑器的每个动作（创建 / 删除 / 改字段 / 拖拽 / 撤销 / 保存）都是一条命令，
  与 AI 走同一条队列；不存在"编辑器专用写路径"。
- 面板是查询结果的视图：Hierarchy / Inspector / Assets / Console / Profiler /
  Render Graph / Physics Debug；查询走会话，读的是 revision 快照。
- 撤销 = 事务日志；Play = 快照 + 恢复；暂停 / 单步 = 仿真控制；时间轴 = 快照重放。
- 编辑态内联仿真（零输入延迟）；Play 态 worker 仿真（不卡 UI）。

### 11.2 生成式 Inspector

- 字段来自注册表：类型 → 控件（滑块 / 数值框 / 颜色 / 曲线 / 资源选择器）；
  范围、单位、步长、只读、分组全部来自 `#[field]` 声明。
- 预制体实例显示 override 徽标（来自资产 / 已覆盖），支持单字段 Revert
  （修当前"只能整树 Revert"的差距）。
- 多选混合值、批量改、撤销合并成一个事务。
- 自定义编辑器（动画状态机、材质、粒子曲线）用注册表声明，不写一次性面板。

### 11.3 控制台 / 诊断

- 日志与诊断统一；每条有 code / subject / span；点击跳到源码或实体。
- 过滤 / 分组 / 计数；错误样本可导出为 JSON，AI 直接读。
- 运行态与编辑态共用；Play 中脚本错误不打断编辑器。

### 11.4 Headless-first

**每个编辑器能力都必须有一个 headless 命令**，否则不算完成：

| 编辑器动作 | Headless 等价 |
|---|---|
| 打开项目 | `xgc project.open` |
| 改字段 | `edit.apply` |
| 保存 | `project.save` |
| Play | `sim.run` |
| 截图 | `render.capture` |
| 检查 | `project.check` |
| 构建 | `project.build` |

这条规则让 CI、AI、编辑器三者永远同源；也防止"只有 GUI 能做的事"变成自动化死角。

### 11.5 UI 与布局

- egui 继续用（成熟、即时模式、与 wgpu 集成好）；但面板逻辑与数据分离，
  便于测试与替换。
- 尺寸 / 颜色 / 行高只从主题取；图标矢量绘制；三档窗口宽度截图自检
  （继承当前编辑器 UI 约定）。
- 偏好 / 布局有 schema，坏布局回退出厂，写盘只在松手 / 关闭 / 退出。
- 多窗口 / 多视口：v2；当前先保证单窗口 + 可停靠。

---

## 12. 代码库如何让 AI 可维护

"架构便于 AI 操作"不只是对外协议，也包括**AI 改引擎代码的成本**。当前最贵的三件事：
加组件 ≈20 处、单体文件 250–282 KB、手写 schema / 清单 / 文档各自漂移。新框架用
四条制度解决。

### 12.1 文件与模块预算

| 规则 | 阈值 | 判据 |
|---|---|---|
| 单文件行数 | 软 600 / 硬 1000 | `xtask size-check`；超硬阈值 PR 拒绝 |
| 单模块公开项 | ≤ 12 | 超过说明"一个模块不止一个概念" |
| 单函数行数 | 软 80 / 硬 150 | clippy + 审查 |
| 结构体字段 | 软 16 / 硬 24 | 超过拆成组件 / 子结构 |
| `World` 这类核心类型 | 只放数据 + 入口，逻辑在 service / system | 审查 |

**没有 god struct。** `Simulation` 由多个 service 组成：

```rust
pub struct Simulation {
    world: World,
    physics: PhysicsService,
    animation: AnimationService,
    ui: UiService,
    audio: AudioService,
    script: ScriptService,
    extract: ExtractService,
    schedule: Schedule,
    clock: FixedClock,
}
```

每个 service 有自己的状态与测试；`World` 不认识它们。

### 12.2 注册表 + 代码生成

- 组件、命令、查询、导入器、渲染 pass、脚本 API 都通过**声明 + 注册**接入。
- `xgc-xtask` 扫描源码里的 `#[derive(Component)]` / `#[command]` / `#[importer]`，
  生成：
  - 组件注册表（ID / 字段 / 默认值 / 序列化 / 校验）；
  - schema JSON；
  - Lua 绑定 + 清单 + 文档表；
  - Inspector 字段描述；
  - MCP / Agent 方法的 JSON Schema；
  - 生成式测试（默认值往返、schema 一致性、字段穷举）。
- 生成物带源码哈希；源码改了没重新生成 → CI 红。生成物可读（不是一坨宏展开），
  AI 能读懂、能定位。
- 显式注册优于隐式链接期魔法：`inventory` / `linkme` 在 cdylib 与增量编译下有坑，
  生成一个 `registry_generated.rs` 更可控。

### 12.3 架构门禁

`cargo xtask check` 一条命令跑完：

| 门禁 | 检查 | 失败后果 |
|---|---|---|
| `dep-check` | crate 依赖方向（[主文档 §3.2](引擎框架设计.md)） | 编译期之外再兜一层 |
| `size-check` | 文件 / 模块 / 函数阈值 | 阻止 god file 回潮 |
| `no-compat` | `V2` / `legacy` / `shim` / `deprecated` / `serde(alias)` | 阻止双轨 |
| `alloc-gate` | 热路径零分配（所有系统，不只窄路径） | 阻止每帧分配 |
| `det-gate` | 1 / 2 / 8 线程 state hash 一致 | 阻止伪并行 |
| `schema-drift` | 生成物与源码一致 | 阻止手改生成物 |
| `docs-links` | 文档链接 / 索引 / 横幅 | 阻止文档腐烂 |
| `public-api` | 公开 API 快照 | 阻止无意破坏 |
| `unsafe-audit` | `unsafe` 只允许出现在白名单模块 | 阻止 unsafe 扩散 |
| `panic-audit` | 用户内容路径禁止 `unwrap` / `expect` | 阻止坏资产崩引擎 |

### 12.4 机器可读的架构地图

生成 `architecture.json`（随代码更新，进版本库）：

```json
{
  "crates": [{"name": "xgc-world", "deps": ["xgc-base", "xgc-reflect"]}],
  "components": [{"id": 257, "name": "core.transform", "fields": ["position", "rotation", "scale"]}],
  "systems": [{"name": "move_players", "stage": "FixedSim", "reads": ["PlayerInput"], "writes": ["Velocity"]}],
  "commands": ["entity.create", "component.set", "prefab.instantiate"],
  "methods": ["world.query", "edit.plan", "sim.run"],
  "invariants": ["no_alloc_in_hot_path", "deterministic_parallel", "single_write_path"]
}
```

- AI 先读这张图，再决定读哪几个文件；不用把 12 万行全塞进上下文。
- 每个 crate 一份 `AGENTS.md`：入口、不变量、常见改法、测试命令、性能预算。
- 文档的"参考"部分由注册表生成；手写文档只保留"为什么"（ADR / 决策记录）。

### 12.5 测试金字塔

| 层 | 内容 | 生成方式 |
|---|---|---|
| 单元 | 纯函数 / 组件 / 查询 | 手写 + 生成默认值往返 |
| 属性 | 序列化往返、命令逆运算、查询过滤 | `proptest` |
| 集成 | 场景 / 预制体 / 导入 / 保存 | 固定夹具 |
| 像素 | 每个渲染特性开 / 关 A/B | golden + 容差 |
| 回放 | 同输入 state hash 一致 | 轨迹夹具 |
| 性能 | 每系统 / 每场景预算 | 基准套件 |
| 模糊 | RON / 命令 / 查询输入 | `cargo-fuzz` |
| 并发 | 调度器 / 通道 | `loom` |
| unsafe | arena / FFI | Miri |

**测试也是文档**：每个功能至少一条"能跑的例子"，AI 改代码先跑测试，不靠猜。

### 12.6 PR 契约

每个 PR 必须回答（模板化，CI 检查字段）：

1. **schema diff**：有没有改落盘 / 协议？改了必须带迁移与生成物。
2. **删除清单**：删了哪些旧类型 / 文件 / 测试？没有就写"无"。
3. **测试**：新增 / 修改了哪些测试？先红后绿证据。
4. **性能**：热路径改动必须带同机前后数字；不达标说明原因。
5. **文档**：哪份文档的哪一节改了？没有就写"无"。

**编译时间是 AI 迭代成本**：目标增量 `cargo check` ≤ 15 s，全量 ≤ 5 min；
用 crate 拆分、减少单态化、`cargo nextest`、可选 sccache；超过预算先修构建，
不加功能。

### 12.7 三个"加东西"的对照

| 任务 | 当前 | 目标 | 关键机制 |
|---|---|---|---|
| 加组件 | ≈20 处 + 测试 | 1 个文件 + 1 行注册 | derive + xtask 生成 |
| 加渲染 pass | `render()` + 3 宿主 + BGL | 1 个 graph node | RenderGraph |
| 加脚本 API | 4 处 + 计数断言 | 1 个命令 / 查询声明 | 注册表生成绑定 |

如果新框架做完，这三件事仍然要改很多处，那"AI 可维护"就没有兑现。


---

## 附录 C：Agent 协议草案

**传输**：JSON-RPC 2.0，换行分隔，stdio 或 TCP；stdio 模式与 MCP 兼容。
**方法名**：`engine.describe`、`world.query`、`edit.plan`、`sim.run`、`render.capture`…
**错误**：协议错误用 JSON-RPC `error`；工具错误用 `result.isError:true` + 诊断对象。

```json
// 请求
{"jsonrpc":"2.0","id":7,"method":"world.query",
 "params":{"scene":"scenes/level1.scene.ron","select":["entity.id","entity.name"],
           "filter":{"has":"game.health"},"limit":10}}
```

```json
// 成功
{"jsonrpc":"2.0","id":7,
 "result":{"revision":42,"columns":[{"name":"entity.id","type":"u64"},
           {"name":"entity.name","type":"string"}],
           "rows":[["17","Slime_01"]],"next_cursor":null}}
```

```json
// 工具错误
{"jsonrpc":"2.0","id":8,
 "result":{"isError":true,
   "diagnostics":[{"code":"QUERY.UNKNOWN_FIELD","severity":"error",
     "subject":{"kind":"query","field":"game.health.mana"},
     "message":"字段不存在：game.health 没有 mana",
     "fix":{"kind":"suggest","value":"game.health.hp"}}]}}
```

**兼容层的边界**：旧 MCP 工具名 / 字段名如果需要保留，只做"字段映射 + 调用新方法"，
不在适配层里复制校验 / 事务 / 查询逻辑。适配层是协议边界，不是第二份引擎。

---

## 附录 D：术语表

| 术语 | 含义 |
|---|---|
| **archetype** | 组件集合相同的实体分组；查询按 archetype 匹配 |
| **chunk** | archetype 内的连续存储块（16 KiB），查询与并行的基本单位 |
| **extraction** | 从仿真世界生成不可变 `RenderFrame` / `AudioFrame` 的过程 |
| **render graph** | 声明式帧图：pass 声明读写，自动屏障 / 别名 / 队列 |
| **bindless** | 材质 / 贴图放进大数组，用索引访问，减少 bind group 切换 |
| **meshlet / cluster** | 网格的簇级 LOD 单位，用于 GPU 剔除与虚拟几何 |
| **HZB** | 层级深度金字塔，用于遮挡剔除 |
| **revision** | 世界版本号；事务提交 +1，查询绑定 revision |
| **transaction** | 一组命令的原子提交；可逆、可审计 |
| **state hash** | 世界状态的确定性哈希；用于回放 / 分叉定位 |
| **safe point** | 世界对外可见的同步点（帧边界 / 阶段屏障） |
| **lease** | 多 Agent 写租约；未持有租约的破坏性写被拒绝 |
| **Tier 0/1/2** | 渲染能力分层；低层兜底，高层按设备启用 |

---

> **分册导航：** 返回[主文档](引擎框架设计.md) ｜ 返回[运行时](引擎框架设计-运行时.md)
