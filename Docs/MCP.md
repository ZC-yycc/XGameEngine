# MCP 接口（AI 智能体工具面）

> `xgame-mcp` 是本引擎的 **MCP server**（Model Context Protocol）：把"读说明书、读脚本
> 契约、读项目、改场景、动实体、跑检查、无头实跑"暴露成标准工具，Cursor / Claude 等
> 任何 MCP 客户端接上就能直接操纵引擎做游戏开发 —— 这是 Agent-Native 路线（见
> [`差距分析.md`](差距分析.md) 文末战略章）的第 ① 块 + 第 ② 块（无头运行经
> `run_headless` 一并接上），并承载第 ③ 块的机器可读契约（`schema_read` /
> `script_api_read`）。
>
> **判据**：智能体没有鼠标时，这条路走得通吗？—— 读说明书 / 脚本契约、改、查、看效果，
> 四步齐了。

---

## 快速接入

**Cursor / 任意 MCP 客户端**（`mcp.json` 或等价配置）：

```json
{
  "mcpServers": {
    "xgame": {
      "command": "D:\\GitHubClone\\XGameEngine\\dist\\xgame-mcp.exe"
    }
  }
}
```

**手工冒烟**（协议是"按行 JSON-RPC over stdio"，管道即可驱动，不需要客户端）：

```powershell
# PowerShell 5.1 控制台默认按 GBK 解码 stdout，先切 UTF-8 再看中文
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
@'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
'@ | .\dist\xgame-mcp.exe
```

---

## 协议面（刻意小）

- **传输**：stdio，**换行分隔的 JSON-RPC 2.0**（一行一条消息，不是 LSP 的
  Content-Length 帧）；stdout 只出协议消息，日志走 stderr。
- **方法**：`initialize`（握手，回显双方都支持的协议版本）、`ping`、`tools/list`、
  `tools/call`。其余方法回 `-32601`；通知（无 `id`）不回包；坏 JSON 回 `-32700`。
- **错误分两层**（MCP 规定，别混）：
  - **协议错误**（JSON-RPC `error` 对象）：消息不合法 —— 坏 JSON、未知方法、
    未知工具名（`-32602`）、缺 `params.name`；
  - **工具结果错误**（`result.isError: true` + 文本）：工具跑了但没干成 ——
    项目打不开、RON 解析失败、实体不存在/重名歧义。文本就是给人和模型看的失败原因。
- **无会话状态**：除 `component_catalog_read` / `schema_read` / `script_api_read` 外，每个工具都带 `project`
  （项目根路径）参数，server 不维护"当前项目"—— 可并发调用、可开多个实例、测试
  喂行进喂行出。

---

## 工具参考（15 个）

最小完备集，覆盖「读说明书 / 读脚本契约 → 读 → 改 → 查 → 看效果」闭环，外加资产侧
的「看见 → 导入 → 整理」。除 `component_catalog_read` / `schema_read` / `script_api_read` 外，全部工具必填
`project`（含 `project.ron` 的项目根目录，绝对或相对 server 工作目录）；除
`component_catalog_read` / `schema_read` / `script_api_read` / `project_info` / `project_check` / `asset_list` /
`asset_import` / `asset_move` / `run_headless` 外都可带 `scene`
（项目内相对场景路径，如 `Scenes/scene.ron`，缺省 = 清单入口场景；**绝对路径与
`..` 一律拒绝**）。

### 0. `component_catalog_read`

项目无关的内建组件目录：`builtin:*` 地址、是否可添加 / 移除 / 重复、落盘类型、
`default_ron` 与字段表。字段来自 `schema_read` 的同一份 `scene_schema()`，Render
只列当前旧格式的 `Entity.model` / `lod` / `material_asset` / `morph_weights`。
Rust 插件组件仍须由实际加载的项目注册表提供字段；Lua 不反射运行时全局变量。
返回的 `address_stability` 是 `legacy`。无需 `project`，也不读写项目文件。

### 1. `schema_read`

写第一个实体之前先读它：场景格式的**机器可读说明书**。

- **入参**：无（格式契约属于引擎，不属于某个项目 —— 不需要 `project`）
- **返回**（JSON 文本）：与 `xgame-editor --export-schema` **完全同源**
  （`xgame_core::schema::scene_schema`），含 `format_version` / `engine_version` /
  `roots`（v1/v2 场景 `.ron` 与预制体 `.prefab.ron` 各一项）/ `conventions` / `primitives` /
  `asset_references` / `types`。
- **内容规模**：53 个落盘类型。每个类型有 `summary` / `kind` / `default_ron` /
  `example_ron`；字段有 `name` / `ty` / `required` / `default_ron` / `note`；
  枚举有 `variants`（unit / struct / newtype 三种形态，各带可解析示例）。
- **确定性**：没有时间戳 / 随机 ID；同一份代码两次调用逐字节相同。
- **资产引用**：`asset_references` 明确每个字段在磁盘（`guid://<uuid>`）与内存
  （项目相对路径）里的形态 —— **MCP 读写走的都是内存形态**，GUID 转换只发生在
  `ProjectHandle` 的读写边界。
- **闭环用法**：`schema_read` 拿字段契约 → `scene_read` 看现状 → `scene_write` /
  `entity_upsert` 改 → `project_check` / `run_headless` 验证。
  这组 MCP 修改工具仍只操作 v1；v2 的根和字段目前供格式查阅，不能据此使用
  `scene_write` 或 `project_check` 操作 v2。

### 2. `project_info`

读项目概要。

- **入参**：`project`
- **返回**（JSON 文本）：`name` / `root` / `format_version` / `entry_scene` /
  `window`（标题与尺寸）/ `runtime`（重力、固定步长，已过容错夹取 —— JSON
  里不会出现 NaN）/ `render`（`exposure` / `tone_map` / `bloom`
  {`intensity` / `threshold` / `knee`} / `sky` {`enabled` / `intensity` /
  `zenith` / `horizon` / `ground` / `sun_size` / `sun_intensity`} /
  `fxaa` {`enabled`} —— 画面设置
  真相源，
  `run_headless` 截的图按它渲染；泛光与天空数值按 f32 存储，JSON 里是精确加宽的
  f64，比较时按 f32 精度留容差）/ `scenes` / `scripts` / `prefabs` 清单。

### 3. `scene_read`

读场景真身。

- **入参**：`project`，可选 `scene`
- **返回**：场景的**完整 RON 文本**（与文件同构：同一序列化配置），可原样传回
  `scene_write`。没有摘要、没有私有格式 —— 智能体拿到的就是人类在编辑器里存的那份。

### 3a. `entity_components_read`

按场景内的实体 ID 查询当前挂载组件（只读），不按名字猜目标。先用 `scene_read`
取得 ID，尤其在同名实体并存时。

- **入参**：`project`、`entity_id`（**十进制字符串**，不能传 JSON 数字，否则超过
  2⁵³ 后可能丢失精度），可选 `scene`。
- **返回**（JSON 文本）：`scene` / `entity_id`（十进制字符串） / `entity_name` /
  `address_stability: "legacy"` / `components` 数组。每项包括 `address`、`kind`
  （`builtin` / `rust` / `lua`）、`type_id`、`name`、`data`。内建数据是当前字段的
  JSON 视图；Rust 数据是已保存的 `Inspect` 参数表（缺省为空对象，不含插件运行态）；
  Lua 数据只有脚本资产路径，不代表脚本的运行时变量。
- **地址规则**：`builtin:transform` 恒存在且不可移除；Render 由旧格式中的模型、LOD、
  材质资产及形变权重组合而成。其它已挂载内建组件使用 `builtin:camera` 等种类地址。
  Rust、Lua 使用 `rust:0`、`lua:0` 等列表槽位地址，**删除或重排后地址可能指向另一项，
  使用前重新查询**。当前尚无可跨重排引用的组件实例 ID，返回字段显式标记为 `legacy`。
- **边界**：该工具只读；添加和移除继续通过 Inspector 的 Add Component / 各组件移除
  控件完成。离线批量编辑可用 `scene_read` / `scene_write`，并遵守两端不能同时写场景
  的现有约定。

### 3b. `component_upgrade_preview`

预览 v1 实体将来升级时的组件身份，不落盘；可在动预制体资产之前检查来源关联。

- **入参**：`project`、`entity_id`（十进制字符串），可选 `scene`。
- **返回**：`status: "preview_only"`、实体 ID / 名称、可选预制体来源路径、
  `next_component_id`，以及每个已挂载物的旧地址、名称、拟分配的 `component_id`
  与可选 `source_component_id`（ID 全是十进制字符串）。来源 ID 由资产实体的组件
  种类、Rust 注册名或 Lua 路径匹配；本地重排不改变对应的来源 ID。
- **拒绝**：场景或来源预制体有重复实体 ID、旧实体同名 Rust 行为或同路径 Lua
  脚本、没有行为对应的参数，或来源文件 / 实体无法读取。预览 ID 尚未写入文件，
  不能拿它当稳定寻址 ID 修改组件；正式转换必须单独执行显式迁移。

### 4. `scene_write`

整体替换一个场景。

- **入参**：`project`，`ron`（完整场景文本），可选 `scene`
- **返回**（JSON）：`scene` / `path` / `entities` / `lights`
- **契约**：**先解析、后落盘** —— 坏 RON 返回 `isError`，一个字节都不写。
- **适用**：成批改结构（加实体、改层级、动灯光）；小改用下面的 `entity_upsert`。

### 5. `entity_upsert`

按名字创建或更新单个实体（**部分更新**）。

- **入参**：`project`，`name`（必填），可选 `scene` / `model` / `scripts` /
  `position` / `rotation_euler_deg` / `scale` / `parent` / `layer`
- **返回**（JSON）：`action`（`created` | `updated`）/ `name` / `scene` / `entities`
- **语义**：
  - 没传的字段保持原值；新建实体未指定的字段取默认；
  - 变换是**局部坐标**；`rotation_euler_deg` 是**度、XYZ 顺序**；
  - `parent` 按名字查找，空串 = 断开到根；**先挂父、再改变换**，所以你给的
    局部值就是最终值（挂父本身保世界坐标）；
  - 场景内该名字**重名多个 → 拒绝**（按名字寻址，歧义时猜一个迟早出事）。
  - `layer` 是 0..31 的层号（与 `project.ron` 的 `layers` 节、Inspector 的层下拉同一个
    数）：**越界或非整数直接拒绝**，不做取模（这里没有"手写 RON 的用户"需要兜底，
    而静默改值会让智能体以为设置生效了）。层决定"和谁碰"，默认 0 = 全通。

### 6. `entity_remove`

按名字删除单个实体。

- **入参**：`project`，`name`，可选 `scene`
- **返回**（JSON）：`action: "removed"` / 剩余实体数
- **语义**：不存在 → `isError`；重名多个 → `isError`（歧义）；**子节点不连坐**，
  会提升到被删实体的父节点下（层级不散架）。

### 7. `project_check`

无头验证项目（与编辑器 `xgame-editor --check` 是**同一份** `xgame-core::check_project`）。

- **入参**：`project`
- **返回**（JSON）：`scenes` / `entities` / `lights` / `script_refs` / `audio_refs` / `material_refs` /
  `assets` / `missing_models` / `missing_scripts` / `missing_audio` / `missing_prefabs` /
  `missing_materials` / `missing_material_textures` / `invalid_materials` /
  `invalid_layers`（实体层号越界，形如 `Scenes/X.ron: 实体 -> layer 40`）/
  `dangling_guids`（场景里 `guid://…` 查无此资产）/ `invalid_scenes`（非入口场景加载失败）/
  `missing_metas` / `orphan_metas` / `invalid_metas` / `guid_conflicts` / `has_issues` /
  `issue_count` / `render`（人读摘要）
- **场景范围**：入口场景 + `Scenes/` 下全部 `.ron`；`entities` / `lights` 是全部场景合计。
  入口场景坏掉 → 工具失败（对应 `--check` 退出码 1）；非入口场景坏掉 → `invalid_scenes`
  （退出码 2），其余场景照常检查。
- **只读契约**：它**不会**给缺 `.meta` 的资产生成身份证（那会让"检查一遍"改变项目）。
- **闭环用法**：改（3/4/5/6）→ `project_check` → `has_issues` 必须为 `false`。

### 8. `asset_list`

智能体的"资产树视野"：看清项目里有什么资产，再去写 `entity_upsert` 的 `model` /
`scripts` —— 不需要猜。

- **入参**：`project`，可选 `kind`（`Model` / `Texture` / `Audio` / `Script` /
  `Prefab` / `Material` / `Shader`，大小写不敏感）
- **返回**（JSON）：`count` / `assets`（每项 `path` / `kind` / `guid` / `import`）/
  `diagnostics`（`missing_meta` / `orphan_meta` / `invalid_meta` / `guid_conflicts` /
  `has_issues`）
- **只读**：与 `project_check` 同一条纪律，不写任何文件。
- **闭环用法**：`asset_list` 找模型路径 → `entity_upsert` 引用它 → `project_check` 验证。

### 9. `run_headless`

无头跑项目 N 帧（**透传**编辑器 `xgame-editor --headless`：spawn 同目录的编辑器
可执行文件，报告从它的 stdout 原样返回）。

- **入参**：`project`，可选 `frames`（默认 300，上限 100000）/ `dt`（默认 = 项目
  固定步长）/ `include_scene`（默认 `false`）/ `screenshot`（默认 `false`，
  需要 GPU）
- **返回**（JSON 信封）：`exit_code`（0 干净 / 2 跑完但有脚本 error 日志或非有限
  变换）+ `report`（帧耗时分布 `frame_us`、实体/刚体计数、脚本日志计数与
  `error_samples`、音频状态、`screenshot` 机位与绘制读数 `render_stats`……字段
  契约见 [`无头运行.md`](无头运行.md)）；`include_scene: true` 时另带
  `scene_ron`（**跑完后**的场景 RON，可读回、可 diff —— 观察物理落点 / 脚本
  改过的变换的出口）；`screenshot: true` 时信封带
  `{ "screenshot": { "bytes": n } }`，**PNG 本体以 MCP `image` 内容块（base64）
  随结果返回** —— 视觉模型直接看到画面。
- **语义**：每帧固定 dt、无输入、无墙钟依赖 —— 同一项目两次运行逐位一致
  （确定性回归的底座）。`report.ok` 为 `false` **不是工具失败**：问题表达在报告里
  （智能体读 `error_samples` 定位）；只有项目打不开（退出码 1）才是 `isError`。
- **截图的取景**：自动拟合全场 + 顺光机位（同 CLI 默认），实际机位写在报告的
  `screenshot` 节（`eye` / `target`，可复现）；绘制读数在
  `screenshot.render_stats`（`draw_calls` / `culled_instances` /
  `shadow_only_instances` 等）—— 自动取景不会剔东西，`culled_*` 非零说明画面
  里确实少画了（或用了自定义机位）。只有一张图，不给相机参数开洞。
- **闭环用法**：改（3/4/5/6）→ `project_check`（静态引用）→ `run_headless`
  （动态实跑）→ 看 `report.ok` 与 `error_samples`；调了渲染/材质再看
  `screenshot` 的图。

### 10. `script_api_read`

写第一个 `.lua` 之前先读它：脚本 API 的**机器可读清单**。

- **入参**：无（脚本契约属于引擎，不属于某个项目 —— 不需要 `project`）
- **返回**（JSON 文本）：与 `xgame-editor --export-script-api` **完全同源**
  （`xgame_core::script_api::script_api`），含 `format_version` /
  `engine_version` / `conventions` / `hooks` / `value_types` / `categories`。
- **内容规模**：160 个全局函数、17 个类别、6 个生命周期钩子、7 个值类型。
  每个函数有 `name` / `signature` / `params` / `returns` / `summary`；每个类别有
  稳定的 ASCII `id`（如 `transform` / `physics` / `ui`）与中文名。
- **确定性**：没有时间戳 / 随机 ID；函数与类别按 [`脚本API.md`](脚本API.md) 的
  顺序排列，同一份代码两次调用逐字节相同。
- **防漂移**：`xgame-script` 从 `lua_api.rs` / `ui_api.rs` 的 `globals.set("…")`
  提取注册名并逐名比对；`xgame-editor` 从 `Docs/脚本API.md` 的 API 表提取函数名
  再比对；运行时还确认每个名字都是 Lua 函数。改注册不同步清单，测试立刻红。
- **闭环用法**：`script_api_read` 拿函数签名 / 返回值 → 写 `Scripts/*.lua` →
  `project_check`（脚本引用）→ `run_headless`（脚本真的跑得动、看 Console 日志）。

### 11. `asset_import`

给**单个**资产生成身份证（`.meta` + 新 GUID），并可选地写入导入设置。

- **入参**：`project`、`path`（项目内相对路径；反斜杠会归一成 `/`），可选
  `model_scale` / `texture_srgb` / `texture_nearest` / `audio_streaming`
- **返回**（JSON）：`path` / `kind` / `guid` / `created`（这次是否新建了 `.meta`）/
  `changed`（设置是否真的变了）/ `import`（**落盘后的实际值**）；`model_scale`
  被钳过时另带 `model_scale_requested` + `model_scale_clamped`
- **语义**：
  - **不给导入参数 = 只补 `.meta`**，幂等 —— 已有合法 `.meta` 就原样返回，不换 GUID，
    `changed` 为 `false`；
  - **给了参数 = 部分更新**：只覆盖传了的字段，其余保持原值（只调缩放不会冲掉
    用户改过的 `texture_srgb`）；
  - **只碰点名的那个资产**：不像编辑器打开项目时那样给整个资源目录补 `.meta` ——
    一次点名就该是一次范围明确的写操作；
  - `model_scale` 非有限值直接**拒绝**（静默退回 1.0 会让智能体以为设置生效了）；
    有限但越界（`[0.001, 1000]`）交给 core 钳制，两个值都在返回值里；
  - 场景（`.ron`）不归资产库管，拒绝。
- **闭环用法**：`asset_list` 看见资产 → `asset_import` 调导入参数 →
  `project_check` 自证引用完整。

### 12. `asset_move`

移动 / 改名一个资产，**资产与 `.meta` 一起走，GUID 保持不变**。

- **入参**：`project`、`from`、`to`（都是项目内相对路径）
- **返回**（JSON）：`from` / `to` / `guid` / `kind` / `references_preserved`
- **为什么不需要改引用**：磁盘上的场景 / 预制体存的是 `guid://…`，GUID 没变就
  天然不断链 —— 这条工具只搬两个文件，**不碰任何引用文件**。
- **拒绝**（全部在动盘之前判定）：源没登记（缺 `.meta`，先 `asset_import`）/
  目标已存在（绝不覆盖）/ 改变资产类型（`.glb` → `.png`）/ 目标是场景（场景按
  路径被 `project.ron` 引用，搬它会断入口与发布清单）/ 绝对路径 / `..` / 构建
  缓存目录。目标目录会自动创建；`.meta` 搬失败时资产文件会回滚。
- **闭环用法**：`asset_move` 整理目录 → `project_check` 的 `dangling_guids` 必须
  为空。注意编辑器里**已打开**的场景内存中仍是旧路径，重新打开该场景再保存。

---

## 已知边界（写给正在用它的智能体）

- **并发写**：编辑器每秒轮询内容目录，资产 / 脚本 / `.meta` 的外部改动会**自动热
  重载**；但**场景文件不会自动重载**（防冲掉未保存的编辑），编辑器之后保存也会覆盖
  MCP 的修改。**同一时刻只让一边写同一个场景**。资产被移动后，编辑器里已打开的
  场景内存中仍是旧路径 —— 重新打开该场景再保存。
- **MCP 写入对其他 handle 不可见**：server 每次调用新开一个 `ProjectHandle`（独立
  内存索引）。同进程内的另一个 handle 要 `refresh_assets()` / 重新打开才看得见工具
  写到盘上的 `.meta`（编辑器侧由 1 Hz 扫描负责）。
- **能读不能写的清单**：`project.ron` 的修改（改入口场景、窗口尺寸……）目前请走
  直接编辑文件；MCP 写工具只覆盖场景与实体。
- **构建**：`project_build`（打包 / 编译插件）在计划里，还没上；无头运行已就绪
  （工具 `run_headless` / CLI `xgame-editor --headless`）。
- **schema 的边界**：`schema_read` 只描述**场景 / 预制体落盘格式**；脚本 API 用
  `script_api_read`（参数 / 返回值 / 约定以清单为准，叙述与示例见
  [`脚本API.md`](脚本API.md)）。schema 也不是 JSON Schema —— 它是引擎自己的领域
  目录（默认值是可解析的 RON 示例）。
- **破坏半径**：`scene` 参数拒绝 `..` / 绝对路径，写入止于项目目录内；重名歧义
  一律拒绝而不是"猜一个"。

---

## 实现速查

| 文件 | 职责 |
|---|---|
| `crates/xgame-mcp/src/lib.rs` | 协议层：`handle_line()`（喂行进、断言行出）、握手、两层错误 |
| `crates/xgame-mcp/src/tools.rs` | 15 个工具的 Schema 与实现、路径安全、重名歧义检查 |
| `crates/xgame-mcp/src/main.rs` | stdin/stdout 管道（只有这一层碰 I/O） |
| `crates/xgame-core/src/schema.rs` | 场景格式目录 + 确定性示例 + 防漂移测试（`--export-schema` 与 `schema_read` 共用的唯一内容源） |
| `crates/xgame-editor/src/schema_cli.rs` | `--export-schema` 的参数解析、JSON 序列化、stdout / `--out` 出口 |
| `crates/xgame-core/src/script_api.rs` | 脚本 API 清单（182 函数 / 18 类 / 7 钩子 / 值类型 / 约定）+ 确定性数据 + 防漂移测试（`--export-script-api` 与 `script_api_read` 共用的唯一内容源） |
| `crates/xgame-editor/src/script_api_cli.rs` | `--export-script-api` 的参数解析、JSON 序列化、stdout / `--out` 出口；文档函数名比对 |
| `crates/xgame-core/src/check.rs` | `check_project()`（编辑器 `--check` 与 `project_check` 共用） |
| `crates/xgame-core/src/asset.rs` | `AssetGuid` / `AssetMeta` / `AssetDatabase`（`asset_list` / `asset_import` / `asset_move` 与 `--check` 的资产诊断都读它） |
| `crates/xgame-editor/src/headless.rs` | 无头运行的实现（CLI `--headless`；`run_headless` 工具 spawn 它） |

回归测试：`cargo test -p xgame-mcp`（协议、工具行为与信封；
其中 `run_headless` 的几条会嵌套 `cargo build -p xgame-editor` 保证被测 exe 新鲜，
再经真实进程 spawn 跑通）。`schema_read` 另有 `xgame-core` 的 10 条目录测试与
`xgame-editor` 的 5 条 CLI 测试钉住内容与确定性；`script_api_read` 另有
`xgame-core` 的 6 条清单测试、`xgame-editor` 的 6 条 CLI / 文档比对测试，以及
`xgame-script` 的 1 条“注册名逐名一致 + 运行时都是函数”测试。
