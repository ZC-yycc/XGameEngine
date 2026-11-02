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
cargo test --workspace --all-targets         # 测试（要求全绿；1182 条 + 17 条基准 ignore）
# CI 在 GitHub Actions（windows-latest）跑 check / clippy / test + 示例项目冒烟，
# 命令带 --locked；workflow 见 .github/workflows/ci.yml，取舍见决策记录 §三十四。
cargo run -p xgame-editor                    # 编辑器（先弹项目枢纽）
cargo run -p xgame-editor -- --check projects/SampleScene       # 无头检查（退出码 0/1/2）
cargo run -p xgame-editor -- --export-schema --out schema.json  # 场景格式说明书（JSON；无 --out 打 stdout）
cargo run -p xgame-editor -- --export-script-api --out script_api.json  # 脚本 API 清单（179 函数 / 6 钩子；无 --out 打 stdout）
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
- **落盘只有一个入口**：`xgame_core::fsx`（`write_atomic` / `write_atomic_new` /
  `sweep_staging`）。别再在别的 crate 里抄一份"同目录暂存 + rename"——这条坑已经
  踩过三次，三份副本的差错点各不相同。暂存名 `<主名>.<扩展名>.tmp.<进程号>.<序号>`
  同时是**回收判据**，改命名必须同步 `is_staging_name`；回收挂在保存成功之后，
  绝不挂在 `ProjectHandle::open`（那里是 `--check`，必须只读）。规则见
  `Docs/决策记录.md` §四十九。
- **场景 schema**：`xgame-core::schema::scene_schema()` 是 `--export-schema` 与 MCP
  `schema_read` 的**唯一内容源**（别在编辑器 / MCP 里再抄一份）。目录是手写的，靠
  **穷举解构 + 枚举穷举匹配 + 默认值 / 示例反解 + 类型引用校验 + ProjectHandle 往返**
  钉住；给落盘类型加字段必须同步 `schema.rs`，否则测试编译不过。输出必须确定性
  （无时间戳 / 随机 ID；示例里的 `EntityId` 固定）。规则见 `Docs/决策记录.md` §三十二。
- **脚本 API 清单**：`xgame-core::script_api::script_api()` 是 `--export-script-api` 与
  MCP `script_api_read` 的**唯一内容源**。清单是手写的（Lua C 函数没有参数名反射），
  但 `xgame-script` 从 `lua_api.rs` / `ui_api.rs` 的 `globals.set("…")` 提取注册名
  逐名比对，`xgame-editor` 从 `Docs/脚本API.md` 的 API 表提取函数名比对；改注册 /
  文档必须同步清单。规则见 `Docs/决策记录.md` §三十三。
- **层与碰撞（`core::layers`）**：`Layers` 只存**覆盖**（`names` / `masks` 两张
  `BTreeMap`），默认全通 ⇒ 老 `project.ron` 逐位不变；`collides()` 是**两侧相与**
  （与 rapier 同语义），`set_collides()` **对称写两侧**。改层必须走
  `World::set_entity_layer()`（写场景 + 立刻 `physics.set_body_groups`）——
  等 `sync_bodies` 就是永远不生效（早退判据里没有"层"）。射线掩码用
  `QueryFilter::predicate` 只看**成员位**、不看矩阵（Unity `layerMask` 语义）；
  角色控制器例外，它的 `QueryFilter.groups` 必须带自身组。规则见
  `Docs/决策记录.md` §三十六。
- **形变目标（morph target）**：`crates/xgame-render/src/morph.rs`。**CPU 变形 + 顶点缓冲
  按需重写**（不是 GPU storage 累加）：权重变了才 `write_buffer`，没变零开销。增量数据
  target-major 平铺且**已乘 `model_scale`**；权重优先级 = **剪辑权重轨道 > `Entity.morph_weights`
  > 模型默认**（动画驱动的属性不听手动覆盖，Unity 同款）。图元没有 `morph` 就是静态网格,
  不要为它留空分支。规则见 `Docs/决策记录.md` §三十七。
- **编辑器热重载**：`crates/xgame-editor/src/hot_reload.rs` 每秒只扫内容目录的 metadata；
  改动由 `EditorState::hot_reload_tick` 应用，**动作后必须 `rebase`**（自己补的 `.meta`
  不能当外部改动，否则每秒自激）。场景文件永不自动覆盖；`--ui-shot` 不启用。
  规则见 `Docs/决策记录.md` §三十一。

## 任务收尾约定

- 每次任务完成后必须提交改动并推送到远端；仅保留本地改动或本地 commit 不算完成交付。
- 验证、发布或推送遇到阻塞时如实记录结果与未完成项，不把代码已推送等同于发布验收通过。

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
- **`--ui-shot` 的十一个状态开关**：`--ui-select` / `--ui-settings` / `--ui-play` /
  `--ui-profiler` / `--ui-preferences` / `--ui-command` / `--ui-restore-prefs` /
  `--ui-autosave-recover` / `--ui-ortho` / `--ui-source <路径>:<行号>` / `--ui-size WxH`。改 UI 必须截图自检，配 `.workbuddy/tools/crop.py`（局部放大）与
  `rowscan.py`（按行扫颜色分段量像素）。**用 `--ui-size` 至少扫 1280 / 1440 / 1920
  三档** —— 窄窗口的溢出只有跑过才看得见。截图默认**只读**：不写 `editor.ron`、
  不写 `.autosave/`、不弹恢复框（磁盘残留会让像素不确定）。

## 项目布局

- `crates/` — 引擎与工具（15 个 crate，名字见 workspace Cargo.toml）
- `projects/` — 游戏项目（`project.ron` + `Scenes/` + `Resources/` + `Scripts/` + `Prefabs/` + `input.ron`）；
  `BehaviorsDemo` 例外——它本身是 workspace 成员（cdylib），用来演示 Rust 插件项目形态
- `Docs/` — 脚本 API、无头运行、MCP、差距分析、决策记录、双轨语言架构计划
- `.github/` — GitHub Actions（CI workflow）
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
| 脚本沙箱 | 两半：指令预算（20 万条/钩子）挡死循环，**内存上限 128 MiB/VM** 挡无限增长；超限**只毁掉那一次调用**（VM 不许报废）；报错必须写「内存超限」而不是 Lua 原文 `not enough memory`；设不上限只警告，不许拖垮引擎 | §六十 |
| 钩子里的结构改动 | 钩子里 `spawn`/`destroy` 对 Scene **立即生效**、命令**下一帧**才结算；链式销毁深达 8 环也要求每环**恰好一次**且按序；`unload_scene` 期间 spawn 只属于正在拆掉的旧世界，**不许漏进下一场景** | §五十七、§六十一 |
| 日志（控制台 / `Editor.log`） | 唯一后端 `xgame-editor::logging`：一条 `log::warn!` 同时去 stderr（`RUST_LOG` 语义不变）/ 控制台有界队列 / `logs/Editor.log`；收信策略「自家 info + 所有人 warn/error，第三方 info 与 debug/trace 不收」不许松（落档网比 stderr 宽是刻意的）；时间戳是**启动以来秒数**不是本地时钟；面板筛选只影响显示、`Clear` 才清消息；右侧一栏必须 `main_wrap = false`（默认 true 会在 1280 折行） | §六十二 |
| 视口导航（F / 数字键） | F 只动位置**不改朝向**（包围球装框，半径下限 0.25 / 边距 1.15）；数字键轴视角只转朝向**不动位置**、**不是正交投影**（别混着叫）；`PITCH_LIMIT = π/2 − 0.01`防 `forward × Y` 退化出 NaN；导航键在算 view_proj **之前**；快捷键必须过 `wants_keyboard_input` | §六十三 |
| 路径（场景 / 资产 / 预制体 / 清单 / 引用） | 判定只有 `core::normalize_project_relative` **一处**（越界 + 冒号 / 结尾空格与点 / 控制字符 / Windows 保留设备名）；读写两个边界（`load_*` / `write_*`）都在碰盘前校验；`--check` 用 `unsafe_paths` 点名并进退出码 2（标签纯 ASCII、不与"文件缺失"重复计）；从**名字**生成文件名走 `sanitize_dir_name`（清洗），从**输入**拿路径走 `normalize_*`（拒绝） | §六十五 |
| 写读稳定性 | `save → load → save` **第二轮起**必须逐字节不变（`tests/scene_roundtrip.rs` 五条门禁，进 CI）；保存**会吃掉注释**是已知取舍（要保注释走 `--migrate-assets`）；待写文本与盘上相同时**不写盘**（mtime 是热重载判据）；给 `Entity` 加字段时 `PartialEq` 的穷尽解构会逼你决定它是否进脏检查 | §六十六 |
| 存档（SaveGame） | 存"状态覆盖层"：实体**局部**变换 + 刚体速度 + 全局 / 每实体变量；**不存** `state_*` / 时钟 / 生成物；读档**替换**不合并、对不上的实体跳过并计数、存档场景不同先切场景；实体变量跟实体走（切场景 / 销毁 / 读档后只留当前场景存在的 ID）；写前体检不合法即拒；存 / 读是帧末请求；读档实体查找**必须一次建索引**（`find_entity` 是线性的，逐条调就是 O(n²)） | §六十八 |
| 运行时读路径 / `search_paths` | 模型、脚本与音频的读取只有 `core::resolve_read_path` 一处（先 `normalize_project_relative`，再直接命中，再按 `search_paths` 兜底）；`search_paths` 是**读的兜底根**（有序、条目自身也要合规、不合规运行时跳过且 `--check` 报），只影响读不影响写；GPU 模型 / CPU 动画 / `LuaHost` / `AudioEngine` / `--check` / 打包必须拿到**同一份**列表 | §六十九 |
| 性能 / 物理 | `rapier3d` 必须开 `simd-stable`（稀疏动态 -36% / 密集接触 -6.5%）；代价是跨版本浮点不保证（同版本逐位确定性仍成立）；`World::update` 的五次全实体扫描**刻意不优化**（24 ns/实体，已在两万 FPS 上限）；`parallel` 特性不启用（打乱体间顺序） | §六十七 |
| 插件 / 组件 | `Plugins/` 唯一目录；临时副本加载；旧 DLL 句柄永不卸载；参数键用注册名 | §四 |
| 音频 | 脚本层不引 kira；监听者/主音量不进命令束；`follow` 是实体绑定；每渲染帧同步。总线是**固定三组**（`Sfx`/`Bgm`/`Voice`），有效音量 = 声源 × 总线（主音量在 kira 主轨）；**混音默认必须是 1.0**（0 = 全局静音，这个坑踩过一次）；总线音量变了才逐句柄重设音量 | §五、§七十一 |
| 无头 / MCP | 不做第二份帧循环；退出码 2 不是工具失败；JSON 字段名是契约 | §六 |
| PBR / 材质 | `intensity × π` 亮度标定；材质逐 draw；截图顺光；`MIN_ROUGHNESS = 0.04` | §七 |
| 材质资产（`.mat`） | 资产是完整材质（替换 PBR，albedo 空 = 纯白）；优先级 模型 → 资产 → 覆盖；批次键必须带材质 | §十七 |
| 编辑器偏好 / 布局 | 截图默认只读出厂值；坏布局回退出厂；七个面板缺一不可；写盘只在松手 / 关闭 / 退出 | §二十一 |
| HDR / Bloom | 亮度基线不动；Bloom 阈值在曝光后；光晕在曲线前；默认关且整段跳过 | §八 |
| 天空 / IBL | 默认关；资源固定分配按 key 重建；太阳 = 第一盏方向光；IBL 与 ambient 相加 | §九 |
| 实例化 | 分组键 = 「图元 + 材质 key」；顺序 = 首次遇到；半透明不合批；材质组收集阶段预热 | §十 |
| 剔除 | 保守性硬合同（开/关逐位一致）；相机外投影物保留；蒙皮按当前姿态逐关节盒，50% 绑定盒只作兜底 | §十一、§八十八 |
| 遮挡剔除 | 主通道后查询分组 AABB；只复用相机 / 实例 / 骨骼 / 批次代际 / 包围盒**逐位相同**的历史；待定 / 失败 / 主通道 morph 不透明批次 / 低成本 / 低收益一律完整绘制；小尾段与相机在盒内恒可见；主相机结果不进阴影 pass；重映射走 instance-rate `Uint32` 顶点流，`first_instance=0` | §八十七 |
| 质量档位 | 只管"机器向"三颗螺丝（MSAA / 阴影默认分辨率 / LOD 缩放），**不碰**画面风格；采样数只有 **1× / 4×**（2×/8× 非 wgpu 保证档，硬开会管线创建失败）；没配 `quality` 节 = 今天的行为；灯写了 `shadow_resolution` 以灯为准；LOD 阈值 = `render.lod_bias × quality.lod_scale`；MSAA 启动期定，运行期换档要重建管线 | §七十二 |
| LOD / 绘制距离 | `lod.levels` 非空时 `model` **被忽略**（第 0 级 = 最近级）；距离从相机量到**实体轴心**（不是包围盒）；超出全部有限级 = 不画且**不投影**；批次必须记级号（`DrawBatch::lod_level`，不同级是不同网格）；各级模型进 GUID 管线且 `--check` 逐级点名；`render.lod_bias <= 0` = 关掉 LOD | §七十 |
| FXAA | 默认关；显示空间 luma；`textureSampleLevel(..., 0.0)`；平坦区回原值 | §十二 |
| TAA | 默认关；抖动**只**叠相机（阴影拟合 / 剔除用未抖动矩阵）；重投影要补上一帧抖动；历史必须被邻域钳制；首帧 / 越界直通；抖动是纯函数（截图确定性）；开着时 FXAA 让位 | §二十九 |
| 资产管线 | 磁盘 GUID / 内存路径；GUID 永不变；`--check` 只读；迁移只替换字面量（不吃注释） | §十四 |
| 写盘 / 暂存 | 落盘只有一份实现 `xgame_core::fsx`；暂存名即回收判据（改命名必须同步 `is_staging_name`）；回收只在保存成功后（打开项目是 `--check` 的入口，必须只读）；Windows 无目录 fsync | §四十九 |
| 打包 / 播放器 | 只认**源码指纹一致**的播放器（`xgame-src-v1:…`，不含 profile）；`#[used]` 锚点不能删，删了常量会被链接器回收、门禁永远报旧引擎；插件门禁用另一份含 profile 的 `xgame-build-v1:…` | §五十 |
| 场景 schema | 唯一源 `scene_schema()`；CLI / MCP 同源；输出确定性；运行时字段不进目录；加落盘字段必须同步目录 | §三十二 |
| 自动保存 | 副本只在 `<项目>/.autosave/`（进 `Scenes/` 会被当场景）；**不算保存**（不动 `saved_snapshot`）；手动保存 / 正常退出即清；写盘格式与 `write_scene` 同源 + 原子写；空闲停帧时靠 `request_repaint_after` 唤醒 | §二十四 |
| 场景切换 | 只在**帧末**一处应用（`apply_pending_scene_load`）；脚本 `load_scene` 只登记请求（钩子期间场景正被可变借用）；装不上就**不动** + 一条 error 日志；只重置"与实体绑定"的状态（物理 / 粒子 / 音频 autoplay / UI 交互 / `started` / `states`），**日志缓冲与脚本全局变量不能清**；装载来源由宿主注入 `with_scene_loader`，没接是**可诊断的错误** | §二十五 |
| 关卡顺序 | `build.scenes` 顺序即关卡索引（下标 **0 起**）；**未配置 = 空**（不退回 `[entry_scene]`，脚本 `scene_index()` 返回 nil）；`entry_scene` 仍是唯一启动来源，清单第一项不兼任；`--check` 里"缺场景"算问题（exit 2）、"未列入"只提示（exit 0）；构建前逐关试装，路径规则只有 `normalize_scene_relative`（core） | §二十六 |
| 实体身份 | ID 是**跨进程**契约：反序列化必须**预留**计数器；删除按**索引**掩码（不按 ID 匹配）；复制 / 粘贴按**位置**分配新 ID（不以旧 ID 建映射）；父子解析与**所有** ID→索引建表一律**首占位**（含 `UiTree`）；名字也是身份（复制带序号去重）；加载期修复必须配**只读**检测（`--check` 的 `duplicate_ids`）；预制体**不**修重复 ID（`source_id` 是跨文件引用）；`u64::MAX` 是**坏数据哨兵**（读盘拒绝，否则顶死计数器 → 两实体共用 ID） | §三十八、§三十九、§五十二 |
| 预制体实例化 | 装载 trait `SceneLoader` 在 **core**（脚本层要能命名它，runtime 只 `pub use` 转出）；`spawn_prefab` / `spawn_prefab_under` **立即生成**（返回的 ID 当帧可用）；位置是**设置不是叠加**（世界 / 局部两种，子节点相对位置原样保留）；每个新实体逐条推 `ScriptCommand::EntitySpawned`（运行时据此补刚体）；失败三种（没装载器 / 读不到 / 空预制体）各自一条日志 + 返回 `nil`，**不留半个实例** | §二十七 |
| 预制体 Revert | 删了重建**不是**还原：Revert 必须**保留实例里已有实体的 ID**（按 `source_id` 认回，`Prefab::instantiate_with` 的唯一理由）；资产新增→新 ID、资产删掉→实体消失；**空资产不动手**（旧行为先删实例再发现没东西可放）；返回的是**新实例根索引**（子树整块重建并追加到末尾，索引会变）；世界变换 + 父节点由宿主保住 —— 还原的是**内容**，位置留在原地 | §五十八 |
| 欧拉角 / 旋转约定 | 换算只有 `core::{euler_deg_to_quat, quat_to_euler_deg}` 一处，顺序固定 **`YXZ`**（分量序始终 `[x,y,z]`）。**单轴旋转在 `XYZ`/`YXZ` 下同解**，所以只用单轴断言等于没测；改顺序 = 重新解释所有已落盘意图 | §六十四 |
| 场景语义体检 | 能通过 RON 解析 ≠ 能用。判定只有 `validate::scene_defects` 一份（七类）；MCP `scene_write` **非空即拒绝**且不落盘；`--check` 报三类新诊断并进退出码 2；**编辑器保存刻意不拦**（拒绝保存 = 用户改不动也救不回）；不自动修复 | §六十四 |
| 打包自包含 | 只复制清单内容目录 **+ 清单里每关的父目录**；发布前 `audit_package` 对包与源码工程各跑 `check_project` 取 `reference_issues()` **差集**，非空即失败（判据 = 构建有没有引入新问题，早就存在的坏引用不挡构建）；`reference_issues()` **只**收引用/装载类；报错里 `guid://` 必须翻回源码路径；自检放在 `publish` 之前 | §五十九 |
| 点光立方体阴影 | 6 面深度图 + `texture_depth_cube`；与方向光**并列**（`GpuLight.shadow_params.x` = 级联、`.z` = 立方体）；参考深度用**主轴分量** `max(|dx|,|dy|,|dz|)`（用 `length()` 会让整面向轴外越远越黑）；**u 轴必须镜像**（`scale(-1,1,1)`，不翻影子会翻到另一侧）；up 向量按 WebGPU 映射表选；只认第一盏 `casts_shadow` 且 `range > 0` 的点光，关着时逐位一致 | §八十 |
| 点光逐面剔除 | 面掩码是**保守**判据（球 vs 棱锥的必要条件，只许多画不许漏画；真理源 = 与投影矩阵同解的那条测试）；剔除走**索引重映射表**（`face_list` 每条 4 字节 + 顶点着色器换槽位），别改成"复制实例数据"或"着色器塌零面积三角形"——两条都实测过，一条 CPU 贵 100 µs、一条 GPU 一点不省；光照半径外整实体不投；`face_bit` 放在阴影 pass 的**分段**里且整段打包一次写；相机外的投影物也要进 `shadow_batches`（点光那一侧） | §八十三 |
| 软阴影 / 级间混合 | `render.shadow` 节（`softness` / `cascade_blend`）**默认全 0 = 关**，关着走 3×3 PCF + 硬级间切换老路径逐位一致（`is_default` 是整体相等判据）；采样盘是**固定泊松常量表**（禁随机 —— 截图逐字节确定的硬合同）；blocker search 用 `textureLoad` 取原始深度且**下标必须夹进图内**（uv=1.0 越界读 0 会被当"深度 0 遮挡物"）；`texture_depth_2d_array` 的 `textureLoad` 是 **4 参**（t, coords, array_index, level）；级间混合要**重叠拟合**（非第 0 级近端铺 `overlap × 本级跨度`），着色器 `b.y > 0.5` 守卫**宁可不混绝不混"全亮"**；band = split×blend 与 overlap_d = 级跨度×overlap 是**同一个旋钮**两处消费 | §八十四 |
| 逐条转移时长 | `AnimTransition.duration` 覆盖所在机器的缺省时长（基础 `Animator.fade_time` / 分层 `AnimLayer.fade_time`；`Some(0.0)` = 硬切）；**除数在切换那一刻定格**（`Prev.duration`，别每帧现读配置）；缺省 `skip_serializing_if` 不落盘 ⇒ 老场景逐字节不变 | §八十一、§八十五 |
| 控制台溯源 | 报错行的位置优先级 = **正文里的脚本 `path:line`**（只认 `.lua:`，别的后缀不猜）> 引擎 `file!()/line!()`；去重时补位置；源码查看器是**窗口**不是第八个面板（布局校验是"七个面板各一次"）；只读、512 KiB 上限、行号越界夹到最后一行 | §八十二 |
| 动画脚本控制 | 状态机优先（配了可用机器时 `clip/speed/looping` 由状态表决定）；`set_anim_clip` 只写 `Animator.clip` 并复用 `AnimationDriver` 的淡入；叠加层 / 层剪辑硬切；越界值钳住、非有限值忽略 | §三十 |
| 形变目标（morph） | CPU 变形（权重不变就不重写缓冲）；增量 target-major 且已乘 `model_scale`；权重优先级 剪辑 > 实体 > 模型默认；长度不自洽**整段放弃**；索引逐下标、钳 0~1、上限 256；`set_morph` 立刻生效（不走命令队列） | §三十七 |
| 动画插值布局 | `CUBICSPLINE` 切线**读入即保留**（每帧入切线 / 值 / 出切线三份，不降级）；`Track::values` 长度必须 = `times.len() × frame_len()`，帧内偏移只用 `frame_start` / `value_start`（**值在中间那份**，写错不 panic 只是动画整体偏移）；切线单位是**每秒**，进 Hermite 前乘段长；旋转逐分量 Hermite 后**必须归一化**，长度退化退回 slerp | §五十一 |
| UI 指针语义 | 位置（`pos: Option<Vec2>`）与按键（`down`/`pressed`/`released`）**分开**：`pos = None` 只表示不在视口，边沿仍要送达，否则"视口外松手"收不了尾；捕获状态用 `World::ui_pressed()` 断言，别只看 `state.pressed`（它要等指针回视口才归零，测试会假绿）；父节点隐藏 = 整棵子树不进命中表 | §五十二 |
| 实体 ID | ID 的唯一性是**每个文件内**的：场景的重复 ID 可在加载边界静默重分配（同文件 `parent`），**预制体的不许**——它被场景的 `PrefabInstanceRef.source_id` 跨文件引用，重分配会让所有既有实例失配、`Apply` 时整棵树被当成新增；`u64::MAX` 是坏数据哨兵，读盘即拒 | §五十二、§五十三 |
| UI 与脚本时序 | `update_ui()` 在 `update()` **第 0 步**（早于 start/fixed/update）：`ui_clicked` 必须读本帧状态；代价是脚本本帧改的 UI 矩形**下一帧**才参与命中，这是刻意取舍，`script_moved_ui_takes_effect_on_the_next_frame` 钉住它 —— 想调顺序先改这条测试 | §五十二.4 |
| 热重载 | 1 Hz 轮询内容目录（只查 metadata）；`poll` 推进基线、**动作后必须 `rebase`**（防自激）；场景永不自动覆盖；脏材质草稿保留；`--ui-shot` 不启用 | §三十一 |
| 性能改动 | 先量后改；同机背靠背；基准必须断言"真的跑了"；不为机器上调验收线 | §十三 |
| 热路径分配 | `World::update` 预热后**每帧零分配**由 `alloc_steady_state` 门禁钉住（不 ignore，进 CI）；新增缓冲要提前分配复用，`collect`/`format!` 进每帧路径即回归 | §五十四 |
| CI | Windows + stable + `--locked`；单 job 顺序跑；不跑 `--screenshot` / 基准 / fmt | §三十四 |
