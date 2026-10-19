# XGameEngine — Agent 指南

Rust 实现的类 Unity 游戏引擎（wgpu 渲染 + egui 编辑器 + rapier 物理 + 脚本 Lua + Rust 组件插件）。

## 必读文档（按顺序）

1. `ROADMAP.md` — 阶段状态总表（先看「整体路线图」）
2. `PLAN.md` — 历史各轮的实施记录（含每个决策的理由与取舍，改代码前先查这里有没有已定结论）
3. `Docs/差距分析.md` — 与 Unity/UE/Godot 的能力差距及优先级
4. `Docs/双轨语言架构计划.md` — 双轨语言架构计划（**P0–P3 已全部完成**）：Rust（UE 插件模式）+ Lua（Unity 模式 + 热更）双轨，Rhai 已于 P3 删除。其「决策记录」一节（D1–D11）是脚本层全部取舍的真相源，改脚本层代码前必读

## 常用命令

```sh
cargo check --workspace --all-targets        # 检查（先跑这个）
cargo clippy --workspace --all-targets       # lint（要求 0 警告）
cargo test --workspace --all-targets         # 测试（要求全绿）
cargo run -p xgame-editor                    # 编辑器（先弹项目枢纽）
cargo run -p xgame-editor -- --check projects/SampleScene       # 无头检查（退出码 0/1/2）
cargo run -p xgame-editor -- --headless --frames 300 projects/SampleScene  # 无头运行（JSON 统计）
cargo run -p xgame-editor -- --headless --frames 150 --screenshot shot.png projects/SampleScene  # 无头截图（需 GPU；默认顺光取景，同机逐字节确定）
cargo run -p xgame-game -- projects/SampleScene  # 独立游戏
cargo test -p xgame-runtime --release --test update_throughput -- --ignored --nocapture  # 分档性能基准（默认 ignore）
```

发布打包：双击 `一键构建.cmd`（产物在 `dist/`）。

## 架构速记

- **crate 依赖方向**（不许反向依赖）：`core ← math ← {render, physics}`；`input`/`script` 在中层；`runtime`（World 编排）在上；`editor`/`game` 是两个宿主。
- **编辑器 Play 与独立游戏跑同一段 `World::update`**（`xgame-runtime`）——改运行时行为两端自动一致；不要在 editor/game 里复制运行时逻辑。
- 脚本层（`xgame-script`）：`LuaHost`（`lua_host.rs`）管编译缓存/生命周期派发，**单轨只认 `.lua`**（Rhai 已随双轨 P3 删除，见计划文档 D11）；API 真相源是 `GameCtx` 方法层（`ctx.rs`；`lua_api.rs` 只是每函数一行的转发壳）；`host.rs` 是宿主公共类型模块（`Hook` / `PreloadReport` / `CommandReport`）；`ScriptCtx` 是地址束（unsafe 有严格调用窗口约定，见 `context.rs` 模块文档，别破坏该约定）；路径驻留表 `intern.rs` 的 id **永不清**（热重载只清编译缓存，见 D10）；每脚本环境表的 `__index` 必须挂**真元表**（D10 ①）。
- 场景格式 RON，纯文本、可 diff；新字段一律 `#[serde(default, skip_serializing_if = ...)]` 保证旧文件兼容。

## 代码风格约定

- **注释与文档写中文**，且遵循本仓库的风格：解释"为什么"而不是"是什么"；重要的取舍、别推翻的决策要写明理由（参考 `PLAN.md` 各轮的「刻意的取舍」小节）。
- 回归测试必须先证明它在旧实现下会红（仓库惯例，见 PLAN.md 第十节）。
- 性能改动"先量后改"，量数据写进记录。
- clippy 0 警告是硬要求。
- `一键构建.cmd` 保持 ASCII-only（cmd.exe GBK 解码问题）。

## 项目布局

- `crates/` — 引擎与工具（13 个 crate，名字见 workspace Cargo.toml）
- `projects/` — 游戏项目（`project.ron` + `Scenes/` + `Resources/` + `Scripts/` + `Prefabs/` + `input.ron`）；
  `BehaviorsDemo` 例外——它本身是 workspace 成员（cdylib），用来演示 Rust 插件项目形态
- `Docs/` — 脚本 API、差距分析、双轨语言架构计划
- `dist/` — 发布产物（构建输出，不入库改动）
- `tools/` — 构建脚本（build-release.ps1 等）

## 当前工作上下文（2026-10）

- 双轨语言架构已决策收敛为 **Rust + Lua**（C#/Verse/多语言中立层已否决，理由见计划文档决策记录 D1）。
- **P0 已完成**（2026-10-14，实施记录见 PLAN.md 十五）：`xgame` 门面 crate + `GameCtx` 方法层（api.rs 平移为转发壳）+ `Lifecycle` trait + 组件分发 + 注册表 + 插件边界（`export_plugin!` + 版本握手）+ 示例插件 `examples/spinner-plugin` 全链路。落地偏离已回写计划文档 D6。
- **P0.5 已完成**（2026-10-14，实施记录见 PLAN.md 十六，决策见计划文档 D7）：编辑器 PIE —— 项目根有 `Cargo.toml` 即插件项目，按 ▶ Play 自动后台 `cargo build`（UI 不阻塞）、产物同步进 `<项目>/Plugins/`、经**临时副本**加载进 Play 会话；Inspector 有 Behaviors 区；玩家与打包器都认 `Plugins/`；演示项目 `projects/BehaviorsDemo`（Rust 组件 + Rhai 脚本同场景共存）。
  - **三条不能推翻的约束**（D7）：`Plugins/` 是唯一规范目录；加载永远走临时副本（Windows 文件锁）；旧 DLL 句柄永不卸载（悬垂 vtable 是 UB）。持有句柄的字段必须声明在 `game_runtime` / `world` **之后**。
- **P1 已完成**（2026-10-14，实施记录见 PLAN.md 十七，决策见计划文档 D8）：`#[derive(Inspect)]` + Inspector 按字段表自动渲染 + 编辑器调参（Edit 改惰性构造的副本、Play 改 World 里的活实例）。
  - **四条不能推翻的约束**（D8）：① 组件契约是 `Behavior: Lifecycle + Inspect`（**不是** `Lifecycle`）——编辑器不链接用户代码，拿 `&mut dyn Inspect` 只能靠实例自带的字段表，存 `Box<dyn Lifecycle>` 时这条降级做不到（需 downcast，跨 DLL 无共享类型信息）；② `Inspect` 是"表 + 下标"，别改成闭包（跨边界闭包要处理 panic 隔离与函数指针生命周期）；③ 寻址名永远是 Rust 字段名，`label` 只改显示名（重命名显示名不能破坏脚本）；④ 组件字段改动**不进**场景撤销栈（组件参数不在场景文件里，动它会凭空多出一条场景撤销）。
  - **两条实现事实**：`PluginSession` 留的常驻注册表是每帧路径（查字段表 / 构造编辑态副本）的唯一通道，别改回 `build_registry()` —— 后者每次重跑插件入口且失败时 `log::error!`，错位时会变成**每帧一条错误日志**；**插件注册表是跨编译单元的内存布局**，给 `BehaviorEntry` 加字段 = 改 ABI，会让过期的 `target/*.dll` 变成 `0xc0000005`（进程暴毙无 panic）。而 **`cargo test` 不产出 cdylib**，那些 DLL 极易过期 —— `plugin_loading` 现在取 DLL 前先嵌套构建一次。
- **P1.5 已完成**（2026-10-14，实施记录见 PLAN.md 十八，决策见计划文档 D9）：组件参数落盘进场景文件 —— `Entity::behavior_params: BTreeMap<String, FieldValues>`（**注册名** → 字段寻址名 → 值），`World::instantiate_behaviors` 装配期套用、编辑副本种子场景参数。
  - **两条不能推翻的约束**（D9）：① 参数表的键是**注册名**而非 `behaviors` 数组下标 —— 下标会错位（中间插组件后续全变），参数是持久化数据，错位写参表现为"改 A 组件的参数写进 B 组件"且两边都是 `f32` 时无从发现；② `FieldKind` / `FieldValue` 的 serde **变体标签即磁盘格式**（`field_value_ron_variant_labels_are_stable` 钉死），改标签名等于作废所有旧场景。
  - **推翻 D8 第 ④ 条**（组件字段改动不进场景撤销栈）：参数进场景文件后已成为场景数据，不进栈会凭空丢一条场景编辑。现改为组件字段编辑接上场景撤销栈（拖拽挂 `undo_pending`、松手入栈、离散编辑当场 push），与 Name/Transform 同通道。**Play 中热调参仍不写回**（停止靠场景快照还原，既有语义）。
- **P2 已完成**（2026-10-14，实施记录见 PLAN.md 十九，决策见计划文档 D10）：Lua 宿主（`lua_host.rs` + `lua_api.rs`，mlua 0.10：`lua54`+`vendored`+`send`）+ 两轨按扩展名分流（`world.rs` 依次派发、缓冲合并不共享指针）+ 修两宿主每帧 3 处路径 `String` 克隆（路径驻留表 `intern.rs`：队列与状态键都用驻留 `u32`，Rhai 轨分发 819.6 → 713 µs/帧，同机背靠背 -13%）+ 编辑器双轨接入（`list_scripts` 扫 `rhai`/`lua`，证红测试在案）。
  - **四条不能推翻的约束**（D10）：① 每脚本环境表的 `__index` 必须挂**真元表**（往 env 塞同名普通字段对读取无效，表现是"脚本里宿主 API 全变 nil"，踩过：首跑 9 条红）；② 两轨分流必须恰好全覆盖（`is_rhai = !is_lua`，漏 = 脚本静默不执行，重 = 每帧假报编译错）；③ **驻留表 id 永不清**（`invalidate*` 只清编译缓存 —— 清表 = 热重载后每实体状态静默错位，双宿主 `state_survives_hot_reload_even_when_intern_order_changes` 钉死，红对照已实测）；④ 沙箱预算两轨同量级（Rhai `set_max_operations(200_000)` 与 Lua 指令钩子预算 200_000），改预算两轨一起改。
  - **一条实现事实**：借 `self.intern` 的 `&str` 跨不过 `&mut self` 方法调用（借用检查器看不出它就是 self 的一个字段）—— `ensure_loaded` 因此拆成 path 外壳 + `ensure_loaded_id` 内核、`run_hook` 收 `path_id`，路径解析都在方法**体内**做（字段级拆借）。
- **P2.5 已完成**（2026-10-15，实施记录见 PLAN.md 二十，规则见计划文档 §六）：三负载闸门 `language_gate.rs`（同进程背靠背，`measure()` 有错误日志守卫 —— 脚本编译失败时"空调度"会被当成假快，首跑就栽在这：Rhai 无 `mut` 关键字）。**判定：Lua 三负载全占优（dispatch 1.31×、api 1.26×、compute 9.16×）→ P3 删 Rhai**。
  - **两条不能推翻的教训**：① 基准工具必须断言"脚本真跑了"（宿主对编译失败只记日志是产品正确语义，但基准要自己 panic，`language_gate::measure` 的守卫在案）；② Rhai 语言**没有 `mut` 关键字**（变量天然可变），写 `let mut x` = 编译错 + 脚本静默不跑（Rhai 已删，但教训对任何"宿主只记日志"的基准工具都成立）。
  - P2.5 顺带修了 Lua 宿主热路径两处实现开销（先量后改，探针数据在 PLAN.md 二十）：① 钩子函数句柄**编译期缓存**（`detect_funcs` 留下 `Option<Function>`，省每钩子 `env.get` ~194 ns）；② ctx 改**宿主常驻槽 + 轻量指针传参**（`LuaHost.ctx_slot: Box<MaybeUninit<LuaCtx>>`，脚本侧 `FromLua` 解引用槽地址，省每钩子现造 userdata ~284 ns；安全论证在 `LuaCtx` 类型文档 —— 指针永远指向宿主持有的槽，窗口内槽 = 本次 ctx，脚本无法伪造 lightuserdata）。Lua 轨分发 **1670 → 576 µs/帧（-65%）**。
- **P3 已完成**（2026-10-15，实施记录见 PLAN.md 二十一，落地注记见计划文档 D11）：按闸门判定删 Rhai，脚本层收敛单一 Lua 轨 —— `host.rs` 瘦身为宿主公共类型模块（`Hook` / `PreloadReport` / `CommandReport`，Lua 宿主一直共用，路径不变）、`api.rs`（Rhai 转发壳）删除、`rhai` 依赖移除、`ScriptCtx<S>` 泛型收成非泛型（状态表直接持 `mlua::Table`，地址束与 unsafe 窗口约定原样未动）；`World` 只持一个 `scripts: LuaHost`（两轨缓冲合并代码全删）；`list_scripts` 只扫 `.lua`（`list_scripts_scans_lua_only` 证红：残留 `.rhai` 不进下拉）；4 个示例 `.rhai` 移植 `.lua`（`math.sin` 走沙箱 MATH 库，`sample_project_runs` 覆盖）；`Docs/脚本API.md` 整篇重写为 Lua 版。闸门工具 `language_gate.rs` 退役删除、`behavior_dispatch_throughput` 去 Rhai 档（历史数据留模块文档表）。
  - **三条 P3 取舍**（防好心人翻案）：① API 名一字不改（`spawn_entity` 后缀是 Rhai 保留字时期的历史产物，迁移成本 > 违和感）；② `host.rs` 不改名不搬家（`lua_host.rs` 的 `use crate::host::` 与再导出路径全不动，纯删代码 diff 最小）；③ `check.rs` 悬空引用检查仍只查文件存在、不查扩展名 —— 旧项目残留 `.rhai` 引用不被误报。
- **第八阶段音频系统已完成**（2026-10-15，实施记录见 PLAN.md 二十二）：选型 **kira 0.12.5**（自带空间化轨道与 symphonia 解码）；分层 `core::audio`（纯数据：`AudioPort` / `PlaySound` / `AudioSource` / `ListenerPose`）← `xgame-audio`（`AudioEngine` 封装 kira）← `World`（持 port + engine，**每渲染帧末尾** `sync_audio` → `flush_audio` 结算）；`Entity::audio: Option<AudioSource>`（clip/volume/pitch/looping/autoplay/spatial/min-max dist，Inspector Audio 区块可配、随场景落盘）；脚本 6 API（`play_sound` / `play_sound_at` / `stop_sound` / `stop_all_sounds` / `set_listener` / `set_master_volume`）；编辑器 Play 每帧把相机姿态推给监听者；`check_project` 加 `audio_refs` / `missing_audio`。
  - **四条不能推翻的约束**（防好心人翻案）：① **脚本层不引 kira** —— 脚本音频 API 只往 `AudioPort` 塞命令，`World` 帧末统一结算（`GameCtx.audio: Option<&mut AudioPort>`，None 静默 no-op）；设备失败走 **silent 降级**（照常受理/计数/GC，只是不出声），别改成"没设备就报错"；② **监听者与主音量是 port 上的状态字段，不进命令束**（姿态每帧被覆盖，混进"发生过的事"会变成回放一串状态更新的荒诞语义）；③ **`PlaySound.follow` 是实体绑定、不是空间化开关** —— 空间化只看 `position.is_some()`，非空间化声音**也要绑实体**（否则"删实体音还在响"，`removed_entity_or_component_stops_its_sound` 钉死该来回）；④ **音频每渲染帧同步一次、不跟物理固定步同频**（人耳对 16 ms 的位置更新不敏感，固定步一帧可能跑多个）。
  - **两条实现事实**：`AudioEngine` 的 silent 模式用**墙钟** GC、设备模式看 `handle.state()==Stopped`；测试要断言"声音在响"必须用 ≥1 秒的 WAV（50 ms 的会被 GC 先真掉，曾造成假红）；kira glam 0.33 与引擎 glam 0.29 **类型不出 `xgame-audio` 墙**——跨界只传 `[f32;3]`/`[f32;4]` 数组。
  - **热路径快速路径（先量后改，数据在 PLAN.md 二十二·九）**：`sync_audio` 的实体表扫描在无音频场景下实测 +5.7 µs/帧（2000 实体纯视觉），故加 `audio_sources_seen` / `audio_scan_count` 守卫 —— 没挂过音频源、无 autoplay 标记、实体数不变 ⇒ 整段跳过；实体数变化（spawn/destroy）重新武装扫描（`audio_rescan_picks_up_new_entities` 钉死）。**刻意取舍**：给已有实体运行中直接贴 `audio` 组件（不动实体数）不触发重扫 —— 动态发声正路是脚本 `play_sound`（不经扫描）。
- **Agent-Native ①② 已完成**（2026-10-16，实施记录见 PLAN.md 二十三）:MCP 工具面收尾 + 无头运行 —— `xgame-editor --headless [--frames N] [--dt 秒] [--report 路径] [--dump-scene 路径] <项目>`（实现 `crates/xgame-editor/src/headless.rs`：跑**同一段** `World::update`，每帧固定 dt、无输入、无墙钟 ⇒ 两次运行**模拟结果**逐位一致；退出码 **0=干净 / 1=加载失败或参数错 / 2=跑完有问题**；报告 = 帧耗时最近秩分布 / 实体刚体计数 / 脚本日志含 error 样本 / 非有限变换 / 音频 silent 计数）；MCP 第 7 个工具 `run_headless`（**spawn 同目录编辑器 exe 透传**，信封 `{exit_code, report, scene_ron?}`，校验归属编辑器唯一）；文档 `Docs/无头运行.md` + `Docs/MCP.md`（6→7 工具）。
  - **三条不能推翻的约束**：① 无头运行的实现住在编辑器二进制（`--check` 的邻居），**不做第二份帧循环**（单一运行时铁律；MCP 只透传）；② 退出码 `2`（跑完有问题）**不是工具失败** —— 报告照常产出、`isError` 只留给"打不开项目"；③ 报告 JSON 字段名是稳定契约（`json_contract_has_stable_keys` 钉死，消费方 = CI + MCP）。
  - **两条实现事实**：MCP 测试 spawn 编辑器**前必须嵌套 `cargo build -p xgame-editor`**（`cargo test` 不产出常规 exe，过期编辑器不认 `--headless` 会弹窗挂死测试；纪律同 `xgame/tests/plugin_loading.rs`）；`find_editor_exe` 找同目录 + **上一级**（`cargo test` 时 server 在 `deps/` 下跑）。
  - **实测（PLAN.md 二十三·三）**：SampleScene 300 帧墙钟 ~80 ms（帧循环 8.6 ms、帧均值 28.5 µs）；BehaviorsDemo（插件 + Lua）~70 ms（帧循环 1.3 ms）；进程启动固定成本 ~60 ms（与 `--check` 同量级）。
- **渲染 PBR 片已完成**（2026-10-19，实施记录见 PLAN.md 二十四）：Cook-Torrance（GGX + Smith + Schlick）替换 Blinn-Phong + **逐实体材质覆盖**（`Entity.material: Option<MaterialOverride>`，base_color 乘子 / metallic / roughness 替换）+ `--screenshot` 无头截图（默认顺光取景、同机逐字节确定）+ MCP `run_headless` 的 `screenshot: true`（PNG 走 MCP `image` 内容块）。
  - **四条不能推翻的约束**：① 灯亮度按 `radiance = intensity × π` 标定 —— 别"顺手修正"成物理严格的 intensity，全部旧场景会暗 3.14 倍；② 材质参数是**逐 draw** 的（`DrawUniform` 112 B + 动态偏移），别改回"每材质一个绑定组"（逐实体覆盖下按资产缓存要么重建绑定组、要么缓存键随数值膨胀）；③ `scene_signature` 必须哈希 material（漏了 = 暂停时调色视口不刷新）；④ 默认截图机位必须**顺光**（相机方位 = 主方向光 XZ 方位角 + 35°、仰角 26°；背光机位会把 PBR 画面拍成"渲染坏了"）。
  - **三条实现事实**：`MIN_ROUGHNESS = 0.04`（GGX 在 roughness=0 时除零 → NaN 扩散整帧）；`--screenshot` 是**唯一**推进动画时钟的无头路径（静态截图里角色不能停在绑定姿态）；像素断言必须算一遍镜面项（光与相机同轴时 GGX 副瓣会在中心像素叠 ~0.24 线性白，把 tint 的效果糊掉一半 —— 实测踩过）。
  - **已知限制（写进差距分析）**：无 IBL（金属只有 ambient×F 兜底、偏暗）、无 ACES（高光直接裁剪）；脚本侧没有 `set_material`（入口 = Inspector / 场景 RON / MCP `scene_write`）。
- 对照数据（微基准 `behavior_dispatch_throughput`，1000 no-op 实体 × 300 帧，P2.5 会话）：Lua 轨分发 **562 µs/帧**、Rust 组件轨 71.7 µs、无轨基线 7.54 µs。（历史 Rhai 轨 735.6 µs，P3 起已从基准移除。）历史：P0 = 493.9 µs（当时机器）、P0.5 = 68.4 µs、P1 = 65.2 µs、P2 = 713 µs（组件/基线口径的历次漂移见基准模块文档的完整记录表）。⚠ 绝对值随机器状态浮动 1.5× 以上，**跨版本比较必须同机背靠背**。**组件轨 < 50 µs 的验收线在本机持续红**（P0.5 起已知，验收线未为迁就机器上调 —— 红了先对同机数字再判回归）。
- **工具链注意**：本仓库的源文件是 UTF-8 **无 BOM** 且含大量中文注释。用 PowerShell 做文本替换时必须显式 `-Encoding UTF8` 读写，否则 PS 5.1 会按 GBK 解码把中文**有损**毁掉（已发生过一次，PLAN.md 十六第七节）。改中文源文件优先用编辑器工具，shell 只做只读操作。
