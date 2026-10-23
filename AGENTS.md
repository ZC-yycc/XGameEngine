# XGameEngine — Agent 指南

Rust 实现的类 Unity 游戏引擎（wgpu 渲染 + egui 编辑器 + rapier 物理 + 脚本 Lua + Rust 组件插件）。

## 必读文档（按顺序）

1. `ROADMAP.md` — 阶段状态总表（先看「整体路线图」；只记状态，不记理由）
2. `Docs/差距分析.md` — 与 Unity/UE/Godot 的能力差距 + **优先级排序**（唯一优先级真相源）
3. `Docs/决策记录.md` — **改任何子系统之前先查这里**：不可推翻的约束、踩过的坑、实测数据
4. `PLAN.md` — 历史各轮的实施记录（含每个决策的完整推理与取舍；要查"当时为什么"看它）
5. `Docs/双轨语言架构计划.md` — 脚本层决策记录 D1–D11 的**原文**（P0–P3 已完成，仅作决策档案）
6. `FLOW.md` — 运行流程（启动 / 每帧 / 构建 / 数据流 / 关键文件速查）

## 常用命令

```sh
cargo check --workspace --all-targets        # 检查（先跑这个）
cargo clippy --workspace --all-targets       # lint（要求 0 警告）
cargo test --workspace --all-targets         # 测试（要求全绿；799 条 + 7 条基准 ignore）
cargo run -p xgame-editor                    # 编辑器（先弹项目枢纽）
cargo run -p xgame-editor -- --check projects/SampleScene       # 无头检查（退出码 0/1/2）
cargo run -p xgame-editor -- --export-schema --out schema.json  # 场景格式说明书（JSON；无 --out 打 stdout）
cargo run -p xgame-editor -- --migrate-assets projects/SampleScene  # 老项目迁移到 GUID 引用（生成 .meta + 改写场景/预制体）
cargo run -p xgame-editor -- --headless --frames 300 projects/SampleScene  # 无头运行（JSON 统计）
cargo run -p xgame-editor -- --headless --frames 150 --screenshot shot.png projects/SampleScene  # 无头截图（需 GPU；默认顺光取景，同机逐字节确定）
cargo run -p xgame-game -- projects/SampleScene  # 独立游戏
cargo test -p xgame-runtime --release --test update_throughput -- --ignored --nocapture  # 分档性能基准（默认 ignore）
cargo test -p xgame-render --release --test culling_cost -- --ignored --nocapture  # 视锥剔除开/关 A/B 基准（默认 ignore）
```

发布打包：双击 `一键构建.cmd`（产物在 `dist/`）。

## 架构速记

- **crate 依赖方向**（不许反向依赖）：`core ← math ← {render, physics}`；`input`/`script` 在中层；`runtime`（World 编排）在上；`editor`/`game` 是两个宿主。
- **编辑器 Play 与独立游戏跑同一段 `World::update`**（`xgame-runtime`）——改运行时行为两端自动一致；不要在 editor/game 里复制运行时逻辑。
- 脚本层（`xgame-script`）：`LuaHost`（`lua_host.rs`）管编译缓存/生命周期派发，**单轨只认 `.lua`**（Rhai 已随双轨 P3 删除，见 D11）；API 真相源是 `GameCtx` 方法层（`ctx.rs`；`lua_api.rs` 只是每函数一行的转发壳）；`host.rs` 是宿主公共类型模块（`Hook` / `PreloadReport` / `CommandReport`）；`ScriptCtx` 是地址束（unsafe 有严格调用窗口约定，见 `context.rs` 模块文档，别破坏该约定）；路径驻留表 `intern.rs` 的 id **永不清**（热重载只清编译缓存，见 D10）；每脚本环境表的 `__index` 必须挂**真元表**（D10 ①）。
- 场景格式 RON，纯文本、可 diff；新字段一律 `#[serde(default, skip_serializing_if = ...)]` 保证旧文件兼容。
- **资产引用（P0 资产管线）**：磁盘上存 `guid://…`，**内存里存项目相对路径**。转换只在
  `ProjectHandle::load_scene` / `write_scene` / `load_prefab` / `write_prefab`（`core::asset`
  的 `encode_*` / `decode_*`）。别把 GUID 往热路径或 243 个消费点上引；也别让 `.meta` 生成
  混进 `--check`（检查必须只读）。规则见 `Docs/决策记录.md` §十四。
- **场景 schema**：`xgame-core::schema::scene_schema()` 是 `--export-schema` 与 MCP
  `schema_read` 的**唯一内容源**（别在编辑器 / MCP 里再抄一份）。目录是手写的，靠
  **穷举解构 + 枚举穷举匹配 + 默认值 / 示例反解 + 类型引用校验 + ProjectHandle 往返**
  钉住；给落盘类型加字段必须同步 `schema.rs`，否则测试编译不过。输出必须确定性
  （无时间戳 / 随机 ID；示例里的 `EntityId` 固定）。规则见 `Docs/决策记录.md` §三十二。
- **编辑器热重载**：`crates/xgame-editor/src/hot_reload.rs` 每秒只扫内容目录的 metadata；
  改动由 `EditorState::hot_reload_tick` 应用，**动作后必须 `rebase`**（自己补的 `.meta`
  不能当外部改动，否则每秒自激）。场景文件永不自动覆盖；`--ui-shot` 不启用。
  规则见 `Docs/决策记录.md` §三十一。

## 代码风格约定

- **注释与文档写中文**，且遵循本仓库的风格：解释"为什么"而不是"是什么"；重要的取舍、别推翻的决策要写明理由（参考 `PLAN.md` 各轮的「刻意的取舍」小节）。
- 回归测试必须先证明它在旧实现下会红（仓库惯例，见 PLAN.md 第十节）。
- 性能改动"先量后改"，量数据写进记录。
- clippy 0 警告是硬要求。
- `一键构建.cmd` 保持 ASCII-only（cmd.exe GBK 解码问题）。

### 编辑器 UI 约定（2026-10-19 起）

- **尺寸与颜色只从 `crates/xgame-editor/src/panels/theme.rs` 取**：行高 22 / 控件 20 /
  分类条 22 / 工具栏 34 / 页签 26 / 状态栏 24 / 内边距 6 / 标签列 58 / 轴标 9 /
  数值框 54。面板里不写魔法数、不就地 `Color32::from_rgb`（否则下次调风格要全局翻）。
- **图标一律矢量绘制**（`panels/icons.rs`），禁 emoji / 字形图标 —— 字形宽度与基线
  不一致，是"图标对不齐"的根源。
- 行用 `theme::field_row`（Inspector 用本地 `row`，它会补轴标占位）；长中文说明用
  `theme::note`（自己排 galley），别用裸 `ui.label` 画长句。
- **`allocate_ui_with_layout` 只按子 ui 的 `min_rect` 占位**：要定宽就必须在子 ui 里
  `ui.set_min_width(w)`，否则列宽 = 内容宽，值列会左右乱跳。
- **三轴并排的宽度必须算，不能写死**：用 `theme::axis_field_width(ui)`，且必须在放
  第一个控件**之前**取一次、三行共用同一个值（放完一个再算会越算越窄，列就错开）。
  写死 `FIELD_WIDTH` 的话 1280 窗口下 Z 框会顶出面板右边缘。
- **`add_sized` 只保证"至少这么大"**：控件自己的 `min_size` 更大时会溢出。数值框走
  `theme::drag_field_w`，别绕过它。
- **运行态（Play/Pause）的界面语义**：`scene_dirty_memo` 恒为 false（运行时副本不算
  改动）、`save_scene` / `save_scene_as` 硬拒绝、Pause/Stop 用 `add_enabled_ui` 禁用。
  任何"只在编辑态有意义"的新按钮都要照这三条办。
- **`--ui-shot` 的八个状态开关**：`--ui-select` / `--ui-settings` / `--ui-play` /
  `--ui-profiler` / `--ui-preferences` / `--ui-restore-prefs` / `--ui-autosave-recover` /
  `--ui-size WxH`。改 UI 必须截图自检，配 `.workbuddy/tools/crop.py`（局部放大）与
  `rowscan.py`（按行扫颜色分段量像素）。**用 `--ui-size` 至少扫 1280 / 1440 / 1920
  三档** —— 窄窗口的溢出只有跑过才看得见。截图默认**只读**：不写 `editor.ron`、
  不写 `.autosave/`、不弹恢复框（磁盘残留会让像素不确定）。

## 项目布局

- `crates/` — 引擎与工具（15 个 crate，名字见 workspace Cargo.toml）
- `projects/` — 游戏项目（`project.ron` + `Scenes/` + `Resources/` + `Scripts/` + `Prefabs/` + `input.ron`）；
  `BehaviorsDemo` 例外——它本身是 workspace 成员（cdylib），用来演示 Rust 插件项目形态
- `Docs/` — 脚本 API、无头运行、MCP、差距分析、决策记录、双轨语言架构计划
- `dist/` — 发布产物（构建输出，不入库改动）
- `tools/` — 构建脚本（build-release.ps1 等）

### 文档分工（新增内容只写一遍）

- **能力状态** → `ROADMAP.md`（只写阶段 / 勾选 + 链接，不写理由）
- **不可推翻的约束 / 踩过的坑 / 实测数据** → `Docs/决策记录.md`
- **差距与优先级** → `Docs/差距分析.md`（唯一排序真相源）
- **"当时为什么这样做"** → `PLAN.md`（历史记录，允许过时）
- **用户怎么用** → `README.md`；**运行流程** → `FLOW.md`；**脚本 API** → `Docs/脚本API.md`

发现同一件事在两处出现，删掉旧的那处并留链接，别让两份一起漂移。

## 约束索引（详细见 `Docs/决策记录.md`）

| 子系统 | 一句话红线 | 详情 |
|---|---|---|
| 脚本层 | 单轨 `.lua`；环境表挂真元表；驻留 id 永不清；API 名不改 | 决策记录 §二、§三 |
| 插件 / 组件 | `Plugins/` 唯一目录；临时副本加载；旧 DLL 句柄永不卸载；参数键用注册名 | §四 |
| 音频 | 脚本层不引 kira；监听者/主音量不进命令束；`follow` 是实体绑定；每渲染帧同步 | §五 |
| 无头 / MCP | 不做第二份帧循环；退出码 2 不是工具失败；JSON 字段名是契约 | §六 |
| PBR / 材质 | `intensity × π` 亮度标定；材质逐 draw；截图顺光；`MIN_ROUGHNESS = 0.04` | §七 |
| 材质资产（`.mat`） | 资产是完整材质（替换 PBR，albedo 空 = 纯白）；优先级 模型 → 资产 → 覆盖；批次键必须带材质 | §十七 |
| 编辑器偏好 / 布局 | 截图默认只读出厂值；坏布局回退出厂；七个面板缺一不可；写盘只在松手 / 关闭 / 退出 | §二十一 |
| HDR / Bloom | 亮度基线不动；Bloom 阈值在曝光后；光晕在曲线前；默认关且整段跳过 | §八 |
| 天空 / IBL | 默认关；资源固定分配按 key 重建；太阳 = 第一盏方向光；IBL 与 ambient 相加 | §九 |
| 实例化 | 分组键 = 「图元 + 材质 key」；顺序 = 首次遇到；半透明不合批；材质组收集阶段预热 | §十 |
| 剔除 | 保守性硬合同（开/关逐位一致）；相机外投影物保留；蒙皮 50% 余量 | §十一 |
| FXAA | 默认关；显示空间 luma；`textureSampleLevel(..., 0.0)`；平坦区回原值 | §十二 |
| TAA | 默认关；抖动**只**叠相机（阴影拟合 / 剔除用未抖动矩阵）；重投影要补上一帧抖动；历史必须被邻域钳制；首帧 / 越界直通；抖动是纯函数（截图确定性）；开着时 FXAA 让位 | §二十九 |
| 资产管线 | 磁盘 GUID / 内存路径；GUID 永不变；`--check` 只读；迁移只替换字面量（不吃注释） | §十四 |
| 场景 schema | 唯一源 `scene_schema()`；CLI / MCP 同源；输出确定性；运行时字段不进目录；加落盘字段必须同步目录 | §三十二 |
| 自动保存 | 副本只在 `<项目>/.autosave/`（进 `Scenes/` 会被当场景）；**不算保存**（不动 `saved_snapshot`）；手动保存 / 正常退出即清；写盘格式与 `write_scene` 同源 + 原子写；空闲停帧时靠 `request_repaint_after` 唤醒 | §二十四 |
| 场景切换 | 只在**帧末**一处应用（`apply_pending_scene_load`）；脚本 `load_scene` 只登记请求（钩子期间场景正被可变借用）；装不上就**不动** + 一条 error 日志；只重置"与实体绑定"的状态（物理 / 粒子 / 音频 autoplay / UI 交互 / `started` / `states`），**日志缓冲与脚本全局变量不能清**；装载来源由宿主注入 `with_scene_loader`，没接是**可诊断的错误** | §二十五 |
| 关卡顺序 | `build.scenes` 顺序即关卡索引（下标 **0 起**）；**未配置 = 空**（不退回 `[entry_scene]`，脚本 `scene_index()` 返回 nil）；`entry_scene` 仍是唯一启动来源，清单第一项不兼任；`--check` 里"缺场景"算问题（exit 2）、"未列入"只提示（exit 0）；构建前逐关试装，路径规则只有 `normalize_scene_relative`（core） | §二十六 |
| 预制体实例化 | 装载 trait `SceneLoader` 在 **core**（脚本层要能命名它，runtime 只 `pub use` 转出）；`spawn_prefab` / `spawn_prefab_under` **立即生成**（返回的 ID 当帧可用）；位置是**设置不是叠加**（世界 / 局部两种，子节点相对位置原样保留）；每个新实体逐条推 `ScriptCommand::EntitySpawned`（运行时据此补刚体）；失败三种（没装载器 / 读不到 / 空预制体）各自一条日志 + 返回 `nil`，**不留半个实例** | §二十七 |
| 动画脚本控制 | 状态机优先（配了可用机器时 `clip/speed/looping` 由状态表决定）；`set_anim_clip` 只写 `Animator.clip` 并复用 `AnimationDriver` 的淡入；叠加层 / 层剪辑硬切；越界值钳住、非有限值忽略 | §三十 |
| 热重载 | 1 Hz 轮询内容目录（只查 metadata）；`poll` 推进基线、**动作后必须 `rebase`**（防自激）；场景永不自动覆盖；脏材质草稿保留；`--ui-shot` 不启用 | §三十一 |
| 性能改动 | 先量后改；同机背靠背；基准必须断言"真的跑了"；不为机器上调验收线 | §十三 |
