# XGameEngine 开发计划

> 从"编辑器原型"到"可运行的游戏引擎"的演进路线。
>
> **这是一份历史实施记录（archive）**：每一节是"当时那一轮做了什么、为什么这样做、
> 量到了什么数据"。它不反映当前状态 —— 状态看 [`ROADMAP.md`](ROADMAP.md)，差距与
> 优先级看 [`Docs/差距分析.md`](Docs/差距分析.md)，不能推翻的结论看
> [`Docs/决策记录.md`](Docs/决策记录.md)。
>
> **索引**：零–十四 = 项目分离 / 编辑器 / 玩法层 / 阴影 / 交互硬化 / 帧调度；
> 十五–二十一 = 双轨语言架构 P0–P3（含性能闸门与删 Rhai）；
> 二十二–三十一 = 音频 / Agent-Native / PBR / HDR / Bloom / 天空 IBL / 实例化 /
> 编辑器 UI / 剔除 / FXAA。

---

## 八十三、视口导航：F 聚焦与数字键对轴（2026-10-28）

视口操作便利性（差距分析 P2 #28）缺了两个"每天用几十次"的动作：框住选中物（F）和
对齐到轴（数字键）。飞行相机、gizmo、拾取都齐了，但找东西靠手滚、看侧面靠手转，
比 Unity / Blender 慢一个数量级。

### 一、F 聚焦：只动位置，不改朝向

- 聚焦对象 = 选中实体的**子树**（没有选择就聚焦全场景 —— 迷路时按 F 是本能）。
  可见体并成一个世界 AABB（与拾取共用"本地盒 → 世界 AABB"的 8 角点折算），
  UI 元素与空节点不参与（拿单位盒兜底会把画面撑得莫名其妙）。
- 距离 = 让**包围球**落进画面（球对朝向不敏感；代价是看到的东西略小。Unity / Blender
  的 F 都是这个取舍）。球半径取对角线一半 —— 取最长边的话，薄而长的物体四角露在球外。
- `fit_distance(radius, fov_y, aspect)`：垂直与水平半视角取**较小**者，宽窗口限制来自
  垂直、窄窗口来自水平；只按垂直算会在窄窗口里把物体切掉两条边。半径下限 0.25（点状
  目标不能把相机放到物体中心上），再乘 1.15 边距（"刚好贴边"视觉上顶到画面边缘）。
- 朝向不变是刻意的：用户先转好角度再按 F 是常见操作，聚焦时把视角归正会让人重新找
  方向。要对轴就按数字键，两件事分开。

### 二、数字键对轴：只改朝向，不动位置

`1` / `3` / `7` = 前视 / 右视 / 顶视，配 `Ctrl` 取反（后视 / 左视 / 底视）—— 与 Blender
数字小键盘同构。**这不是正交投影**：渲染管线只有透视相机，给的是"正对着某根轴看"。
`AxisView` 名字里不出现 Ortho 就是这个原因。先对轴再按 F 是最常用的组合（顶视图里
框一片再聚焦），所以对轴不动位置。

pitch 硬边界 `PITCH_LIMIT = π/2 − 0.01`（≈ 0.57° 的斜角，肉眼看不出）：正对着轴时
`right = forward × Y` 退化成零向量，`pan_right` 除零得 NaN 把相机位置污染成 NaN
（画面上就是全黑）。顶/底视故意偏这一点角度，测试容差相应放宽到 2e-2。

### 三、两处接线细节

- 导航键处理必须在算视图投影**之前**：这一步改的是相机，同一帧的渲染与拾取都该用
  新值。放到后面就是按键那帧白等一帧，用户会以为没生效。
- 所有视口/层级快捷键都过 `!ui.ctx().wants_keyboard_input()`：在「Search…」框里打
  "f" 不该让相机飞走、按退格不该顺手删掉选中的实体。面板一多谁也说不清焦点在谁
  身上，由 egui 统一回答。

对轴后控制台留一行回执（`视角：顶视`，同键连按去重成 ×N 不刷屏）；按 F 没有可聚焦
物体时给 warning 而不是静默 —— 静默失败最难受，用户分不清是键没生效还是没东西。

### 四、验证

7 条新测试：`fit_distance` 三条（半径/FOV 单调、窄视口按水平半视角算、点状目标有
下限）、`focus_on` 一条（朝向不变 + 目标居中 + 距离等于 `fit_distance`）、轴视角一条
（六个方向对、pitch 不触边界、平移一次仍是有限值 —— 退化朝向在这里炸 NaN）、
端到端两条（真按键：`pressing_f_frames_the_selected_entity`、
`number_keys_snap_the_camera_to_axis_views`，走 `show_content` 的完整帧）。

全量 **994 passed / 0 failed / 9 ignored**（+7），clippy 0 告警。**仍未做**（P2 #28
剩余）：真·正交投影相机、视口锁定、相机速度的 UI 调节入口虽有（滚轮 + Preferences）
但没有快捷键。

## 八十二、日志的第三个去处：控制台与 Editor.log（2026-10-28）

这一轮不从差距表里挑功能，从**最疼的那个空白**挑：出问题的时候我们手上没有现场。

具体症状：`dist` 里的 exe 双击就能跑，用户没有终端，而引擎所有 `log::warn!`（资产装
不上、骨骼超上限、脚本报错、wgpu 降级）**只写 stderr**。控制台面板显示的从来只有
编辑器自己 `state.log_*` 的那几十条。于是"打开项目就白屏""模型是黑的"这类问题，
用户能给我们的只有一句描述 —— 没有一行日志。

### 一、三去处，一个后端

装一个 `log::Log` 实现（`logging` 模块），把一条 `log::warn!` 同时送三个地方：

```text
log::warn!("…") ──┬─▶ stderr       env_logger 原样转发，RUST_LOG 语义不变
                  ├─▶ 控制台队列     有界 1024 条，帧首被 EditorState 取走
                  └─▶ Editor.log    追加写，4 MiB 轮转一次，只留一份 .1
```

磁盘上的位置跟偏好、最近项目同一处（`config::dir()`，Windows 下
`%APPDATA%\XGameEngine\logs\`）。用户不用记路径：控制台右上角有「Log 目录」，
点一下打开文件管理器。

### 二、收谁不收谁（这是本轮唯一需要讲道理的地方）

| 等级 | 自家 target（`xgame*`） | 第三方（wgpu / naga / …） |
|---|---|---|
| error / warn | 收 | 收 |
| info | 收 | **不收** |
| debug / trace | **不收** | 不收 |

- 第三方 info 不收：wgpu/naga 开局就吐几十条 adapter/feature 清单，收进来就把真正
  有信息量的行挤到屏幕外。它们只在**出问题**时说话（warn/error），那时候收。
- debug/trace 一律不收：`RUST_LOG=trace` 跑一次能把日志写成 GB 级。要看细粒度日志
  照旧看 stderr —— 那是**当次**的现场，本来也不需要留档。
- 注意落档的网比 stderr **宽**：`RUST_LOG=error` 时 warn 不进终端，但仍进控制台和
  文件。终端的过滤是"现在想看什么"，落档是"以后要查什么"。

### 三、时间戳用"启动以来第几秒"

不用本地时钟。理由三条，都不是洁癖：

1. `std` 只能拿到 UTC 的 `SystemTime`，自己转本地时间是**错的**（时区规则不在标准
   库里），而为一个时间戳引一个依赖不值。
2. 跨时区 / 夏令时下 `[+1.234s]` 的含义不变。
3. 它与 `frame_probe` 的 `t_us=` 是同一族时间轴 —— 想对齐"加载完之后那 3 帧发生了
   什么"可以直接对。

墙上时间由文件本身的创建时间给出。

### 四、面板：等级开关 + 关键词

控制台标题栏加四个等级开关（Info / Warn / Error / OK）和一个关键词框。筛选**只影响
显示**，`Clear` 才是清 —— 否则"点两下筛选日志就没了"会变成新的支持问题。条数标签在
筛选生效时显示 `显示数 / 总数`。

搜索要零分配：面板每帧对最多 500 条各匹配一次，`to_lowercase()` 会变成 500 次分配/
帧。改成按字节 `windows` 比较 + ASCII 忽略大小写（ASCII 字节不可能出现在多字节 UTF-8
内部，所以不会"半个汉字匹配上"），关键词每帧只转一次小写。

### 五、排版踩到的坑：`Layout::right_to_left` 会换行

第一版把右侧那一组（条数 / messages / 丢弃计数 / Log 目录）放进默认的
`Layout::right_to_left` 里，1280 宽实测最后一项**折到了下一行**（截图里 `+6 略`
出现在消息列表那一行的右边，看着像串行）。原因：这个 layout 的 `main_wrap` 默认是
`true`。

修法：`ui.allocate_ui_with_layout(vec2(ui.available_width(), FIELD_HEIGHT), layout, …)`
自己给一列，把 `main_wrap` 关掉。1280 / 1440 / 1920 三档截图逐像素稳定。
顺序也要注意：右对齐布局里**先加的靠右**，所以加的顺序是「Log 目录 → 丢弃计数 →
messages → 条数」，读起来才是 `16 messages +6 略 … Log 目录`。

### 六、量到的

- 全量 `--workspace --all-targets`：**987 passed / 0 failed / 9 ignored**（+12）。
- 新增测试 12 条：轮转（备份是更早的一半、只留一份、超长单行照写、目录不可写降级）、
  队列有界（丢最旧并计数、丢了也照落盘）、策略表（第三方 info 不进）、target 压缩、
  定宽行格式、筛选（关等级不清消息 / ASCII 忽略大小写 / 中文原样 / 上限取最新一段）。
- clippy `-p xgame-editor --all-targets`：0 警告。
- 实跑：`XGAME_CONFIG_DIR=<tmp> --check` 与 `--headless --frames 3` 都生成了 UTF-8 的
  `logs/Editor.log`（首行是"日志写入 …"）；`--ui-shot` 三档尺寸下控制台里能看到
  `render::gltf_loader: No NORMAL — computing flat normals` 和
  `wgpu_hal::vulkan: Unrecognized present mode` —— 前者以前**只**在终端里。
- 边界：进程被强杀时最后一行可能丢（不 fsync，只保证 write 到内核缓冲）；这是刻意的，
  掉电场景不承诺。轮转只留一份备份，也不能当审计日志用。

---

## 八十一、R11 的两条剩余：深链销毁与卸载期 spawn（补覆盖，无红灯，2026-10-28）

R11/121 上轮关掉的是"两环链式销毁"和"钩子里 spawn 落地一次"。剩下两条症状最隐蔽、
却一直没测试：

### 一、8 环深链：每一环恰好收到一次 `on_destroy`

两环只证明"下一环也能被通知"。深链要问的是别的问题：

- 跨帧结算的命令会不会在某一帧丢（链走到第 4 环断掉）；
- 某一环会不会被通知两次（掉道具 / 扣分的逻辑跑两遍）；
- 顺序会不会乱（`state` 表按实体 id 存，顺序乱意味着作者的状态机对不上）。

测试造 8 个实体 `link0…link7`，每环 `on_destroy` 里销毁下一环；第 0 环由自己的
`update` 自毁（脚本侧没有"手动触发"入口，自毁也是最贴近真实用法的形态：子弹销毁自己）。
跑 `LINKS + 3` 帧收日志，断言 `dead:0 … dead:7` **按序各一次**，且最后场景里一个实体
都不剩。

### 二、卸载期间 `spawn` 出来的实体不许漏进下一场景

`unload_scene` 会照常派发 `on_destroy`（R11 的语义：钩子要能读到旧状态），脚本完全
可以在里面 spawn。但那属于**正在被拆掉的旧世界**：`unload_scene` 末尾清结构命令、
`swap_scene` 整块换掉场景。漏出去的症状是"切关后多出几个来历不明的实体，甚至带着旧
场景的脚本继续跑"。

测试：A 场景的实体 `on_destroy` 里 `spawn_entity(ctx, "ghost", "")`，切到 B 之后断言
① 日志里有 `dying:spawn`（证明钩子真跑过，否则这条测试证明不了任何事）、
② B 场景的实体名字**只有** `fresh`。

### 两条都是补覆盖，没有红灯

行为本来就是对的 —— 这一点写清楚，不把它们说成"修了 bug"。之所以值得补：坏掉时的
症状是"偶发漏派发 / 幽灵实体"，比崩溃难查得多，而它们各自只需要 40 行测试。

顺带确认了一个容易误读的边界：钩子里的 `spawn` 对 **Scene 立即生效**（`ghost` 当场
进旧场景），但**命令下一帧才结算** —— 这正是 §五十七 写的那条，卸载路径也一样。

### 量到的

- 全量 `--workspace --all-targets`：**975 passed / 0 failed / 9 ignored**（+2）。
- clippy `-p xgame-runtime --all-targets`：0 警告。
- 边界：真·异常退出（进程被杀 / 断电）仍未验证，那是测试覆盖不了的。

---

## 八十、脚本内存上限：OOM 杀进程是比"脚本报错"糟得多的结局（124 的记忆配额那一半，2026-10-28）

124 剩下的"完整内存配额"里，最要命的一条是：**脚本可以无限吃内存**。今天一个
`local t = {} while true do t[#t+1] = string.rep("x", 1 << 20) end` 的结局不是
"脚本失败"，而是操作系统把整个进程杀掉 —— 编辑器里未保存的场景一起没。脚本是
**用户内容**（可能是下载的 mod），防护不能靠"作者会写对"。

现有沙箱只有**指令预算**（`INSTRUCTION_BUDGET`，20 万条指令内掐断死循环）：
它挡的是"算不完"，挡不住"存不下"。

### 做法：给 VM 设一条内存上限

```rust
if let Err(err) = lua.set_memory_limit(MEMORY_LIMIT) { log::warn!(...) }
```

- `MEMORY_LIMIT = 128 MiB`（每 VM 一次）。刻意宽松：脚本 API 的持久状态表、编译后的
  chunk、几百个实体的状态加起来通常不到 1 MiB；留这么大是为了不误伤"用 Lua 搓了张
  大表"的正当玩法，同时让无限增长在跨过某个数量级时**立刻失败**。
- 限额参数化：`LuaHost::with_memory_limit(bytes)`，`new()` 就是
  `with_memory_limit(MEMORY_LIMIT)`。参数化不是为了给用户调（项目级旋钮没开），而是
  让测试能用 8 MiB 验证同一条路径 —— 为了触发上限去真吃 128 MiB 会把测试变成内存炸弹。
- **报错要说人话**：Lua 原文是 `not enough memory`，作者看到只会怀疑引擎坏了。
  `describe_error` 把 `MemoryError` 翻成"脚本内存超限（上限 128 MiB），已中断本次调用。
  常见成因是在钩子里不断往表里塞东西"。
- 设不上限（模块模式下分配器不归 mlua 管）只 `log::warn!`：不能因为设不上限就让引擎
  跑不起来。

### 两条契约（都有测试）

1. **超限被挡住**：8 MiB 限额下分配 32 MiB，必须报内存超限，且 `used_memory()` 停在
   限额以内。
2. **超限只毁掉那一次调用**：下一帧同一份脚本照常执行。这条是"上限可用"与"上限有毒"
   的分界线 —— 如果一次超限之后 VM 就废了，作者会看到"改完脚本还是全都不动了"，
   比 OOM 更难查。

### 先证红

把 `set_memory_limit` 那三行注释掉（等于旧行为）实跑：

```
a_runaway_script_hits_the_memory_limit_instead_of_the_oom_killer  FAILED  日志 []
the_vm_recovers_after_a_memory_limit_hit                          FAILED  第一帧应当超限，实际 []
```

两条都是"分配直接过去了、一个错误都没有" —— 正是最危险的形态。恢复后转绿。

### 量到的

- 全量 `--workspace --all-targets`：**973 passed / 0 failed / 9 ignored**（+2）。
- clippy `-p xgame-script --all-targets`：0 警告。
- 边界：配额是**整 VM 一档**，不区分"这个脚本吃多了"（报错会归到触发分配的那次
  钩子上）；宿主调用成本、单脚本配额、以及项目级可调仍未做。

---

## 七十九、打包不能靠"复制几个目录"：发布前必须验自包含（R01/R02 的"清单外依赖"，2026-10-28）

R01/R02 剩下的那句"清单外依赖"，具体是这个形状：构建只复制 `content` 里声明的
目录（外加入口场景的父目录），**复制目录本身不会报错**。于是清单之外的引用会**静默**
落不进包里 —— 编辑器里一切正常，`--check` 也只看源码工程，玩家拿到的成品却少一整
棵子树，或者在切到第二关那一刻黑屏。

### 两个缺口

1. **关卡可以放在 `scenes_dir` 之外**：`--check` 认 `Extra/boss.ron`（预制体/关卡
   都能在自定义目录），构建却只带 `scenes_dir` 与入口场景的父目录 → 第二关没进包。
2. **引用可以指向任何地方**：预制体放在 `Assets/`、脚本放在自定义目录时，资产库照样
   登记（它扫整个工程），`--check` 全绿，包里却没有这些文件。

### 做法

- `content_dirs` 补上**发布清单里每一关的父目录**（与入口场景同款处理）。这条解决
  缺口 1，且不改变"只打清单里的目录"这个基本盘。
- 发布前自检 `audit_package`：对**包**跑一次 `check_project`，再对**源码工程**跑一次，
  取**差集**。差集非空 = 构建把东西弄丢了 → `BuildError::PackageIncomplete`，构建失败，
  上一版发布目录原封不动（自检放在 `publish` 之前）。
- 判据是**"构建有没有引入新问题"**，不是"工程有没有问题"：打包前后一样缺的东西归
  `--check` 管，一条早就存在的坏引用不该让作者连包都打不出来（`already_missing_assets_do_not_block_the_build` 钉住这条）。
- `CheckReport::reference_issues()` 是差集用的那一份列表：**只**收引用 / 装载类
  （模型、材质、贴图、脚本、音频、UI 图、预制体、悬空 GUID、坏材质、坏场景）。
  `missing_metas` / `guid_conflicts` / `duplicate_ids` / `invalid_layers` 这类**身份与
  格式**问题前后一模一样，让构建替它们背锅没有意义。
- 错误里的 `guid://…` **翻译回源码工程路径**（`explain_guids`）。场景存的是身份，
  只说"悬空 GUID `3444c856…`"等于没说：那句话既不能定位文件，也不能提示怎么修。

### 先证红

两条测试在旧实现下实跑：

```
levels_outside_content_dirs_are_packaged     FAILED  （包里没有 Extra/boss.ron，
                                                       packed_dirs = [Prefabs, Resources, Scenes]）
build_rejects_a_package_that_lost_references FAILED  （构建"成功"了，返回了 BuildReport）
```

第三条 `already_missing_assets_do_not_block_the_build` 是防"过拦"的护栏，它从第一版
就是绿的 —— 记在这里是为了说明这个门禁的边界，而不是把它当成红灯。

### 真实 CLI 正负对照（`dist` 里的二进制）

把一个预制体从 `Prefabs/` 挪到 `Assets/`（GUID 不变），再跑同一份 `dist`：

```
--check  → check: OK (all referenced assets exist)          exit 0   （源码工程没问题）
--build  → build FAILED: 打包后的游戏缺少 1 项引用…          exit 1
             - Scenes/scene.ron: Pillar -> guid://3444c856…（源码工程里是
               Assets/Pillar.prefab.ron，它没进包）
```

这正是这个门禁存在的理由：`--check` 看得见的部分它全绿，看不见的那一半由构建兜住。

### 量到的

- 全量 `--workspace --all-targets`：**971 passed / 0 failed / 9 ignored**（+3）。
- clippy `-p xgame-core -p xgame-editor --all-targets`：0 警告。
- `dist` 重建后：样例项目 `--check`=0、`--build` 成功（新自检在真项目上无误报）；
  **构建出来的包**直接跑 60 帧无头 `ok=true` / `non_finite_transforms=0`；
  BehaviorsDemo 与 dist 样例同样 60 帧通过。
- 已知没做的：包内**真实窗口 + 键鼠输入**的玩家端到端、并发发布、断电事务
  （R01/R02 的其余部分）仍未关闭。

---

## 七十八、Revert 不许换 ID：一次"还原"不该把整棵子树的身份洗一遍（R07/103，2026-10-28）

R07/103 只剩一句没关：**Revert 之后实例里的实体 ID 还在不在原位**。老实现走的是
"删掉整棵子树 → 按资产重新实例化"，于是每次还原都把子树里每个 `EntityId` 换成新
的。用户看到的后果有两条：

1. 场景文件里凭空多出一大段纯 ID 变化 —— 明明只点了一下"还原"；
2. 任何**按 ID 记住**这些实体的地方（外部工具、用户脚本里存的 id、跨帧缓存）全部
   失联。Unity 的 Revert 也是保留 ID 的。

### 做法：ID 由调用方给，不写死在 `Prefab` 里

`Prefab::instantiate` 拆成两层：

```rust
pub fn instantiate(&self, source: &str) -> Vec<Entity>          // 新增实例：全新 ID
pub fn instantiate_with(&self, source: &str,
                        id_for: impl FnMut(EntityId) -> EntityId) // Revert：按 source_id 认回
```

`id_for` 的入参是**资产里的**实体 ID（也就是实例的 `source_id`），所以 Revert 只要
先把「旧实例的 `source_id → 当前实体 ID`」做成对照表，再原样传进去即可 —— 父子重
定向仍然按资产 ID 建索引，一处逻辑，两条入口共用。

### 三条语义（都有测试）

| 情况 | ID 从哪来 |
|---|---|
| 实例里已有（按 `source_id` 对上） | **沿用旧 ID** |
| 资产里新增 | 新 ID |
| 实例里多出来（资产里没有） | 删除 |

`Scene::revert_prefab(root, &prefab, source)` 是新入口，返回**新实例根索引**（子树
被整块重建、脱离原位追加到末尾，索引会变，调用方必须用返回值）。它刻意**不动根的
世界变换** —— 那是宿主的事：用户把实例拖到别处再点"还原"，要的是内容回到出厂，
位置留在原地。

### 两个顺手关掉的坑

- **空资产不动手**：旧路径是"先把实例删掉，再发现没东西可放"，用户点一下还原反而
  丢了整棵子树。现在 `prefab.is_empty()` 直接返回 `None`，场景逐位不变。
- **手改过的场景撞 ID 要自愈**：沿用旧 ID 的前提是旧数据本身合法。文件里若两条实体
  共用一个 ID（只有编辑器外部改文件才会这样），沿用会让重复进结果；`revert_prefab`
  末尾补一次 `ensure_unique_ids()`，只可能动到"多出来的那条"。

### 先证红

测试写在编辑器侧（`crates/xgame-editor/src/state.rs`），先用老路径实跑：

```
assertion `left == right` failed: Revert 换掉了实例里的实体 ID
  left: [EntityId(7), EntityId(8)]
 right: [EntityId(4), EntityId(5)]
```

换成 `scene.revert_prefab` 后转绿；core 侧另补三条（保 ID / 空资产 / 撞 ID 自愈）。

### 量到的

- 全量 `--workspace --all-targets`：**968 passed / 0 failed / 9 ignored**（+4 条）。
- clippy `-p xgame-core -p xgame-editor --all-targets`：0 警告。
- 已知没做的：**嵌套预制体**（实例里再嵌实例）与**字段级覆盖**（实例只改一个字段、
  Revert 只还原那个字段）仍未关闭 —— 那是 Unity 的 Revert 的另一半语义。

---

## 七十七、销毁钩子里的结构性改动：补上两条没有被钉住的契约（R11/121 的一半，2026-10-28）

R11/121 说"递归销毁 / 卸载中新增、异常退出仍待验证"。异常退出（断电、强杀）不是
测试能覆盖的，先做能覆盖的那一半：**在 `on_destroy` 里再 spawn / 再 destroy**。

这两条此前完全没有测试，而它们恰好是最容易在重构中悄悄坏掉的地方 —— 命令是在
**派发途中**产生的，缓冲已经在 `take_commands()` 里被取走，新命令只能等下一帧。

### 契约一：`on_destroy` 里 spawn，落地一次

`consume_script_commands` 的循环里，`report` 是循环前 `take_commands()` 取走的快照；
`dispatch_destroy` 期间脚本 `spawn_entity_at` 产生的命令进的是**新的**缓冲，下一帧
才消费。所以要做到：

- 新实体**立即**进场景（`GameCtx::spawn_entity` 当场写 scene）—— 同帧 `find_entity`
  就该找得到；
- 位置 / 模型等字段立即生效；
- `on_destroy` 只派发一次，spawn 只结算一次，后续帧不重复。

### 契约二：`on_destroy` 里再 destroy，第二个实体也必须收到通知

链式销毁跨帧结算。如果实现把"处理途中产生的删除"当成同一批次忽略掉，被牵连的
实体就永远收不到 `on_destroy`、状态表也永远不释放。测试断言第二个实体能收到通知
**且能读到自己销毁前的状态**（`state_get_or` 返回 `alive`），以及最后只剩触发者的
状态表。

### 顺带发现的能力边界（不是 bug，但要写下来）

`spawn_entity(ctx, name, model)` **不带脚本**，而 Lua 侧没有给已有实体挂脚本的 API
（`lua_api` 的全局函数里没有 `add_script` 一类）。所以"死亡即分裂出**带行为**的
碎片"必须走 `spawn_prefab`（预制体里的实体自带 `scripts`），不能靠 `spawn_entity`
再补挂。

第一版测试就是按"spawn 后能带脚本"写的，实跑直接红：`scripts.len()` 是 0
（`left: 0`）。**这条红灯是测试自己的假设错，不是引擎缺陷** —— 按实际语义改写后
绿，并把边界记在这里。

### 验证与边界

- 全量 `test --workspace --all-targets`：**964 passed / 0 failed / 9 ignored**
  （上轮 962，+2）；clippy 0 告警。
- 两条都是**补覆盖**，没有红灯（行为本来就对）。之所以仍然值得写：这类契约一旦
  坏掉，症状是"偶发漏派发 / 实体泄漏"，比崩溃更难查。
- 仍然没覆盖：`unload_scene` 期间的 spawn（场景已经换掉，新命令该落到哪个场景）、
  真·异常退出（进程被杀时的暂存残留）。

---

## 七十六、确定性：同样的输入必须走出同一条轨迹（R10/084 的一半，2026-10-28）

R10/084 说的是"父层级与三宿主轨迹对照尚未专项验证"。先看三宿主接线：
`game_runtime::update_with_scene` 只是把编辑器的场景 `swap` 进 `World` 再调
**同一个** `world.update(dt)`，headless 直接调 `world.update(dt)`。所以"三宿主
跑同一段逻辑"是结构保证的，不需要一个跑三次的测试去证明它。

真正会分叉的是**同一个 `World` 逻辑内部**的顺序。于是本轮钉的是更强的性质：

> 同样的初始场景 + 同样的每帧 `dt`，两次运行必须走出**逐位相同**的轨迹。

### 为什么这条性质值得单独钉

它是"回放 / 复核 bug / 将来做网络同步"的地基，也是"引擎行为可解释"的底线 ——
轨迹不可复现，用户报的 bug 就永远复现不了。

具体抓的是**迭代顺序**这一类隐患：`HashMap` 的 `RandomState` **每个实例都用不同
种子**，所以"同一个进程里跑两遍"就足以暴露问题。凡是"按哈希顺序决定谁先谁后"的
地方（建体、删体、派发）都会让轨迹分叉，而浮点加法不满足结合律，顺序一变结果就变。

### 记录方式：位级，不用容差

`crates/xgame-runtime/tests/determinism.rs` 每帧把所有实体的位置 / 旋转按
`f32::to_bits` 拼成 `u32` 序列。**不用近似比较**：真值轨道的差异就该是 0，容差比较
会把"微小分叉"吞掉，而微小分叉正是这类 bug 的第一形态。分叉时直接报出帧号、实体
槽位和两个位的十六进制值，别让调试从"两个大数组不同"开始。

场景设计要**顺序敏感**：一个静态地面 + 6 个动态方块叠成两列（要接触、要碰撞、要
堆叠）。纯静态场景等于没测。

### 查出来的一处隐患（防御性加固，无红灯）

`sync_bodies` 的清理段原来是：

```rust
let dead: Vec<EntityId> = self.body_of_entity.keys().copied() /* ... */ .collect();
```

`body_of_entity` 是 `HashMap`，所以**多个物理实体同一帧被删时，`remove_body` 的
先后是哈希顺序**。rapier 会回收 body handle，理论上回收顺序会影响下一次建体拿到的
handle，进而影响求解器内部顺序。

实测结论要如实写：**当前 rapier 对删体顺序不敏感** ——
`removing_several_bodies_at_once_is_order_stable` 在加排序**之前**就是绿的（两次
运行的 `HashMap` 种子不同，轨迹仍然逐位一致）。所以这不是修 bug，是**去掉一个
未受契约保护的顺序依赖**：rapier 换成顺序敏感的求解器（或调参）时，症状会是"偶发
对不上"，极难查。排序只发生在结构性变化的帧，稳态快路径走不到这里。

### 验证

- 两条测试连跑 5 次全绿（确认不是 flaky）。
- 全量 `test --workspace --all-targets`：**962 passed / 0 failed / 9 ignored**
  （上轮 960，+2）；clippy 0 告警。
- `world.rs` 变了 → 指纹变化 → dist 与 Demo DLL 同批重建。

### 边界

只覆盖"单进程内两次运行一致"。跨机器 / 跨编译器的浮点一致性、`wrap` 模式的差异都
没覆盖；`Physics` 若将来启用并行求解（rayon）需要重新测这条性质。R10 的"父层级
对照"仍然没做。

---

## 七十五、脚本错误限流：一个坏脚本不该淹掉日志、也不该每帧分配（2026-10-28）

124 里"错误熔断"一直没做。现状是：`run_hook` 捕获到 Lua 错误就
`format!("{err}")` + `logs.push(...)`，**每帧每实体每条**。后果两条：

1. 一个每帧出错的脚本 = 60 条/秒。日志面板瞬间被淹，用户看不到别的问题；
2. 每次 `format!` 都在热路径上分配 —— 一个坏脚本就能把上一节刚建立的
   "稳态零分配"整个破坏掉（而且那条门禁测的场景不带脚本，正好测不到）。

### 做法：指数退避，成功清零

`LuaHost` 新增 `error_streak: HashMap<(实体, 脚本驻留 id, 钩子序号), u32>`：

- 出错 → 计数 +1，**只在 2 的幂次（1, 2, 4, 8…）才推日志**，100 帧持续报错
  只有 7 条；
- 日志文案在第 2 次起带上「该钩子已连续报错 N 次，后续只在 2 的幂次再报」——
  只报第 1 次而不说明还在错，用户会以为已经好了，那是拿噪音换误导；
- 同一钩子**成功执行一次即清零**（`else if !self.error_streak.is_empty()` 短路，
  正常运行时这张表是空的，热路径不该为"可能没出错"付一次哈希查找）；
- `invalidate*` / `reset_runtime_state` / `reset_scene_state` 都清空：热重载后
  脚本变了、换场景后实体 ID 变了、新 Play 会话应当重新报错。**不清的后果是
  "修好之后的第一条错误"被当成第 N 次静默掉，正好吞掉用户最想看的那条。**
- 表超过 1024 条时扫一遍、删掉已不在场景的实体条目 —— 防"每帧 spawn + 报错 +
  destroy"这种病态脚本把表撑大。正常项目远达不到该量级。

### 为什么不"错够几次就禁用脚本"

禁用会悄悄改变游戏行为（一个渲染脚本报错后连移动逻辑也不跑了），比刷屏更难查。
限流的目的是让故障**可见而不刷屏**，不是替用户决定"这个脚本别跑了"。

### 红绿

- 红灯：`repeated_script_errors_are_throttled` 在旧实现下 100 帧报 **100 条**，
  断言 `total <= 10` 实跑失败（`lua_host.rs:768`）。
- 配套两条：`a_recovered_script_reports_its_next_error_again`（4 帧里第 3 帧成功，
  断言逐帧日志数为 `[1, 1, 0, 1]`，钉住"成功清零"）、
  `throttled_error_says_how_many_times_it_failed`（文案必须带次数）。

### 验证

全量 `test --workspace --all-targets`：**960 passed / 0 failed / 9 ignored**
（上轮 957，+3）；clippy 0 告警。`xgame-script` 的 `src` 变了 → 指纹变化 →
dist 与 Demo DLL 同批重建（见下）。

### 边界

只限流引擎**捕获到的** Lua 错误。脚本自己 `log(ctx, "…")` 打的内容不受影响 ——
那是用户的输出，不该被宿主节流。指令预算沙箱（死循环）走的是另一条路，错误
同样进这套计数，所以死循环脚本也不会 60 条/秒地刷。

---

## 七十四、稳态零分配门禁：把"热路径零分配"变成可执行判据（2026-10-28）

项目纪律里一直写着"稳态热路径零分配"，但它此前只是一句**说法**：没有任何测试
会因为有人往 `World::update` 里加一句 `collect()` 而变红。帧时间基准也帮不上忙
—— 60 FPS 下每帧几块临时 `Vec` 不会体现在均值里，只会让帧时间抖动、让分配器
变成热点。

### 怎么数

新增 `crates/xgame-runtime/tests/alloc_steady_state.rs`：`#[global_allocator]` 包
一层 `System`，`alloc` / `alloc_zeroed` / `realloc` 先记账（无锁原子）再原样转交。
计数用**原子开关**控制窗口 —— 预热阶段允许分配（建刚体、扩容查询结构都是一次性
的），窗口只覆盖预热之后的 60 帧。

放在 `tests/` 而不是 `src/` 是必须的：自定义全局分配器是进程级的，只能在独立的
test binary 里用。

### 三个差点让它变成假绿的坑

1. **没有活跃性证明**。第一版只断言"分配 0 次"——如果 `update` 因为任何原因没
   真的跑，0 次分配照样成立。加上"世界时间必须推进约 1.0s"之后**立刻红了**：我把
   预热 30 帧也算了进去，期望 1.0s 实得 1.5s。改成测**窗口内的增量**才稳。这条
   断言救的正是"测试自己空转"这一类假绿。
2. **UI 锚点参数搞错**。`UiElement::at(anchor, x, y, w, h)` 的第一个参数是**归一化
   锚点**，我按位置传了 `[10.0, i * 50.0]`，锚点越界 → 布局走异常分支，"零分配"
   就变成了"没跑"。修正后加了元检查：指针放在第一个按钮矩形中心，`ui_hovered()`
   必须给出它，否则 UI 变体的数字不说明任何问题。
3. **同 binary 内的测试并行会互相污染计数**。两条变体各自开关窗口的话，对方线程
   的分配会记进自己的账 → 随机假红。合成**一条**测试（单线程跑完所有变体）比加锁
   简单也更可靠 —— 锁对象自身的分配同样会落进窗口。

### 负对照与结果

- 负对照：临时在 `World::update` 里加 `black_box(format!("probe{}", len))`，实跑
  报 `60 帧稳态里分配了 60 次 / 600 字节（每帧 1.0 次）`、`left: 60` —— 门禁真的
  抓得住。还原后绿。
- 实测（dev 与 release 都跑过，都是 0）：
  - `entities+physics`（200 实体、一半静态碰撞体）→ **0 次 / 0 字节**
  - `ui`（12 个按钮 + 1280×720 视口，布局与命中每帧重算）→ **0 次 / 0 字节**

即当前 `World::update` 在两条主要稳态路径上确实一次都不分配。这条测试是**门禁**
不是基准，它不 ignore，跟着全量测试进 CI。

### 验证

全量 `test --workspace --all-targets`：**957 passed / 0 failed / 9 ignored**
（上轮 956，+1）；clippy 0 告警。

收尾重建 dist 时撞见一件值得记的事：源码一字未改（只多了一个 `tests/` 文件，指纹
只扫 `crates/*/src`），但 `xgame-editor.exe` 的字节**变了**。逐字节比对是 **19 个
字节**不同、首处 `0x100` —— 正是 PE 可选头的 `TimeDateStamp`，代码段完全相同。
结论：本项目的 release 产物**不可逐字节复现**，判断新鲜度只能看**内嵌指纹**，
不能看 SHA-256。详见决策记录 §五十.5（那条的上半段也是本轮早些时候实测出来的：
`dist` 与 `target/release` 哈希相同只说明"同一份拷贝"，不说明新鲜）。

---

## 七十三、预制体 ID 是跨文件引用：`--check` 的两条新诊断（101 收尾，2026-10-28）

101 上一轮只剩「跨文件诊断」这一项，卡点是"预制体 `source_id` 算不算和场景 ID
冲突"。想清楚之后结论是：**这不是冲突，是两个不同的作用域**，而真正缺的是
"作用域内唯一性"的检查和"跨文件引用存不存在"的检查。

### 规则：唯一性的作用域决定了能不能静默修

- **场景文件的 `id` 只在场景内唯一**。它被同文件里的 `parent` 引用，所以
  `Scene::ensure_unique_ids` 可以静默重分配（已知局限：层级可能差一层，但
  "删一个连带删别的"这种破坏性后果不会再发生）。
- **预制体文件的 `id` 只在预制体内唯一**，但被**另一个文件**引用：场景里每个
  实例的 `PrefabInstanceRef::source_id` 指向它。所以在加载边界重分配是**错的**
  —— 会让所有既有场景实例的 `source_id` 集体失配；`Apply` 时
  `capture_subtree_ids` 取不到值就退回 `EntityId::new()`，于是资产里凭空多出
  一份重复实体，越传越广。
- 于是"场景与预制体 ID 重叠"完全合法，跨文件唯一的是 `(source, source_id)`
  这个**二元组**。

`load_prefab` 一直没有 `ensure_unique_ids`，看上去像和场景不一致的疏漏，其实是
对的 —— 本轮把它写成契约（文档 + 测试），免得以后有人"顺手对齐"。

### 新增两条诊断（只读，不改数据）

1. `duplicate_prefab_ids`：遍历 `list_prefabs()` 回到**磁盘原文**统计重复 ID。
   没被任何场景用到的预制体也查 —— 打开它就立刻掉进同一个坑。
2. `dangling_source_ids`：场景里每个 `PrefabInstanceRef.source_id` 必须能在
   对应的预制体里找到。按 `(场景, 预制体)` **聚合成一条**（一个 20 节点实例
   不该刷 20 行），报缺失数量 + 首个缺失实体的名字与 ID。

顺带把重复统计抽成 `id::duplicate_ids()`，场景与预制体共用一份"按首次出现
排序"的实现 —— 两边作用域不同，但"怎么看"必须一致，否则同一份坏数据在两个
入口给出不同顺序的报告。

### 一处自找的坑：输出文案必须纯 ASCII

第一版把文案写成了中文（"里找不到 N 个 source_id（首个: …）"），实跑时在
PowerShell 控制台是乱码。`CheckReport::render` 的契约是**纯 ASCII**（躲控制台
代码页），CI 读的就是这行文本。改成
`missing N source_id(s) (first: <name> -> id <raw>)`。实体名本身含中文不受影响
—— 那是数据，不是文案。

### 红绿与正负对照

- 红灯：临时把两处 `push` 换成 `let _ = …`（等价于旧行为：字段永远为空）实跑，
  `2 failed`（其余 14 条 check 测试全绿，说明只打新行为）。
- 正负对照（真实 CLI，不是单元测试）：
  - 负：把 SampleScene 复制一份，注入 `Prefabs/Ghost.prefab.ron`（内部 id=1）
    与一个 `source_id: (9999)` 的实例 → `--check` 输出
    `dangling source: Scenes/scene.ron: Prefabs/Ghost.prefab.ron missing 1
    source_id(s) (first: StaleInstance -> id 9999)`，退出码 **2**。
  - 正：把 `source_id` 改成 `(1)` → 该行消失，问题数 2→1（剩下的
    `missing .meta` 是手工放文件不带 meta 的预期结果），不误报。
- 全量 `test --workspace --all-targets`：**956 passed / 0 failed / 9 ignored**
  （上轮 954，+2 即两条新诊断）；clippy 0 告警。

### 边界

101 到此关闭。仍然**没做**的：不存在"跨场景 ID 唯一性"这种东西（也不需要）；
预制体解析失败（坏 RON）本轮刻意静默跳过，内容级诊断不在范围。

---

## 七十二、三个 G0 数值 / 交互边界（2026-10-28）

接 041（极小体积）、101（极值 ID）、067（视口外松键）。三项都是"实现缺一条边界"，
不是新功能；共同点是有缺陷时**不报错**，只是行为悄悄不对。

### 1. 极小碰撞体的质量守恒（R12 / 041）

`scene_body` 按 `mass / unit_mass` 求密度。半径 1e-20 的球体积在 f32 下下溢成 0，
density 随之变 inf，rapier 拿它乘体积后**实际质量 = 0** —— 声明 2.5 kg 的实体变成
0 kg，物体会飘走或原地抖。旧实现的 `unit_mass > 0` 只挡住了"别算密度"，没挡住
"算出来的密度不可用"。

改：密度不可表达时（`unit_mass` 非正 / 非有限，或 `density` 非有限）退回**附加质量**
（`RigidBody.mass`），惯量仍按形状几何算。两条路径相加恒为声明值。

红灯：`tiny_collider_still_weighs_the_declared_mass` 在旧实现下（半径 1e-20）实得质量 0；
1e-6 / 1e-9 两档旧实现本来就对，一并钉住。

### 2. 极值实体 ID（101）

`EntityId::new()` 是 `fetch_add`，`reserve` 用 `fetch_max(raw + 1)`。文件里写
`id: (18446744073709551615)` 会把水位顶到 `u64::MAX`，下一次 `new()` 返回**同一个值**
—— 两个实体共用一个 ID，随后 `remove_subtree` 按 ID 匹配就连坐删除。

改：`MAX_ASSIGNABLE_ID = u64::MAX - 1`，读盘严格大于它即返回 serde 错误（错误信息
带原值与上限，便于定位是哪个文件被手改）；`new()` 换成 CAS 循环并跳过 0
（`fetch_add` 回绕会落到 0，而 0 从未被分配过）。

红灯：临时把校验短路，`max_id_is_rejected_at_the_load_boundary` 实得
`Ok(Holder { id: EntityId(18446744073709551615) })`。

边界：`--check` 遇到这种场景会因加载失败退出 1（预期行为）；"跨文件诊断"
（同一 ID 在场景与预制体之间是否冲突）仍未做 —— 预制体的 `source_id` 是跨文件引用，
语义上不该算冲突，要做先得定规则。

### 3. 视口外松手（067）

`UiPointer` 曾把"位置"和"按键"合成一个 `Option`：指针不在视口就整条丢掉。于是
"按下在按钮上 → 鼠标移出视口 → 松手"那一帧的 `released` 边沿消失，捕获残留在按钮上，
`Release` 事件也发不出去（要等指针回到视口、被"幽灵按下"清理兜底才恢复）。

改：`UiPointer.pos: Option<Vec2>`，按键电平 / 边沿始终有效；runtime 永远传 `Some(...)`
（视口尺寸为 0 时仍传 `None`，那表示"没有 UI 可交互"）。新增 `World::ui_pressed()`
与 `ui_hovered()` 对称 —— 捕获状态比 `state.pressed` 更早暴露这类缺陷，因为后者的
可见症状要等指针回到视口才消失。

红灯：`releasing_outside_the_viewport_clears_the_button` 在旧映射下第二帧捕获仍在
（实跑失败）。

顺带补了 `hidden_parent_makes_children_unhittable`：父节点隐藏后子按钮不进命中表
（`layout::walk` 早就是整棵子树跳过，此前没有测试钉住），以及 UI 层的
`holding_outside_the_viewport_keeps_the_capture`（拖出视口再拖回来松手仍算点击）。

### 4. 脚本改 UI 的当帧时序（067 收尾）

`World::update` 里 UI 阶段排在**脚本之前**：`ui_clicked(ctx, id)` 是"本帧"状态，
脚本在 `update` 里读它时点击标志必须已经写好。代价是"脚本本帧挪的矩形本帧不算"——
下一帧才参与命中。这个取舍此前只写在注释里，没有测试钉住。

新增 `script_moved_ui_takes_effect_on_the_next_frame`：第一帧点**新**位置断言不命中
（布局还是旧位置），松手；第二帧点同一点断言命中。为了证明它真的钉住顺序，临时把
`self.update_ui()` 从第 0 步移到 `run_dispatch(Phase::Update, …)` 之后实跑，结果是
`right: None` → `0 passed; 1 failed`（红灯成立），还原后绿。**这一条不改代码**，
只是把已有契约变成回归。

附带效用：以后若有人为了"更跟手"把 UI 调到脚本之后，这条测试会先红。

### 验证与边界

- `check` / `clippy -D warnings` 通过；`test --workspace --all-targets`
  **954 passed / 0 failed / 9 ignored**（上轮 953，+1 即本轮新增的时序回归）。
- 曾观察到**一次** `xgame-render --lib` 失败，随后 6 次单跑 + 3 次全量复跑（共 9 次）
  未复现；结合历史同类现象判为 wgpu 并发建设备的环境抖动，未改代码，如实记录。
- **dist 陈旧**：收尾重建时 `xgame-editor.exe` / `xgame-game.exe` 的字节与旧 dist **不同**
  （`xgame-mcp.exe` 相同），而源码指纹前后一致（`xgame-src-v1:6c6ca992…`）。说明上一轮
  的 dist 虽然与当时的 `target/release` 互相一致，但那两份产物并非当时源码的产物 ——
  「dist 与 target/release 哈希相同」只能证明两者同源，**不能**证明它们新鲜。现在已按
  当前源码重建；紧接着再构建一次，三个 EXE 全部字节相同（可复现构建成立），插件 DLL
  同步刷新至 `4039B487…`。

---

## 七十一、CUBICSPLINE 三次样条采样（R04 / 032 关闭，2026-10-28）

`Docs/差距分析.md` 里 R04 / 032 的表述是"逐关键帧取值与 morph stride 已修；
仍线性降级，Hermite 与多节点动画保真待做"。本轮把 Hermite 做掉，顺带把
"轨道形状"从隐式约定变成显式 API。

### 问题：切线在读入时就被扔掉

`gltf_loader::take_spline_values` 对 `CUBICSPLINE` 只挑每帧中间那份值，入 / 出切线
直接丢弃，运行时退化成线性。数值上"没崩"，但动作的加减速被抹平 —— Blender /
Maya 默认导出的插值就是三次样条，所以这不是罕见分支，而是**大多数资产会被降级**。
而且这是单向损失：切线一旦没进内存，运行时再想补也补不回来。

### 实现

- `Interpolation` 加第三个变体 `CubicSpline`；读入端 `parse_interpolation` **穷举**
  三种映射（不用 `_ =>`），gltf 升级后若新增插值类型会编译不过，逼着做决断。
- `Track::values` 的布局变成"每帧 `frame_len()` 个浮点" = `stride()`（值分量数）×
  `slots()`（每帧组数：样条 3，其余 1）。三个派生量收在 `Track` 上 ——
  `frame_start(k)` / `value_start(k)` / `frame_len()`，"值在中间那份"由
  `Interpolation::value_slot()` 一处表达；采样、加载、测试读同一处，不各算一遍。
- 采样：`hermite_basis(u)` 给 `(h00,h10,h01,h11)`，`eval_vec3` / `eval_quat` 按插值
  分派。切线单位是**每秒**，进 Hermite 前乘段长 `dt`（glTF 规范 C.2）。
- 旋转：逐分量 Hermite 出的四元数不在单位球上，**必须归一化**；长度接近 0 时
  （手工改过的曲线）退回 `slerp`，避免 NaN 顺着蒙皮矩阵扩散到整块网格。
- `sample_morph_weights` 同样按 `slots()` 分组；形状不严丝合缝
  （`values.len() != times.len() * frame_len()`）就返回 `false` —— morph 的 stride
  是现算的，串一位画出来就是一张扭曲的脸。
- 线性 / 阶梯两条路径的读数与从前逐位一致：同样取 `value_start(i0)`、同样 `lerp`、
  阶梯同样令 `u = 0`。

### 一个自己踩的坑

`value_start(k)` 第一版写成 `frame_start(k) + (slots-1) * stride`，于是把**出切线**
当成值读了：段中点实得 1.25 而不是 0.75。`slots-1` 只在"三份里取最后一份"时成立，
而值在中间。改成显式的 `value_slot()` 后才对。这种错不会 panic，只会让动画
"看起来怪"，所以形状必须由测试钉住，不能靠肉眼。

### 红绿

新增 4 条测试，其中 3 条在旧实现下**实跑失败**（`2 passed / 3 failed`）：

| 测试 | 旧实现读数 | 期望 |
|---|---|---|
| `cubic_spline_midpoint_is_hermite_not_linear` | 0.5 | 0.75 |
| `cubic_spline_scales_tangent_by_segment_length` | 0.5 | 1.0 |
| `morph_spline_three_keys_two_targets` | 6 个值（切线被丢） | 18 个值 + 中点 0.45 / 0.55 |

`cubic_spline_rotation_is_normalized` 在旧实现下也过（slerp 本身落在单位球上），
它的作用是钉住新路径：样条出来的四元数必须仍归一，且跨度内不得出现 NaN。
原 `regression_r04_spline_three_keys_vectors_and_rotations` 测的正是"只取中间那份"
这一被推翻的行为，改写为 `spline_keeps_tangents`（断言 `frame_len == 3 × stride`
且第 1 帧三段数据原位保留）。

### 成本

新增基准 `crates/xgame-render/tests/animation_cost.rs`（默认 `#[ignore]`）：
28 骨 × 3 通道 = 84 条轨道、16 帧，单次 `sample_clip_into`（同机 release，
7 轮取最快轮，重复三次）：

| 档位 | 单次采样 |
|---|---|
| 线性 | 2031–2061 ns |
| 阶梯 | 891–902 ns |
| 三次样条 | 1652–1662 ns（0.80–0.81 × 线性） |

样条**比线性便宜**，原因在旋转通道：`Quat::slerp` 要 `acos` 加两次 `sin`，
而 Hermite 是 4 次乘加 + 一次 `normalize()`。所以"保真"这一刀在本实现下不付成本，
反而不亏。数字只用于同机背靠背比较，跨机器无意义。

### 验证与边界

- workspace `check --all-targets` / `clippy --all-targets -- -D warnings` 通过；
  `test --workspace --all-targets` **945 passed / 0 failed / 9 ignored**
  （上轮 942 / 8；+4 新增 −1 改写删除 = +3）。
- 多节点动画由真实 Fox 资产（24 骨）的既有端到端用例覆盖（`compute_bones` 走
  同一条 `sample_clip_into`），本轮未改这条路径。
- **未做**：CUBICSPLINE 的 morph 多网格（仍只取第一条权重轨道，v1 边界，与插值无关）、
  根运动、动画 Phase B。

---

## 七十、G0 资产引用迁移词法保护（2026-10-27）

`--migrate-assets` 原先对已解析出的资产路径做整份文本 `replace`：同一段引号文字出现在注释或实体名称里也被改成 GUID，违背「迁移只改引用且保留注释」的契约。新增场景回归在旧实现实跑失败，显示行注释、块注释与实体名称均被改写。现在先解析文件确认有效与收集引用，再单遍扫描 RON token：跳过行注释、嵌套块注释、字符字面量，识别转义与 raw 字符串，只在资产字段把值存在索引中的路径替换为 GUID；其他文本原字节保留。额外词法回归覆盖嵌套注释、转义字符串及脚本数组，迁移相关 4 项定向测试通过。全工作区 check、clippy -D warnings、test --all-targets 顺序复验通过；最初测试与 release 并行时插件装载测试 3 项指纹不匹配，串行复验未复现。release workspace 与一键构建通过，dist 三 EXE 已刷新；打包示例 --check=0（1 场景、24 实体、15 资产），30 帧无头 ok=true、错误 0、非有限变换 0。沿用现有同目录独占暂存写盘，改动只发生在低频显式迁移路径。

## 六十九、G0 最近项目列表写盘保护（2026-10-27）

项目枢纽的 `RecentProjects::save` 原本直接 `fs::write`，目标文件允许写入但拒绝替换时旧内容可能被截断。Windows 锁定目标回归先在旧实现实跑失败（错误地返回成功）；修复后与 `EditorPreferences::save_to` 共享内部 `write_staged`：同目录独占暂存、`sync_all`、替换目标，失败不动旧文件、解锁可重试。旧版固定暂存残留不阻断，最近项目 4 项、偏好 12 项定向测试通过。读取损坏文件仍退空列表，UI 保存失败仍记录警告。两份修改文件 rustfmt --check 通过，全工作区 check、clippy -D warnings、test --all-targets 全绿。release workspace 完成，dist 三 EXE 与 target/release 各自 SHA256 一致；发布版 SampleScene `--check` 返回 0（1 场景、24 实体、15 资产），30 帧无头 `ok=true`、错误 0、非有限变换 0。仓库整体 `cargo fmt --all -- --check` 仍受其他文件历史格式差异影响，未扩大格式化范围。

## 六十八、G0 编辑器偏好保存与输出目录（2026-10-27）

`outputs/` 是本地构建、截图和验收的混合产物目录，新增文件不应进入版本控制；根级忽略规则覆盖整个目录，已被跟踪的历史截图继续留在 Git 中。修复 `EditorPreferences::save_to`：原实现固定 `editor.ron.tmp` 且替换失败会直接覆写目标，Windows 上目标句柄拒绝删除但允许写入时会突破旧文件保护。改成独占同目录暂存、`sync_all` 后替换；失败不覆盖目标，旧暂存残留不阻断重试。锁定目标回归在旧实现下实跑失败（预期错误却返回成功），新实现偏好 12 项通过。全工作区 `check` 与 `clippy -- -D warnings` 通过，`test --workspace --all-targets` 全绿；本轮文件独立 rustfmt 检查通过，整个仓库 `cargo fmt --all -- --check` 因此前未格式化的其他文件失败（未扩大改动范围）。release 构建刷新 dist 三个 EXE；发布版 SampleScene `--check` 返回 0（1 场景、24 实体、15 资产），30 帧无头 `ok=true`、错误日志 0、非有限变换 0，dist 编辑器 EXE 与 release 源哈希一致。

## 零、引擎与游戏项目分离（已完成）

引擎（本仓库 `crates/`）与游戏项目（磁盘上任意目录）彻底分家。编辑器启动后先进入
**项目枢纽**，选定或新建项目之后才进编辑器主界面；所有资产路径都按**项目根目录**
解析。编辑器与独立游戏都不再依赖进程当前工作目录，项目目录可以随便移动。

```
XGameEngine/                  # 引擎源码仓库
├── crates/                   # 引擎 + 编辑器 + 游戏二进制
└── projects/<项目名>/         # 游戏项目（默认位置，可放到任意目录）
    ├── project.ron           # 项目清单：名称 / 入口场景 / 窗口 / 内容目录
    ├── Scenes/
    └── Resources/
```

关键改动：

| 位置 | 内容 |
|---|---|
| `xgame-core::project` | `Project` 清单 + `ProjectHandle`（open / create / save / 路径解析） |
| `xgame-editor::hub` | 项目枢纽界面：新建 / 打开 / 最近项目（原生目录选择器 `rfd`） |
| `xgame-editor::recents` | 最近项目持久化（`%APPDATA%\XGameEngine\recent_projects.ron`） |
| `xgame-editor::main` | `EditorApp` 拆成「项目枢纽 / 编辑器会话」两态 |
| `xgame-editor::scene_renderer` | 按项目根目录解析 `entity.model` |
| `xgame-game::main` | 接收项目路径，入口场景与资产均按项目根解析 |

> 遗留：`GameRuntime`（`xgame-editor/src/game_runtime.rs`）与 `xgame-game` 的物理
> 初始化 / 同步逻辑仍是两份重复实现。要彻底去重，需要把运行时提到独立 crate
> （见架构原则 3）。

---

## 一、现状：两个入口

当前架构经过两次重构，有两个可运行的二进制：

```
                          xgame-game（新）
                              |
xgame-core                    | 独立游戏二进制
    ↑                        ├── winit + wgpu 原生窗口
xgame-math                   ├── ScenePipeline 渲染
    ↑                        ├── PhysicsWorld 物理
xgame-render                 └── 场景从 RON 加载
    ↑
xgame-physics          xgame-editor（已有）
    ↑                       ├── eframe 窗口 + egui 面板
    └────┬──────            ├── ScenePipeline + 离屏纹理
         │                  ├── GameRuntime 封装物理
    xgame-editor            └── Play/Pause/Stop 模式
    xgame-game
```

### 渲染路径收拢

之前有两个平行的渲染实现，现在统一为 `ScenePipeline`：

```
旧：
  xgame-render::RenderContext（无人使用）
  xgame-editor::SceneRenderer（实际在用，内联 WGSL/pipeline）

新：
  xgame-render::scene_pipeline::ScenePipeline（共享）
    ├── WGSL 着色器
    ├── Pipeline + BindGroup + Uniform buffers
    └── 逐实体遍历 + draw call 发放
  xgame-editor::SceneRenderer → 薄 wrapper（离屏纹理 + egui）
  xgame-game::GameApp → 直接使用 ScenePipeline（swapchain）
```

### 当前能力

| 能力 | 编辑器 | 独立游戏 |
|---|---|---|
| 场景加载 (RON) | ✅ | ✅ |
| glTF 模型渲染 | ✅ | ✅ |
| Blinn-Phong 光照 | ✅ | ✅ |
| 固定步长物理 (60Hz) | ✅ | ✅ |
| 飞行相机 | ✅ | ❌（固定轨道相机）|
| Physics Debug 叠加 | ✅ | ❌ |
| Play/Pause/Stop | ✅ | N/A |
| 实体增删 | ✅（Play 模式守卫）| N/A |
| ScenePipeline 共享 | ✅ | ✅ |

---

## 二、三阶段路线图

```
阶段一（已完成）        阶段二（当前目标）        阶段三（远期）
┌─────────────┐       ┌─────────────┐       ┌─────────────┐
│ 层重构       │       │ 固定步长循环  │       │ 独立游戏二进制 │
│ GameRuntime  │ ───→  │ 编辑器持续集成 │ ───→  │ IPC/分离架构  │
│ 物理集成     │       │ Physics Debug │       │ 完整游戏管线  │
│ 渲染路径统一  │       │ 独立游戏二进制 │       │ 资源浏览器   │
│ 共享管线     │       │              │       │ Gizmo 操作  │
└─────────────┘       └─────────────┘       └─────────────┘
```

---

## 三、阶段二：编辑器强化 + 独立游戏

### 3.1 固定步长物理（已完成）

`GameClock` + `GameRuntime` 内部累加器，物理 60Hz 与渲染帧率解耦。

### 3.2 编辑器期间实体增删保护（已完成）

Play 模式下 Hierarchy 面板禁用增删。

### 3.3 渲染路径统一 + ScenePipeline（已完成）

从 `SceneRenderer` 提取 WGSL、pipeline 创建、uniform 管理和绘制逻辑到
`xgame-render::scene_pipeline::ScenePipeline`。编辑器离屏渲染器和独立
游戏二进制共用同一份渲染代码。

### 3.4 独立游戏二进制（已完成）

创建 `xgame-game` 二进制入口，使用原生 winit + wgpu 循环，
通过 `ScenePipeline` 渲染，直接运行 `Scenes/scene.ron` 中的场景。

### 3.5 Physics Debug 绘制（待完成）

- `GameRuntime` 已暴露 `debug_lines()`
- `PhysicsWorld::debug_lines()` 已实现
- `scene_view.rs` 已有叠加绘制代码
- 需要确认叠加层的视觉正确性（碰撞体线框对齐）

### 3.6 状态栏 Play 模式信息（已完成）

状态栏显示 `Playing | 3 entities | 60.0 FPS | Bodies: 3 | Colliders: 3`

### 3.7 场景层级 Scene Graph（已完成）

`Entity::parent`（按 ID 单向引用）+ `Scene::world_matrix` / `world_matrices` 世界矩阵
解算；层级面板是**可折叠的树**，拖拽重挂父节点保留世界变换并自动防环；渲染 / 物理 /
Gizmo 全部走世界变换；复制 / 粘贴 / 删除是**子树**语义。

### 3.8 预制体 Prefab（已完成）

把一棵实体子树存成 `Prefabs/*.prefab.ron` 资产并反复实例化：

- `xgame-core::prefab`：`Prefab { name, entities }` + `from_subtree()` 提取 +
  `instantiate()` 实例化（重映射 ID / 父引用，并贴上 `PrefabInstanceRef` 来源标记）；
- `Entity::prefab` 记录「来自哪份资产、对应资产里哪个实体、是不是实例根」；
- 编辑器 Prefabs 面板：列资产 / 搜索 / 点击实例化 / 从选中创建 / 删除；
- **Apply** 把实例改动写回资产，**Revert** 用资产覆盖回实例（保留实例根的世界位置）；
- 构建（🔨 Build）会把 `Prefabs/` 一并打进产物。

---

## 四、阶段三：完整游戏管线

### 4.1 资源浏览器

文件浏览器面板，拖拽 glTF 到场景中创建实体。

> 进度：**Prefabs 面板已落地**（列资产 / 搜索 / 实例化 / 删除）；
> 还缺 `Resources/` 里模型资产的列举与"拖进场景"。

### 4.2 Gizmo 变换工具

Scene View 中的拖拽操作柄：平移 / 旋转 / 缩放。

> 进度：三轴平移 / 旋转 / 缩放已可用（世界空间换算回局部）；**Rect 工具**与
> **吸附**（`gizmo_snap`）尚未接线。
>
> 轴拖拽的数学见 [FLOW.md](FLOW.md) 的「Gizmo 的三个坑」——轴上的位移量必须由
> **直线投影**（`axis_param_under_mouse`）求，不能用「过实体、垂直于轴的平面交点」，
> 后者在轴上的投影恒为 0。一次拖拽进撤销栈**一步**（拖拽前快照，松手才入栈）。

### 4.3 编辑器 - 运行时互操作

- 当前：编辑器内嵌 `GameRuntime`，原地运行
- 未来：子进程方案，编辑器启动 `xgame-game` 并通过 IPC 通信

### 4.4 后续大特性

按开发优先级：

1. **动画系统**：glTF 动画通道解析、关键帧插值、骨骼蒙皮
2. **音频系统**：kira/rodio 集成、3D 空间音频
3. **后处理**：阴影（CSM）、Bloom、ToneMapping、天空盒
4. **脚本系统**：Lua/Rhai 嵌入或 Rust 热重载

---

## 五、架构原则

1. **层间单向依赖**：core ← math ← {render, physics} ← editor / game
2. **渲染路径共享**：ScenePipeline 是唯一渲染入口，编辑器（离屏）和游戏（swapchain）共用
3. **GameRuntime 不依赖编辑器**：核心业务逻辑与 UI 解耦（当前位于 editor crate，后续可提取）
4. **固定步长优先**：物理 / 未来动画系统使用固定时间步长
5. **功能先于性能**：先跑通，再优化
6. **引擎与项目解耦**：引擎不假设游戏内容在哪；`project.ron` + `ProjectHandle`
   是访问资产的唯一入口，资产路径一律相对项目根目录解析
7. **编辑器是工具，不是项目的一部分**：编辑器二进制可打开磁盘上任意项目，
   游戏项目目录不出现在 `crates/` 里

---

## 六、文件清单

### 已完成

| 文件 | 阶段 | 说明 |
|---|---|---|
| `crates/xgame-core/src/transform.rs` | 一 | Transform 从 xgame-math 移入 |
| `crates/xgame-core/src/scene.rs` | 一 | Entity/Scene 从 xgame-render 移入 |
| `crates/xgame-editor/src/game_runtime.rs` | 一 | 编辑器无关的游戏运行时 |
| `crates/xgame-render/src/scene_pipeline.rs` | 二 | 共享渲染管线（WGSL + pipeline + uniform）|
| `crates/xgame-game/Cargo.toml` | 二 | 独立游戏二进制 |
| `crates/xgame-game/src/main.rs` | 二 | 独立游戏循环（现按项目根解析资产）|
| `crates/xgame-core/src/project.rs` | 零 | 项目清单 + `ProjectHandle` |
| `crates/xgame-editor/src/hub.rs` | 零 | 项目枢纽界面 |
| `crates/xgame-editor/src/recents.rs` | 零 | 最近项目持久化 |
| `projects/SampleScene/` | 零 | 从仓库根迁出的示例项目（原 `Scenes/` + `Resources/`）|

### 后续需创建

| 文件 | 说明 |
|---|---|
| `crates/xgame-editor/src/panels/resource_browser.rs` | 资源浏览器面板 |

### 后续需修改

| 文件 | 说明 |
|---|---|
| `crates/xgame-editor/src/panels/state.rs` | Gizmo 状态 |
| `crates/xgame-editor/src/panels/scene_view.rs` | Gizmo 交互叠加层 |

---

## 七、商业引擎对齐（本轮）

对照商业引擎补齐了一批最显眼的短板，详见 `ROADMAP.md` 的「商业引擎对齐」表。

关键改动：

| 位置 | 内容 |
|---|---|
| `xgame-core/src/light.rs` | 新增 `Light` / `LightKind`；光源是场景级数据 |
| `xgame-core/src/scene.rs` | `Scene` 增加 `lights` / `ambient`（`#[serde(default)]` 兼容旧文件）|
| `xgame-render/src/scene_pipeline.rs` | 引入 `group(1)` 材质绑定组、Blinn-Phong、多光源数组、透明管线 |
| `xgame-render/src/gltf_loader.rs` | 读 glTF 材质因子（base color / metallic / roughness / alphaMode）；新增 `GltfMesh::load` 统一入口 |
| `xgame-render/src/mesh.rs` | 新增内置图元生成（Cube / Sphere / Plane / Cylinder）与 `BuiltinMesh` |
| `xgame-editor/src/state.rs` | 新增 `UndoStack`（快照式撤销）、实体剪贴板、`create_primitive` |
| `xgame-editor/src/panels/inspector.rs` | 所有属性编辑接入撤销栈；新增内置图元快捷切换 |
| `xgame-editor/src/panels/hierarchy.rs` | 创建菜单加入 Create Primitive；新增 Copy / Paste 与快捷键 |

> 渲染管线改动是**破坏性 API 变更**：`ScenePipeline::render` 现在多收
> `device` 与 `SceneLighting` 两个参数，调用点（`scene_renderer.rs`、
> `xgame-game/src/main.rs`）已同步更新。

---

## 八、玩法层（本轮完成）

到目前为止引擎"能摆场景"，但还"不能做游戏"—— 缺的是**输入**、**脚本**，
以及一个把各子系统编排起来的**运行时**。这一轮的四个新 crate：

```
core ← math ← {render, physics}
  ↑        ↑
  └── input / script        ← 设备抽象与脚本宿主
             ↑
         runtime            ← 编排：场景 + 物理 + 输入 + 脚本
             ↑
     editor(Play) / game    ← 两个宿主共用同一段 update
```

| 位置 | 内容 |
|---|---|
| `xgame-input` | `KeyCode`/`MouseButton`（与 winit 解耦）、`InputState`（按住 / 本帧按下 / 本帧松开）、`InputMap`（项目级 `input.ron`：多绑定 / 修饰键组合 / 双向轴）；窗口库翻译只在接窗口的二进制里发生 |
| `xgame-script` | 嵌入式 Rhai；`ScriptHost` 负责编译缓存、生命周期派发、命令回收；宿主 API 覆盖变换 / 实体 / 物理 / 输入 / 时间 / 日志；`state_*` 提供按 `(实体, 脚本)` 分开的持久状态 |
| `xgame-runtime` | `World`：唯一编排者。固定步长与渲染帧解耦；**动态体由物理驱动、运动学体由脚本驱动**；`FrameReport` 供 UI 读 |
| `xgame-core` | `Entity::{scripts, physics}`、`PhysicsBody`/`CollisionShape`/`BodyKind`、`PhysicsHandle`（运行时组件）、`Ray`/`RayHit`、`Light::{casts_shadow, shadow_resolution}`、`ContentConfig::scripts_dir` |
| `xgame-physics` | `scene_body::body_from_scene`（声明 → rapier 刚体）、`raycast` / `raycast_with_filter` / `RaycastFilter`、`set_velocity` / `teleport` / `set_rotation` / `sync_queries` |
| `xgame-editor` | `game_runtime.rs` 变成 `World` 的薄壳（场景所有权用 `mem::swap` 交接，O(1)）；Inspector 的 Scripts / Physics 区块；Hierarchy 徽标；工具栏 `⟳ Scripts`；`input_bridge` 把 egui 键鼠灌进运行时 |
| `xgame-game` | 改用 `World`；窗口事件翻译成引擎输入；读项目 `input.ron`；脚本日志转发到 `log` |

### 两个刻意的取舍

1. **场景所有权用 `mem::swap` 交接，而不是把场景包进 `Rc<RefCell<_>>`。**
   `World` 拥有场景，但编辑器几十处面板代码都直接读 `state.scene`。改所有权模型
   要动几十个调用点；包 `RefCell` 会把借用检查推到运行期。交换整个 `Scene` 是
   **O(1)**（换三个指针），比每帧克隆整份场景便宜几个数量级。
   前提是"`update` 期间没人读 `state.scene`"——这一点写在 `update_with_scene` 的注释里。

2. **脚本的结构性改动立即生效，而不是攒成命令延后执行。**
   延后执行会让 `let id = spawn_entity(...)` 之后的 `set_entity_pos(id, ...)`
   静默失效 —— 而"先生成、再摆位置"是最自然的写法。立即执行之所以安全，
   是因为派发脚本时遍历的是一份 `(实体 ID, 脚本路径)` 快照，且所有查找都按 ID，
   增删实体最多让后面的脚本少看到一个实体。运行时通过一份**通知**
   （`EntitySpawned` / `EntitiesRemoved` / `Reparented`）知道该补建 / 清理哪些刚体。

<!-- markdownlint-disable-file -->

---

## 九、渲染质量：方向光阴影（本轮）

`Light::casts_shadow` / `shadow_resolution` 从"已进场景、渲染侧没接"变成真的生效。
做法是经典 shadow mapping，落在共享的 `ScenePipeline` 里 —— 编辑器离屏渲染与独立
游戏因此**同时**获得阴影，不需要各自接一遍。

| 位置 | 内容 |
|---|---|
| `xgame-render/src/gltf_loader.rs` | 新增 `Bounds`（AABB）；`GltfMesh` 记录本地包围盒（三条构造路径都填） |
| `xgame-render/src/scene_pipeline.rs` | 阴影深度图 + 比较采样器 + `ShadowUniform`；独立的深度-only 阴影 pass 与 `shadow_bgl` / `shadow_pipeline`；`GpuLight.shadow_params` 标记使用阴影的灯；主片元用 3×3 PCF；纯函数 `scene_bounds` / `fit_directional_shadow` / `pick_shadow_slot` |
| `xgame-editor/src/scene_renderer.rs` | 指纹加入 `casts_shadow` / `shadow_resolution`（漏掉的话勾选后画面不动） |

### 三个刻意的取舍（别推翻）

1. **正交视锥按"场景世界包围盒的外接球"拟合**，而不是固定大小的盒子。阴影贴图分辨率
   固定，视锥开太大 → 精度被稀释成噪点；开太小 → 边缘物体没影子。用球（而不是贴合
   方向的盒）是因为光源角度任取时球在各个方向投影都一样大，不必为每个角度重算 ——
   代价是浪费一些纹素，对"先把阴影做出来"是划算的。
2. **只支持一盏方向光**（第一盏 `casts_shadow` 的方向光）。多盏各自的阴影贴图意味着
   每盏一次全场景重绘。点光阴影（6 面立方体贴图）明确不在这一版范围内。
3. **阴影 pass 用独立的 `shadow_bgl`**，不含深度贴图本身。若沿用全局布局，就会在
   "把深度画进 `shadow_view`"的同时又在 group(0) 里绑同一张图 —— wgpu 判为
   "同一资源既作渲染目标又作绑定"。

### 验证

- 单测 +12：`Bounds` 求并/变换/空盒退化；`fit_directional_shadow` 把场景包围盒的
  八个角点全部映射进 NDC `[-1,1]²` 与深度 `[0,1]`（**这是阴影能工作的前提**）、
  深度顺序与"离光源远近"一致、零方向不产生 NaN、空场景返回 `None`；
  `pick_shadow_slot` 跳过点光与 `casts_shadow=false`；`build_lights` 把标记落在
  正确的下标上。
- 真机 A/B：同一场景、同一机位，只切 `casts_shadow` → 14343 像素变化，且差异区域
  **正好**是柱子在地面上的投影范围。
- 编辑器打开示例项目：60 FPS、无 wgpu 校验错误。

### 遗留

- **无色调映射**，太阳强度一高地面就过曝成纯白（示例项目 `intensity: 3.2` 就是）。
  这属于第九阶段剩下的 ToneMapping（ACES）。
- 阴影没有级联（CSM），大场景远处会糊。

---

## 十、稳定性与交互修复（本轮）

用户报「界面卡顿 / 物体拖拽抽搐」。先量后改：给管线与编辑器加临时探针，用真机悬停流
和关 vsync 的播放器拿到数据，再逐个定位。

### 一、找到并修掉的真 bug

**1. 轴拖拽是正反馈 —— 这就是"拖拽抽搐"的根因**

`axis_param_under_mouse` 量的是「过 `ctx.entity_pos`、方向为轴」的直线，而 `ctx` 每帧
按**已被拖走的**当前位置重建 → 参考线跟着物体跑，`s_now` 里混进"物体自己刚挪的那一段"：

```text
Δ_k = A_k − Δ_{k-1}     ← 增益 −1 的正反馈，极点 z = −1（奈奎斯特）
```

物体会在相邻两帧之间**来回跳**，稳态只走到鼠标位移的一半，还把鼠标的像素级噪声放大成
可见抖动。修法：`GizmoDrag` 新增 `axis_anchor`（拖拽起点的世界位置），轴坐标一律以它
为参考线原点（`axis_param_anchored`）。

**2. 拾取用包围球 —— "想选的选不中、想取消的取消不掉"**

旧实现半径取 `scale.length() * 0.5`。一块 `16 × 0.5 × 16` 的地面半径算成 11.3，而相机
离场景中心只有 8.7 —— **相机落在球内部**，任何射线都命中它：点天空也选中那块地面。
改成「网格本地包围盒（`GltfMesh::bounds`）经世界矩阵变换成 AABB」后做 slab 求交。

**3. 选中索引可能越界 → 编辑器直接 panic 退出**

`selected_entity` 是按位置记的索引，而改实体数量/顺序的操作有十几处（撤销 / 删除 /
复制 / 粘贴 / 预制体实例化与还原……）。任何一处漏同步，下一帧 `entities[idx]` 都会
panic。新增 `EditorState::clamp_selection()` 在**每帧入口**夹一次（O(1)），并让
gizmo 路径改用 `get` / `get_mut` 兜底。

**4. 阴影视锥半径不封顶 → 有物体飞远时深度精度崩掉**

示例项目里就有自由落体掉出视野的箱子。包围盒被它拉大 → 深度范围拉长 → 固定分辨率的
阴影贴图精度当场崩。加 `MAX_SHADOW_RADIUS` 封顶（超出范围的不投影）。

**5. `hub_is_quiet_when_idle` 是不稳定测试（既有）**

枢纽里的 `ScrollArea` 滚动条淡入动画期间会请求重绘（正常、短暂），而旧测试用墙钟只跑
4 帧 —— 帧快就"动画没结束"、帧慢就"已结束"，于是时灵时不灵（实测单独跑必挂、和几十个
测试并行反而常过）。改成给 egui 显式时钟推进到动画结束再断言。

### 二、测出来的结论（都没问题，附证据）

| 项目 | 数据 |
|---|---|
| 阴影 pass 的代价 | 同一静态场景、关 vsync，只切 `casts_shadow`：p50 **0.32ms vs 0.29ms** —— 差 ~0.03ms，可忽略 |
| 悬停时的离屏重画 | 666 次鼠标移动里**只重画 1 次**；渲染目标只重建 1 次（初始） |
| 应用侧每帧开销 | 无热点；`dock.rs` / `theme::install` / 面板都没有每帧磁盘 IO 或整场景克隆 |

遗留（外部因素，非应用代码）：

- 拖动窗口时 Windows 的模态移动循环（`WM_ENTERSIZEMOVE`）会节流出帧 —— winit 0.30 上游
  问题。
- 窗口被遮挡时 DXGI present 会阻塞（实测：无人值守跑播放器 6 秒只有 272 帧、dt 被
  250ms clamp 反复触发），前台运行不受影响。

### 三、验证方式的一个新教训

真机投递合成鼠标输入在这个环境里**不可靠**：同一脚本同样的坐标，两次运行结果不同
（一次点中、一次没反应），`SetForegroundWindow` 在调用方不是前台进程时会静默失败。
所以交互类修复以**进程内测试**为准（`egui::Context::run` 合成事件驱动 `show_content`），
真机只做"能起来、渲染正常、无 GPU 校验错误"的冒烟。

配套要求：**每条回归测试都必须先证明它在旧实现下会红**。本轮 4 条新测试全部做过这个
对照（临时把实现改回旧写法，确认失败，再改回来）。

---

## 十一、手感优化：MSAA + 帧序 + 飞行相机（本轮）

用户报「右键转视角 + 同时移动相机时抽搐卡顿、画面锯齿严重、拖拽有严重延迟」。
三件事三个原因，都不是"性能不够"：

### 一、画面锯齿严重 → 全流程 sample_count = 1

一次都没开过 MSAA。现在：

- `ScenePipeline::new(device, format, sample_count)`；`render()` 多收一个
  `msaa_color_view`，开了就把它当颜色附件、把最终视图设为 `resolve_target`。
- 采样数**由适配器能力决定**（`supported_sample_count`）：不支持的格式硬开 4×
  会让管线创建直接失败。颜色附件与深度附件的采样数必须与管线一致。
- 编辑器：多采样颜色 + 多采样深度 → resolve 到交给 egui 的**单采样**纹理（egui 只认
  单采样）。游戏：多采样颜色 → resolve 到**交换链视图**（交换链视图本身不能多采样）。
- 阴影 pass 单独一条管线、永远 1×（只存深度，不需要抗锯齿）。

**验证**：同一场景同机位，旧（无 MSAA）vs 新（4×）。边缘放大图肉眼可见台阶消失；
场景视图上半区（避开 gizmo）的"硬台阶"像素数 438 → 307（−30%）。

### 二、拖拽有严重延迟 → 离屏渲染排在输入处理**之前**

`show_content` 原来是「先重画 3D → 再处理输入」，于是这一帧画出来的是**上一帧**的
相机与场景 —— 拖物体、转视角都白白多一帧延迟，手感发"黏"。

改成「飞行相机 → 交互（gizmo / 拾取 / 快捷键）→ 才重画 → 叠加层」。叠加层必须画在
3D 之上，所以 gizmo 被拆成"更新状态"与"绘制"两步（`handle_gizmo_interaction` 现在
只改状态并返回 `GizmoCtx`）。

**验证**：`the_frame_is_rendered_after_this_frames_camera_input` —— 把渲染那一刻用的
`view_proj` 记进 `EditorState::last_render_view_proj`（`#[cfg(test)]` 字段），断言它等于
本帧输入处理完之后的相机矩阵。旧帧序下这条会红（已对照验证）。

### 三、转视角抽搐 → 飞行相机自身的两条规则

1. **旋转不再逐帧要求"指针压在视口上"**。飞行态是"右键按下且指针在视口里"那一刻开启
   的；开启之后仍逐帧用 `hovered()` 当闸门的话，光标被带出视口时旋转**卡住**、移回来
   又**跳一下** —— 快速转视角就是"抽搐"。现在飞行态一开就一路转到松手。
2. **位移改用 `stable_dt`（平滑真实帧间隔），不用 `predicted_dt`**。后者是"下一帧大概
   多久"的预测，预测不准时同一段按键时长走出不同距离，速度一冲一顿。编辑器的物理步进
   与 FPS 读数一并换成 `stable_dt`（FPS 用预测值会把卡顿藏起来）。

副带影响：`fly_state` 现在是**每帧由右键推导**的，`tool_shortcuts_yield_to_fly_camera`
测试里"直接写死 `fly_state`"的写法不再成立，已改成真的按住右键（更贴近真实路径）。

**验证**：`check` / `clippy` 0 警告；`test --workspace` **148 全绿**；真机截图：
编辑器与播放器都 `MSAA x4`、无 GPU 校验错误。

---

## 十二、全面硬化：稳定 / 流畅 / 性能（本轮，无新功能）

用户要求"全面完善整个引擎，暂不添加新功能，先做到稳定、流畅、性能强劲"。
所以这一轮**不加任何能力**，只做三件事：先量后改、把每帧的重复开销拿掉、把会崩 /
会涨的地方堵上。

### 一、先建量尺（这是本轮最重要的产出物）

新增 `crates/xgame-runtime/tests/update_throughput.rs` —— 一个分四档的微基准，
默认 `#[ignore]`，手动跑：

```sh
cargo test -p xgame-runtime --release --test update_throughput -- --ignored --nocapture
```

刻意分档的理由：只跑一档很容易把 **rapier 的耗时误记到引擎头上**。
`纯视觉` 档把物理完全排除在外，量的才是引擎自己的每帧开销。

| 场景（2000 实体） | 改前 | 改后 | |
|---|---|---|---|
| 纯视觉（无物理） | 226 µs/帧 | **13 µs/帧** | ≈17× |
| 全静态体 | 1026 µs/帧 | **89 µs/帧** | ≈11.5× |
| 稀疏动态 ×64 | 1685 µs/帧 | **724 µs/帧** | ≈2.3× |
| 密集接触 ×1000 | 13989 µs/帧 | 13005 µs/帧 | 物理主导 |

**第四行同样重要**：1000 个互相挤压的动态体吃掉 13 ms/帧，那是 rapier 求解器的成本，
不是编排层的。这一条把"下一步该优化哪儿"讲清楚了 —— 想在那档提速得动作业系统 /
减少接触岛，不是继续抠编排。

### 二、运行时热路径（收益主要来自这里）

1. **`sync_bodies` 的稳态快路径。** 旧实现无条件跑三段扫描（补建 + 建存活 `HashSet` +
   清理），最后还无条件 `sync_queries()` 重建射线 BVH —— 而 `physics.step()` 每步已经
   维护了查询流水线，稳态再重建一次纯属浪费。现在先做**一次无分配扫描**同时数出
   "待补建"与"物理实体总数"，两者都表明集合没变就整段跳过。
   判据完备性写在 `sync_bodies` 的文档注释里（新增 / 删除 / 一增一删抵消 三种情形都覆盖）。
2. **`push_kinematic_bodies` / `pull_dynamic_bodies` 早退。** "场景里有没有运动学 /
   动态体"由上面那次扫描顺带记进 `has_kinematic` / `has_dynamic`，两步据此直接返回 ——
   纯静态场景连世界矩阵都不解算，也不再各自扫一遍全场。
3. **`world_matrices_with_parents()`。** `pull_dynamic_bodies` 每步两样都要（父索引取父
   矩阵、世界矩阵做局部换算），旧写法先 `parent_indices()` 再 `world_matrices()`，而后者
   内部又要算一遍父索引 —— 每个固定步白跑一次 `hierarchy()`。
4. **`world_matrices` 自身两处。** (a) 链缓冲从循环内提到循环外复用 —— 平铺场景
   （很常见）原来每算一个实体就为一条长度 1 的链分配一次，整次调用 n 次小分配；
   (b) 输出用 `Mat4` + done 位图取代 `Vec<Option<Mat4>>`（`Mat4` 没有 niche，
   `Option` 会从 64 字节涨到 80）。
5. **`Scene::parent_indices()` 不再白建子表。** 原来它转调 `hierarchy().0`，而
   `hierarchy` 会顺手 `vec![Vec::new(); n]` 建一整张子表 —— 平铺场景全是空 `Vec`，
   纯浪费。

### 三、渲染与编辑器热路径

6. **模型矩阵批量上传。** `ScenePipeline::render` 原来每个实体一次 `queue.write_buffer`
   （每次都要从 staging belt 领一段 + 记一条拷贝命令，256 实体就是 256 次）。
   现在攒进管线持有的复用 `Vec<u8>`（按 `uniform_alignment` 排布），最后**一次**写完。
7. **指纹不再需要先解算世界矩阵。** `SceneRenderer::scene_signature` 原来收世界矩阵，
   而算世界矩阵要先跑一遍 `world_matrices()`（几条长度为 n 的 `Vec` + n 次矩阵连乘），
   是**每一帧**（哪怕什么都没动）都要付的固定成本。世界矩阵是 `(局部变换, 层级)` 的
   纯函数 ⇒ 改哈希「局部变换 + 父引用」是等效的，却让静态帧的指纹变成零分配、零矩阵
   运算。父引用必须一起哈希，回归测试 `signature_notices_reparenting` 盯这条
   （已做"去掉父引用即变红"的对照验证）。

### 四、稳定性

8. **`PhysicsWorld::add_collider` 不再 panic。** 旧实现 `.expect("unknown body")` ——
   一次把 body_id 传串（或脚本在刚体已删之后挂碰撞体）就带走整个进程。现在返回
   `Option`、记警告、退化成"少一个碰撞体"。配套测试确认不 panic 且不留残留记录。
9. **控制台消息表加上限（1000 条，成批丢最旧）。** 去重只挡得住"连续完全相同"的消息；
   一个每帧打一条**不同**日志的脚本能在几分钟内把表撑到几十万条。成批丢（200 条/次）
   而不是逐条 `remove(0)`：后者是 O(n) 整表搬运，而 `log` 在刷屏时就是每帧热点。
10. **复核过、判定不需要动的两处**（记录决策，免得下次又查一遍）：
    - `draw_physics_debug_lines` 只在用户显式打开 `show_physics_debug`（默认 false）时
      才跑，不在常规热路径上；改成复用缓冲要把 `&EditorState` 变 `&mut`，风险大于收益。
    - 层级面板的 `scene.hierarchy()` 里 `vec![Vec::new(); n]` 只有 **1 次**堆分配
      （`Vec::new()` 不分配堆），不是热点。面板真正的成本是 egui 逐行渲染，
      那属于"面板虚拟化"，不在本轮范围内。
    - 撤销栈本来就有 100 步上限；模型 / 材质缓存按"每个模型路径只加载一次"有界，
      当前没有模型热重载路径，所以不会无界增长。

### 五、验证

- `check` / `clippy --workspace --all-targets`：**0 警告 0 错误**。
- `test --workspace --all-targets`：**154 全绿**（新增 6 条：控制台上限、clamp_selection、
  `add_collider` 不 panic、指纹认父引用、指纹认旋转/缩放、深链世界矩阵、成环世界矩阵）。
- 归因对照：`signature_notices_reparenting` 已做"去掉父引用即变红"验证。
- 基准：见上表；`dist/` 刷新后做真机冒烟（编辑器打开示例项目正常渲染、无 GPU 校验错误）。

---

## 十三、交互延迟：空闲空转 + 缩放重建（本轮）

用户报「窗口操作有延迟、拖拽物体有延迟、窗口拖拽会卡顿」。
上一轮我把"拖窗口卡顿"归给了 winit 上游，这次用**先量后改**推翻了这个结论 ——
真正的原因是编辑器**从来没闲下来过**。

### 一、怎么找到的（方法本身值得记下来）

1. `cpu_probe.py` 量空闲 CPU：**5.2–7.2%**。一个什么都不做的编辑器不该吃 CPU。
2. 加临时探针量"每秒帧数"：**空闲时 61 FPS**，且 `renders=0`（3D 没重画）——
   说明是 egui 在空转，不是 3D。
3. 逐段插桩问"谁请求的重绘"：全都不认账 —— 说明请求发生在**我们的 UI 代码之外**。
4. 换成 **egui 自带的 `Context::repaint_causes()`**（0.30 有，带调用点 `file:line`），
   直接指名道姓。这一步把"猜"变成了"看"。
5. 再量"每帧有几个输入事件"：**空闲时约 160 个/秒**，且种类只有一种 ——
   `PointerMoved([1073.0 517.0])`，**坐标始终不变**。

### 二、根因与修法

**Windows 在指针静止时会反复投递同一坐标的 `WM_MOUSEMOVE`**，egui-winit 逐条转成
`PointerMoved`；而 egui 在帧末只要看到「本帧有输入事件」就请求**立即**重绘
（`InputState::wants_repaint_after` 的 `!events.is_empty()` 那一支）。于是：

```text
WM_MOUSEMOVE(同一坐标) → PointerMoved → egui 认为"要再画一帧"
   → 编辑器每 16.7ms present 一次 → 窗口的模态移动/缩放循环抢不到消息线程
   → 拖窗口卡顿、拖拽/缩放有延迟
```

**修法**（`drop_redundant_pointer_moves`）：在 `EditorApp::update` 开头丢掉**位置与
上次放行位置相同**的 `PointerMoved`。位置相同的指针事件不携带任何新信息 —— egui 的
指针状态本来就停在那儿，所以这是语义上的 no-op。`PointerGone` 时把记录清空，
否则"离开再回到同一点"会被误判成没动、悬停从此失效。

> 实测：空闲 CPU **5.2–7.2% → 0.00–0.20%**；编辑器真正闲下来了。
> 交互侧没有退化：合成悬停时 `events≈59/s`（真实移动逐帧放行），拖拽正常。

### 三、顺带修掉的第二处：缩放窗口每帧重建离屏目标

`set_viewport_size` 在拖边框时每帧都被调，旧实现每次都重建 3 张纹理
（单采样颜色 / **MSAA 颜色（4 倍采样，最贵）** / 深度）+ 注册一张新 egui 原生纹理 +
释放旧的；尺寸一变指纹必变，于是还要每帧全量重绘一次 3D。

**实测一次拖拽（约 1.7 秒）：重建离屏目标 43 次、重绘 43 帧。**

改成**节流重建**（`plan_resize` 纯函数 + `RESIZE_REBUILD_INTERVAL = 100ms`）：

| 情况 | 动作 |
|---|---|
| 还没有渲染目标（首帧） | 立刻重建（否则第一帧是空白） |
| 尺寸与上一帧请求相同（= 用户松手了） | 立刻按最终尺寸重建 |
| 距上次重建已超过 100ms（还在拖） | 重建一次，让视口保持约 10Hz 更新 |
| 其余（还在拖、又刚重建过） | 本帧跳过 3D，画面由调用方拉伸上一帧纹理 |

> 实测：**43 次 → 1 次**。缩放结束后截图确认视口比例正确、无拉伸残留、阴影正常。

### 四、验证

- `check` / `clippy --workspace --all-targets`：0 警告 0 错误。
- `test --workspace --all-targets`：**161 全绿**（新增 7 条：指针过滤 2 条 +
  缩放节流状态机 5 条）。指针过滤那条已做"把过滤器改成 no-op 即变红"的对照验证。
- 真机：空闲 CPU 5.2–7.2% → 0.00–0.20%；缩放重建 43 → 1；缩放后画面正确。

### 五、下一层能做的（本轮没做，先记下）

- 窗口**移动**期间的出帧仍然由 winit 的模态循环决定（上游行为）。若要进一步压到
  零，需要 `SetWindowLongPtr` 子类化窗口、拦截 `WM_ENTERSIZEMOVE`/`WM_EXITSIZEMOVE`
  主动停帧 —— 平台相关且侵入，收益待量。
- 游戏侧（`xgame-game`）本来就是连续出帧，未受影响。

## 十四、帧调度器：frame_policy（本轮）

### 一、问题

用户报"鼠标没事件时 FPS 不刷新、编辑器用起来还是发涩"。定位：上一轮修掉空闲空转后，
编辑器退化成**纯事件驱动**（eframe 默认）—— 全工程只剩 Play 模式与飞行相机两处
`request_repaint`，Edit 模式下"来一个事件才算一帧"，鼠标一停画面冻住。

### 二、方案

- `crates/xgame-editor/src/frame_policy.rs`：三档策略（Always / Hybrid / Reactive，
  `XGAME_FRAME_POLICY` 切换）+ `FrameClock`（宽限期 220ms）+ 纯函数 `plan()`。
  决策输入：`playing` / `interacting`（指针任意键按住）/ `had_input` / 宽限期。
- `frame_probe.rs`：`XGAME_FRAME_LOG` 每帧一行帧间隔日志（探针**不用 BufWriter**，
  `taskkill /F` 收尾会丢尾段数据）。
- `EditorApp::update` 统一决策，**全工程唯一的主动 `request_repaint`**；Play 模式与
  飞行相机的散装请求全部移除。
- 顺带修掉：`drop_redundant_pointer_moves` 同时丢弃 `Event::MouseMoved`（原始位移，
  传感器噪声载体，实测指针静止时以它续命宽限期 ~16fps）。

### 三、实测（同机同脚本三档对比）

- 拖拽：Hybrid 61.2fps / p99 35.6ms ≈ Always（60.0 / 34.7）；Reactive（旧）58.6fps
  但死时间 30%（发涩的量化）。
- 空闲：Hybrid 帧数约为 Always 的 1/3；完全无输入时整段归零（实测 6.27s 零帧空档）。
- 测量注意：这台机器有真实指针活动（漂移 + 偶发按键），idle 平均 fps 会被污染，
  判"是否真停"要看有没有连续零帧空档。

### 四、验证

- clippy 0 警告；workspace 测试全绿（新增 frame_policy 7 条 + MouseMoved 过滤 1 条，
  后者做过"禁用过滤即红"的对照）。
- 真机冒烟：无 panic、无 GPU 校验错误。

### 五、遗留

- ~~p90≈33ms 的双峰（vsync 相位）三档一致，属 eframe/vsync 层，策略管不到。~~
  已在 10-12 第三轮定位为交换链 acquire+present 双重等待（33ms 输入盲区），
  以 `present_mode = AutoNoVsync` 治本，见 FLOW「上屏节奏」节。
- eframe 层"事件到达即请求重绘"无法从应用侧拦截（噪声事件仍会各触发一帧，
  但不再连锁续命）。

---

## 十五、双轨语言架构 P0：门面 + 方法层 + Rust 组件 + 插件边界（本轮）

按 `Docs/双轨语言架构计划.md` §五 的六件事执行，**方法层先行**（其余五项都依赖
它的形状，现有 Rhai 测试当平移回归网）。六件事全部完成，验收七条全过。

### 一、六个产物

1. **`GameCtx` 方法层**（`xgame-script/src/ctx.rs`）：api.rs 约 750 行的函数体
   平移为 `impl GameCtx` 方法（含 辅助函数 + RayHit）；`api.rs` 每个函数退化为
   ≤3 行转发壳；`ScriptCtx` 瘦身为 { GameCtx 地址束, rhai state }；
   `host.rs::run_hook` 构造 GameCtx。15 条 Rhai 测试语义零改动，全绿。
2. **`xgame` 门面 crate**：`ENGINE_VERSION`（版本握手基准）+ prelude 拍平深路径；
   `xgame-game/main.rs` 的 7 行 `xgame_render::`/`xgame_script::` 直引清零。
3. **`Lifecycle` trait**（`xgame-runtime/src/lifecycle.rs`）：四钩子默认空实现，
   与 Rhai `Hook` 同名同语义；`Ctx` 是 `GameCtx` 的类型别名。
4. **组件分发**（`world.rs`）：`run_dispatch` 平行加组件路径，沿用字段级拆借；
   `on_destroy` 接线在命令消费时（实体已离场，与脚本轨同款时机）。
5. **注册表 + `Entity.behaviors`**：`register::<T>("Name")`；behaviors 与 scripts
   完全同款的 serde 兼容模式，旧场景文件加载测试通过。
6. **插件边界**（`xgame/src/plugin.rs` + `examples/spinner-plugin`）：
   `export_plugin!` 宏生成 `unsafe extern "C" fn xgame_plugin_entry`；
   `#[repr(C)] PluginHost { EngineApi{version, log}, registry 句柄 }`；
   版本码 = `0x58` 魔数 + 三段 8 位（永不为 0，不与错误码撞）；`load_plugin`
   双侧握手（插件拒载 + 日志报自身版本，宿主翻译成可读报错）。

### 二、刻意的取舍（三处对计划文档的偏离，已回写其 D6）

1. **`GameCtx` 落在 `xgame-script` 而非计划文档 C1 写的 `xgame-runtime`**：
   api.rs 转发壳要调 GameCtx，放 runtime 会形成 script → runtime 反向依赖；
   script 现有依赖恰好覆盖 GameCtx 所需，零新增。
2. **`EngineApi` 只带 `version` + `log`**，不带 C4 列的 pos/set_rot 等 ~10 函数：
   C1 落地后那些**全是 `Ctx` 的方法**，钩子本来就拿到 `&mut Ctx` —— 再铺裸函数表
   等于同一套能力两份真相源。只留钩子之外（入口期）用得到的东西。
3. **组件热路径零哈希**：钩子函数指针在装配期拷进 `BehaviorInstance`，`start`
   标记从 `HashSet<EntityId>` 下沉到组件自身。理由与数据见下节。

### 三、先量后改：组件分发微基准（验收第 7 条）

新增 `crates/xgame-runtime/tests/behavior_dispatch_throughput.rs`（`#[ignore]`，
1000 实体纯视觉三档：无轨道基线 / no-op 组件 / no-op Rhai）：

| 档 | µs/帧 |
|---|---|
| 无轨道基线 | 4.36 |
| 组件轨分发（扣基线） | 68.95 → **37.61**（验收线 50 µs） |
| 脚本轨分发（扣基线） | 493.89（= 组件的 **13.1×**） |

首跑 68.95 µs 超线（验收线先红），归因：每帧 3 阶段 × 1000 实体 × 两次
`HashMap<String>` 字符串哈希（装配期解析好的钩子每帧重新查表）+ Start 阶段
`HashSet<EntityId>` 哈希。改法即取舍 3，两处哈希归零；脚本轨不受影响。

### 四、先证红：组件测试首跑全红揪出装配 bug

三条组件测试（驱动实体 / on_destroy / 未注册跳过）首跑全红 —— 钩子一个没跑。
根因：`World::new` 在空注册表下装配，在实例表里插入"未注册"空占坑；
`instantiate_behaviors` 的 `contains_key` 短路让注入注册表后的重装永不发生。
修法：`World::new` 不再装配（注释写明理由）；`with_behavior_registry` 先清实例
再重装配；分发早退判据改 `declared == 0`。

### 五、验证

- workspace **280 项测试全绿**；clippy 0 警告（顺手修掉 editor 一条既有的
  `collapsible_match`）。
- 端到端：`examples/spinner-plugin`（cdylib）经 libloading 加载 → 版本握手 →
  注册 Spinner/Reaper → `World` 驱动四钩子（调用计数经 `#[no_mangle]` 导出符号
  回读：start 恰好 2、on_destroy 恰好 1）→ Reaper 销毁 target 全链路走通。
- dll 缺失回退：单独 `cargo test -p xgame` 时集成测试现场触发嵌套
  `cargo build -p spinner-plugin`（工作线程 + 超时，防与外层 cargo 抢锁死锁），
  实测可用。
- 版本不匹配拒载：`run_entry` 单测（拒载 + 经日志报自身版本 + 注册表不被写入）
  + `handshake` 报错文案含两端版本，可读性断言在案。

### 六、遗留

- 脚本轨 `dispatch_destroy` 仍无调用方（历史遗留，勿提前接线）：组件轨的
  `on_destroy` 在命令消费时派发；脚本轨接线需要命令携带脚本路径，留待后续
  （避免"命令在帧内何时带路径"的语义漂移），已写进 lifecycle.rs 文档。
- 插件 panic 隔离（D5 的 `catch_unwind` 纪律）本轮未做：P0 边界是同版本工程内
  自用（示例插件 + 集成测试），隔离留到插件生态开放时（P0.5 PIE 之后）。
- 编辑器只编辑数据：Behaviors 字段 Inspector 暂不显示（C5 范围声明）；
  编辑器内 Play 跑 Rust 插件（PIE）是 P0.5。

---

## 十六、双轨语言架构 P0.5：编辑器 PIE（后台编译 + 插件会话）

接 §十五。目标是把 UE 模式那条"改代码 → 按 Play → 自己就跑起来了"的链路打通，
验收七条全部完成。四个产物：

### 一、四个产物

1. **`plugin_build.rs`（编辑器）**——探测 / 新鲜度 / 后台构建 / 产物同步。
   探测只看"项目根有没有 `Cargo.toml`"（一次 `stat`）；新鲜度是纯时间戳比较的
   纯函数 `is_fresh`（三个输入：`Plugins/` 里每个 DLL 的 mtime、项目源 mtime、
   编辑器 exe mtime），新鲜就**连 cargo 进程都不起**；过期才 `spawn_build` 起
   后台线程跑 `cargo build --message-format=json-render-diagnostics`，产物路径
   从 stdout 的 JSON 行解析（比扫 `target/` 可靠），诊断走 stderr 直接进 Console。
2. **`plugin_session.rs`（编辑器）**——DLL 句柄保活 + 每次 Play 重建注册表。
   句柄只增不减：旧代挪进 `_retired`，`Drop` 里 `mem::forget`，**永不 FreeLibrary**
   （UE Hot Reload 同款策略，理由见 D7）。每次 Play 调 `build_registry()` 在已
   加载的 DLL 上重跑一遍入口（`LoadedPlugin::register_into`，微秒级）拿到全新
   `BehaviorRegistry`，保证"停止再播放"回到干净状态。
3. **编辑器状态机**——`enter_play_mode` 改成"先看要不要编译"：过期就转后台
   构建 + `pending_play = true` + 退回 Edit，编译完成后由每帧入口的
   `poll_plugin_build` 同步产物并自动进 Play。UI 全程不阻塞，工具栏显示
   转圈 + 已用秒数，并提供手动「🔨 Build」按钮。编译失败留在编辑态、错误进
   Console，**不阻塞 Play**（项目里可能还有上次的产物）。
4. **玩家与打包器**——`xgame-game` 启动时 `load_plugins_dir("Plugins")`；打包器
   把 `Plugins/` 复制进 `Build/<项目名>/Plugins/`，并在"有 Cargo.toml 却没 DLL"
   时把警告塞进 `BuildReport.warnings`（走报告而不是 `log::warn!`，因为那条只到
   stderr，而这个警告恰恰是点图形界面按钮的人需要看见的）。

另外补了一块 UI：Inspector 在 Scripts 之后加 **Behaviors** 区（组件名列表 + ✕ +
`+ Add Behavior` 菜单，未注册的名字标红 ⚠）—— 与 Scripts 完全同构。

### 二、两个真 bug：都由测试先证红抓出

1. **`needs_build` 写反**（返回了 `is_fresh` 而不是 `!is_fresh`）。语义完全颠倒，
   而两种写法的返回类型都是 `bool`，编译器一声不吭。`freshness_against_real_files`
   首跑红，改对后绿。这条同时说明了那个函数存在的理由：调用方只该问"要不要编"，
   不该各自再翻转一次。
2. **cargo 参数拼错**：`json-render-diagnostic`（单数）—— cargo 的可选值是
   `json-render-diagnostics`（复数）。这个错**单测抓不到**（`run_cargo_build`
   不在单测覆盖里），只有真按 Play 才会炸。手工端到端验证时抓出并修正，
   同时把 `parse_cdylib_artifacts` 的固定 JSON 换成实测形状（`crate-type =
   ["cdylib","rlib"]` 的目标一次给五个文件名，`.dll.lib` / `.pdb` / `.rlib`
   都要靠 `Path::extension` 正确排除，不能用 `contains("dll")`）。

### 三、一个差点上线的死循环

新鲜度里"编辑器 exe 比 DLL 新 ⇒ 要重建"这条，与"cargo 源码没变就不重写产物"
会打架：引擎改了 → 编辑器重编 → 按 Play → 检查说 DLL 旧 → 起 cargo → cargo 说
没改动、什么都不做 → DLL 还是旧的 → 再按 Play 又来一遍，**每次 Play 都白跑一轮
cargo**。

解药是 `sync_plugins_dir` 同步完盖一个"现在"时间戳（`touch`）。回归测试
`sync_makes_plugins_fresh_even_when_cargo_did_nothing` 把待同步产物的时间戳
**倒退回一小时前**来模拟"cargo 说没改动"的情形 —— 注释掉 `touch` 该测试立刻变红
（已实测验证红）。

### 四、演示项目 `projects/BehaviorsDemo`

**它本身就是一个 cdylib crate**（workspace 成员），是"用户项目形态"的插件 ——
与 `examples/spinner-plugin`（引擎自带示例）互补。场景里同时挂：

- `RustSpinner`（Rust 组件，75°/s）+ `RustBobber`（Rust 组件，上下浮动，
  组件自己持有跨帧状态）；
- `RhaiSpinner`（Rhai 脚本，同款 75°/s）—— **两条轨道共用一个运行时**的证明。

三条单元测试在 crate 内直接跑组件逻辑；另有两条集成测试在 `xgame` 里经
**真实 dlopen** 加载 `behaviors_demo.dll`，验证"跨边界注册的表"与"单测里那张表"
完全一致（否则会出现"单测绿、场景里挂组件却纹丝不动"）。

### 五、验证

- workspace **296 项测试全绿**；clippy 0 警告。
- 端到端手工验证：在 `projects/BehaviorsDemo` 里跑真实
  `cargo build --message-format=json-render-diagnostics`，确认产物 JSON 形状、
  只挑出 `behaviors_demo.dll`。
- 无头检查：`xgame-editor --check projects/BehaviorsDemo` →
  `entities=4 lights=1 script_refs=1`，`OK`。

### 六、遗留

- **Play 中途热换插件**未做（现在是"停止 → 编译 → 再播放"），Inspector 的
  组件徽章、release profile 的插件出货（P3）同属遗留。
- cargo 构建**没有超时**：首次拉依赖可能几分钟，用户只能干等（工具栏有秒数）。
  加超时需要决定"杀不杀 cargo 子进程"，留到有真实诉求时。
- 新鲜度把"编译期间恰好又存了文件"这个窄窗口交给下一次存盘（cargo 的指纹
  本来也有同款窗口），已在 `sync_plugins_dir` 的文档里写明。

### 七、一条工程教训

本轮用 PowerShell 的 `Get-Content`/`Set-Content` 做了一次文本替换，**把
`plugin_build.rs` 的中文注释全毁了**（PS 5.1 默认按 GBK 读、按 UTF-8 写，
且部分字符是有损的）。教训：改本仓库的中文源文件一律走编辑器工具，
shell 只做只读操作。

## 十七、双轨语言架构 P1：`#[derive(Inspect)]` + 编辑器调参（本轮）

### 一、"可检视"为什么是组件契约的一部分

编辑器**不链接用户代码**（插件是用户自己编的 DLL）。要在 Inspector 里画出一个
组件的字段，编辑器只有两条路：要么拿到类型信息（跨 DLL 边界做不到），要么让
**实例自己带着字段表**。所以：

```rust
pub trait Behavior: Lifecycle + Inspect {}   // 空 body + blanket impl
```

关键在 `Box<dyn Behavior>` 能直接 `as_mut()` 成 `&mut dyn Inspect`。若存的是
`Box<dyn Lifecycle>`，编辑器想要 `&mut dyn Inspect` 就必须 downcast ——
而 downcast 需要两边共享类型信息，跨 DLL 边界正是这条不通。**"可检视"因此不是
锦上添花，是让编辑器这条边能成立的前提**，注册约束相应从 `Lifecycle` 提到
`Behavior`（`impl_no_inspect!` 是"我不开放字段"的一行逃生舱）。

### 二、`Inspect` 的形状：表 + 下标，不是回调

`crates/xgame-core/src/inspect.rs`：`FieldDesc`（`'static` 描述）/
`FieldKind`（10 种）/ `FieldValue`（拥有值）/ trait `Inspect`。

- **为什么不是"每个字段一对闭包"**：检视对象活在插件 DLL 里，跨边界传闭包要
  处理 panic 隔离与函数指针生命周期；而"读第 i 个 / 写第 i 个"是两个对象安全的
  方法，下标与表一一对应，热路径零哈希。
- **`inspect_field` 返回 `&'static FieldDesc`**：调用方（编辑器）可以跨组件变更
  持有描述符而不借用实例。表是编译期常量，`'static` 是白给的。
- **`FieldValue` 是拥有的**：字段里可能有 `String`，借用版需要 `FieldValueRef`
  与 `Cow`，把 API 复杂度推给每一个消费者。要优化将来加
  `inspect_get_into(&self, i, &mut FieldValue)` 即可，表结构不动。
- **检视是旁路**：`inspect_set` 遇到类型不符的变体**静默忽略**（Lua 传错类型是
  日常），越界 `inspect_get` 返回该类型的零值 —— 一个下标算错不该拖垮宿主进程。

### 三、derive：三处下标必须同源

`crates/xgame-derive` 生成 `impl Inspect for T`。**表、get、set 全部由同一个
`index` 变量生成** —— 这就是"不要手写这个 trait"的理由：手写最常犯的错是三处
下标错位，表现是"改 A 字段动了 B 字段"，两边都是 `f32` 时编译器一声不吭。

- **硬编码 `::xgame::` 路径**（serde 惯例）：派生出来的代码要能在任何 crate 里
  编译。引擎内部测试组件解析不到 `::xgame`，用 `xgame_core::impl_no_inspect!`。
- **`#[inspect(skip / label= / min= / max= / speed= / step= / unit= / tooltip=)]`**。
  `label` 只改**显示名**，寻址名永远是 Rust 字段名 —— 重命名显示名不能破坏已有脚本。
- 回归测试 `crates/xgame/tests/inspect_derive.rs`：核心是"把每个字段写成一个
  **可辨认**的值，再逐个读回来"（数值刻意不取 0，否则错位了也读到 0 测不出来）。

### 四、踩到的四个坑（共同点：报错指向用户代码，而不是宏本身）

1. **`INSPECTABLE` 两列都写了小写类型名** → 生成 `FieldKind::f32` →
   报"no variant named `f32`"，而错误位置指向**用户的结构体**。
2. **`descs` 里字面量之间漏了一个尾逗号** → `expected one of ',' ... found '::`。
   只有两个以上字段才复现，所以第一个字段的用例全绿。
3. **`split_for_impl` 摆错位**（`for #name #impl_g`）—— `DeriveInput` 上没有
   `split_for_impl`，要先 `let (impl_g, ty_g, where_g) = input.generics.split_for_impl()`。
4. **`String` 字段在 `inspect_get` 里是 `&self` 上的移动** → E0507。
   数值 / bool / glam 向量都是 `Copy`，只有 `String` 要 `.clone()`。

**诊断方法**（下次照做）：proc macro 里 `eprintln!("{ts}")`，用环境变量门控，
再 `cargo check -p <pkg>` 读 stderr。`cargo expand` 本仓库没装，别指望它。

### 五、编辑器：Edit 模式也要有"实例"

组件实例只在 Play 时由注册表构造，Edit 模式下没有东西可改 —— 但"先拖滑块再按
Play 看效果"是 Unity 工作流的常态。于是 `EditorState.edit_components`（惰性构造，
声明位置在 `game_runtime` 之后、`plugins` **之前** —— 值里有插件函数指针）。

- **名字对齐校验**：用户把 `behaviors[0]` 从 A 换成 B 后，旧副本必须作废重建。
  否则会把 A 的参数写进 B 的字段里 —— 两边都是 `f32`，编译器管不着。
- **`seed_play_from_edit_components` 逐字段复制，不换实例**：World 那份已经装配
  完毕，换掉等于作废装配状态；而且只有可检视字段该被外部影响，
  `#[inspect(skip)]` 的状态字段本来就属于实例自己。
- **Play 中的改动不回写**：停止 Play 即丢弃（Unity 默认语义）。
- **组件字段改动刻意不碰 `touched`**：那个标志意味着"改了 `working`，要写回场景
  +推一条撤销"。组件参数不在场景文件里，动它会凭空多出一条场景撤销记录，
  按 Ctrl+Z 会撤掉一个并不存在的改动。

**顺带修掉的隐患**：`PluginSession` 现在把加载时已经装配好的注册表**留了下来**
（原先读完名字就丢）。Inspector 每帧要查字段表、要构造编辑态副本，若走
`build_registry()`（每次现跑插件入口），一旦出现"名字在 `names` 里、重跑却没
注册上"的错位，构造会**每帧失败**，而入口重跑失败会 `log::error!` —— 日志洪水。
常驻缓存把它变成一次 `HashMap` 查询。

### 六、一个会崩进程的测试陷阱（P0.5 埋的，P1 才暴露）

`plugin_loading` 集成测试直接 dlopen `target/*.dll`。而 **`cargo test` 根本不
产出 cdylib**（它只要 rlib 来链接测试），所以那些 DLL 停在上次 `cargo build`
的那一刻。P1 给 `BehaviorEntry` 加了 `fields` 字段 —— **插件注册表是跨编译单元
的内存布局**：DLL 那侧写、宿主这侧读。旧 DLL 按三段写、新宿主按四段解，
`HashMap` 的桶直接读歪，下一次经构造函数指针跳过去就是
**`STATUS_ACCESS_VIOLATION`（0xc0000005，进程暴毙，连 panic 都没有）**。

修法：取 DLL 前先**嵌套构建一次**（`build_package_once`，每包每进程一次，
持锁跨整次构建以串行化并行测试），把"新鲜"交给 cargo 的完整指纹去判断。

**教训**：P0.5 当时没踩到纯属运气 —— 那会儿 DLL 刚由验证流程里的 `cargo build`
产出、恰好新鲜。**产物与源码是否一致不能靠运气**，尤其当它变成内存布局的时候。

### 七、性能：先量后改（这次差点白改一版架构）

换 `Box<dyn Lifecycle>` → `Box<dyn Behavior>` 时担心：supertrait 方法调用要多一层
vtable 间接（`dyn Behavior` 的 v表里存着 `Lifecycle` 的 v表指针）。量之前先回测：

| 轨 | P0.5 `3d77f12` 同机 | P1 同机背靠背 |
|---|---|---|
| Rust 组件轨分发 | 68.4 µs | **65.2 µs** |
| Rhai 脚本轨分发 | 728.8 µs | 729.3 µs |

**P1 无回归**（甚至略低，在噪声内）—— 所以没有为这个猜想动架构。

注意 50 µs 的验收线在本机是**红**的（P0.5 也红，同机测得 68.4 µs），而 P0 当时
记录的是 37.6 µs：三条轨同步变慢而基线不动，判定为当时机器状态不同，不是某次
提交把分发搞慢了。**没有为迁就机器而上调验收线**；数字连同"必须带同机对照"
的说明一起写进了 `behavior_dispatch_throughput.rs` 的模块文档。

### 八、验证

- `cargo check --workspace --all-targets` / `cargo clippy`：0 错误 0 警告。
- `cargo test --workspace --all-targets`：**307 项全绿**（P0.5 是 296）。
- 无头检查：`xgame-editor --check projects/BehaviorsDemo` → `OK`。

### 九、遗留

- `FieldKind` v1 只有 10 种：`Quat` / `Color` 未进（等有真实组件需要再说）。
- **组件字段不进场景文件**（P1.5）：现在 Edit 态调的参数活在编辑器内存里，
  存盘不带走。序列化格式与 Lua 侧的绑定都等 P1.5 / P2。
- 字段级编辑**没有撤销**（场景级有）—— 接上场景撤销时再一起做。
- 没有 `trybuild`：「类型不支持又没标 `skip` ⇒ 编译期报错」这条无法断言，
  只能靠报错文案本身足够指向性。

---

## 十八、双轨语言架构 P1.5：组件参数落盘进场景文件（本轮）

接 §十七。P1 打通了「可检视 + 可调参」，但参数只活在编辑器内存里 —— 关掉即丢、
不能进版本库、prefab 没有字段位。本轮把组件参数变成**场景数据**，验收四条全部完成。

### 一、数据模型：注册名做键，不是下标

`Entity::behavior_params: BTreeMap<String, FieldValues>`（注册名 → 字段寻址名 → 值）。

- **为什么是注册名而不是 `behaviors` 数组下标**：下标会错位（用户在中间插一个组件，
  所有后续下标全变），而参数表是**持久化数据** —— 错位写参的表现是"改 A 组件的参数
  写进了 B 组件"，两边字段都是 `f32` 时编译器和运行时都发现不了。注册名天然稳定。
- **`FieldValues = BTreeMap<String, FieldValues>` 的有序性**：`BTreeMap` 保证 key 有序，
  序列化顺序稳定 → 场景文件 diff 不会因为哈希随机序而抖动。
- **`FieldKind` / `FieldValue` 补 serde**：变体标签即磁盘格式，钉死变体名测试
  （`field_value_ron_variant_labels_are_stable`）—— 改标签名等于作废所有旧场景。

### 二、capture 存全部，apply 吸收陈旧

- **capture 存全部可检视字段**（不做"非零才存"的半截规则）：半截规则会让"用户显式
  调回默认值"与"从没碰过"无法区分，且 diff 不可读。
- **apply 静默吸收陈旧/错型条目**（`apply_field_values`）：组件字段表改了之后，
  旧场景里的陈旧参数不该让加载崩溃。这与世界层 `apply_field_values` 既有取舍一致
  （检视是旁路，一个下标算错不该拖垮宿主）。

### 三、装配期套用 + 编辑副本种子

- **`World::instantiate_behaviors`**：装配时按注册名查 `behavior_params` 并套用。
  注册不到的名字**静默跳过**（与"参数表只是数据"的既有取舍一致，不 panic）。
  行为字段级拆借未动（Lua 侧仍走 `FieldValue`）。
- **`EditorState::inspect_component`**：构造编辑态副本时种子场景参数（找不到字段表
  时不种）。这样 Edit 模式下打开 Inspector 看到的就是场景里的真实值，而不是组件默认值。

### 四、撤销语义的推翻（P1 遗留的"不进撤销栈"）

P1 刻意让组件字段改动**不进**场景撤销栈（理由：参数不在场景文件里，动它会凭空多出
一条撤销记录）。参数进场景后这条取舍**失效** —— 它已成为场景数据，不进栈会凭空丢
一条场景编辑。改法：组件字段编辑接上既有撤销两段式（拖拽挂 `undo_pending`、松手入栈；
离散编辑当场 push），与 Name/Transform 同一通道，不单开支路。

**Play 中的改动仍不写回**：停止 Play 靠场景快照还原（既有语义，与本轮无关）。

### 五、一个测试后门

`PluginSession` 加 `#[cfg(test)] register_for_test<T>`：编辑器测试要构造带字段表的
组件实例，但生产路径只有 `build_registry()`（重跑插件入口）。后门只在测试编译期存在，
保住"注册表只构建一次"的每帧性能取舍。

### 六、验证

- 先证红（仓库惯例）：`behavior_params_from_scene_are_applied_at_assembly`
  （旧实现场景参数被丢弃 → 红）、`edit_component_is_seeded_from_scene_params`
  （无种子路径 → 红）。改后全绿。
- 新增回归：capture roundtrip、陈旧条目容错、RON 变体标签钉死、场景 roundtrip/
  旧文件兼容、装配套用、编辑副本种子 —— 共 6 条。
- `cargo test --workspace --all-targets`：**312 项全绿**（P1 是 307）。
  clippy 0 警告；`cargo check` 0 错误。
- 无头检查：`xgame-editor --check` 对 BehaviorsDemo 与 SampleScene 均 `OK`。

### 七、遗留

- **prefab 的组件字段**未做（prefab 目前只覆盖 Transform 等基础数据）——
  待 prefab 系统扩展到组件层时一起。
- `FieldKind` 仍只有 10 种（`Quat` / `Color` 未进），序列化变体随之受限。
- Play 中热调参**不写回**是既有语义；若将来要"Play 中调参也持久化"需另开决策。

## 十九、双轨语言架构 P2：Lua 宿主 + 路径驻留表（本轮）

接 §十八。完成计划文档 §四的 **P2** 两件事（Lua 宿主、修每帧 String 克隆）
与双轨的编辑器接入。验收：`cargo test --workspace --all-targets` **332 项
0 失败**（2 个手动基准 ignore）、clippy **0 警告**、`xgame-editor --check`
OK。决策注记已回写计划文档 D10。

### 一、Lua 宿主（`lua_host.rs` + `lua_api.rs`，两文件共约 1600 行）

- **mlua 0.10**（`lua54` + `vendored` + `send`）：没有 `'lua` 生命周期，
  `Value` / `Table` / `Function` 全是 `'static` —— 与 Rhai 一样能把句柄放进
  跨帧结构。`ScriptCtx` 泛型化 `ScriptCtx<S: 'static>`（Rhai 侧
  `ScriptCtx<rhai::Map>` **零改动**，Lua 用 `ScriptCtx<mlua::Table>`）。
- **每脚本一张环境表（沙箱）**，钩子从 env 里查。关键：`__index` 必须挂
  **真元表**（`meta.set("__index", lua.globals())` 后 `env.set_metatable(...)`）
  —— 往 env 塞名叫 `__index` 的普通字段对读取完全无效（本轮最贵的坑：
  脚本里所有宿主 API 变成 nil，9 条测试因此红，查元表语义才定位到）。
- **指令计数沙箱**：`set_hook(every_nth_instruction(1024))`，预算 200_000，
  扣到 0 返回 `Error::RuntimeError` 中断本次调用 —— 死循环脚本只毁掉自己
  这一帧，不卡死进程。与 Rhai 的 `set_max_operations(200_000)` 同量级。
- **StdLib 裁剪**：只开 `TABLE | STRING | MATH | UTF8 | COROUTINE`；
  io / os / package / debug / ffi 不给 —— 脚本是用户内容（可能是下载的 mod）。
  四个钩子与 Rhai 同名同语义；顶层 `local` 脚本级共享，每实体状态走
  `state_get_or` / `state_set`。
- mlua API 细节坑（留档）：`load(chunk)` 只收 1 个参数（chunk 名用
  `.set_name(...)`）、`AsChunk` 只对 `&[u8]` 实现、`set_metatable` 返回 `()`、
  `Table::set` 无 turbofish。测试 15 条（沙箱/死循环/状态隔离/钩子，与
  Rhai 侧对称）。

### 二、两轨按扩展名分流（`world.rs` 合并）

- `.lua` → `LuaHost`，其余 → `ScriptHost`（`is_rhai = !is_lua`）。两轨
  **合起来必须恰好覆盖全部脚本**：漏了是"两边都不管、脚本静默不执行"，
  重了是"两边都编译、假报编译错"。证红测试 `rhai_host_skips_lua_scripts`
  （旧实现把 `.lua` 丢给 Rhai → 每帧一条"脚本编译失败"日志 → 红）。
- `World` 帧管线依次派发两轨；日志/命令缓冲**合并但不共享指针**
  （各持各的 `Vec`，每帧取走追加到 world 侧）。
- `dispatch_destroy` / `forget_entity` 两宿主都**未接线** —— 与 Rhai 对称的
  既有遗留（运行时本来就没调用它们），不借 P2 之名单方面接线。

### 三、热路径 String 克隆 → 路径驻留表（先量后改）

**量**（临时探针 `clone_cost_probe`，量完即删，数据在此留档）：1000 实体
场景、每帧 2 次派发 × 300 帧，同机会话：

| 热路径点 | String 版 | 驻留 u32 版 | 克隆贵出 |
|---|---|---|---|
| 建派发队列（`collect_*_queue` / `dispatch_start`） | 53.52 µs/帧 | 20.91 µs/帧 | **32.61 µs/帧** |
| 状态表 entry（`state_key`） | 93.84 µs/帧 | 16.00 µs/帧 | **77.84 µs/帧** |

合计上限 ≈ **110 µs/帧** —— 占当时 Rhai 轨分发（819.6 µs）的 13%。
三处克隆：两宿主各自的 `collect_*_queue`、`dispatch_start` 内联队列、
`run_hook` 的 `state_key`（Lua 侧与 Rhai 侧一一对称）。

**改**（`xgame-script/src/intern.rs`，两宿主各持一份 `PathIntern`）：

- 路径 → 稳定 `u32`；队列存 `(EntityId, u32)`，状态键 `(EntityId, u32)`；
  `resolve(id)` 用 `Vec` 下标换回 `&str`（日志 / `GameCtx::source`），零分配。
  每帧每路径只多一次哈希查表（探针已计入），换掉一次 String 分配+复制。
- **id 永不回收、驻留表永不清空**：状态表按 id 找状态，`invalidate` /
  `invalidate_all` 只清编译缓存。表一清，热重载后每实体状态会**静默错位**
  （A 脚本读到 B 的计时器，两边都是 `f32` 时无从发现）。回归测试
  `state_survives_hot_reload_even_when_intern_order_changes`（两宿主各一条）：
  跑帧 → `invalidate_all` + **把实体顺序倒过来**（逼驻留顺序改变）→
  断言状态延续（a=2、b=200）。**证红**：把"清空驻留表"这个看似合理的实现
  写进 `invalidate_all`，两条测试都红（b=100，即状态从 0 重计）——
  这是新不变量的红对照（旧实现没有驻留表，性能改动本身在行为测试下恒绿，
  正因此需要这条不变量网）。
- **借用检查的形状**（实现要点）：借 `self.intern` 得到的 `&str` 没法跨
  `&mut self` 方法调用传（借用检查器看不出"它就是 self 身上的一块"）。
  解法：`ensure_loaded(path)` 拆成"先 intern 再转"的外壳 + `ensure_loaded_id`
  内核（在方法体内自己 resolve）；`run_hook` 改收 `path_id`，体内 resolve
  （O(1) 下标）。`CompiledScript.path` 字段随之删除（两个用途都被驻留表
  取代，留着是死字段）。

**验**（端到端，`behavior_dispatch_throughput` 4 轮背靠背）：Rhai 轨分发
819.6 → **713 µs/帧**（-106 µs，-13%），与探针预测 ≈110 µs 吻合 ——
微基准与端到端交叉验证，该对照已写进基准模块文档（P2 列）。

### 四、编辑器 / 工具链接入

- `Project::list_scripts` 扫 `rhai` **和** `lua`（证红测试
  `list_scripts_includes_both_tracks`：旧实现只列出 `["Scripts/a.rhai"]` → 红）。
  Inspector 的 Add Script 菜单、MCP 项目信息、热重载扫描都走它，一处改完全通。
- 三处文案（state 扫描说明、工具栏 ⟳ Scripts 提示、Inspector 空态）改为
  "双轨"；热重载链（state → game_runtime → world）本就两个宿主各
  `invalidate_all`，无需改。
- `xgame-editor --check`：SampleScene OK（entities=14、script_refs=3）。

### 五、验证汇总

- 全量：**332 项 0 失败**、clippy 0 警告、`--check` OK。
- 新增回归 5 条：intern 稳定性 2、双宿主状态延续 2、list_scripts 双轨 1
  （其中 3 条按惯例先证红再改绿，状态延续 2 条的红对照见上文）。
- 基准 P2 列：基线 7.43 / 组件分发 64.7 / Rhai 分发 713 µs。基线与组件轨
  相对上一轮的漂移（10.39 → 7.43、57.1 → 64.7）是机器状态 —— 那两档的
  执行路径不经过驻留表（脚本列表为空，扫描循环逐字未变）；判回归只看
  Rhai 轨的 -106 µs 是否消失。

### 六、遗留

- **下一步 = P2.5 基准闸门**：扩 `behavior_dispatch_throughput` 加 Lua 轨、
  三负载对比出报告（规则见计划文档 §六）。Rhai 去留在闸门结论前不动。
- `dispatch_destroy` / `forget_entity` 双轨都未接线（既有遗留，等运行时
  真正需要"销毁钩子"时两轨一起）。
- 编辑器没有"新建脚本"功能（两轨一致）：`.lua` 文件放进 `Scripts/` 即被
  扫描、挂载、执行。
- Lua 轨的**每帧性能还没有实测**（本轮基准只加了驻留表对照）——P2.5 会出
  第一个数；若 Lua 轨明显慢于预期，优先怀疑方向是每钩子的 `LuaCtx` 构造
  与 env 查表，不是驻留表（已修的部分有交叉验证）。

## 二十、双轨语言架构 P2.5：基准闸门 Rhai vs Lua（本轮）

接 §十九。完成计划文档 §六 的 **P2.5** 基准闸门：三负载实测 Rhai vs Lua，
按事先声明的规则出判定。**判定结果：Lua 三负载全占优（1.31× / 1.26× /
9.16×，比值 >1 = Lua 更快）→ 按 §六 规则进入 P3「删 Rhai、样例文档切
Lua」**。闸门过程中顺带修掉了 Lua 宿主热路径的两处实现开销（先量后改，
数据在下文），把负载 1 从"Rhai 反超 2.25×"翻转为"Lua 快 31%"。

### 一、闸门工具（`language_gate.rs`，`#[ignore]` 手动基准）

三负载、同进程背靠背（同一台机器同一时刻，消除跨会话机器状态漂移）：

| 负载 | 场景 | 测什么 |
|---|---|---|
| 1 dispatch | 1000 实体 × no-op 四钩子脚本 | 宿主调度 + 边界开销 |
| 2 api | 500 实体 × 每帧 20 组 `pos/set_pos/action_pressed` | API 边界穿越 |
| 3 compute | 200 实体 × 每帧 1000 次浮点循环 | 解释器本体 |

- **脚本两轨同源同循环数**：`api_rhai()/api_lua()`、`compute_rhai()/compute_lua()`
  用 `format!` 从同一组常量（`K_API`/`I_COMPUTE`）拼出来 —— 单一真相源，
  改负载只改一处，两轨永远跑同样的工作量。
- **错误日志守卫**（首跑栽在这，见下）：`measure()` 预热后 `take_logs()`，
  有 `Error` 级日志直接 panic —— 脚本没真跑时测出来的"快"是假数据，
  不许端到端通过。证红：把 Rhai 脚本改回 `let mut`，断言如期触发。

### 二、首跑假数据事故（Rhai 侧，已修）

首跑"Rhai 负载 2/3 大胜"是假的：**Rhai 语言没有 `mut` 关键字**（变量天然
可变，`let x = 0.0; x = 1.0;` 直接重赋值），我写脚本时按 Rust 习惯加了
`let mut x` —— Rhai 把 `mut` 当变量名解析后撞上下一个 token 报编译错
（`Expecting ';' to terminate this statement`），脚本体根本没执行，
"快"的只是空调度。铁证：假数据里 Rhai 计算档 0.28 µs/实体 < no-op 档
0.73 µs/实体（计算档比空跑还快不可能）。`repo` 里既有 `.rhai` 脚本
（`FoxLocomotion.rhai:23-25`）都是 `let` 直接重赋值，无一用 `mut`。

**教训**：宿主对编译失败只记日志不 panic（产品语义正确 —— 一个坏脚本
不该拖垮整个游戏），但基准工具必须自己断言"脚本真的跑了"—— 这就是
`measure()` 日志守卫存在的理由。

### 三、负载 1 的 Lua 慢 2.25×：定位 + 修复（先量后改）

修好脚本后负载 1 真实数据：Rhai 743.59 vs Lua 1670.43 µs/帧（Lua 慢 2.3×）。
纯调度不该差这么多 —— 分两步量证：

**探针 1**（`_probe_lua_call_cost.rs`，量完即删，数据留档）复刻宿主环境
（同 StdLib 裁剪、同 1024 指令 hook、同 env+元表），分项量单次钩子调用：

| 分项 | 成本 |
|---|---|
| `env.get` 从环境表取函数 | **195.4 ns** |
| `call` 无参 | 84.0 ns |
| `call` 带 userdata | **368.5 ns**（比无参多 284） |
| 带 userdata 关指令 hook 对照 | 357.5 ns（**hook 税仅 ~11，排除嫌疑**） |
| get+store+entry+call（≈宿主全形状） | 588.6 ns |

**探针 2**（`_probe_run_hook_segments.rs`，量完即删）在 `run_hook` 上分段
计时（1000 实体 × 300 帧）：`func.call` 442.7 µs/帧（39.8%）、`env.get`
192.3 µs/帧（17.3%）是大头。两处合计 ≈478 ns/钩子 × 每帧 2000 钩子
（update + fixed_update 两相）≈ 956 µs/帧 —— 与实测差值 927 µs 严丝合缝。

**userdata 方案对比**（探针 1 第二轮）：现造 365.2 / 预建 `AnyUserData`
克隆 159.8 / **轻量指针 81.3** / 无参参照 78.0 ns。

**改**（`lua_host.rs` + `lua_api.rs`）：

1. **钩子函数句柄编译期缓存**：`Hooks`（bool 版）→ `HookFuncs`
   （`Option<Function>` × 4）。`detect_funcs` 本来就要 `raw_get` 验存在性，
   句柄直接留下 —— 热路径零查表。
2. **ctx 改宿主常驻槽 + 轻量指针传参**：`LuaHost` 持
   `Box<MaybeUninit<LuaCtx>>`，每钩子入口写入本次地址束、传槽地址
   （`LightUserData`，mlua 在 `send` 下 `unsafe impl Send` 在案）。
   安全论证写在 `LuaCtx` 类型文档：指针永远指向宿主持有的槽；任何脚本
   可见的执行都发生在钩子调用窗口内（窗口内槽 = 本次 ctx）；脚本违约
   存句柄下帧再用，读到的也是下帧调用的**有效** ctx（语义="当前运行中的
   上下文"，不悬垂）—— 比旧方案（userdata 存过期拷贝、下帧解引用已死
   栈帧）更不容易踩坑；脚本无法伪造 lightuserdata（Lua 语言层无此操作），
   `FromLua` 手写校验仍给清晰报错。
3. `CompiledScript` 不再存 env（钩子函数的 `_ENV` upvalue 持有它）。

**验**（同机背靠背，`behavior_dispatch_throughput` + `language_gate` 复跑）：

| 负载 | 修复前 | 修复后 |
|---|---|---|
| 1 dispatch（1000 实体） | Rhai 743.59 / **Lua 1670.43** | Rhai 753.94 / **Lua 575.92** |
| 2 api（500 实体） | Rhai 18984.57 / Lua 18367.18 | Rhai 19402.71 / **Lua 15452.31** |
| 3 compute（200 实体） | Rhai 44962.36 / Lua 5092.78 | Rhai 45788.25 / **Lua 5001.30** |

负载 1 的 Lua **-65%**（1670 → 576），负载 2 的 Lua -16%（`FromLua` 走
轻userdata 解引用也省了每次 API 调用的 `as_userdata+borrow`），
负载 3 基本不动（纯解释器，不吃边界开销）。行为回归 34 条全绿
（沙箱/死循环/状态隔离/钩子派发均未受影响）。

### 四、闸门判定（规则事先声明，无中间地带）

§六 规则："Lua 三负载全占优 → P3 删 Rhai；任何一负载 Rhai 反超 >20% →
保留双宿主并重新评估"。最终数据（同进程背靠背）：

| 负载 | Rhai | Lua | 比值（Rhai/Lua） |
|---|---|---|---|
| 1 dispatch | 753.94 µs | **575.92 µs** | 1.31× |
| 2 api | 19402.71 µs | **15452.31 µs** | 1.26× |
| 3 compute | 45788.25 µs | **5001.30 µs** | **9.16×** |

**三负载全占优 → 判定：P3 删 Rhai**。判定稳健性：负载 3 的 9.16× 是
AST 解释 vs 字节码 VM 的**语言级代差**，任何宿主侧派发优化都翻不了盘；
负载 1/2 的 1.31×/1.26× 也都在 20% 线外 —— 即使对 Rhai 侧做对称优化
（它也有 `call_fn` 每次按名字查 AST 的空间），量级上也无法扭转负载 3。

### 五、验证汇总与遗留

- 全量：clippy 0 警告、`xgame-script` 34 项 0 失败、闸门与分发基准跑通。
- 临时探针 3 个（`_probe_gate_diag` / `_probe_lua_call_cost` /
  `_probe_run_hook_segments`）全部删除，原始数据留在本文档。
- 组件轨 71.7 µs 与历史 64.7 的漂移在机器噪声内（同机基线也漂了 2%），
  本机验收线持续红是 P0.5 起已知状态，不修线。
- **下一步 = P3**：按判定删 Rhai 宿主、样例/文档切 Lua（范围见计划文档
  §四 P3 与 ROADMAP）。`language_gate.rs` 在 P3 删 Rhai 后自然退役
  （判定已出，它的一次性使命完成）。

## 二十一、双轨语言架构 P3：删 Rhai、脚本层收敛单一 Lua 轨（本轮）

接 §二十。按闸门判定（§二十 四、计划文档 §六 / D11）执行删除：Rhai 宿主整体
下线，脚本层从"双轨按扩展名分流"收敛为**单一 Lua 轨**。全量验证绿
（clippy 0 警告、`cargo test --workspace --all-targets` 全绿、`--check` OK）。

### 一、删了什么

| 删除物 | 位置 | 说明 |
|---|---|---|
| Rhai 宿主本体 | `xgame-script/src/host.rs` | `ScriptHost` / `CompiledScript` / bool 版 `Hooks` 及其派发、编译缓存、测试全删 |
| Rhai 转发壳 | `xgame-script/src/api.rs` | 整文件（P0 起就是"每函数一行转发"，函数体早在 ctx.rs） |
| `rhai` 依赖 | `xgame-script/Cargo.toml` | 连带 `f32_float` 注释（那是 Rhai 的 f64 问题，Lua 侧无此事） |
| 闸门工具 | `tests/language_gate.rs` | 一次性使命完成，判定数据已入档（§二十 + 计划文档 §六） |
| 基准 Rhai 档 | `tests/behavior_dispatch_throughput.rs` | 保留基线 / 组件 / Lua 三档；模块文档的 Rhai 列**留作历史记录表** |
| `LuaPreloadReport` 别名 | `lua_host.rs` | 双轨对称期的纯别名，单轨后无人使用（连 `PreloadReport` 都在同模块导出） |

**保留物**：`host.rs` 瘦身为**宿主公共类型**模块（`Hook` / `PreloadReport` /
`CommandReport`）—— `lua_host.rs` 一直 `use crate::host::{...}` 共用这三个类型，
模块路径与 `lib.rs` 再导出不变，纯删代码不制造无关 diff。

### 二、单轨化改动

1. **`World` 单字段**：`scripts: ScriptHost` + `lua: LuaHost` → 一个
   `scripts: LuaHost`。`take_logs` / `reload_scripts` / `consume_script_commands`
   的"两轨缓冲合并"代码全删（合并语义本来就是"消费方不关心来源"，单轨后
   天然成立）；组件轨的日志 / 命令缓冲仍走 `scripts.buffers_mut()`，通道未变。
2. **`ScriptCtx<S>` 收成非泛型**：状态表直接持 `mlua::Table`。泛型是双轨期
   "Rhai `Map` / Lua `Table` 各一个实例化"的产物，单轨后无意义。**地址束模式、
   `ErasedMut`、unsafe 调用窗口约定原样保留** —— 不在删语言的同一轮里动
   unsafe 核心（churn 风险 > 收益）。
3. **`list_scripts` 只扫 `.lua`**：残留 `.rhai` 不进编辑器下拉。证红测试
   `list_scripts_includes_both_tracks`（P2 引入）使命完成，更新为
   `list_scripts_scans_lua_only` —— 断言方向**反转**（写一个 `.rhai` 进去，
   必须不出现在列表里；旧实现在这条断言下会红）。
4. **编辑器三处双轨文案**收敛（state.rs / inspector.rs / toolbar.rs）；
   `xgame-core` 的注释示例（`Scripts/Player.rhai` → `.lua` 等）同步。

### 三、样例与文档迁移

- **4 个 `.rhai` 移植 `.lua`**（`git mv` 语义：删旧建新）：SampleScene 的
  Spinner / Jumper / FoxLocomotion + BehaviorsDemo 的 Spinner。语法点：
  `let` → `local`、`fn` → `function ... end`、`!` → `not`、方法式调用本就
  没有（Rhai 函数式写法同形，迁移零摩擦）、`sin` → **`math.sin`**（沙箱
  MATH 库已开 —— `sample_project_runs` 跑 FoxLocomotion.lua 180 帧无错误日志，
  链路验证在案）。场景 ron 引用（scene.ron ×3 + 注释 ×2、main.ron ×1）同步改。
- **`Docs/脚本API.md` 整篇重写为 Lua 版**：原"两个语法坑"一节收敛 ——
  "全局 `let` 在函数里看不见"随 Rhai 消失（Lua 顶层 `local` 是 upvalue，
  函数体天然可见），换成"顶层 `local` 是**脚本级共享**、每实体状态走
  `state_*`"的语义说明；数学节改 `v*` 具名函数 + `math.*`；容错表去掉
  Rhai 专属的"递归 32 层"数字，按 Lua 指令预算语义写；`raycast` / 向量的
  返回形状按 `lua_api.rs` 真实实现核对过（表、全字段恒填）。
- README（脚本章节 + 目录树 + Fox 示例）、差距分析两处、ROADMAP 架构图同步。
  `Docs/双轨语言架构计划.md` 补 **D11 落地注记**、路线图表 P2.5/P3 打勾、
  一页结论与 §六 收尾更新。

### 四、刻意的取舍

1. **API 名一字不改**：`spawn_entity` 的 `_entity` 后缀是 Rhai 保留字时期
   的历史产物，`state_*` / `set_anim_*` 等全部保持原名 —— 迁移成本
   （既有脚本 + 文档 + 测试）远大于后缀的违和感。注释里写明历史原因。
2. **`host.rs` 不改名不搬家**：有人会想把它并进 `context.rs` 或改名
   `types.rs` —— 但 `lua_host.rs` 的 `use crate::host::` 与 `lib.rs`
   再导出路径全都不用动，纯删代码的 diff 最小、最好审。
3. **`ScriptCtx` 泛型收掉，但 unsafe 半步不迈**：见上文二.2。地址束 +
   `ErasedMut<'static>` 是 P2 安全论证的原物，本轮只把 `S = rhai::Map`
   这个默认参数摘了。
4. **`language_gate.rs` 直接删**而不是留作"将来接第三门语言再用"：
   一次性工具，脚本两轨同源的写法与 Rhai 侧强耦合（`api_rhai()` /
   `compute_rhai()`），留着就是死代码 + 误导（仓库有"不留死代码"惯例）。
   判定数据已完整入档，未来要做语言对比按 §二十 的方法重写即可。
5. **`check.rs` 不加扩展名校验**：悬空引用检查仍只查"文件在不在"，
   不查"是不是 `.lua` / 能不能编译"—— 旧项目场景里残留的 `.rhai` 引用
   （文件还在）会继续通过存在性检查，不被误报；真正的"这脚本不会跑了"
   会在 Play 时以"没定义钩子"的静默形态出现（与"空脚本"同语义）。
   迁移期不制造惊吓，是刻意选择。

### 五、验证汇总与遗留

- 全量：`cargo check --workspace --all-targets` 绿、clippy **0 警告**、
  `cargo test --workspace --all-targets` 全绿（xgame-script 16 项 Lua 测试
  = 原 11 项 Rhai 测试的等价覆盖：沙箱 / 死循环预算 / 状态隔离 / 驻留表
  热重载不变量 / 分流判定 / 顶层 local 可见性）；
  `xgame-editor --check` OK（`script_refs=3`，三个 Lua 脚本全解析）。
- 基准复跑（同机）：Lua 轨分发与 P2.5 的 562 µs/帧在噪声内，组件轨 /
  基线同机器状态口径不变 —— 删 Rhai 没有触碰 Lua 轨热路径。
- 行为验证：`sample_project_runs`（Spinner.lua 转动 + Jumper.lua +
  FoxLocomotion.lua 的 `math.sin` / `set_anim_*` 全链路）、
  `sample_project_assets_all_load`（三脚本编译 0 失败）、BehaviorsDemo
  场景加载。
- 遗留不变：`dispatch_destroy` / `forget_entity` 仍未接线（单轨后只剩
  Lua 宿主一份，语义同 P2）；组件轨 <50 µs 验收线本机持续红（P0.5 起已知）。
- **双轨语言架构计划 P0–P3 全部分期完成**。

## 二十二、第八阶段：音频系统（本轮）

引擎第一次有声音。选型 **kira 0.12.5**（纯 Rust、CPAL 后端、自带空间化轨道
与 symphonia 解码；对比 rodio —— rodio 无内置 3D 定位，空间化得自己拿
`SpatialSink` 手搓距离衰减，等于把本阶段最核心的一块自己写一遍）。

全量验证绿：`cargo check --workspace --all-targets` 绿、clippy **0 警告**、
`cargo test --workspace --all-targets` 全绿（新增 21 条音频测试，全部先证红）。

### 一、分层与数据流

```
xgame-core::audio   纯数据（无 kira 依赖）
  AudioSource（组件） / ListenerPose / PlaySound / AudioCommand / AudioPort / AudioStats
        ↑ 只依赖 core
xgame-audio         AudioEngine：kira 封装（设备 / 空间轨道 / 解码缓存 / GC）
        ↑
xgame-runtime       World 持 port + engine，帧末 drain 结算
        ↑
xgame-script        GameCtx.audio: Option<&mut AudioPort>（脚本 6 个 API 只进 port）
```

**脚本层不引 kira** —— 脚本 API 的实现是"往 `AudioPort` 里塞命令"，与
物理射线 / 生成实体同一模式：脚本产生**意图**，`World` 帧末统一结算。
这样设备失败 / 沙箱 / 热重载全都与音频实现解耦。

### 二、核心设计决策

1. **监听者与主音量是状态，不进命令束**。`AudioPort` 持
   `listener: ListenerPose` + `master_volume: f32` + 私有 `commands`。
   每帧被反复覆盖（编辑器每帧推相机姿态），命令束语义是"发生过的事"，
   姿态是"现在是什么"—— 混在一起会让"第 3 帧听者在 A、第 5 帧在 B"
   变成一串需要回放的状态更新。

2. **`PlaySound.follow` 是实体绑定，不是空间化开关**。`spatial` 只看
   `position.is_some()`；`follow` 只决定 `StopEntity` / `EmitterPosition`
   按哪个实体匹配。**非空间化的声音也绑实体** —— UI 音挂实体时照样要能
   "实体删了音就停"。实现期踩过：初版只给空间声源绑实体，导致
   "非空间化 autoplay + 删实体"音频还在响（`removed_entity_or_component_stops_its_sound`
   红后修复，`play.follow` 恒设挂载实体）。

3. **无声降级而不是报错**。`AudioEngine::open` 失败（无设备 / 无头 CI /
   容器里没有音频服务）进入 `Playback::Silent`：照常受理命令、解码、计数、
   GC（墙钟），只是不出声。**所有音频回归测试跑在 silent 引擎上** ——
   不用 MockBackend（那会把 kira 内部结构钉死在测试里），用**真 WAV 文件**
   走完整解码路径，测试因此同时验证了"解码 + 缓存 + 停止 + GC"整条链。

4. **`sync_audio` 每渲染帧一次，不跟物理固定步同频**。人耳对 16 ms 的
   声源位置更新完全不敏感，固定步一帧可能跑好几个 —— 音频不值得同频。

5. **帧末结算**：`sync_audio()`（发命令）→ …物理 / 脚本… → `flush_audio()`
   （drain → `engine.apply`）。放在 `World::update` 内部，编辑器 Play 与
   独立游戏天然一致（仓库铁律）。

### 三、组件与落盘

`Entity::audio: Option<AudioSource>`（一等字段，`#[serde(default, skip_serializing_if)]`，
与 `physics` / `animator` 完全同款的旧文件兼容策略）。字段：
`clip / volume(1.0) / pitch(1.0) / looping(false) / autoplay(true) / spatial(false) /
min_distance(1.0) / max_distance(50.0)`，结构体级 `#[serde(default)]`。

`autoplay` 默认 `true` 是刻意的：给实体挂音频源的作者意图几乎总是"进场景就响"；
要"等脚本触发"就关掉 autoplay、脚本里 `play_sound`。首播去重靠
`World.audio_autoplayed: HashSet<EntityId>`（标记进集合后不再触发，组件摘除 /
实体删除 → `StopEntity` + 清标记 → **重挂再响**，`removed_entity_or_component_stops_its_sound`
的后半段钉死这个来回）。

### 四、空间化与监听者

- kira spatial sub-track：`add_spatial_sub_track(listener.id(), pos,
  SpatialTrackBuilder{distances{min,max}})`，Drop 即从混音树移除；
  `set_position` 每帧推发射位置（组件 spatial 声源每帧从世界矩阵取位置）。
- 监听者姿态：`ListenerPose::new(position, forward)` —— **只收位置与朝前向量**，
  up 恒 +Y、`right = forward × up`（相机帧约定）；`forward` 存归一化值，
  竖直朝向（与 up 平行）退化取 +X 而不是 NaN（`vertical_forward_does_not_produce_nan`）。
- 编辑器 Play 每帧 `set_listener(编辑器相机 eye/forward)`；`xgame-game`
  固定机位与 `ListenerPose::default()` 一致，不需每帧推。
- kira 类型（glam 0.33）不出 `xgame-audio` 墙 —— 跨界只传 `[f32;3]` / `[f32;4]` 数组
  （引擎 glam 是 0.29，两个版本不能混类型）。

### 五、脚本 API（6 个，`Docs/脚本API.md` §4.11）

`play_sound(ctx, clip, volume, pitch)` / `play_sound_at(ctx, clip, x, y, z, volume)` /
`stop_sound(ctx, clip)` / `stop_all_sounds(ctx)` / `set_listener(ctx, x, y, z, fx, fy, fz)` /
`set_master_volume(ctx, v)`。全部是 `GameCtx` 方法（`ctx.rs` 真相源，
`lua_api.rs` 每函数一行转发）；`FrameCtx.audio` / `GameCtx.audio` 为 `None` 时
静默 no-op（编辑态、测试构造不需音频也能跑）。

### 六、编辑器接入

- Inspector 新增 **Audio** 区块：clip 路径 / volume / pitch / loop / autoplay /
  spatial / min-max distance，离散编辑走场景撤销栈（与 Name/Transform 同通道）；
- `GameRuntime` 注入 `AudioEngine::open(root)`，每帧 `set_listener(编辑器相机)`；
- 状态栏加 **Audio** 行（`AudioStats`：设备可用 / 活跃声数 / 缓存 clip 数）；
- `xgame-core::check` 加 `audio_refs` / `missing_audio`（clip 断链进悬空引用报告，
  `--check` 与 MCP `project_check` 同源）；
- `ProjectHandle::list_audio()` 扫 wav/ogg/mp3（`content.audio_dir` 子树）。

### 七、示例与资产

SampleScene：`Resources/Audio/Beep.wav`（44.1 kHz / 16 bit / mono 880 Hz
衰减正弦，`tools/gen-beep-wav.ps1` 生成，88 KB 入库）。Spinner 实体挂
空间化循环蜂鸣（走近响、走远轻），Jumper 起跳播一次性音效
（`play_sound` pitch 1.5）。`bundled_sample_project_loads` 补了音频引用
存在性断言（clip 断链在编辑器里不显形 —— 解码失败只记日志然后无声，
必须在加载前断言才能被 CI 抓住）。

### 八、回归测试清单（21 条新增，全部先证红）

| 层 | 测试 | 钉死什么 |
|---|---|---|
| core | `audio_source_ron_round_trips` / `_defaults_when_fields_omitted` / `audio_source_survives_ron_roundtrip` / `plain_entity_omits_audio_field` / `audio_participates_in_entity_equality` | 落盘格式 + 旧文件兼容 |
| core | `audio_port_collects_commands_in_order` | 命令束顺序 |
| core | `listener_orientation_matches_camera_convention` / `vertical_forward_does_not_produce_nan` | 姿态数学（竖直朝向不 NaN） |
| core | `source_maps_to_play_sound` | 组件 → 播放请求映射（spatial 只看 position） |
| audio | `plays_and_stops_a_real_wav_in_silent_mode` | 真 WAV 解码 + 停止 + GC（墙钟） |
| audio | `missing_clip_is_counted_but_not_retried` | 解码失败进 failed 表、不重试 |
| audio | `entity_scoped_stops_and_emitter_updates` | StopEntity / EmitterPosition 按实体匹配 |
| audio | `linear_volume_maps_to_decibels_safely` | 音量→分贝换算 + clamp（0 音量不炸） |
| script | `audio_api_buffers_commands_or_noops` | GameCtx 音频方法缓冲 / None 静默 |
| script | `script_audio_calls_reach_the_port` | Lua 6 个函数 → port 端到端 |
| runtime | `autoplay_sound_requests_play_exactly_once` | 首播一次（标记表去重） |
| runtime | `real_wav_autoplay_becomes_active` | 真 WAV 进 ActiveSound |
| runtime | `spatial_source_pushes_emitter_positions_each_frame` | 空间声源每帧推位置 |
| runtime | `removed_entity_or_component_stops_its_sound` | 删实体 / 摘组件停音 + 重挂再响 |
| runtime | `script_play_sound_reaches_the_engine` | 脚本 → World → 引擎全链路 |
| runtime | `audio_rescan_picks_up_new_entities` | 快速路径的重扫时机（实体数变化必须重新武装扫描） |

### 九、性能：音频同步"先量后改"（同机背靠背）

`update_throughput`（2000 实体 × 300 帧，同机交替跑 before / after 二进制）：

| 档位 | 音频前（均值） | 音频后·无快速路径 | 音频后·有快速路径 |
|---|---|---|---|
| 纯视觉 | **19.3 µs/帧** | 25.0（**+5.7**） | **21.9**（含跨二进制布局漂移 ~2.5，见下） |
| 全静态体 | 101.0 | 109.7 | 105.5 |
| 稀疏动态 64 | 737.7 | 747.2 | 742.5 |
| 密集接触 1000 | 13281.7 | 13256.5 | 13282.5 |

- 起因：`sync_audio` 每帧扫一遍实体表找音频源 —— 2000 实体纯视觉场景下
  这是一条额外的内存流（实体结构每条几百字节），实测 +5.7 µs/帧。
- 修复：`audio_sources_seen` / `audio_scan_count` 快速路径 —— 没挂过音频源、
  没有 autoplay 标记、实体数也没变 ⇒ 整段跳过；实体数变化（脚本 spawn /
  destroy）重新武装一次扫描（`audio_rescan_picks_up_new_entities` 钉死）。
  **刻意取舍**：给已有实体在运行中直接贴 `audio` 组件（不增删实体）不会触发
  重扫 —— 动态发声的正路是脚本 `play_sound`（不经扫描），组件走场景配置。
- 收尾判定：把 `sync_audio` / `flush_audio` 两个调用整体注释掉再量得
  21.5~21.9 µs/帧 ≈ 快速路径版同值 —— 残余的 ~2.5 µs 差距来自**跨二进制
  的代码布局漂移**（同一份源码 batch 重编也会浮动），不是每帧音频工作。
  按仓库口径：±1.5× 的系统性漂移内，音频对热路径的增量 ≈ 噪声。

### 十、刻意的取舍

1. **不用 MockBackend**：无声模式走真实 `AudioEngine` 代码路径（只是不碰设备），
   测试用真 WAV。Mock 会把 kira 内部 trait 结构钉进测试，升级即碎。
2. **不做混音总线分组**（BGM / SFX 各自音量）：主音量 + 每声源音量已够 v1；
   总线要动 kira 的 track 层级设计，等有真实需求（游戏要"音乐音量滑块"）再开。
3. **不做流式长音频**：`StaticSoundData` 全量解码进内存（Beep.wav 88 KB 无感）；
   一小时 BGM 会吃几十 MB —— 流式播放（`StreamingSoundData`）留给需要时。
4. **`play_sound` 固定参数不接表**：Lua 侧签名 `(ctx, clip, volume, pitch)` 是
   位置参数，不支持 `play_sound(ctx, {clip=..., volume=...})` 的表式可选参数 ——
   与 `raycast` / `spawn_ball` 等既有 API 的固定参数风格一致，转发壳每函数一行
   的模式也不被破坏。
5. **`AudioEngine` 藏在 `xgame-audio`，`World` 持有但不暴露**：外部只能
   `with_audio_engine` 注入、`audio_stats` 读数 —— 防止有人绕过 port 直接
   摆弄 kira 句柄（那会绕开命令束的时序保证）。

### 十一、实现期踩的坑（防好心人翻案）

- **非空间化 autoplay 没绑实体** → 删实体音还在响。修复：`play.follow`
  恒设挂载实体（见二.2）。
- **测试 WAV 太短被 GC 干扰**：初版测试用 50 ms WAV + `Instant` 墙钟 GC，
  断言"active == 1"时声音已自然结束 → 红。改用 1~5 s WAV 后绿。
  教训：**GC 语义（墙钟 vs 句柄状态）要在测试里显式选择**。
- **`AudioManager` 六百多字节内联进 `Playback` 枚举** → clippy
  `large_enum_variant`。`manager` 装箱（热路径只按引用解引用，零成本）。
- **`map_or(true, …)`** → clippy `unnecessary_map_or`，改 `is_none_or`。

## 二十三、Agent-Native：MCP 工具面 + 无头运行（本轮）

差距分析「近期计划」第 ② 条落地：**无头运行**。判据还是那一条 ——「智能体没有
鼠标时，这条路走得通吗？」。`--check` 已回答"项目能不能加载"，本轮回答后半句：
**"跑起来会怎样"**。MCP 侧同时收获第 7 个工具 `run_headless`（透传壳），
至此工具面覆盖「读 → 改 → 查 → 看效果」完整闭环。

全量验证绿：`cargo check --workspace --all-targets` 绿、clippy **0 警告**、
`cargo test --workspace --all-targets` 全绿（**359 条**，较上轮 +20：headless
模块 15 条自带测试 + MCP 工具 5 条；其中 MCP 侧 4 条 spawn 行为测试在接线前
**先证红** —— 红的是 `UnknownTool("run_headless")` 与工具数 6≠7，随后实现转绿）。

### 一、形态与契约

```sh
xgame-editor --headless [--frames N] [--dt 秒] [--report 路径] [--dump-scene 路径] <项目>
```

- **实现位置在编辑器二进制**（`headless.rs`，`--check` 的邻居）：跑的是
  `World::update` —— 与编辑器 ▶ Play、独立游戏**同一段**代码，无头跑出的行为
  与玩家实跑一致。MCP 只做 spawn 透传，不做第二份帧循环。
- **退出码契约**（与 `--check` 对齐，给 CI / 智能体）：
  **0 = 干净跑完 / 1 = 加载失败或参数错误（报告不产出）/ 2 = 跑完但观察到问题
  （脚本 error 日志、非有限变换；报告照常输出）**。`2` 与 `1` 的分野是刻意的：
  "发现问题"的报告是完整可用的，智能体读 `error_samples` 就能定位。
- **报告字段即契约**：`frame_us`（最近秩百分位 —— 报告里每个数都是真实出现过
  的一帧，不是插值）、`logs`（含装载期脚本编译失败这种"帧 0"日志）、
  `error_samples`（前 10 条）、`non_finite_transforms`、`audio`（silent 引擎的
  真实解码 / 缓存计数）、`fixed_steps`。`json_contract_has_stable_keys` 钉死。
- **`--dump-scene`**：跑完后的场景落盘为 RON（与 `write_scene` 同一序列化配置）
  —— 观察物理落点 / 脚本改过的变换的出口，也可直接喂 MCP `scene_write`。

### 二、确定性（刻意的）

每帧固定 `dt`（默认 = 项目固定步长）、无输入、无墙钟依赖 —— 同一项目两次运行
**逐位一致**（测试里两次运行落盘场景 SHA256 相等钉死；是"改前改后对 diff"的
底座）。`--dt` 允许偏离固定步长（时钟补多个固定步），但**要确定性就沿用默认**。

### 三、实测（先量后改的"量"；本机样本，看数量级）

| 场景 | 300 帧墙钟 | 帧循环 | 帧均值 | p50 / p99 |
|---|---|---|---|---|
| `projects/SampleScene`（14 实体 / 10 刚体 / 3 脚本 / 1 音源） | 78~120 ms | 8.6 ms | 28.5 µs | 26 / 37~61 µs |
| `projects/BehaviorsDemo`（Rust 插件 + Lua 共存） | 62~82 ms | 1.3 ms | 4.4 µs | 3.7 / ~10 µs |
| 基线：`--frames 1` / `--check`（进程启动 + 装载） | 62~72 ms | — | — | — |

读法：**一次无头调用的固定成本 ≈ 进程启动 60 ms**（Windows 加载链接的 DLL），
帧循环本身个位数毫秒 —— "跑一下看看"在智能体循环里是廉价操作。

### 四、MCP `run_headless`（第 7 个工具，spawn 透传）

- **信封**：`{ "exit_code": 0|2, "report": {…}, "scene_ron": "…（include_scene
  才带）" }`。`report.ok=false`（退出码 2）**不是工具失败** —— 问题表达在报告里；
  只有打不开项目（退出码 1）才走 `isError`。
- **为什么走 spawn**：无头实现住在编辑器二进制里（同一段 `World::update`）；
  server 装在哪编辑器就在哪（`dist/`、`target/debug/` 都是同目录布局）——
  `find_editor_exe` 同时找同目录与**上一级**（`cargo test` 时 server 在
  `target/*/deps/` 下跑，编辑器 exe 在它上一级）。
- **校验归属编辑器唯一**：`frames` / `dt` 的合法性不做双侧校验（必然漂移），
  MCP 只做 JSON 类型检查、其余透传。
- **Windows 细节**：spawn 带 `CREATE_NO_WINDOW`（MCP server 常被 GUI 客户端
  无控制台拉起，不设会闪控制台窗）；孩子 stderr 进错误消息前**截尾 2000 字符**
  （`RUST_LOG` 开着时头部是成片日志噪声，不拿它灌智能体上下文）。

### 五、实现期踩的坑（防好心人翻案）

- **过期的编辑器 exe 会把测试挂死**：旧版编辑器不认 `--headless`，会把它当
  项目路径继续 `eframe::run_native` 弹窗 → 测试进程挂住。而 `cargo test` 不产出
  常规 bin 产物，`target/*/xgame-editor.exe` 可能停在上次 `cargo build`。
  处理：MCP 测试沿用 `xgame/tests/plugin_loading.rs` 的纪律 —— 取 exe 前先
  **嵌套 `cargo build -p xgame-editor` 一次**（OnceLock，每进程一次，带 240s
  超时防与外层 cargo 抢目标目录锁）。
- **PowerShell 管道会把子进程 stdout 的 UTF-8 按 GBK 解码弄坏**（中文变 `?`，
  JSON 直接解析失败）—— 手工冒烟要 `cmd /c "exe > file"` 做字节级重定向，
  或在管道前先 `[Console]::OutputEncoding = UTF8`。这是本仓库的老坑（PS 5.1），
  与 AGENTS.md 的工具链注意同源。
- **`find_editor_exe` 的上一级命中路径拼写带 `..`**（`deps/../xgame-editor.exe`）：
  测试若按字节比较路径会假红（功能是对的）。断言应比较**解析后的同一文件**
  （`canonicalize` 后比较），或者干脆只断 `is_some` + 文件名 —— 别把 `join` 的
  拼写行为写死进契约。

### 六、刻意的取舍

1. **`include_scene` 默认关**：默认响应只带报告（智能体大多只要"跑通没有"）；
   场景 RON 可能很大，要观察模拟结果再显式打开 —— 不拿大响应占上下文。
2. **`--headless` 不重编 Rust 插件**：`Plugins/*.dll` 按现状加载（与打包游戏
   同路径）；改了插件源码要先在编辑器 Play 一次或走 PIE 构建。
3. **不做输入模拟、不推进动画时钟**：无头没有输入源；动画采样在渲染宿主侧
   （`AnimationDriver`），脚本写的 `set_anim_param` 照常进场景、只是没人消费。
   两条都写进了 `Docs/无头运行.md` 的边界一节，不给"它应该能"的误解留空间。
4. **`--frames` 上限 100000**：防"手滑写个大数挂住一个智能体"；100k 帧 @60Hz
   已是 ~28 分钟模拟时长，真实验证远用不到。
5. **Worktree 工作区状态**：验证 BehaviorsDemo 时把 `target/debug/behaviors_demo.dll`
   同步进了 `projects/BehaviorsDemo/Plugins/`（该 DLL 是构建产物、未入 git；
   编辑器 PIE 平时也这么干）—— 过期 DLL 跨 ABI 加载是 0xc0000005 的已知雷
   （P1 记录在案），验证前刷新是纪律不是可选项。

## 二十四、渲染层 PBR + 逐实体材质覆盖 + 无头截图（本轮）

差距分析里"观感落后一代"的 P0 主因：光照模型停在 Blinn-Phong，材质参数
**读进来就丢掉**。本轮三件事一次闭环：① Cook-Torrance 替换 Blinn-Phong；
② 逐实体材质覆盖（`MaterialOverride`，"材质系统"的入口）；③ `--screenshot`
无头截图（渲染改动的唯一可靠验证手段）——截图接上 MCP 后，智能体能**看到**
自己造的画面。全量验证绿：`cargo check --workspace --all-targets` 绿、
clippy **0 警告**、`cargo test --workspace --all-targets` **391 条全绿**
（2 条基准默认 ignore；较上轮 +32：截图 CLI 18 种边角、材质 7 条、
合并语义 3 条、逐实体 tint 1 条、指纹 1 条、MCP 3 条）。

### 一、PBR：Cook-Torrance 替换 Blinn-Phong

- 材质参数（metallic / roughness）**原样进 GPU**，片元着色器实现完整
  Cook-Torrance：GGX 分布 + Smith 几何 + Schlick 菲涅尔；`F0 = mix(0.04,
  albedo, metallic)`、漫反射项 `kd = (1 - F)(1 - metallic)`。金属没有漫反射、
  绝缘体有 4% 基础反射 —— 从"每盏灯两个经验项"变成有物理意义的模型。
- **环境项 = `ambient × (albedo×(1-metallic) + F(N·V))`**：均匀环境近似。
  真 IBL（环境贴图 / 预过滤）留给 RenderGraph 轮 —— 但结构上先给金属留了
  环境反射的挂点，到时候只换这一项。
- **radiance × π 的亮度标定**：历史场景的灯强度是按 Lambert 手调的
  （`diffuse = albedo × intensity × ndotl`）。PBR 的漫反射是
  `albedo/π × radiance`：着色器里取 `radiance = intensity × π`，与旧公式
  **数值等价** —— 换模型不换亮度，材质调参才有可比较的基线（否则全场先暗
  3.14 倍，所有场景内容都得重调）。
- **`MIN_ROUGHNESS = 0.04`**：GGX 在 roughness = 0 时分母为 0（除零 → NaN
  随第一个像素扩散到整帧），夹到 0.04 的镜面感与真镜面肉眼无差。
- **色调映射（ACES）刻意暂缓**：HDR 目标要动 Presenter / 游戏 / 编辑器三个
  宿主的渲染目标，超出本轮；当前高光仍会裁剪，属已知限制（差距分析同步
  改了措辞）。

### 二、逐实体材质覆盖（`MaterialOverride`）

- **场景格式**：`Entity.material: Option<MaterialOverride>`，三个字段全
  `Option`（base_color / metallic / roughness），serde
  `default + skip_serializing_if` —— 旧场景逐位兼容；RON 里 `Option` 显式写
  `Some(...)`（与 `physics` / `audio` 同款，见 `material.rs` 文档的示例）。
- **字段语义**：base_color **相乘**（tint 乘子：内置白模 = 直接上色，
  glTF 带贴图的模型 = 调整体色调，a 乘透明度）；metallic / roughness
  **替换**（"乘 0.5 的金属度"没有直觉含义）；**blend 不参与**（透明与否是
  管线级行为，不是颜色参数）。
- **渲染侧结构改动（防好心人翻案）**：材质参数从"每材质一个绑定组"改为
  **逐 draw uniform** —— `DrawUniform { model, params, base_color, material }`
  （112 字节），staging 按**图元**分段、动态偏移取段；group(1) 只剩
  贴图 + 采样器。为什么必须换：逐实体覆盖让"同一 material_id 的两个实体
  材质不同"，按资产缓存参数要么改色时重建绑定组、要么缓存键随数值无限膨胀；
  逐 draw 段天然正确。上限 `MAX_RENDER_DRAWS = MAX_RENDER_ENTITIES × 2`
  （多材质网格一只实体出十几个 draw，不是一对一），超出静默丢弃 ——
  动态偏移越过缓冲 = 整帧校验失败，宁缺勿炸。
- **编辑器**：Inspector 新增 Material 区（Override 勾选 → Tint 色盘 +
  metallic / roughness 勾选式 DragValue）；`scene_signature` 纳入材质哈希
  （漏掉的话"在 Inspector 里调颜色"视口纹丝不动 —— 指纹门控的缓存帧）；
  组件字段编辑走既有撤销通道（P1.5 结论不变：参数是场景数据，编辑进撤销栈）。

### 三、无头截图 `--screenshot`

```sh
xgame-editor --headless --frames 150 --screenshot out.png \
    [--shot-size 1600x900] [--camera-pos x,y,z --camera-target x,y,z] <项目>
```

- 尺寸默认 1280×720、上限 4096²；`--camera-pos` / `--camera-target` **必须
  成对**（单给一侧 = 参数错误）；`--shot-size` 等只在配合 `--screenshot` 时
  才合法（给了没截图 = 报错，不给"静默忽略"留歧义）。
- **默认取景 = 拟合全场 + 顺光机位**：先 `frame_bounds` 框住全部可绘制实体
  （含背景模型），再算**相机方位 = 主方向光的 XZ 方位角 + 35°、仰角 26°**。
  为什么是顺光：太阳在 -x/-z 时把相机放在 +x/+z，画面全是背光面 + 一丝环境
  光，白模看起来像"渲染坏了"——实测第一版截图如此，不是 PBR 的问题而是机位
  问题。无可用方向光时退回固定方向（纯函数 + 两条测试钉死）。
- **无 GPU：退出码 1、不降级**。音频可以 silent 降级（照常计数只是不出声），
  截图没有"静默降级"的余地 —— 拿不到适配器就是拿不到图。
- 报告新增 `screenshot { path, width, height, eye, target }`：`eye/target` 是
  **实际使用**的相机（自动取景时是拟合结果）——"这张图从哪拍的"必须可复现。
- **确定性**：同机两次进程运行 PNG **逐字节一致**（本轮实测 SHA256 相等）；
  跨机器不承诺（GPU 光栅化差异），文档按此口径。
- **`--screenshot` 是推进动画时钟的唯一例外**：静态截图里角色停在绑定姿态
  没有信息量，截图路径每帧 tick `AnimationDriver`；其余无头路径维持"不推进
  时钟"的边界（脚本 `set_anim_param` 照常进场景、没人消费）。

### 四、MCP：`run_headless` 加 `screenshot`

- `screenshot: true` → spawn 编辑器带 `--screenshot <临时 PNG>`；读完即删，
  PNG 以 **MCP `image` 内容块**（base64）随结果返回 —— 视觉模型直接看到
  画面本身，而不是"渲染没报错"的口头保证。信封加
  `{"screenshot": {"bytes": n}}`，实际机位在报告的 `screenshot` 节里。
- **只有一张图、默认取景，不给相机参数开洞**：智能体很难凭一组 xyz 取景，
  自动取景 + 可复现机位报告已经够用；要精细取景走 CLI。
- base64 自写 30 行（不引新依赖），RFC 4648 已知向量 + 长度关系测试钉死；
  「ToolOutput → content 块」抽成纯函数 `tool_result_content` 单独测 ——
  `mimeType` 是协议拼写，写错客户端会把图片块静默丢掉。

### 五、示例内容（SampleScene 上色）

- 内置白模第一次有颜色：地面混凝土（0.55/0.58/0.62，rough 0.9）、平台三级
  暖沙、木箱三兄弟深浅、蓝半光泽 Spinner（rough 0.35）、红塑料球
  （rough 0.30）、抛光金属柱（metallic 1.0 / rough 0.35，与红球对照
  "金属 vs 塑料"）。
- **太阳强度 3.2 → 1.4**：3.2 是 Blinn-Phong 时代"白模也要亮"的手调值 ——
  白 albedo 地面在 ndotl 0.87 × 强度 3.2 下辐射度 ≈ 2.7，早已是裁剪纯白。
  PBR 的 π 标定保持了这个亮度（不是回归），所以过曝是**内容**问题：上色 +
  回调强度后，受光面落在"高光但不死白"的区间。
- 实拍：`target/shots/after3_sample.png`（自动取景，顺光）、`hero1.png`
  （近景：高光、软影、材质差异全部可见）。

### 六、测试与先证红

- 先证红（仓库纪律）：材质 uniform 两条测试在旧实现下红（旧代码把
  metallic=1 写成 params[0]=0.6）；金属/绝缘像素测试在旧 Blinn-Phong 下
  **方向相反**（184 vs 152 luma；PBR 后 95 vs 147）——换实现后转绿即证。
- 新增：core 7 条（material.rs 4：字段级解析 / 空表 / None 不落盘 / 往返；
  scene.rs 3：落盘往返 / 普通实体不写字段 + 旧文件解析 / 脏检查）、
  gltf_loader 合并 3 条、render 逐实体 tint GPU 测试 1 条（覆盖 → 像素变蓝、
  清掉覆盖 → 与基线**逐位一致**）、editor 指纹 1 条、MCP 3 条
  （base64 向量 / 截图像素块 / content 块协议形状）。
- **像素测试的一个实测账**：tint 测试最初把光与相机放同轴，GGX 副瓣在中心
  像素叠了 ~0.24 线性白（roughness 0.45 时 D≈7.8），看去像"乘子没生效"
  （实测 176 vs 基线 255，比值 0.69 > 阈值 0.5 假红）。把光源错开轴线后
  中心像素是纯漫反射，断言才有判别力 —— **写像素测试必须算一遍镜面项**。

### 七、刻意的取舍

1. **不做 IBL**：金属在均匀环境近似下偏暗（只有 ambient×F 兜底），这是
   物理上诚实的"没有环境可反射"；真 IBL 属 RenderGraph/后处理轮。
2. **不做 ACES 色调映射**：见第一节；现在的高光裁剪是已知限制。
3. **材质不接脚本 API**：本轮入口 = Inspector / 场景 RON / MCP
   `scene_write`；脚本 `set_material` 留给下一轮（连带动画/材质曲线一起做）。
4. **blend 不可覆盖**：见第二节；要"开关透明"得换材质而不是改参数。
5. **材质随 prefab 自动走**：实例化走 `entity.clone()`，新字段零改动自动
   进预制体实例（这是"Entity 字段一律 Clone + PartialEq"惯例的红利）。

## 二十五、第九阶段：HDR 渲染路径 + ACES 色调映射 + 曝光控制（本轮）

二十四节的"刻意的取舍 2"（不做 ACES 色调映射）在本轮兑现 —— 差距分析里
"PBR 落地后最划算的下一项"（高光直接裁剪是最扎眼的已知限制）。三件事一次
闭环：① 场景 pass 从直写 sRGB 目标改为**渲进 `Rgba16Float` HDR 中间标**；
② 新增全屏后处理 pass 做**曝光 + ACES 色调映射**写进 LDR 显示目标；③ 画面
设置落 `project.ron` 新增的 `render` 节，三家宿主（编辑器视口 / 独立游戏 /
无头截图）**同一份配置、同一条链**。全量验证绿：`cargo check --workspace
--all-targets` 绿、clippy **0 警告**、`cargo test --workspace --all-targets`
**407 条全绿**（3 条基准默认 ignore；较上轮 +16 条、+1 条 GPU 基准）。

### 一、HDR 渲染路径：为什么中间标是必须的

- LDR 目标的写入是**钳制**的：8 位 sRGB 存不下 4.0 的高光，超出的部分在写
  下去那一刻就被裁掉 —— 高光细节不是"丢了"，而是**从来没被看见过**。色调
  映射的全部意义就是在这次裁剪**之前**拿到真实亮度。
- `HDR_FORMAT = Rgba16Float`：WebGPU 核心格式里唯一同时满足"浮点、可混合、
  可 MSAA、可线性过滤"的颜色目标；存储带宽只有 `Rgba32Float` 的一半。
- **`HdrTargets` 收拢三家宿主的附件创建**（HDR 颜色 + MSAA 颜色 + 深度）：
  编辑器要节流重建、游戏跟 resize、截图建一次，但"需要哪几张纹理、采样数
  怎么对"是同一件事 —— 三份手写纹理描述迟早漂移，而采样数不匹配的渲染通道
  在提交时**直接校验失败**。
- MSAA 采样数按 **HDR 格式**问 `supported_sample_count`，不是显示格式：采样
  数是格式相关的，拿显示格式问出来的答案在 HDR 附件上不一定成立。

### 二、后处理 pass 与"不做 RenderGraph"

- 后处理是**一趟全屏单采样 pass**（全屏三角形，位置由 `vertex_index` 现算）：
  采样 HDR → ×曝光 → ACES（或 Off）→ 钳制 →（必要时手动 sRGB 编码）→ LDR。
- **为什么不做通用 RenderGraph**：通用帧图是给"十几个 pass 互相借纹理"的
  规模准备的；当前链是线性的一串。现在抽图只是抽象税 —— 等出现真正带依赖
  分叉的 pass（SSAO 要深度、TAA 要历史缓冲、CSM 要重投影）再抽，那时是
  **有信息**地抽。这条写进 `postfx.rs` 模块文档，防好心人提前上框架。
- 绑定组按宿主维护的 `target_gen`/`source_gen` 缓存：源视图只在目标重建时
  才换，每帧新建绑定组是白给的分配；但**不能用指针判等** —— `TextureView`
  没有稳定身份，指针在释放后可能被复用（表现是画面停在旧尺寸的最后一帧）。
- `needs_manual_srgb(format)`：sRGB 目标交给硬件编码（着色器输出线性值），
  非 sRGB 目标（`Bgra8Unorm` 之类，交换链格式由系统挑）必须着色器手动编码 ——
  赌"交换链一定是 sRGB"会在部分机器上整幅偏暗。

### 三、`render` 配置节（project.ron）

```ron
render: (exposure: 1.0, tone_map: Aces),
```

- `RenderConfig { exposure, tone_map }`，serde `default` +
  `skip_serializing_if`：**旧清单没有这一节照常加载**
  （`old_manifest_without_render_defaults`）。
- `sanitized()` 把曝光夹进 `[0.05, 20]`（0 或负数会让整帧发黑 / 负值进曲
  线），NaN/∞ 落回 1.0 —— 与 `runtime` 节同款契约，入口（设置面板 / MCP）
  统一走它。
- `ToneMapMode` 的 **RON 变体标签即磁盘格式**（`tone_map_ron_labels_are_stable`
  钉死）：改标签名 = 作废所有旧清单。
- 编辑器：设置面板新增"画面（曝光 / 色调映射）"区；视口 `scene_signature`
  纳入 `RenderConfig`（漏了表现为"在设置里调曝光视口纹丝不动" —— 指纹门控
  的缓存帧，`signature_tracks_render_settings` 钉死）。
- 无头运行报告与 MCP `project_info` 各带一个 `render` 节（字段名进稳定契约，
  `json_contract_has_stable_keys` 同步）："这张图按什么设置拍的"与机位一起
  可复现。

### 四、ACES 与亮度基线

- **曲线 = Stephen Hill 的 RRT + ODT 有理函数拟合**（三.js / Unity 同款近似）：
  先转到 ACEScg（AP1）做曲线，再回 sRGB。比"对 RGB 直接套一条曲线"的朴素
  做法最大收益是**过曝处的色相漂移小**：纯红高光烧出来仍红偏白，不会变橙黄。
- **曲线前 /0.6**：拟合把 0.18 中灰映到 ~0.10（整幅暗一档）。本引擎亮度按
  "白面 ≈ 1.0 线性"标定（PBR 的 π 校准），除以 0.6 把中灰拉回 ~0.21 ——
  **整体亮度基线不因接入色调映射而变**，变化只在高光那一端（`mid_gray_stays_mid_gray`
  钉死：0.18 中灰显示值仍落 110~150）。
- **`ToneMapMode::Off` 是兼容模式不是调试开关**：曝光 + 钳制的输出与色调映射
  上线前的旧管线**逐位一致**（`off_mode_matches_the_pre_tonemap_pipeline`）——
  旧项目迁移与像素回归对照都靠它。
- 曝光是**曲线之前**的亮度乘子：挪到曲线之后会把高光端钳平，不是风格问题
  而是语义问题（`exposure_scales_before_the_curve` 与 GPU 侧同款测试双钉）。

### 五、测试与先证红

- **先证红（实测）**：把 `HDR_FORMAT` 换回 `Rgba8UnormSrgb` 后
  `aces_keeps_highlight_detail_that_off_clips` 立刻红（ACES 像素 226 vs
  预期 243）—— 场景直写 sRGB 目标时写入即钳制，"ACES 与 Off 的差异"根本
  无从谈起；换回 HDR 路径转绿即证接通。
- CPU 镜像 `postfx::cpu`（与 WGSL 逐式对应，改一边必须改另一边）：8 条纯函数
  测试 —— 端点、单调性（0..32 扫描无回折）、滚降（4.0 与 16.0 映射到两个不同
  的非 255 值）、曝光在曲线前、Off = 旧管线、手动 sRGB 编码对表（0.5 → 188）、
  `needs_manual_srgb` 判定、中灰基线。
- GPU 像素测试 2 条（`screenshot.rs`）：
  `aces_keeps_highlight_detail_that_off_clips`（Off 必须纯白 255、ACES 必须
  <250 且与 CPU 镜像对表 ±4）、
  `exposure_scales_highlights_before_the_curve_on_gpu`（0.25/1/4 三档显示亮度
  严格递增 + 对表一档）。
- core 5 条：`render_config_defaults_to_aces_and_unit_exposure` /
  `render_config_sanitized_rejects_garbage` / `old_manifest_without_render_defaults` /
  `render_survives_manifest_roundtrip` / `tone_map_ron_labels_are_stable`；
  editor 1 条（指纹纳入画面设置）；headless `json_contract_has_stable_keys`
  扩展 `render` 节断言；MCP 断言并入 `project_info` 既有用例。

### 六、性能实测（先量后改：GPU 时间戳口径）

新增基准 `crates/xgame-render/tests/postfx_cost.rs`（`#[ignore]`，手动跑）：
同机背靠背测"旧 LDR 直渲"与"新 HDR + ACES"的每帧成本（1280×720，地面 +
64 立方体 + 跟随阴影，MSAA x4）。**口径教训**：首版用墙上时钟，被本机噪声
打穿（同一路径轮间 207~515 µs、胜负都能翻转），改为 **GPU 时间戳查询**
（帧头 / 场景末 / 帧末三个点）后轮间波动 <1 µs —— 基准测的是 GPU 成本就
别让 CPU 与调度混进来。

| 段 | 旧 LDR 直渲 | HDR + ACES |
|---|---|---|
| 场景 pass（含 MSAA resolve） | 121.2 µs | 135.2 µs |
| 后处理 pass | 0（无） | 22.6 µs |
| **合计（GPU）** | **122.0 µs** | **157.8 µs** |
| CPU 编码 + 提交 | 137.2 µs | 155.5 µs |

**差额 +35.8 µs/帧（+29.3%）**：后处理 pass 22.6 µs 是纯新增；场景段
+13.9 µs 是 fp16 附件写带宽 + fp16 resolve。折算：60 FPS 预算（16.6 ms）的
0.22% —— 换来高光不再裁剪，账很清楚。CPU 侧 +18 µs 是"多记录一趟 pass +
多写一只 uniform"。

### 七、实拍对照（曝光 4.0）

- `target/shot_off4.png`（Off）：地面 / 顶面整体爆白，**24747 个纯白像素**
  （1280×720 隔点抽样 10.7%）；红盒成为生硬纯红。
- `target/shot_aces4.png`（ACES）：纯白像素 **0 个**；地面保留明暗层次、
  红盒高光向珊瑚色柔和滚降（高光去饱和是 ACES 的特征形态）。
- 校准基线：线性 1.0 → 显示 ~226/255、4.0 → ~250（不裁顶）；0.18 中灰 →
  ~127（Off 模式下 4.0 与 16.0 都是 255）。

### 八、刻意的取舍（防好心人翻案）

1. **不做通用 RenderGraph**：见第二节 —— 线性链抽图只是抽象税。
2. **`Off` 保留而不是删掉**：它是旧管线的兼容出口与像素对照基准；默认
   `Aces`，但"想看裁剪语义"是合法需求。
3. **1/0.6 的亮度基线不许动**：动它 = 全部旧场景整体明暗挪一档，与 PBR 的
   π 校准是同一条"换模型不换亮度"的原则。
4. **不做自动曝光**：需要帧间状态的亮度统计（直方图读回或计算着色器 + 历史
   缓冲），先把静态曝光 + ACES 铺平 —— 与 Bloom 一起留在后处理链的下一片。
5. **无 IBL 的限制仍在**：金属的世界只有 `ambient × F` 兜底 —— 色调映射把
   高光滚下来后它更好看了，但"没有环境可反射"这件事没变（差距分析同步记）。
6. **不给任何宿主留"绕过色调映射"的旁路**：截图 / 视口 / 交换链三个目标
   同源，旁路会让三者画面分叉 —— 与"单一运行时"是同一条纪律。

## 二十六、第九阶段：Bloom 泛光（本轮）

二十五节把 Bloom 与自动曝光一起留给了"后处理链的下一片"，本轮兑现 Bloom：
**亮部提取（软膝）→ 三级 mip 降采样 → 可分离高斯模糊 → 与色调映射同一趟
合成 pass 叠加**。默认**关**（`intensity == 0` 是唯一关闭形态），开启才惰性
建链；参数落进 `project.ron` 的 `render.bloom`（intensity / threshold / knee），
编辑器设置面板可调、智能体改一行文本即可，三宿主（编辑器视口 / 独立游戏 /
无头截图）同一条链。全量验证绿：`cargo check` / clippy **0 警告**、
`cargo test --workspace --all-targets` **419 条全绿**（3 条基准默认 ignore；
较上轮 +12 条：core 配置 2 + CPU 镜像 8 + GPU 像素 2）。

### 一、链的形状：为什么是"亮部提取 + 三级 mip + 可分离高斯"

```text
HDR ──亮部提取(软膝)·½降采样──▶ a0 ──降采样──▶ a1 ──降采样──▶ a2
                                 │            │            │
                                 ├─横/纵模糊──┤（每级 a、b 两块纹理乒乓，
                                 ▼            ▼   同纹理不能既当输入又当输出）
                           a0/a1/a2（三级模糊结果，合成 pass 读取）

合成（同一趟全屏 pass）：HDR ─▶ 曝光 ─▶ + Σ(aᵢ×wᵢ)×intensity ─▶ ACES ─▶ LDR
```

- **可分离**：N×N 高斯要 N² 次采样，横一趟 + 纵一趟各 9 抽头只要 18 次 ——
  同一视觉结果，采样数降 4.5×；分辨率减半等效核宽翻倍，三级 mip（½、¼、⅛）
  用同一套核叠出由近及远的柔和光晕。九趟 pass 全部在 ≤½ 分辨率上（三级合计
  像素量 = 主画面的 21/64 ≈ 33%）。
- **9 抽头核 `[1,8,28,56,70,56,28,8,1]/256`**（二项式）：近似正态、系数和恰为
  256，归一化在纸面可验（镜像测试断言合成权重和为 1）。
- **降采样一次双线性到位**：输出像素中心恰好落在源 2×2 texel 块的中点 ——
  一次 `textureSample` 就是一次箱式平均，比四次点采样便宜，也没有接缝。
- **合成权重 `[0.5, 0.3, 0.2]`**：小 mip 更模糊、覆盖更广，权重更低；乘
  intensity 后是"光晕总能量"的线性度量。

### 二、亮部提取：阈值在曝光后判 + 软膝 + 按最大通道

- **阈值是"显示值"语义**：摄影上"多亮算亮"取决于曝光后的画面（threshold = 1.0
  就是显示白点）。提取时先乘曝光，合成时光晕也加在曝光后的颜色上 —— 改曝光
  光晕跟着走，不用重调阈值（`..._applies_exposure_first` 钉死）。
- **软膝**（Unity URP 同款口径）：

  ```text
  soft = clamp(brightness − threshold + knee, 0, 2·knee)
  soft = soft² / (4·knee + 1e-5)
  weight = max(soft, brightness − threshold) / max(brightness, 1e-5)
  ```

  `knee = 0` 退化为硬阈值（`..._knee_zero_is_a_hard_threshold`）；分母的
  `1e-5` 兜住除零。硬阈值的亮斑边缘是一圈"刀切"，软膝让贡献在阈值附近
  二次淡入 —— 那种"比不做还难看"的硬边就是它治的。
- **权重 ≤ 1 的归一化**：软膝区按比例淡入、过膝后全额参与，光晕不会比源像素
  还亮。提取保留原色相（乘权重而非替换颜色）：白斑发白光，红斑发红光。
- **按最大通道判亮度**：纯红的高光不该因为"平均亮度低"而不发光 —— 彩灯的
  光晕是彩色的（`..._selects_by_the_max_channel`）。

### 三、默认关：风格参数不进默认画面

- **默认 `intensity = 0`**：Bloom 是**风格**参数（写实档也常关），不是
  "正确性"参数 —— 与色调映射相对照：后者回答"高光怎么落进显示区间"，是必须
  回答的问题，所以默认 ACES；前者是"要不要发光"的口味问题。要发光的项目把
  intensity 调到 0.4~0.8 即可。
- **不设独立开关**：`intensity == 0` 就是关 —— 两个字段表达同一状态迟早
  互相矛盾。
- **关着时整段跳过**：`PostFx::render` 不建纹理、不记录 pass，还把链丢回
  `None`（不养显存）；是真正的零成本，而不是"跑一遍然后乘 0"。开启时惰性
  建链，链的重建判据是**源尺寸**（尺寸不变就只记录 pass）。
- **合成绑定组缓存键 =（源代, 尺寸, 开关）**：源视图换代 / 尺寸变化 / 开关
  翻转都会拿到新绑定组；关着时 mip 槽位用**源视图占位**（绑定组要求槽位逐项
  填），着色器按 `bloom_intensity`（uniform）分支不去采样 —— uniform 值 ⇒
  整个 draw 走同一分支，`textureSample` 在一致性控制流里才合法。
- **入口三件套**：`project.ron` 的 `render.bloom`（`sanitized()` 夹取安全范围）、
  编辑器 ⚙ 设置面板三控件（范围来自 `BloomConfig::MAX_*`，hover 有说明）、
  MCP / 报告 JSON 只读投影 —— 与曝光 / 色调映射同一条"单入口"纪律。

### 四、wgpu 边界：几个踩过就懂的细节

- **同一张纹理不能既当输入又当输出**（wgpu 提交时校验）：每级 mip 配一对
  a/b 纹理乒乓，横向模糊 a→b、纵向 b→a，最终结果回到 a。
- **每趟 pass 一律 `LoadOp::Clear`**：全屏三角形铺满目标，内容每帧被完整
  覆写 —— "建时清黑"那点状态不需要，还消灭了"采样到未初始化内容"的窗口。
- **纹理本体放手是安全的**：wgpu 的视图持纹理的 `Arc`、绑定组持缓冲的
  `Arc`（查过 wgpu-core 23 源码）；链的字段只存视图与绑定组，不存纹理句柄。
- **`wgpu::Sampler` 不是 Clone**：`BloomChain::record` 收采样器参数而不是
  自己持有一份。
- **pass 顺序就是执行顺序**：链记录在合成 pass 之前，同队列上的隐式屏障
  保证合成当帧读到链的结果，不需要手动同步。
- **亮度提取的源是外部（HDR 附件）**：它的绑定组按 `source_gen` 单独缓存；
  降采样 / 模糊的源与目标都在链内，绑定组建链时一次性建好。

### 五、测试与先证红

- **先证红（两条实测）**：① 强制 bloom 分支为关 → `bloom_glows_around_a_bright_spot`
  红（开 / 关同一点逐位相同，差 0）—— 证明"光晕"不是场景自身的形态；
  ② 把 `fs_bright` 的权重改成恒 1.0 → `bloom_leaves_sub_threshold_content_dark`
  红（阈值以下的微光被错误点亮）。
- **CPU 镜像新增 8 条**（`postfx::cpu`，与 WGSL 逐式对应）：阈值下全黑、
  软膝形状（单调 + 与硬阈值可区分）、knee=0 硬阈值、最大通道选择、曝光先行、
  权重和为 1、合成在曲线前（同权重下"先加光晕再 ACES" ≠ "先 ACES 再加"）、
  关时与纯色调映射逐位一致。
- **GPU 像素测试 2 条**（`screenshot.rs`）：光块轮廓外 ~5px 处开 > 关 +25、
  远角差 < 4（光晕围着亮源、不照亮全场）、强度 2.0 > 0.6 + 5（单调）；
  微光内容（受光面线性 ≈ 0.19，低于软膝下限 0.4）开关两档差 < 3。
- **core 2 条**：默认关 + 默认不落盘（`skip_serializing_if`） / `sanitized`
  容错（NaN / 负数 / 超限；`intensity` 负数 = 关，不是"反向光晕"）。
- **编辑器**：`scene_signature` 纳入 bloom 三字段（3 条 `assert_ne`）——
  防"调泛光视口纹丝不动"的指纹门控 bug。
- **报告契约**：headless 报告与 MCP `project_info` 的 `render.bloom` 三字段
  进稳定契约（`json_contract_has_stable_keys` 同步）；`two_runs_produce_identical_png_bytes`
  改用 bloom 开启（0.7）跑 —— 光晕链逐字节确定。

### 六、性能实测（GPU 时间戳口径）

沿用 `postfx_cost.rs`（1280×720、MSAA ×4、地面 + 64 立方体 + 阴影），
三路径交替 ×8 轮 ×64 帧，取中位数：

| 段 | 旧 LDR 直渲 | HDR + ACES | HDR + ACES + Bloom |
|---|---|---|---|
| 场景 pass（含 MSAA resolve） | 121.8 µs | 134.6 µs | 135.2 µs |
| 后处理 pass | 0.7 µs（连打两点残留） | 24.6 µs | 85.0 µs |
| **合计（GPU）** | **122.5 µs** | **159.2 µs** | **220.2 µs** |
| CPU 编码 + 提交 | 140.4 µs | 157.1 µs | 233.8 µs |

- **Bloom 相对纯 ACES +61.0 µs/帧（+38.3%）**：1 趟亮部提取 + 2 趟降采样 +
  6 趟可分离模糊，全在 ≤½ 分辨率。折算 60 FPS 预算（16.6 ms）的 0.37%。
  同会话重跑合计 220.3 µs、相对 ACES +60.8（+38.1%）—— 两跑互证。
- 三条路径的场景段在断言容差（< 5 µs）内一致（134.6 / 135.2）—— 链没有
  回头改场景渲染。
- CPU 侧 +76.7 µs 是"多记录 9 趟 pass + 每趟写 uniform"的编码开销；链的
  纹理与绑定组只建一次，不进每帧成本。
- ACES 相对旧路径 +36.7 µs（+30.0%），与上一轮（+35.8 / +29.3%）及同会话
  重跑（+37.8 / +31.1%）同量级 —— 三次读数互证，口径稳定。
- 基准自带防"空调度"断言（P2.5 教训）：`bloom_post > hdr_post + 1.0`（链
  缺席必红）、场景段差 < 5 µs、时间戳读数非零。
- 口径残留：旧路径"连打两个点"的后处理段读数不是 0（本次 0.7 µs），那
  就是 `write_timestamp` 本身的开销 —— 把它当噪声底看。

### 七、实拍对照

- `target/bloom_off.png`（默认关）：平台边缘干净利落、阴影锐利。
- `target/bloom_on.png`（演示**刻意调强**：threshold 0.8 / intensity 1.2）：
  整个亮面的边缘往外"渗光"，台上物件带柔和光晕，与环境光的灰底连成雾感。
- 逐像素比对（1280×720 隔点 230400 样本；两图 eye/target 逐位相同，差异里
  没有第二个变量）：全部样本**变亮或持平**（变暗 0 个）、平均 +4.9/通道、
  最大 +122 —— 与"光晕加在曲线前、ACES 单调"的预期一致。
- 对照参考：默认参数（threshold 1.0 / intensity 0.8）下差异更含蓄 ——
  ~1% 像素 Δ > 3（maxΔ 45）。threshold = 1.0 是显示白点，SampleScene 里
  只有高光面越得过去 —— 这与"默认关"呼应：想看见光晕，先把艺术参数往上调。

### 八、刻意的取舍（防好心人翻案）

1. **默认关**：见第三节 —— 风格参数不该改变默认画面；"要不要发光"与
   "高光怎么落进显示区间"是两类问题。
2. **阈值在曝光后判**：挪到曝光前会让"调曝光"与"调阈值"耦合（每次改曝光都得
   重调阈值），摄影语义只有一种直觉。
3. **光晕加在曲线前、不加在曲线后**：曲线后加是"贴纸"（暗部会蒙一层灰、
   高阈值时光晕先在 LDR 端被裁出硬边），曲线前加才等价于"场景里真的有那么
   亮的光"。
4. **不抽 RenderGraph**：链仍是线性固定序列（提取 → 2 降采样 → 6 模糊 →
   合成），理由与二十五节相同 —— 等出现带依赖分叉的 pass 再抽。
5. **不做自动曝光 / 亮度统计**：需要直方图读回或计算着色器 + 历史缓冲，
   继续留给后处理链的下一片（与 FXAA/TAA 排一起）。
6. **不引 Kawase 双滤波 / 各向异性核 / 镜头鬼影**：先把"正确、便宜、可解释"
   铺平；换核是风格问题不是功能问题。

## 二十七、第九阶段：程序化天空盒 + 环境光照（IBL）（本轮）

差距分析 P1 表里的 **IBL / 天空盒** 兑现：天空是一个**解析函数**（天顶 / 地平线 /
地面三段渐变 + 太阳盘 + 太阳光晕），同一个函数喂三个消费者 —— 背景天空盒、
漫反射环境光（L2 球谐）与镜面环境光（GGX 重要性采样预过滤立方体 + BRDF LUT）。
参数落进 `project.ron` 的 `render.sky`（七字段），编辑器 ⚙ 面板可调、智能体改
一行文本即可、三宿主（编辑器视口 / 独立游戏 / 无头截图）同一条链。
**默认关**（`enabled = false`）：关着时只写 80 字节 uniform（开关位 0），
不建内容、不采样三张环境纹理 —— 旧项目画面与上线前**逐位一致**；SampleScene 里
开启作为演示（那里有一个 metallic 1.0 / roughness 0.35 的金属件，正好展示镜面
环境反射）。全量验证绿：`cargo check` / clippy **0 警告**、
`cargo test --workspace --all-targets` **434 条全绿**（4 条基准默认 ignore；
较上轮 +15 条：天空 CPU 镜像 8 + GPU 像素 5 + core 配置 2）。

### 一、天空是一个解析函数：一个真相源喂三个消费者

```text
                      ┌─ 背景：天空盒 pass（全屏三角形，场景 pass 内、不透明之后）
sky_radiance(dir) ────┼─ 漫反射：CPU 数值积分 L2 球谐（存 E/π）→ binding 8
                      └─ 镜面：GPU GGX 重要性采样 → 预过滤立方体（5 级 mip）→ binding 9
```

- **不做 Preetham / Hosek 那类物理大气**：前者回答"长什么样"，后者回答"为什么
  长这样"。参数化渐变 + 太阳盘已经能交出可信的画面，而出口结构（"方向 → 辐射
  亮度"的一个函数）与物理模型**完全同构** —— 以后想换模型，下游（天空盒 /
  IBL / 测试）一行都不用改。
- **渐变在 `dir.y == 0` 处连续**：天空段与地面段在视线水平方向**都**取地平线色
  （`mix(horizon, zenith, y^0.5)` / `mix(horizon, ground, (-y)^0.5)`），没有
  "缝合线"这种需要专门修的伪影（`sky_gradient_is_continuous_at_the_horizon`
  从上下两侧夹逼地平线，断言各自与地平线色差 < 2e-3）。
- **太阳盘的面辐射亮度写死 40**（常数名 `SKY_SUN_DISK_RADIANCE`）：太阳在现实里
  远超"白面 1.0"，40 让它在 ACES 后是一小块近白、同时给 Bloom 留下可抓的过曝。
  盘面半径（度）来自 `sun_size`，光晕 `0.35 × cos^16` 是宽而软的近似二次散射。
- **太阳的位置与颜色来自场景**：`packed`（渲染管线实际用的 ≤8 盏灯）里**第一盏
  active 的方向光** —— 方向取传播方向的反向，颜色取灯色。没有方向光就没有太阳
  （`sun_intensity` 写 0，盘面与光晕整段不出现）。"背景里有太阳、物体上却没有
  对应光"这类错位从数据层就不存在：两边读的是同一盏灯。
- 亮部（相机朝上）与暗部（朝下）在 `t = 0` 处共享地平线色，因此"把相机抬起来
  看"只会让地平线色占据更多画面 —— 这正是无头默认机位（俯角 26°）下截图背景
  偏地平线色的原因，不是天空没生效。

### 二、漫反射 IBL：L2 球谐，存 E/π（与 ambient 同标定）

- 漫反射环境是**低频**的（余弦卷积本身就是个低通），L2 球谐（9 个系数）足够；
  预过滤立方体的"漫反射 mip"方案要多建一张纹理，这里不需要。
- **CPU 数值积分**（`cpu::sh_project`，1024 个**斐波那契球**采样点、等权、
  零随机数）：确定性 = 同参数逐位同结果，热重载 / 双跑截图才可能逐字节一致。
  代价摊在重建时刻（实测见第八节），不进每帧。
- **余弦卷积折进系数**：`sh_project` 乘 `[1, 2/3, 1/4]`（L0/L1/L2 的卷积因子），
  存的是 **E/π** —— 着色器端只做"求值"。这让它与常量 `ambient` **同一标定**：
  均匀天空下 `albedo × sh_eval` 逐值等于 `albedo × ambient`，两套环境光因此能
  直接相加（`sh_projection_of_a_constant_sky_reproduces_the_constant` 用常量
  天空断言这一点）；加进场景的 IBL 项走 `kd_env`（`(1-F_rough)(1-metallic)`）
  而不是旧环境项的 `(1-metallic)`，金属不拿漫反射这条物理约束顺带落实。
- 9 个 coeff × vec4 = 3 个 std140 vec4 槽位（`sh: array<vec4<f32>, 9>`）。
- `sh_eval` 的基函数在 WGSL 与 Rust 两侧**逐项同表**（`[Y00, Y1-1(y), Y10(z),
  Y11(x), …]`，系数 0.282095 / 0.488603 / 1.092548 / 0.315392 / 0.546274）。

### 三、镜面 IBL：GGX 预过滤立方体 + BRDF LUT（split-sum 两半）

- **预过滤 128² 基 + 5 级 mip（8² 封顶，共 30 趟小 pass）**：每个 mip 级按
  roughness 做 GGX 重要性采样（Hammersley 序列，**无随机数** ⇒ 逐位确定、
  不会逐帧闪烁），`V = N = R` 是 split-sum 的标准近似（把"看向光源的积分"
  从 BRDF 里解耦出来）。采样数 96 / 48 / 24 / 16 / 12（分辨率低采样跟着降；
  基级给足是为了粗糙度 0 时太阳盘在反射里清晰而不抖）。
- **粗糙度连续靠 mip 线性插值**：场景 FS 采样
  `textureSampleLevel(env_cube, env_smp, R, roughness × ENV_MAX_LOD)`，采样器
  `mipmap_filter: Linear`（这一项是硬的）。5 级是"128² → 8²"的折中：更少的级
  会让粗糙材质在级间跳变。
- **BRDF LUT 128² / Rgba16Float / 256 样本**（Karis 积分，`A / B` 二分表），
  坐标语义 `(N·V, roughness)`；**惰性**：首次真正开启天空时才生成（关着天空的
  项目永远不付这笔钱），生成后常驻。
- **面基向量由 CPU 给**（`cpu::cube_face_basis`）：`dir = forward + right·(2u-1)
  + up·(1-2v)`（v = 0 是面的顶行），面序 = WebGPU 立方体贴图约定
  （+X, -X, +Y, -Y, +Z, -Z）。着色器不现算 `cross` —— ±Y 面那两处"换 up 轴"的
  特判就不存在了，镜像测试与 GPU 走**同一张表**。这条错位的表现是"反射里北方
  的太阳跑到南方"，GPU 测试用"粗糙金属上的太阳反光"专门钉住（先证红见第七节）。
- **预过滤 / LUT 各写各的绑定**：预过滤管线自己的布局（0 = 天空 uniform、
  1 = 动态偏移的 `PrefilterParams`，30 槽一次写全）；LUT 无绑定（采样数是编译期
  常量）。两张表共用同一个 `env_sampler` 与 `env_view` 对象身份 —— 内容重建时
  **不换绑定组对象**（见第四节）。

### 四、生命周期：固定分配 + 按 key 重建（为什么不做每帧重建）

- 环境立方体与 LUT **一次性分配**（合计 < 1 MB），**内容**按
  `uniform_key`（天空 uniform 的 80 字节 FNV-1a 位模式哈希，太阳方向 / 颜色 /
  太阳尺寸 / 三个颜色 / 开关全在内）重建：key 不变 ⇒ 每帧只有一次 80 字节
  `write_buffer`；key 变了（编辑器调参、灯光转向、昼夜循环）才重跑
  brdf（仅首次）+ 球谐投影 + 30 趟预过滤。
- **位模式哈希而不是浮点比较**：`-0.0` / `0.0` 会判成两个 key，代价只是一次
  多余的重建，比"改了参数却没重建"安全得多（`uniform_key_tracks_every_field_…`
  对每个字段各断言一次"改了必换 key"）。
- **绑定组对象身份必须稳定**：全局绑定组是每帧路径（每个 draw 一次
  `set_bind_group`），内容重建若换对象就得每帧重建绑定组。固定分配时视图与
  纹理句柄不变，内容换代对绑定组完全透明 —— 这也是"不做每帧重建"的真正原因：
  重建本身不贵，贵的是它会逼着每帧路径换对象。
- **关着天空时什么都不重建**（只写 uniform）：`built_key` 也**不清空** ——
  关了再开、参数没变就免一次重建。"默认关不改变旧画面"与"开关不浪费"两条
  一起成立。

### 五、集成：绑定号契约、三处各自声明、画在哪里

- **`SKY_COMMON_WGSL`（`sky_radiance` / `sky_sh_eval` + 两个结构体）刻意不声明
  任何绑定**：天空盒 FS（全局组 0/7）、场景 FS（0/8–11）、预过滤 FS（自身布局
  0/1）三处绑定号各不相同，公共片段只带类型与函数（uniform 按值传参）。这样
  "公式只有一份"不靠人肉同步 —— 三处各自 `format!` 拼装，
  `SKY_FRAG_PRELUDE` / `ENV_PRELUDE` 声明各自的绑定与输入结构。
  全局组的 7–11 号契约集中在 `sky.rs::global_layout_entries()`。
- **`CameraUniform` 新增 `inv_view_proj`**（32 字节）：天空盒片元由 NDC 反推
  世界方向（`inv_view_proj · (ndc, 1, 1)` 减去相机位置再归一化 —— 远平面上的
  点与视线同向，透视除法不进这个式子）。场景 FS 不读它，代价是一行声明。
- **天空盒画在场景 pass 内、不透明之后、半透明之前**：全屏三角形（顶点
  `(0,0)(2,0)(0,2)` → NDC 覆盖全屏），片元位置 z = 1.0 + `LessEqual` +
  **不写深度** ⇒ 被几何盖住的像素在这里失败、只有空像素作画；半透明随后混在
  天空之上（半透明物体背后是天，不是清屏色）。
- **`prepare` 必须排在 `begin_render_pass` 之前**（同一 encoder 里 pass 不能
  嵌套）：预过滤 / LUT 各自开自己的 pass；`write_buffer` 与那些 pass 同属一次
  提交，wgpu 按队列顺序保证 pass 读到新 uniform。
- **`ScenePipeline::render` 新增 `sky: SkyConfig` 参数**：编辑器视口、独立游戏、
  无头截图、GPU 基准四个调用点全部接线；`scene_signature` 哈希 sky 全字段
  （漏了 = "调天空视口纹丝不动"的指纹门控 bug）。
- **场景 FS 的 IBL 项与 ambient 相加**，且整段由 `sky.sun_dir.w > 0.5` 开关位
  控制 —— 开关位是 uniform 里的常量，整个 draw 走同一分支，`textureSampleLevel`
  在一致性控制流里才合法（也才便宜）。

### 六、入口三件套（与曝光 / 泛光同一条"单入口"纪律）

- `project.ron` 的 `render.sky`：七字段 + `serde(default, skip_serializing_if)`，
  老清单照常加载（默认关）；`sanitized()` 容错 —— NaN / 无穷整组退回默认，
  有限但离谱的量级夹进安全范围（颜色 [0, 50]、强度 [0, 10]、太阳尺寸
  [0.1°, 20°]）。
- 编辑器 ⚙ Project Settings 新增「天空（程序化天空盒 + 环境光照 IBL）」区：
  启用复选框 + 强度 / 三色拾取 / 太阳盘大小 / 太阳强度，全部带范围与悬停说明
  （范围取自 `SkyConfig::MIN_* / MAX_*` 常量）。
- MCP `project_info` 与无头报告 `render.sky` 只读投影（报告契约测试
  `json_contract_has_stable_keys` 同步断言字段与三通道数组；数值同 Bloom ——
  f32 存储、JSON 里是精确加宽的 f64）。

### 七、测试与先证红

- **先证红（两条实测）**：① 只把 GPU 侧渐变指数 `0.5` 改成 `0.4`
  （CPU 镜像不动）→ `sky_background_matches_cpu_mirror` 红（(160,12) 处
  GPU 174 vs CPU 181，超 ±2 容差）—— 证明"对 CPU 镜像逐值比较"这条测试真的
  敏感；② 把 `cpu::cube_face_basis` 的 -Z 面 up 轴翻成 -Y →
  `metal_reflects_the_sky_sun_at_the_mirrored_screen_position` 红（反射里的
  太阳跑到了反侧）—— 证明面约定测试盯的就是这根轴。两条均已回退。
- **CPU 镜像 8 条**（`sky.rs::tests`）：渐变在地平线连续、太阳盘局部性 +
  无方向光不出现、常量天空的 SH 投影复现常量、SH 漫反射跟随渐变方向、
  面基向量与 WebGPU 约定逐项对齐、key 对每个字段敏感 + 投影确定性、
  太阳 = 第一盏 active 方向光（跳过点光与熄灭灯）、开关位进 uniform。
- **GPU 像素 5 条**（`screenshot.rs`）：背景 5 点对 CPU 镜像（±2）；关天空时
  背景 = 旧清屏色 (0.12/0.12/0.14)（"默认关逐位一致"的钉子）；无光绝缘体
  开天空被球谐照亮（关时 luma < 2、开时 > 20 —— 只建管线不写数据时两边都黑，
  方向性断言必红）；光滑金属平面（metallic 1 / roughness 0.05、全黑天空色）上
  的太阳盘反光出现在**几何镜像点**（饱和像素质心 vs 期望屏幕位置 < 15 px，
  太阳盘 8°；那条 1e-30 强度的方向光只为提供太阳方向 / 颜色）；开天空两跑
  PNG 逐字节一致（无随机数重构）。
- **core 2 条**：`sky_defaults_to_off`（默认关 + 默认不落盘）、
  `sky_config_sanitized_rejects_garbage`（NaN 退回 / 越界夹取 / 合法值原样保留）。
- **编辑器 / 报告契约**：`scene_signature` 纳入 sky 全字段（`assert_ne` 组）；
  无头报告与 MCP `project_info` 的 `render.sky` 进稳定契约。

### 八、性能实测（GPU 时间戳口径）

`postfx_cost.rs` 新增第二条入口 **`sky_ibl_cost`**（同款口径：1280×720、MSAA ×4、
地面 + 64 立方体 + 阴影，三档交替换轮 ×8 轮 ×64 帧取中位数）：

| 段 | 关（基线） | 稳态开（key 不变） | 每帧重建（key 每帧变） |
|---|---|---|---|
| 场景 pass（含 MSAA resolve） | 136.7 µs | 166.2 µs | 507.9 µs |
| 后处理 pass | 24.6 µs | 24.7 µs | 24.8 µs |
| **合计（GPU）** | **161.3 µs** | **190.9 µs** | **532.7 µs** |
| CPU 编码 + 提交 | 168.1 µs | 168.8 µs | 617.8 µs |

- **稳态开相对关 +29.5 µs/帧**（场景段 +21.6%；合计 +18.3%）= 天空盒全屏 pass
  + 场景 FS 的 IBL 采样（环境立方体 + LUT 各一次）。折算 60 FPS 预算的 0.18%，
  "背景与照明同源"买到的是这个价。后处理段三档一致（24.6–24.8）—— 链没被
  天空触碰。
- **每帧重建相对稳态 +341.7 µs GPU / +449 µs CPU**（30 趟预过滤 + 球谐投影
  1024 点 + 30 槽 uniform）。这是"太阳每帧动"的最坏情况：昼夜循环每帧改参数
  就要付这笔账，而上限理由是**每帧路径**（重建本身不贵，CPU 编码 30 趟 pass
  才是大头）。日常形态（参数恒定）只付 uniform 的 80 字节。
- **开启天空那一帧**（BRDF LUT + 首次预过滤 + 球谐）：579.6 µs 场景段 =
  重建档 + 71.6 µs（LUT 256 样本 × 128² + 冷态）。一次性成本，编进 LUT 的
  惰性生成 —— 不在首帧支付就要在别处支付。
- 基准自带防"空调度"断言（P2.5 教训）：稳态场景段 > 关 + 1 µs（天空盒 / IBL
  真在跑）、重建场景段 > 稳态 + 10 µs（预过滤真在重跑）、重建 CPU > 稳态 +
  5 µs（球谐与 30 趟编码真在跑）、时间戳读数非零。key 一旦因回归不再变化，
  后两条必红。

### 九、实拍对照（SampleScene，无头截图）

同一场景开 / 关天空各拍一张（`--headless --frames 150 --screenshot`，机位自动
拟合 —— 两份报告的 `eye` / `target` 逐位相同：eye (-42.5946, 21.7024, -2.1760)、
target (0, 1.1, 0)，差异里没有第二个变量）：

- 1280×720 隔点取 230400 样本，**全部变亮、0 个变暗**：平均 +68.8/255 每通道，
  最大 +119；整幅均值 (110.7, 110.5, 119.5) → (177.0, 181.4, 188.7)。
- 观感：背景从深灰清屏色变成地平线附近的淡蓝 → 天顶方向渐深；金属件的面上
  出现天色反射；所有受光与背光面都抬了一档环境光（ambient + IBL 相加的直接
  读数）。背光面不再发死黑 —— 这正是"环境反射来源"这一片要解决的事。
- 默认机位俯角 26°，画面上沿只略高于地平线，因此天空在默认截图里主要展示
  地平线色（见第一节末）。

### 十、刻意的取舍（防好心人翻案）

1. **默认关**：见 `SkyConfig` 文档 —— 天空是风格参数（写实 / 卡通 / 夜景），
   与 Bloom 同理，"默认改变画面不可接受"。要开就改一行 `project.ron`。
2. **太阳 = 场景第一盏方向光**，不引入独立的"天空光源"：再开一个太阳参数
   必然与灯光漂移（改灯忘了改天空 = 阴影方向与太阳盘不一致）。方向光本来就
   是太阳语义。
3. **IBL 与常量 ambient 相加**，不把 ambient 顶掉：老场景调好的环境底色不能被
   悄悄改写；想只留 IBL 就把 ambient 清零（数学上两者同标定，见第二节）。
4. **固定分配 + 按 key 重建**：`built_key` 不清空、绑定组对象身份不变（第四节）。
   别改成"每帧重建"或"重建时换绑定组"。
5. **不做 compute shader 预过滤**：30 趟 render pass + 全屏三角形在 128² 尺度上
   已经够便宜（实测 +341.7 µs 是在**每帧**重建的最坏情况下；日常只在调参时付），
   引 compute 要动管线布局与队列语义，收益不成比例。等有更大环境贴图 / 运行时
   重烘焙的需求再评估。
6. **不做 Preetham/Hosek、不做脚本 `set_sky`**：天空是画面设置不是玩法参数；
   昼夜循环等真出现"逐帧改天空"的需求时再开 API（那时第一节的出口结构原样
   可用，代价已在第八节量过）。

### 十一、已知限制（写进差距分析）

- **单一全球环境**：没有反射探针 / 局部环境（室内空间看不到室内色，金属反射
  的是室外天空）。
- **天空不投阴影**：太阳盘只是背景辐射；直接光与阴影仍来自场景里的方向光。
- **脚本侧没有 `set_sky` / `set_material`**：入口 = Inspector / 场景 RON /
  `project.ron` / MCP `scene_write`。
- 无折射 / 透射（材质系统本身没有 transmission），IBL 的镜面项是单次反射。

## 二十八、渲染实例化（draw batching）（本轮）

差距分析 P1 表里的 **实例化** 兑现：渲染收集阶段把"同图元"的绘制项并成一批，
每批一次 `draw_indexed(0..n_indices, 0, first..first+count)` —— 逐实例数据
（模型矩阵 + 蒙皮槽位 + 有效材质）走 **storage buffer**，顶点着色器用
`@builtin(instance_index)` 直接索引，**动态偏移 uniform 整个退役**。
流程按"先量后改"走：先把帧绘制统计接口 + 基准作为准备提交（`81ff6e9`）落盘，
旧实现（2000 实体同网格场景 **2001 批 / CPU 2225 µs/帧**）的数字进案；落成后
同机交替复测 **2 批 / 344 µs/帧（-84%）**；画面端到端**逐字节不变**。
全量验证绿：`cargo check` / clippy **0 警告**、`cargo test --workspace
--all-targets` **439 条全绿**（5 条基准默认 ignore；较上轮 +5 = 实例化 GPU 测试组）。

### 一、批次 = 一次实例化 draw：分组键与顺序都是"确定性"的一部分

```text
收集循环（逐实体 → 逐图元）：
  不透明 → batch_index[图元地址] 查/建批次；实例数据攒进 pending_draws
  半透明 → blended_draws（各自一批，带距离键）
           ↓
  blended_draws 按距离远→近排序
  槽位前缀和：批次 i 占 [first, first+count)，半透明接在后面
  一次 queue.write_buffer 写完整段实例数据
           ↓
主 pass：全局组绑一次 → 逐批 [材质组 + 顶点/索引缓冲] → draw_indexed(..., first..first+count)
阴影 pass：同一份批次表逐批重放（不重复收集、不重复计 draw call）
```

- **分组键 = 图元地址**（`prim as *const GltfPrimitive as usize`，只做相等
  判定）：同一份 `GltfMesh` 的图元指针相同 —— 共享网格的实体自然并批，
  各自一份网格的实体自然不合批。指针相等判定是 O(1) 且零分配；换成
  "网格路径字符串哈希"每帧 2000 次是纯浪费，换成"材质进键"则会因为逐实体
  材质覆盖（常态）把合批拆碎 —— 材质本来就要逐实例走（见第三节）。
- **批次顺序 = 首次遇到顺序**（`Vec` 追加，不是 `HashMap` 迭代序）：槽位布局
  因此逐帧确定、跨进程确定 —— 无头截图"同机逐字节确定"契约与基准读数的
  可复现性都依赖这条。别改成哈希序 / 按材质排序。
- **半透明不参与合批**（排序语义优先，见第四节）；**每图元各自成批**
  （不跨图元合并 —— 每个图元通常各有材质，跨并反而要拆材质）。
- 批次表是跨帧复用的字段：`opaque_batches` / `pending_draws` / `batch_index`
  全部 `clear()` 复用容量（稳态零分配，与收集缓冲同一纪律）。

### 二、逐实例数据：storage buffer 取代"逐 draw 动态偏移 uniform"

- 旧方案：每绘制项一段对齐到 256 B 的 uniform，逐项 `queue.write_buffer` +
  逐项 `set_bind_group(0, &bg, &[offset])`。2000 实体 = 2000 次小写 + 2000 次
  带动态偏移的绑定 —— 这就是"每帧 CPU 税"的大头（实测在案）。
- 新方案：`DrawUniform`（**112 B** = 模型矩阵 64 + 蒙皮槽位 16 + 材质
  base_color/material 32）在 `draw_buffer`（`STORAGE | COPY_DST`）里**紧密
  排布**，一次 `write_buffer` 写完整段；全局绑定组与阴影绑定组对它的
  绑定都是**整缓冲 + `has_dynamic_offset: false`**。
- **112 B 必须等于 WGSL 数组步长**：storage 数组元素步长 = align16 后的
  结构大小，112 = 7×16 恰好无填充 —— 改组内字段 = 改 ABI，CPU / WGSL 两侧
  一起改（`size_of` 已有单测钉死）。
- 槽位容量上限仍然是 `MAX_RENDER_DRAWS`（超限静默丢弃的旧语义原样保留）。

### 三、着色器：`instance_index` 索引 + flat 插值传材质

- 顶点着色器 `let draw = draws[instance_index];` —— WebGPU 的
  `@builtin(instance_index)` 从 `first_instance` 起算，与槽位前缀和分配的
  "批起始槽位"正好对齐；`draw_indexed` 的实例范围与数据段一一对应。
- **材质随实例数据到片元，靠 `@interpolate(flat)`**（`@location(4/5)`）：
  片元阶段拿不到 `instance_index`，直接读 storage 会在片元阶段引入 storage
  绑定类别（且每个片元重复索引）；flat 插值取 provoking vertex 的值，同一
  实例所有顶点同值，"取谁都一样"。片元阶段保持"只吃顶点输出"的旧接口。
- 半透明与不透明走**同一套**顶点路径（同一 storage 布局），差别只在管线
  状态（混合 + 不写深度）与 draw 粒度。

### 四、半透明与阴影

- **半透明逐项一次 draw call、按到相机的距离远→近**：alpha 混合顺序敏感，
  排序是语义的一部分（两条同轴半透明立方体的钉子测试在第六节）。项数与
  批次数无关（不合批）。
- **阴影 pass 复用同一批批次表**（同一份实例数据、逐批同样一次 draw）：
  零额外收集、零额外准备；统计口径（`FrameStats`）只记主 pass 的 draw call，
  阴影不重复计。只画不透明的旧语义不变（半透明不投实心影）。
- 收集阶段对每个新批次 / 半透明项**预热材质绑定组**（`material_binding`
  要 `&mut self`，绘制循环里只有 `&self`）—— 绘制循环用
  `material_cache.get(..).expect("材质绑定组缺失")`：这是故意的硬失败，
  静默拿着上一批的材质画会得到"看起来能跑"的错色画面。
- 顺带修一处遗留接线：`sky.rs` 的 `set_bind_group(0, global, &[0])` 改
  `&[]` —— 全局组已无任何动态偏移，多传偏移 wgpu 会直接校验报错。

### 五、统计接口与"先量后改"的流程

- `FrameStats { opaque_batches, opaque_instances, blended_draws }` +
  `draw_calls()`（= 批次数 + 半透明项数）先于实现落盘（`81ff6e9`）：
  合批是否生效、画面是否正确，两者互不覆盖 —— 像素测试守"画得对"，
  计数守"是不是一批画完的"。`ScreenshotRenderer::frame_stats()` 是测试探针
  （与 `ScenePipeline::frame_stats` 直连，无缓存）。
- 微基准 `tests/instancing_cost.rs`（手动、`#[ignore]`）：两个**几何完全
  相同**的场景 —— 共享网格（`builtin://cube` ×2000）与各自网格（同几何、
  各自一份 `GltfMesh`）。后者不是假想最坏情况，而是**回归护栏**：合批逻辑
  一旦把不同顶点缓冲错并成一批，这里与画面测试会同时红。读数口径与
  `postfx_cost` 一致：GPU 时间戳取场景段 + CPU 段单记"编码 + 提交"，
  两档交替 ×8 轮取中位数。

### 六、测试与先证红

- **旧实现下红（3 条，实测）**：把 5 条新测试原样移植进旧实现 worktree
  （`81ff6e9`，仅补一个 `frame_stats()` 访问器）→ ①
  `instancing_batches_shared_mesh_entities` 红（65 批 vs 期望 2）；
  ② `instancing_matches_per_draw_rendering` 红（共享 65 批不小于各自的 65）；
  ③ `batching_keeps_per_instance_material_overrides` 红（2 批 vs 期望 1）。
- **突变证红（2 条契约钉，新实现里人为破坏再回退）**：④ 把 `shadow_enabled`
  写死 false → `instanced_draws_still_cast_shadows` 红（变暗像素 0 = 影子
  没了）；⑤ 把半透明排序翻成近→远 → `blended_draws_render_far_to_near` 红
  （中心像素 r > b）。两条均已回退、复跑绿。
- **GPU 测试组 5 条**（`screenshot.rs`）：批计数（64 立方体 + 地面 = 2 批 /
  65 实例 / `draw_calls() == 2`）；**逐 draw 等价**（共用网格 vs 4 份注入的
  同几何网格，两图**逐字节一致**，且共享档批次更少 —— 开着阴影跑，阴影
  pass 的批复用一起覆盖）；**同批各调各的色**（红/绿实体级覆盖物在同批,
  两个采样点颜色各异 —— 防"材质被提到批次级"）；**合批投阴影**（开/关
  阴影两图比对：变暗像素 ≥100、无像素变亮；统计口径与主 pass 一致）；
  **半透明远→近**（近蓝远红，中心 b > r）。
- **基准自带反退化断言**（P2.5 纪律，印完摘要再断言）：共享档
  `batches × 100 ≤ instances`（退化成逐 draw 必红）、各自档
  `batches ≥ ACTORS`（错并批必红）、两档实例数相等、`s_cpu × 2 < u_cpu`
  （合批没省下 CPU 必红）。

### 七、性能实测（同机交替 A/B，3 轮取中位数）

方法：`81ff6e9` 在独立 worktree 构建旧二进制，与工作区新二进制**交替**
（旧/新/旧/新/旧/新）跑同一基准，抵消机器状态漂移。口径：RTX 2060 /
Vulkan / 1280×720 / MSAA ×4 / 2000 实体 ×（共享网格、各自网格）两场景 ×
8 轮 ×64 帧取中位数；CPU 段 = "编码 + 提交"（实例化真正要砍的税）。

| 指标 | 旧（2001 批） | 新（2 批） | 变化 |
|---|---|---|---|
| 共享网格：批次数 | 2001 | **2** | −99.9% |
| 共享网格：CPU 编码 + 提交 | 2225 µs/帧 | **344 µs/帧** | **−84%** |
| 共享网格：场景段（GPU） | 287 µs/帧 | 241 µs/帧 | −16% |
| 各自网格护栏：批次数 | 2001 | 2001 | 持平（护栏语义） |
| 各自网格护栏：CPU | 4139 µs/帧 | 3287 µs/帧 | −21% |
| 各自网格护栏：场景段 | 292 µs/帧 | 267 µs/帧 | −9% |

- 三个数字逐轮一致（旧 CPU 三次 2225.0 / 2206.5 / 2234.1，新 CPU 340.8–344.3，
  交替六轮无交叉）—— 不是机器状态的巧合。两档的轮内读数离散度也小（旧
  2202–2292、新 334–368 单轮制）。
- **各自网格对照档也快了 21%**：那不是合批的功劳（它一批都没合成），而是
  砍掉了每 draw 的 2000 次动态偏移 `set_bind_group(0)` 与 2000 次
  `write_buffer`（逐项 uniform 上传）。新方案每帧只有一次整段实例数据上传。
- 后处理段时间戳两版间有稳定的 ~3.5 µs 差（旧 28 µs 档 / 新 24.7 µs 档，
  后处理代码未动、机制不明），**不计入任何收益口径** —— 上面只引用场景段
  与 CPU 段。
- 半透明项两档均为 0（基准场景无半透明）；阴影开关不进 CPU 段（同一份批次
  表重放，不额外编码命令）。

### 八、端到端（SampleScene 无头截图逐字节一致）

两版编辑器的 `--headless --frames 150 --screenshot` 同一场景（SampleScene，
含共享网格实体 8×cube + 3×Box、Lua 脚本、方向光阴影、天空 IBL、ACES）：

- **两张 PNG 逐字节一致**（SHA256 同为 `314E310E…`）—— 实例化没有改动一个
  像素；连"模拟 150 帧后的动画姿态与脚本位移"都逐位相同。
- 两份 JSON 报告逐行 diff：**仅 18 行不同，全部是耗时字段**
  （`max/mean/p50/p95/p99/run_ms/setup_ms`）与环境路径（worktree 根、截图
  文件名）—— 实体计数 / 刚体计数 / 脚本日志（含 error 样本）/ 非有限变换
  全部逐字符一致。

### 九、刻意的取舍（防好心人翻案）

1. **分组键 = 图元地址、批次顺序 = 首次遇到**：见第一节 —— 槽位布局的
   确定性是截图契约与基准可复现性的地基；别改成哈希序 / 按材质排序 / 排序
   批次表。
2. **逐实例数据只走 storage buffer**：别再引入"逐 draw 动态偏移 uniform"
   的写法 —— 本轮实测它是 2000 实体场景 ~1.9 ms/帧 的纯 CPU 税；且
   storage 数组让"实例槽位"与"数据段"一一对应，`first_instance` 直接就是
   数据下标，没有第二套偏移书签。
3. **半透明不参与合批**：排序是 alpha 混合语义的一部分，逐项一次 draw 是
   正确性优先的选择；将来半透明规模大到 CPU 吃紧再谈（当前无此需求）。
4. **阴影 pass 复用批次表、不为阴影单独收集**：换了收集路径就有了"主 pass
   与阴影 pass 实例数据错位"的可能；复用 = 不可能错位。
5. **材质 flat 插值而不是片元读 storage**：片元阶段保持"只吃顶点输出"，
   不引入 storage 绑定类别；同实例所有顶点取同值，flat 是语义最贴的插值。
6. **统计接口直读字段、不做每帧记账**：`frame_stats()` 只是拷贝 3 个
   `usize`，不进热路径；它是基准与测试的公共读数口（别让消费方从耗时反推）。
7. **每图元各自成批（不跨图元合并）**：图元通常各有材质，"跨图元合并"会
   立刻需要按材质二次拆分，得不偿失。

### 十、已知限制（写进差距分析）

- **LOD / 视锥剔除仍缺失**（差距分析"实例化 / LOD / 剔除"行改为部分完成）。
- 合批粒度是"图元指针"，**跨网格资产不合并**（同一几何两份 glTF = 两批）——
  与 Unity 的资产级实例化对照时注意口径。
- **半透明逐项 draw call**：数量大时是新的 CPU 税源（本轮场景未触及）。
- 无 GPU-driven / indirect draw、无材质排序（同材质跨网格不合批）——
  当前瓶颈数字（见 ROADMAP"每帧开销"行）不需要它们。

## 二十九、编辑器 UI：UE 风格重做（本轮）

### 一、要解决的是什么

用户的原话是「调整画面风格，有些元素大小不合适，有些地方排版不好看，有些地方
不整齐」。三条抱怨里只有第一条是审美，后两条是**几何问题**：行高在 20–26px 之间
飘、标签列宽随文字长短变化、值列左右各错开一档、emoji 字形宽度不一致导致图标
永远对不齐。所以这一轮的主线不是"换个配色"，而是**把尺寸与对齐收进一套常量**。

判据（可截图验证，不靠感觉）：
1. 同一面板里所有输入框的左边缘落在同一条竖线上；
2. 所有行高、控件高度、面板内边距只来自 `theme` 常量；
3. 界面上不再出现 emoji / 字形图标（矢量绘制，尺寸与基线可控）；
4. 改完不引入任何帧调度与渲染路径上的变化（纯 UI 层）。

### 二、尺寸体系（`panels/theme.rs`）

| 常量 | 值 | 用在哪 |
|---|---|---|
| `TOOLBAR_HEIGHT` | 34 | 顶部工具栏 |
| `TAB_BAR_HEIGHT` | 26 | 面板页签条 |
| `CATEGORY_HEIGHT` | 22 | Inspector 分类条 |
| `SECTION_GAP` | 6 | 分类条与上一组内容之间 |
| `ROW_HEIGHT` | 22 | Hierarchy / Prefabs / Console 行 |
| `FIELD_HEIGHT` | 20 | DragValue / TextEdit / ComboBox |
| `STATUS_BAR_HEIGHT` | 24 | 底部状态栏 |
| `PANEL_MARGIN` | 6 | 每个停靠面板的内边距 |
| `LABEL_WIDTH` | 58 | Inspector 标签列 |
| `AXIS_TAG_WIDTH` | 9 | X/Y/Z 轴标占位 |
| `FIELD_WIDTH` | 54 | 数值框标准宽度 |

行高统一之后"紧凑但不挤"是自然结果 —— UE 的信息密度来自**间距的规律性**，
不是把字号压小。

### 三、可复用件

- `theme::section_bar`：22px 分类条 + 左 2px 强调条 + 下沿 1px 线 + 6px 上留白。
  分组靠留白切开，不靠更粗的线。
- `theme::field_row(ui, label, f)`：定宽标签列 + 值区，**返回闭包的返回值**
  （向量行要拿 `changed`）。标签 `truncate()`，长参数名不会顶出控件列。
- `theme::note(ui, text)`：分类内容里的说明文字，**自己排一次版**再交给 Label。
- `theme::drag_field` / `axis_label` / `axis_color`：数值框与轴色。
- `theme::card_frame` / `card_title` / `bold` / `bold_family`：枢纽卡片与粗体族。
- `icons::*`：`paint(painter, rect, Icon, color)` 全几何绘制（18 个图标）、
  `icon_button` / `icon_text_button` / `text_button` / `add_row` / `chip` /
  `close_button` / `small_icon` / `v_separator`。

### 四、对齐的四个陷阱（这轮踩到的，全是"看不出原因"的那种）

1. **`allocate_ui_with_layout` 只按子 ui 的 `min_rect` 占位**，不按你给的
   `desired_size`。于是"58px 标签列"实际只有标签文字那么宽：`Type` 行与
   `Radius` 行的输入框差出 12px，一列控件左边缘参差。修法是在子 ui 里
   `ui.set_min_width(w)` 把占位拉回目标宽度。`field_row` 与枢纽的
   "名字列 / 主列"都吃这一条。
2. **`ComboBox::width(w)` 是内宽，控件实宽 ≥ w**（自带箭头与内边距）。所以
   拿"请求宽度"去排一张列头，偏移会逐列累积：`Weight` 表头最终偏出 10px。
   结论：**不排这张表头**，改成每个控件一条悬停说明。
3. **`FontFamily::Name` 指向未注册的族 = epaint 直接 panic**。`install_fonts`
   里粗体族改成**无条件注册**（找不到粗体文件就用 proportional 链兜底）；
   裸 `egui::Context`（测试）里画的东西只能用 `Proportional`。
4. **`allocate_ui_with_layout(vec2(w, 0.0), ...)`** 会把子 ui 的 `max_rect`
   高度设成 0，内部的 `ScrollArea` 按"可用高度 0"夹视口 —— 表现是列表只画出
   前三行。要给真实可用高度。

### 五、逐面板

- **工具栏**：34px 扁平分组（项目 / 保存 / 播放 / 插件 / 变换工具 / 显示 / 右
  侧 Build + 设置 + 场景名），分隔线用 1px 竖线，激活工具是蓝底。
- **页签**：自绘 `tab_ui`（选中 = 面板底色 + 顶部 2px 强调线 + 粗体），
  页签名与枚举名解耦（`SceneView → Viewport`、`Console → Output Log`）。
- **Hierarchy**：22px 行、缩进 12、选中行直角填充、矢量箭头、行尾 P/S 徽标
  （**选中行改半透明黑底 + 提亮字色**，否则徽标糊进蓝底）。
- **Inspector**：整条滚动；分类内容**零缩进**（`spacing.indent = 0`，代码里
  的 `ui.indent` 只做分块）；标量行补 `AXIS_TAG_WIDTH` 占位，与 Transform 的
  三个轴框共用同一条值列；Physics / Animation / Audio 全部 `field_row` +
  ComboBox；层栈去掉列头改悬停说明。
- **Prefabs**：标题 + 计数 + 刷新图标；搜索框整宽；`Create from Selection` 是
  扁平行；资产行 22px + 紫色菱形 + 右侧子目录暗字。
- **Output Log**：矢量等级图标（Info 圆点 / Warning 三角 / Error 叉 / Success
  对勾）+ 截断 + 右对齐计数。
- **状态栏**：24px 深条 + 分组 + 右侧 fps/版本；**编辑态用小圆点而不是暂停
  图标**（暂停符号在"没在跑"的语境里会被读成"运行中已暂停"）。
- **视口 HUD**：药丸底（`from_black_alpha(96)`）+ 左上场景名 / 右上相机 /
  左下工具 + fps。
- **枢纽**：三张卡片（新建 / 打开 / 最近），固定标签列、蓝底主按钮、最近项目
  名字列定宽 112px。

### 六、自检工具链（这轮新加的）

```
xgame-editor.exe <project> --ui-shot <png> [--ui-frames N] [--ui-select <实体名>]
                            [--ui-settings] [--ui-play] [--ui-size 1280x800]
```
- 首帧 `ViewportCommand::InnerSize(<--ui-size 或 1440x900>)` 固定窗口尺寸；
- **连续两帧逻辑尺寸相同**才发 `ViewportCommand::Screenshot`；
- 像素尺寸 ≠ 逻辑尺寸 × ppp 时最多重抓 3 次（pidpi 缩放下的经典陷阱）；
- 落盘走 `xgame_render::screenshot::save_rgba_png`（与场景截图同一份实现）；
- `--ui-settings` 在抓图前把模态的项目设置窗口拉开 —— 没有它，那个窗口
  **永远截不到**（只能靠点 ⚙）。
- `--ui-play` 抓图前先进 Play。**必须自己把 `play_mode` 推上去**：平时这条
  跃迁由 `editor_ui` 监听到工具栏改了枚举之后补上，截图路径不经过工具栏，
  只调 `enter_play_mode()` 会得到"运行时在跑、界面写着 Edit Mode"的僵尸态。
- `--ui-size` 逐档扫窗口宽度做排版回归（1280 / 1440 / 1920 各跑一遍）。
  解析失败要**报错**不能静默退化，否则那一档截图其实还是 1440×900（有测试钉住）。

顺带修掉设置窗口的两个硬伤：滚动区没有高度上限（窗口按内容长到屏幕外，
标题栏和 Apply/Cancel 一起被裁掉）、文本字段只有 egui 默认宽度（右边缘空出
一大截）。现在滚动区上限 = 窗口高 − 190，字段宽度 = 网格剩余宽度。

配套：`.workbuddy/tools/crop.py`（局部放大）与 `rowscan.py`（按行扫描颜色分段，
用来**量**像素位置而不是靠眼睛判断"对齐了没有"）。

### 七、第二轮：窄窗口与运行态（同日续做）

第一轮只在 1440×900 / 扫描过 1280 一次，第二轮把三档都跑了一遍，修掉三类问题：

1. **Transform 三轴在 1280 下顶出面板**（Z 输入框只剩半个）。根因是
   `FIELD_WIDTH=54` 写死：标签列 58 + 3×(轴标 9 + 框 54) + 6 个间距 ≈ 276px，
   而 1280 时 Inspector 内容宽只有 ~244px。改为 `theme::axis_field_width(ui)`
   按剩余宽度均分（`clamp(MIN_AXIS_FIELD=30, FIELD_WIDTH)`），三行共用**同一个**
   算出的宽度（放完第一个控件再算会越算越窄，列就不齐了）。同时把
   Inspector 的 share 从 25% 提到 28% —— 1440 下正好稳住 54px（与旧布局逐像素
   一致），1280 下从 37px 提到 43px。留 4px 余量给滚动条与取整。
2. **Play 态的三个语义错误**（`--ui-play` 做出来才看见）：
   - Play 中标题/工具栏一直显示 `*`（未保存）—— 运行时的物理每帧都在改
     `self.scene`，但那不是用户的改动。`scene_dirty_memo` 在非 Edit 态直接
     返回 `false`。
   - Play 中 Save 按钮可用，按下去会把"跑了三秒之后的现场"写进场景文件。
     `save_scene` / `save_scene_as` 现在硬拒绝，工具栏按钮跟着禁用。
   - Pause / Stop 在编辑态也是亮的，点了什么都不发生。改用 `add_enabled_ui`
     按 `play_mode` 禁用。
3. **三个图标在 14px 下读不出来**：`Translate` 的箭头只有 2.1×1.6px（整个图标
   是个加号）、`Save` 画了三个框糊成"回"字、`Folder` 的页签 4×1.7px（看着是
   空心方块）。分别改成：箭头放大到臂长的 1/4、软盘只留"外框 + 上半实心"、
   页签占宽度一半且单独提亮 + 接缝压暗。

### 八、刻意的取舍

1. **不引第三方图标库**：18 个图标全用 `Painter` 画。字形图标的宽度/基线/粗细
   不受控，正是"不整齐"的来源；矢量图标永远落在给定方框里。
2. **不恢复列头**：见陷阱 2。表头对不齐比没有表头更显乱。
3. **Inspector 分类内容零缩进**：UE 的 Details 就是平的；缩进会让值列分层。
4. **粗体族无条件注册**：宁可"没有粗体效果"也不能"找不到族就 panic"。
5. **UI 层不动帧调度**：截图与量帧数据表明这一轮没有引入新的重绘源。
6. **Inspector 不设最小宽度**：egui_tiles 的 `Behavior::min_size` 是**全局一个
   值**（0.11 的签名就是 `fn min_size(&self) -> f32`），没法按面板给；所以窄窗口
   靠"三轴自适应 + 调 share"扛，而不是靠 dock 保证宽度。

### 九、已知限制

- 视口 gizmo 与选中高亮的配色未重做（属于 3D 叠加层，不在这轮范围）。
- 窗口小于 1280×800 的排版未验证（Dock 四列到这个尺寸已经很难看，UE 同尺寸
  也一样）。
- Console 的错误/警告等级图标只在高亮时验证过；空列表没有专门的空状态插画。
- 动画层栈在 1280 下四个控件（名字 / 模式 / 剪辑 / 权重）仍然偏挤，
  但都完整可见（列头已按陷阱 2 删除）。

## 三十、渲染剔除：视锥剔除 + 阴影补投 + 绘制读数（本轮）

差距分析 P1 表里的 **视锥剔除** 兑现 —— 实例化解决"同网格的绘制项合成几批"，
剔除解决"这些实体根本不用进批次"。落成后 2000 实体、30° 窄视场（**56.6% 在
视锥外**）的实测：

| 场景 | 批次 / 实例 | CPU（编码 + 提交） | 场景段 GPU |
|---|---|---|---|
| 共享网格（合批成 1 批） | 1 / 2000 → **1 / 868** | 187.6 → **181.7 µs/帧（-3%）** | 76.8 → **71.7 µs（-7%）** |
| 各自网格（2000 批） | 2000 / 2000 → **868 / 868** | 3295.3 → **1146.3 µs/帧（-65%）** | 158.0 → **108.5 µs（-31%）** |

同一基准紧接着复测的方向与比例一致（各自网格 CPU 2290.8 → 982.8 µs/帧，-57%；
共享网格 CPU -3%、场景段 -8%）；绝对值随机器状态浮动 1.4× 是常态，跨版本比较
必须同机背靠背。全量验证绿：`cargo check` / clippy **0 警告**、
`cargo test --workspace --all-targets` **456 条全绿**（5 条手动基准默认 ignore）。

### 一、判据：六个归一化平面 + 网格级世界 AABB

- `frustum.rs` 从 `view_proj` 按 **Gribb–Hartmann** 提六平面，法线朝视锥内侧、
  且**逐平面归一化** —— 点积因此就是世界单位的有符号距离，球体测试可以直接拿
  半径比，AABB 测试也不受平面尺度影响。
- **近平面取第 3 行**（wgpu 的 NDC 深度是 `[0, 1]`，`glam::perspective_rh` 的
  约定），不是 OpenGL 的 `row3 + row2`。搞反的表现是"相机贴脸时近处物体整片
  消失"，单测专门钉了相机原点附近的点。
- `Frustum::from_view_proj` 返回 `Option`：矩阵含 NaN / Inf、或任一平面法线长度
  为零时返回 `None`，调用方**当作不剔除**。相机矩阵坏掉时宁可白画一帧，也不能
  把整个场景判成不可见（那是黑屏）。
- 剔除粒度是**实体 × 网格级 AABB**（`mesh.bounds` 经世界矩阵变换），不是逐图元：
  同一网格内的图元共享一个物体，逐图元求交省下的实例写不值得再乘一次矩阵。
  每个实体只算一次包围盒，随后主 pass / 阴影 pass 共用。
- `Bounds::transformed_affine` 是 8 角点法的**解析等价式**（`|M| · 半长` 再乘
  2 加中心），每帧 2000 个实体时省掉 7 次点变换；`frustum.rs` 里有一条
  "与角点参考实现逐值一致"的交叉单测钉住（旋转 + 缩放 + 平移的矩阵）。
- AABB 测试只测每个平面**最靠内侧的角点**（p-vertex）：它在外侧则整盒在外侧，
  与"8 个角点全测"等价，省 7/8 的点积。

### 二、三种去向：主 pass / 仅阴影 / 整实体跳过

只按相机视锥剔会制造最容易漏的画面 bug —— **相机看不见、但影子落在画面里**
的物体跟着影子一起消失。收集阶段因此对每个实体算两个视锥：

```text
相机视锥内                    → 主 pass（阴影 pass 复用同一批）
相机外、阴影视锥内（不透明）  → shadow_batches（只进阴影 pass）
两边都外                      → 整实体跳过（不蒙皮 / 不建材质组 / 不写实例）
```

- 阴影视锥来自本帧的 `shadow_matrix`（与阴影贴图同一份正交拟合）；阴影关着时
  不保留任何"仅阴影"实体。阴影视锥矩阵退化时**保守地当作全都在内**
  （`is_none_or`），否则一次坏矩阵会让全场影子消失。
- **半透明不投实心影**的旧语义原样保留：仅阴影路径遇到 `material.blend` 直接
  跳过（它不写主深度，混进阴影图会投出一块错误的黑斑）。
- `shadow_batches` 是独立批次表，**不占主 pass 的实例槽位、不预热材质绑定组**
  （深度 pass 不采样材质）；按图元地址分组的规则与主 pass 相同。实例槽位布局
  是"主 pass 不透明 → 半透明 → 仅阴影"三段连续，仍只做一次
  `queue.write_buffer`；阴影 pass 依次遍历 `opaque_batches.chain(shadow_batches)`。
- 统计口径：`opaque_instances` **只算主 pass**；仅阴影实例单独记
  `shadow_only_instances`；`cull_ratio()` 的分母把两类都算进去（仅阴影的投影物
  不是"被剔除"，不能从分母里漏掉）。

### 三、蒙皮：绑定姿态包围盒 + 50% 余量

蒙皮发生在 GPU 顶点着色器里，CPU 侧的 `GltfMesh::bounds` 只是**绑定姿态**的
外壳；走路、挥拳、翻滚都会把肢体甩到盒外。给 `animator.is_some()` 的蒙皮网格
留 **1.5× 半长**（3× 体积）余量：

- 精确做法是逐帧用当前骨骼重算蒙皮包围盒 —— 要在 CPU 上再跑一遍蒙皮，收益
  抵不上成本（剔除省下的是实例写与光栅化，不是骨骼计算）。
- 余量只乘在**蒙皮网格**上：静态网格（含内置图元）走精确解析式，不平白多画
  3× 体积。`cull_bounds_pads_skinned_meshes_only` 与
  `cull_bounds_leaves_static_meshes_exact` 两条单测钉死这两半。
- 余量随包围盒尺寸缩放，所以"离原点很远的大角色"会被保得更宽 —— 这是保守
  方向的误差（多画），不是错判。

### 四、测试与先证红

**纯几何单测 10 条**（`frustum.rs`）：正前方 / 相机背后、近平面两侧、远平面、
跨近平面的盒子、侧面外沿与贴边盒子、球体半径、正交阴影视锥罩住整盒、退化矩阵
（全零 / NaN）不剔除、解析 AABB 与角点法一致。

**GPU 测试 3 条**（`screenshot.rs`）：

1. `frustum_culling_skips_offscreen_instances`：16 可见 + 16 相机背后的交错
   立方体 → 关剔除 32 实例 / 0 剔除，开剔除 16 / 16、`cull_ratio == 0.5`；
   两档批次数相同（剔除只少实例，不重排批次）。
2. `frustum_culling_matches_unculled_pixels`：开 / 关剔除两帧**逐字节一致**
   —— 保守性的硬合同。
3. `offscreen_casters_still_cast_shadows`：相机外的投影物把影子投在可见地面上
   （红灯时"有阴影 / 无阴影"两档都是 236.0；修复后 `lit + 20 < unlit`），
   `shadow_only_instances == 1`、`culled_entities == 0`。

**旧实现 / 破坏性突变下红（3 条，均实测后回退复绿）**：

- 把 `camera_frustum` 强制成 `None`（等价于旧的无剔除实现）→ ① 红：
  `assertion left == right failed: 只剩可见的 16 个实例, left: 32, right: 16`。
- 把阴影 pass 的 `opaque_batches.chain(shadow_batches)` 退回只遍历
  `opaque_batches` → ③ 红：`屏幕外投影物的影子必须落在地面上：有阴影 236.0
  vs 无阴影 236.0`（影子完全消失）。
- 把 `SKINNED_BOUNDS_PADDING` 改成 1.0（旧的无余量行为）→ 蒙皮余量单测红：
  `left: Vec3(1.0, 1.0, 1.0), right: Vec3(1.5, 1.5, 1.5)`。

**微基准 1 条**：`tests/culling_cost.rs`（手动、`#[ignore]`）。同一场景、同一
相机，只切 `set_frustum_culling`；共享网格与各自网格各测一组（原因见下节）。
基准自己断言"关剔除 0 剔除 / 开剔除 > 一半被剔 / 画了 + 剔了 == 2000"，
防止剔除没生效时得到一张假快的表（P2.5 的教训）。

### 五、读数出口：编辑器 HUD 与无头截图

- 视口右下角新增一行 `Draws N   Inst M   Cull K`（等宽 10.5，与左下 fps / 右上
  相机同一套药丸底）：相机一转这个数就变，剔除有没有在干活不用翻日志。
  与缩放提示（`bottom - 26`）同角叠放，不压字。
- `SceneRenderer::frame_stats()` 直连管线；读的是**上一次真正渲染**的结果 ——
  静态场景被指纹门控整帧跳过时，画面就是上一帧那张，统计也停在上一帧。
- 无头截图报告新增 `screenshot.render_stats`
  （`draw_calls` / `opaque_batches` / `opaque_instances` / `blended_draws` /
  `culled_entities` / `culled_instances` / `shadow_only_instances` /
  `cull_ratio`）。自动取景会把整场框进视锥，所以默认截图 `culled_* == 0`；
  显式 `--camera-pos/--camera-target` 时它解释"图里为什么少了东西"。
  JSON 字段名与顶层一样是**稳定契约**（测试钉字段存在性）。

### 六、刻意的取舍

1. **先剔实体、不剔图元**：一个网格的内部图元通常同生共死；逐图元求交的收益
   在"整网格挡住/没挡住"的粒度上几乎为零，还要为每个图元保存世界 AABB。
2. **AABB 而不是包围球**：球半径取半对角线，对长条物体（地面 / 墙）会大到
   永远与视锥相交；AABB 的 p-vertex 测试既准确又只多几次比较。
3. **蒙皮余量而不是精确蒙皮包围盒**：见第三节。50% 是"常见骨骼摆幅能盖住、
   又不至于让角色永远不可剔"的工程值；要改先拿狐狸走路 / 挥拳的截图量。
4. **仅阴影批次独立成表**，而不是"把相机外实体留在主批次里让 GPU 裁"：那样
   主 pass 仍会写实例数据、绑材质、发 draw call，CPU 侧一分不省；也不能把
   它们剔除后只补一遍阴影 pass（会重复收集）。
5. **剔除默认开、开关只给测试/基准**：保守性由逐字节像素等价测试守，生产路径
   没有关闭的理由。相机矩阵退化自动关闭，不暴露"剔除失败"这种状态。
6. **不做遮挡剔除**：视锥剔除只回答"在不在画面里"，不回答"被不被挡住"；
   Hi-Z / 深度预pass 是另一个量级的工程（要读回上一帧深度 + 重写实例表），
   留到批次结构再演进时做。
7. **基准把共享 / 各自网格分两张表**：只报共享网格会严重低估剔除在"素材没做
   合批"项目里的价值（-3% vs -65%），只报各自网格又会把合批的收益算到剔除
   头上。两个数字一起看才知道该先做哪件事。
8. **基准关阴影**：相机外投影物按设计必须保留，混进来测的是"剔除 + 阴影补投"
   的组合而不是剔除本身；阴影正确性另有 GPU 测试。

### 七、已知限制

- **无 LOD**：距离远的物体仍按全精度网格画；剔除只去掉视锥外的，不降低视锥内
  的几何量。差距分析里 LOD 仍是缺口。
- **无遮挡剔除 / Hi-Z**：被墙挡住的一整排物体仍在主 pass 里。遮挡剔除需要
  深度预 pass 或上一帧深度回读，当前管线还没有可用的深度金字塔。
- **剔除是 O(实体数 × 6 平面)**：2000 实体每帧多 ~6 µs 量级的 CPU 成本（已
  包含在"开剔除"的 CPU 读数里），没有空间索引；万级实体前要配合 ECS / 分区
  做 broad phase，而不是给每个实体再乘常数。
- **蒙皮包围盒仍是绑定姿态**：余量覆盖常见摆幅，但"长矛突刺"这类极端姿势
  仍可能在屏幕边缘提前消失；要精确就得 CPU 蒙皮求盒或让 GPU 回读。
- **仅阴影实体仍付蒙皮骨骼计算**（必须算，阴影顶点着色器要用）；剔除省的是
  主 pass 的实例写、材质组与 draw call，不是骨骼。
- **阴影贴图的拟合上限 `MAX_SHADOW_RADIUS` 之外**：物体已不在阴影图里，剔除
  与阴影补投都不会救它（与旧行为一致）。

## 三十一、渲染：FXAA 抗锯齿（2026-10-20）

补齐差距分析里"后处理链"的 FXAA 一格。引擎的场景 pass 已经走 MSAA（默认
按适配器能力取最高档，常见 4×），但 MSAA 只解决**几何边缘**的覆盖问题：
高光的高频细节、细长几何、低采样档位下的斜边仍然是一格一格的硬台阶。FXAA
是叠在合成之后的第二趟全屏 pass，把"显示空间的台阶"磨成中间调。

```text
场景 pass（HDR, MSAA） ──resolve──▶ HDR ──合成（曝光 / Bloom / ACES）──▶
    ┌─ FXAA 关：直接写 LDR 显示目标（与上线前逐位一致）
    └─ FXAA 开：先写 LDR scratch，再 FXAA → LDR 显示目标
```

**默认关**（`RenderConfig.fxaa.enabled = false`）：FXAA 会改变画面（哪怕只在
边缘），与 Bloom / 天空同一条纪律 —— 画质增强不能默认动旧场景。关着时渲染器
**整段跳过**：不建 scratch、不记录 pass、不写 uniform，是真正的零成本，而不是
"跑一遍然后原样输出"。

### 一、算法：FXAA 3.11 的质量路径，阈值保守一档

- **5 抽头 + 亚像素项**（`FXAA_QUALITY__PRESET 12` 一族）：中心 + 四角判边缘
  方向，沿方向取 1/3、2/3 两点平均成 A，再用 ±½ 两点平均成 B；B 跑出局部
  亮度范围时退回 A。亚像素项把中心与四邻均值按局部对比度混入（0.75），
  专治"细高光闪烁"这类 MSAA 管不到的走样。
- **常量取公开口径，边缘阈值保守一档**：亚像素 0.75、方向收缩 1/8（下界
  1/128）、跨度上限 8 texel 都是 FXAA 3.11 原值；边缘阈值 stock 是 1/24 与
  1/8，这里取 **1/12 与 1/6** —— 默认已有 MSAA，stock 阈值叠上去会整幅柔化，
  而 FXAA 只该收拾 MSAA 覆盖不到的走样。这些阈值是 Lottes 按 8 位显示值标定
  的，改一个就要重新评估"平坦区域是否被无辜柔化"—— 因此 `FxaaConfig` 只暴露
  开关，不把调参负担转给用户。
- **luma / 混合在显示空间做**：阈值回答的是"人眼在屏幕上看到的对比度"。
  在线性光里算，暗部边缘会被过度平滑、亮部欠平滑。sRGB 目标上采样值由硬件
  解码成线性，着色器先 `srgb_encode` 到显示空间，混合完再 `srgb_decode` 回
  线性交给硬件第二次编码；非 sRGB 目标（合成 pass 已手动编码）直接原样处理。
- **平坦区必须逐字节不变**：五抽头范围低于阈值且亚像素项为 0 时，原样返回
  中心采样值，不走 encode/decode 往返 —— 8 位量化边界上的往返误差会让"没开
  FXAA 的角落"悄悄变一个 LSB。
- **采样一律 `textureSampleLevel(..., 0.0)`**：边缘检测的分支建立在采样值上，
  WGSL 只允许显式 LOD 的采样出现在非一致控制流里。后处理是 1:1，LOD 恒为 0，
  没有画质代价。

### 二、接线：独立第二趟 pass + 惰性 scratch

- 合成 pass 的目标从"显示目标"变成"FXAA 关时 = 显示目标 / 开时 = scratch"；
  FXAA pass 再把 scratch 写进显示目标。两条路径共用同一条合成管线，管线状态
  没有分叉。
- scratch 是**目标格式**的 LDR 纹理（`RENDER_ATTACHMENT | TEXTURE_BINDING`），
  尺寸跟源走；尺寸变化时整块重建（绑定组引用视图，视图对象换了必须重建）。
  关着时直接 `None`，不养这份显存。
- FXAA 的 uniform（texel 尺寸 + sRGB 标志）在 scratch 建好时一次写死，之后
  每帧只记录 pass —— 参数不随帧变化，没有理由每帧上传。

### 三、先证红

实现之前，`fxaa_smooths_edges_without_touching_flat_areas`（1× MSAA、
320×240、绕 Z 轴旋转的薄盒）在旧实现下**红得干脆**：`fxaa` 只是个未接线的
配置字段，开 / 关两张图逐字节相同，第一条断言直接报
`FXAA 开关没接进后处理链：两张图逐字节相同`。

实现后的读数（同机一次运行）：

| 指标 | 关 | 开 |
|---|---|---|
| 硬台阶数（相邻亮度差 > 0.25，横 + 纵） | 672 | **374（-44%）** |
| 中间调像素（背景 0.389 / 前景 0.953 之间） | 0 | **804** |
| 平坦背景角点 `(4,4)` | — | **逐字节不变** |

同组测试还钉住三条前提：前景 / 背景亮度差 > 0.15（不然测不出边缘）、
关着时硬台阶 > 200（1× MSAA 的斜边必须真有台阶）、开着的平坦角点必须与
关着逐字节相同（早期退出的第一道闸门）。

### 四、确定性与成本

- `two_runs_produce_identical_png_bytes` 改为**同时开满 Bloom + FXAA**：
  scratch 的惰性建、两段 pass 的记录顺序、sRGB 往返都必须确定 —— 两次进程
  运行（连渲染器都重建）PNG 仍逐字节一致。
- 桌面设置里加 `FXAA` 勾选（Inspector 的渲染区块），`scene_signature` 哈希
  `render.fxaa.enabled`（暂停时勾选必须让视口刷新）；无头报告与 MCP
  `project_info` 都投影 `render.fxaa.enabled`，报告字段名是稳定契约。
- **实测**（RTX 2060 / 1280×720 / MSAA×4，`postfx_cost` 新增第四档）：

  | 路径 | 场景段 | 后处理段 | 合计 |
  |---|---|---|---|
  | HDR + ACES | 131.1 µs | 24.7 µs | 155.8 µs |
  | HDR + ACES + Bloom | 131.1 µs | 85.8 µs | 216.9 µs |
  | HDR + ACES + FXAA | 131.3 µs | **72.2 µs** | **203.5 µs** |

  FXAA 相对纯 ACES **+47.6 µs/帧（+30.6% 端到端）**：一张全尺寸 LDR scratch
  + 一趟 9 次采样（5 抽头 + 亚像素 + sRGB 换算）的全屏 pass。这是"开"的
  价格；关着时后处理段仍是 24.7 µs（整段跳过）。

### 五、刻意的取舍

1. **只做 FXAA、不做 TAA**：TAA 要历史缓冲、重投影与运动向量，是另一个量级的
   工程（还要处理镜头移动时的鬼影）。FXAA 是"一张当前帧就能做完"的方案，
   先把低采样下的台阶问题关掉，TAA 留到有速度缓冲时再做。
2. **默认关而不是"MSAA 低档才开"**：自动策略会让同一项目在不同 GPU 上画面
   不同，也违背"画质增强默认不动画面"的纪律。开关交给项目和用户。
3. **不做质量档位**：FXAA 3.11 的各档差异只在边缘搜索的采样数，当前带宽下
   质量档的收益（更长的边缘）抵不上多出来的采样；要档位时再加字段，默认值
   保持不变。
4. **scratch 不池化复用**：开关是手动操作，重建一次无感；池化会让"关着时
   零显存"这条承诺失效。
5. **测试强制 1× MSAA**：4× MSAA 的斜边本来就有中间调，FXAA 的效果会被
   硬件抗锯齿掩盖，测不出"开关是否接线"。1× 才能让 672 个硬台阶暴露出来。

### 六、已知限制

- **FXAA 会柔化高频细节**：细文字、极细的几何、镜面高光边缘都会被抹一点；
  这是空间抗锯齿的固有代价，不是实现缺陷。
- **没有质量档 / 锐化**：不做"FXAA + 锐化"的组合，也不暴露阈值；需要更锐的
  项目应优先提高 MSAA / 分辨率。
- **scratch 是一张全尺寸 LDR 纹理**：4K 下约 33 MB（`Rgba8`），开关打开时
  常驻。关掉即释放。
- **不处理运动**：FXAA 是纯空间滤波，镜头快速平移时仍可能有高频闪烁；
  TAA / 运动模糊是另一条路。
- **不对 HDR 中间标做 FXAA**：边缘检测在显示空间做，HDR 高光在曲线前不参与
  判断；这是有意为之（见"显示空间"一节），但也意味着"曲线前的高频"不会被
  这一步处理。



---

## 三十二、P0 资产管线：GUID + `.meta` + 导入 + 资源浏览器（2026-10-20）

优先级来自 `Docs/差距分析.md`：**唯一的 P0**（资产管线）。不做的代价是
"场景 / 预制体多起来之后改引用格式 = 重写全部资产文件与所有外部工具"。

### 一、为什么先做这一刀

盘点结论（`差距分析.md` 第四节）：场景里 `model: "Resources/Box.glb"` 是路径，
资产改名 / 挪目录即断链，只能靠 `--check` 事后发现再手改。而引用格式是**数据格式**，
越晚改越贵。同时"资源浏览器"这一端完全缺失（只有 Prefabs 面板），
每加一个资产都要手改 RON —— 对 Agent-Native 路线尤其刺眼。

### 二、做法（完整推理见决策记录 §十四）

1. **磁盘存 GUID、内存存路径**。转换只在 `ProjectHandle` 的 load / write 边界：
   `xgame_core::asset` 的 `encode_scene_references` / `decode_scene_references`
   （预制体同构复用）。243 个消费点一行没改。
2. **`.meta`**（`<资产>.meta`，RON）：`guid` + `kind` + `import`。
   GUID 首次导入生成，之后永不变；改名 / 移动只搬文件。
3. **`AssetDatabase`**：一次扫描建 `guid ↔ path` 双向索引 + 诊断
   （缺 meta / 孤儿 meta / 坏 meta / GUID 冲突）。构建产物目录跳过。
4. **`--check` 扩展**：`dangling_guids` / `missing_metas` / `orphan_metas` /
   `invalid_metas` / `guid_conflicts` / `missing_prefabs`。检查仍**严格只读**。
5. **`--migrate-assets <项目>`**：老项目一次性升级。用**字符串字面量替换**
   而不是整份重序列化 —— 示例场景是带注释的教程，重写会吃光注释
   （第一版实现踩了，靠 `git diff` 发现后改成替换）。
6. **资源浏览器**（Assets 面板，与 Prefabs 共页签）：目录树 + 类型图标 +
   搜索 + 详情（路径 / GUID / 导入设置）+ 双击放入场景 + **拖到视口按射线落点放置**。
7. **MCP `asset_list`**：智能体的资产树视野（只读，含诊断）。
8. **仓库内三个项目已迁移**：8 份 `.meta`，`SampleScene` 场景 14 行引用改写，
   注释一字未动。

### 三、验证

- 新回归测试 **33 条**（core 22：资产库 15 / 项目端到端 4 / check 诊断 3；
  编辑器 9：状态 5 / 面板 2 / 落点 2；MCP 2），全量 **491 passed / 6 ignored**
  （基线 458）。
- 核心验收测试：`renaming_asset_keeps_scene_reference_alive`
  （挪资产 + `.meta` → 场景仍解析到新路径）—— 旧实现下必红。
- 冒烟：`--check projects/SampleScene` → `entities=14 lights=1 assets=8` 全绿；
  `--headless --frames 60` 正常；`--ui-shot` 三档尺寸截图检查资源浏览器布局。

### 四、刻意的取舍

1. **内存不引入 `AssetRef` 类型**：见决策记录 §十四.1，避免改 243 个消费点与热路径。
2. **场景文件不登记资产**：它由 `entry_scene` 按路径引用，没有改名需求；
   强行要 `.meta` 会让"新建项目"立刻带一条待处理诊断。
3. **导入设置只落盘不消费**：`model_scale` 的正确实现要连骨骼 / 逆绑定矩阵 /
   动画轨道一起缩放，半吊子接线会与物理不一致 —— 留到下一刀（文档已标注）。
4. **视口落点用 y=0 地面平面**，不做任意表面吸附：项目约定地面在 y=0；
   射线朝上时退回相机前方 5 米。表面吸附等做角色控制器时一起补射线查询。
5. **拖拽不用 egui `DragAndDrop` payload**：视口是自绘离屏纹理，命中与落点本来
   就要自己算；`Option<String>` 更直白，也更好测（收尾放在 `main.rs` 帧末，
   面板被拖成非活动页签时不会漏）。

### 五、已知限制

- 资产被删后，场景里留 `guid://…`（这是刻意的，`--check` 会点名）。
- `Fox.gltf` 的外部 buffer `Fox.bin` 不是独立资产：移动 gltf 时必须连它一起搬
  （`.meta` 目前不记依赖）。
- 重命名资产要在编辑器外做（还没有 in-editor 的 rename / move API）。
- 编辑器保存是全量序列化，会吃掉场景注释（既有行为，见决策记录 §十四.7）。

## 三十三、角色控制器 + 碰撞事件进脚本（2026-10-20）

优先级来自 `Docs/差距分析.md` P1 第 3 项：3D 游戏的主角移动与"踩到触发器"是玩法
闭环的第一块。路线是**封装 rapier 的 `KinematicCharacterController`**，不手写扫掠。

### 一、四个上游缺陷（这一节是本轮的主要成本）

rapier 0.22 + parry 0.17.6 的这条链路对**胶囊角色**有四处硬缺陷，全部先用诊断
测试跑出真实数据、再决定绕过方式（完整推理见决策记录 §十五与
`xgame-physics/src/character.rs` 的模块文档）：

1. `(Capsule, Cuboid)` 的 contact manifold 在 parry 里被注释掉 → `grounded`
   恒为 false，连带 `snap_to_ground` 整体失效。
2. `check_and_fix_penetrations()` 是空函数 → 一旦陷地，扫掠返回 `None`、整段
   位移放行，实测 60 帧沉到 y=-41 并穿墙。**根因链**：静止间隙恰好等于 `offset`
   时本来就会退化 → 所以静止间隙取 `skin_width × 1.1`，着地窗口再宽 20%。
3. 60° 薄板上胶囊扫掠返回 `None`（0°/30° 正常）→ 陡坡滑落自实现（投影到坡面）。
4. `autostep` 的 `handle_stairs` 只在"第一次扫掠撞墙"时触发，且 `min_width`
   不足以把胶囊底面推上台阶 → 关掉，自实现"抬升 → 前进一个半径 → 贴回"。

### 二、分层

| 层 | 内容 |
|---|---|
| `xgame-core` | `CharacterController`（可序列化 + `sanitized()` + `center_height()`/`rest_gap` 约定）、`CharacterState` / `CharacterContacts`（运行时组件，零分配） |
| `xgame-physics` | `move_character`（grounded / snap / 皮肤层硬约束 / 陡坡滑落 / 台阶）、`probe_ground`（胶囊中心出发、法向间隙） |
| `xgame-script` | `character_*` 8 个方法（Lua 与 Rust 组件共用真相源）、`Hook::{CollisionEnter, CollisionExit}` + 带参数派发 |
| `xgame-runtime` | controller 实体的刚体托管、接触面差分 → enter/exit 双向派发、`Lifecycle::{on_collision_enter, on_collision_exit}` |
| `xgame-editor` | Inspector 的 Character 区块 |

### 三、验证

- 物理新增 **21 条**（4 条配置翻译 + 17 条行为）：站立 / 吸附 / 悬空 / 撞墙 /
  沿墙滑 / 上台阶 / 高台阶挡停 / 爬坡 / 陡坡滑落 / 坡面法线 / 跳跃 / 传感器 /
  高速下冲 / 高速横冲 / 陷地自救 / 零位移 / 零台阶高度 / 缺失刚体。
- 运行时新增 **3 条**：角色行走（实体变换跟随 + 状态组件）、跳跃（最高点 + 落回）、
  角色碰撞事件（运动学 vs 静态）；脚本层 **1 条** Lua 端到端（脚本驱动 + 收事件）。
- 全量：`cargo test --workspace --lib` **412 passed / 0 failed**；集成测试
  （runtime / script / xgame / render）另 16 条全绿，6 条 GPU / 截图用例 skip。
  编辑器测试二进制在本机被安全策略拦下，未纳入本次统计。

## 三十四、游戏 UI：屏幕空间 UI 运行时 + 渲染 + 脚本 API（2026-10-20）

优先级来自 `Docs/差距分析.md` P1 第 4 项。评价很直接：**任何可玩的 demo 都需要
HUD、菜单、血条**，而这是"内容层三件套"里唯一完全没有技术底子的一件
（角色控制器有物理底座、粒子有渲染底座，UI 什么都没有）。

路线刻意**先做运行时、不做可视化编辑器**（差距分析里写的就是这句）：egui 的
即时模式布局思路不能复用（没有锚点、没有九宫格、不能做游戏内交互），而
"把锚点算对、把字排对、把点击判对"才是真正难的部分。

### 一、分层

| 层 | 内容 |
|---|---|
| `xgame-core::ui` | `UiElement` / `UiStyle` / `UiTextStyle` / `UiImageStyle` / `UiState`（可序列化；**运行时状态 `#[serde(skip)]`**） |
| `xgame-ui`（新 crate） | 锚点布局（`layout`）、指针状态机（`input`）、文本排版 + 字形图集（`text`）、绘制数据合批（`draw`）。**纯 CPU，不认识 wgpu** |
| `xgame-render::ui_renderer` | 一条实例化管线（圆角 SDF + 纹理采样）、图集 / 图片上传、UI pass |
| `xgame-runtime` | 每帧布局 + 命中测试 → 写回 `UiState`；`set_ui_viewport` / `set_ui_pointer` |
| `xgame-script` | 23 个 `ui_*` API（`ui_ctx.rs` 方法层 + `ui_api.rs` 转发壳） |
| `xgame-editor` | Inspector 的 UI 区块（锚点预设 / 样式 / 文本 / 图片）、Create UI 菜单、Play 态指针映射 |

### 二、五个关键决定

**1. UI 元素就是实体（Unity `RectTransform` 模式），不是独立的 UI 树。**

层级面板、撤销栈、预制体、复制粘贴、Inspector 全部免费复用 —— 对一个小引擎
来说，"少一套平行的编辑器模型"比少写几百行布局代码重要得多。代价是 `Entity`
多了一个字段，`transform` 对 UI 元素不参与布局（位置由锚点决定）。

**2. 锚点模型只做 Unity 的最小可用子集：anchor_min/max + offset_min/max。**

```text
rect.min = parent.min + anchor_min * parent.size + offset_min
rect.max = parent.min + anchor_max * parent.size + offset_max
```

四行公式覆盖"贴角 / 居中 / 四边拉伸 / 通栏"。**故意不做 pivot 与旋转缩放** ——
它们只有做动画 / 世界空间 UI 时才需要，却会让命中测试与文本绘制复杂一档。
分辨率适配靠锚点（Unity 的 "Constant Pixel Size" 档），"设计分辨率缩放"
（Canvas Scaler）留到有需要时再加。

**3. 运行时状态（悬停 / 按下 / 点击）放进 `UiElement` 本身，`#[serde(skip)]`。**

三个好处：脚本查询（`ui_clicked(ctx, id)`）只需要 `&Scene`（不用给 `GameCtx`
加字段，也就不会波及七个构造点）；渲染器读同一份数据画悬停态；实体删除时
状态跟着走，不会留悬垂条目。

**代价是一个必须记住的坑**：`Entity::eq` 是脏检查的真相源，把 `state` 算进去
会让"鼠标划过按钮"变成"场景有未保存改动"。所以 `UiElement` 额外提供
`data_eq()`（只比落盘字段），`Entity::eq` 用它 —— 回归测试
`state_changes_do_not_count_as_edits` 锁着这一条。

**4. 矩形的绘制顺序 = 实体层级的 DFS 先序，命中测试反向扫描。**

不引入 z 序字段：同层按实体在场景里的顺序，后画的盖住先画的。这是"唯一真相"，
没有排序步骤，也不需要处理"两个元素 z 相同"的歧义。

**5. 所有 UI 图元走一条实例化管线（这一版最重要的性能决定）。**

一开始是两条管线（矩形 SDF / 纹理精灵），实测一屏 900 个元素切出 **1200 次
draw call** —— 因为"背景 / 文字"交替出现，而**绘制顺序就是语义**，不能按类型
重排（后一个元素的面板会盖掉前一个元素的文字）。

合并成一条管线后，矩形不采样纹理但绑定组与字形相同（图集），于是相邻的
矩形与文字天然同批：**示例 HUD（7 个元素 / 4 个矩形 / 47 个字形）= 1 次 draw**。
只有图片会打断批次（它是另一张纹理）。无头报告的 `render_stats.ui.draws`
就是这条读数，CI 能直接断言。

### 三、文本排版

自己写（`ab_glyph` 提供度量与轮廓光栅化，不引 cosmic-text / swash）：

- **字形缓存 + R8 图集**：1024² 起、按需翻倍到 2048²，shelf 打包，字形四周留
  1px 透明边防渗色；UV 用 **texel 坐标**存，图集翻倍不影响旧字形。
- **换行**：拉丁按空格断词、中日韩任意字间可断（UAX #14 有一百多条规则，对
  HUD 没有收益）；找不到断点就硬断 —— 溢出到别的元素上比断在单词中间更难看。
- **对齐**：水平三档 + 垂直三档，行宽按 advance + kerning 重算。
- **落整像素**：字形按整像素光栅化，小数坐标只会让双线性采样把边缘糊掉。
- **字体来源**：系统字体（与编辑器同一套候选），找不到**不崩**、只是不画字
  （告警一次）。不内嵌中文字体：10–20 MB + 授权条款各不相同。

已知限制：不做复杂文字整形（阿拉伯文 / 天城文连写不会正确成形）、没有
粗体合成（缺粗体字重时退回常规）、没有字距微调之外的字形变体。

### 四、颜色空间（最容易搞错的一处）

- 组件里存的是**线性 RGBA**（与材质 / 灯光同一约定），RON 里写
  `(0.0070, 0.0086, 0.0116, 0.78)` 这种值；
- **脚本 API 收 sRGB 0..1**（美术 / CSS 的直觉），入口统一 `to_linear()`；
- **egui 的取色器收线性值**（`color_edit_button_rgba_unmultiplied`）——
  第一版多套了一次 `to_srgb()`，色块比画面亮一大截（截图里按钮显示成浅蓝），
  已修正并写进注释。

实测校验：截图里按钮底色 `(38, 92, 168)` = 场景文件里写的 `#265ca8`，逐分量一致。

### 五、验证

- `xgame-core`：序列化兼容（最小 RON）、运行时状态不落盘、`data_eq` 忽略状态、
  坏数据夹取。
- `xgame-ui`：28 条 —— 锚点数学（四角 / 拉伸 / 嵌套 / 尺寸不随窗口变 / NaN
  几何拦截）、DFS 顺序与隐藏子树、命中测试的层级与穿透、点击状态机（按下 →
  拖出 → 拖回 → 松开仍算点击；拖出松开取消；离开视口清悬停）、文本（换行 /
  对齐 / 透明度 / 缓存命中 / 图集翻倍保内容）、悬停与按下明度、合批（矩形与
  字形同批、图片切断批次）。
- `xgame-runtime`（集成，5 条）：**真 Lua 脚本**端到端 —— `ui_clicked` 触发、
  点外部不触发、无视口不交互、脚本 `ui_spawn_button` 生成的元素下一帧可点、
  隐藏元素不可点。
- 编辑器：`viewport_pointer` 的坐标换算（DPI / 视口偏移 / 视口外返回 None）。
- **截图自检**：`--screenshot` 出图（HUD 在 1280×720 下排版正确、中文 / 粗体 /
  圆角 / 血条 / 按钮全部正常）；`--ui-shot --ui-play` 验证编辑器 Play 态视口；
  `--ui-select HUD.Button` 验证 Inspector 区块（并据此修掉了四个被截断的标签、
  取色器色彩空间、以及"UI 区块落在折叠线以下"的布局问题）。
- **坏数据防线**：加载边界 `ProjectHandle::load_scene/load_prefab` 夹取一次
  （NaN / 越界 → 默认值），布局入口再拦一道非有限矩形（整棵子树跳过 + 告警）——
  一个手写的 `nan` 不会变成"整屏 UI 消失"。
- **性能**（`cargo test -p xgame-ui --release --test ui_cost -- --ignored`）：
  900 元素 / 3190 字形 → **753 µs/帧**（占 60fps 预算 4.5%），1200 个命令段
  被渲染器合并成 **1 次 draw**。空场景（1000 个纯 3D 实体）走 `any` 快路径，
  一次布局都不做。

### 六、已知限制（有意为之，不是遗漏）

- 没有裁剪（`RectMask2D` 等价物）：子元素溢出父矩形会画到外面 —— 滚动列表
  要做之前必须先补它；
- 没有 pivot / 旋转 / 缩放动画；
- 没有九宫格（9-slice）与图集打包；
- 没有文字输入框、没有手柄 / 键盘导航；
- UI 不吃指针时游戏输入照常（"点 UI 时不要开枪"需要脚本自己用 `ui_hovered` 判断）；
- 无 Canvas Scaler（设计分辨率缩放）。

## 三十五、材质资产（`.mat`）+ 导入参数接线（2026-10-21）

优先级来自 `Docs/差距分析.md` P1 第 5 项。原文的判断是：材质资产是内容复用的
最小单位，而 P0 资产管线已经把所有前置条件（GUID、`.meta`、资源浏览器）铺好了。
这一轮顺手把 §十四.6 遗留的消费面一起接掉 —— 材质是纹理导入设置的第一个真实
消费者，分开做会为了同一个绑定组改两遍。

### 一、分层

| 层 | 内容 |
|---|---|
| `xgame-core::material` | `MaterialAsset`（`base_color` / `metallic` / `roughness` / `albedo`）、默认值、RON、`sanitized()` |
| `xgame-core::scene` | `Entity::material_asset: Option<String>`（内存路径；磁盘 GUID） |
| `xgame-core::asset` | `AssetKind::Material`、`.mat` 扩展名、实体材质引用编解码、`read_import_settings()` |
| `xgame-core::project` | `list_materials()` / `material_path_for()` / `load_material()` / `write_material()`（albedo 的 GUID⇄路径边界） |
| `xgame-render::material_library` | `.mat` 解析、albedo 上传、绑定组缓存、负缓存、`MaterialKey::{Model, Asset}`、修订号 |
| `xgame-render::scene_pipeline` | 逐实体材料解析（模型 → 资产 → 覆盖）、批次键加材质、项目绑定与失效 |
| `xgame-render::gltf_loader` | 读 `.meta` 的 `model_scale` / `texture_srgb` / `texture_nearest`；资产材质与模型材质合并 |
| `xgame-editor` | Assets 面板新建 / 编辑 / Apply·Revert / 指派；Inspector 资产选择器；拖放语义 |
| `xgame-script` | `material_asset` / `set_material_asset` / `set_material` / `clear_material` |
| `xgame-mcp` | `project_check` 暴露四项材质诊断字段 |

### 二、六个关键决定

**1. `.mat` 是完整材质，不是覆盖层（最重要的一条）。**

- 资产的颜色 / albedo 与模型颜色**相乘**；
- 资产的 metallic / roughness **替换**模型值；
- albedo 为空 = 1×1 白，**不回退模型贴图**。

如果空 albedo 保留模型贴图，"换材质"就会变成"有时换、有时不换"的玄学。
优先级只有一条链：**模型 → 资产 → 实体覆盖**；覆盖在资产结果上逐字段打补丁，
`set_material` 不会清掉 `material_asset`。两者正交，才能同时表达"这是木头"
和"这个箱子偏红"。

**2. GUID 只出现在磁盘边界。**

`Entity::material_asset` 与 `.mat` 的 albedo 在磁盘上都是 `guid://…`，内存里
都是项目相对路径；转换只在 `ProjectHandle` 的 `load_*` / `write_*` 里。渲染热
路径（每帧查表）只认路径，`MaterialLibrary` 的键也是路径 —— 把 GUID 解析塞进
热路径既慢又会让同一资产出现两份缓存。

**3. 材质库住在 `ScenePipeline`，失效必须显式。**

`MaterialLibrary` 按路径缓存 `.mat`、纹理与绑定组，失败也缓存（负缓存），避免
一个坏文件每帧刷日志 + 读盘。编辑器保存材质 / 改导入设置后调
`SceneRenderer::invalidate_materials` / `invalidate_assets`；渲染指纹混入材质
修订号，否则会出现"文件改了、视口纹丝不动"的假象。独立游戏只在启动时
`set_project` 一次，不做文件监听。

**4. 批次键从「图元地址」扩成「图元地址 + 材质 key」。**

这是本轮的硬红线。同一个 `builtin://cube`（或同一份 glTF 网格）的两个实体挂
不同 `.mat` 时，按图元合批会在一次 `set_bind_group` 里把整批盖成最后一份材质，
表现为"两颗箱子同色"。半透明 draw 本来就按距离排序、不合批，但也携带材质 key；
阴影批只比较深度，保持图元地址。回归测试
`material_assets_split_opaque_batches` 钉死：不同资产 = 2 批，改成同一资产 = 1 批。
批次顺序仍是首次遇到顺序，不改哈希序 / 不按材质排序 —— 无头截图的逐字节确定性
依赖它。

**5. `model_scale` 在加载边界烘焙，不乘世界矩阵。**

缩放因子乘四处：顶点位置、节点平移、动画 translation 轨道、逆绑定矩阵平移
（`w_axis.xyz *= s`）。只在世界矩阵外乘一层会让画面缩放、但物理碰撞 / 包围盒 /
射线拾取仍按原尺寸 —— 三者错位比不缩放更难查。非有限 / ≤0 的缩放退回 `1.0`。
纹理的 `texture_srgb` / `texture_nearest` 由 `TextureOptions` 消费，模型贴图与
材质 albedo 走同一条上传路径。

**6. 编辑器材质编辑用浮窗 + 草稿 + 显式 Apply。**

材质编辑器改的是 `MaterialDraft`，点 Apply 才写盘，Revert 丢弃。理由：写盘有
IO + GPU 纹理重传，逐帧写会把一次拖动变成几十次落盘；撤销栈是**场景快照**，
不该混入资产文件历史，所以材质编辑本身不可撤销（Revert 是替代品），播放态整体
禁用。编辑器放在**应用级浮窗**里，而不是 Assets 详情区：1280×800 下底部面板
只有 ~150px 高，四个字段 + 三个按钮必被裁掉；浮窗必须在 dock 之后画，否则后画
的 Output Log 会盖住它。Tint 只在 `response.changed()` 时回写（egui 的
color button 会把 f32 无条件量化写回，导致一打开就显示"未保存"）。
指派材质相反 —— 它是场景改动，会压撤销栈。三个入口共用
`EditorState::assign_material`：Inspector 下拉、Assets 双击（给选中实体）、
拖到视口（优先鼠标下实体，其次选中实体；都没有就提示，**不生成新实体**）。

### 三、检查、迁移与样例

- `--check` 新增 `material_refs` / `missing_materials` / `missing_material_textures` /
  `invalid_materials`；同一路径被 N 个实体引用只检查一次；检查仍严格只读。
- `--migrate-assets` 除场景 / 预制体外，也改写 `.mat` 的 albedo 字面量并给
  `.mat` 补 `.meta`。
- `SampleScene` 新增 5 份材质（Concrete / Wood / PolishedMetal / RedPlastic /
  BlueGloss），给地面、旋转体、弹跳球、三个箱子、柱子指派；箱子仍保留各自的
  颜色覆盖，用来演示"资产 + 覆盖"两层同时生效。
- 空 albedo 走 1×1 白纹理；渲染侧统一夹取 metallic / roughness，存盘保留用户
  原值（与 `MaterialOverride` 同款策略）。

### 四、验证

- `xgame-core`（167 条）：`.mat` 默认值 / RON 往返 / `sanitized`、实体材质引用
  往返与旧文件兼容、材质路径推导、GUID 编解码与迁移、检查的四类诊断。
- `xgame-render`（136 条，有 GPU 时）：材质库缓存与失效、模型 → 资产 → 覆盖的
  合并（颜色乘、PBR 替换、空 albedo 回退白）、`model_scale` 对顶点 / 节点 /
  动画 / 逆绑定的缩放、纹理导入选项、三条像素钉子：
  `material_asset_tints_builtin_mesh`（资产真的上屏、清引用后逐位回基线）、
  `material_asset_albedo_texture_applies`（GUID 贴图经 sRGB 解码后只剩红通道）、
  `material_assets_split_opaque_batches`（批次键必须带材质）。
- `xgame-editor`（115 条）：新建材质不覆盖同名（自动加后缀）、`.meta` 自动补齐、
  草稿 Apply 前不写盘 / Apply 后落盘 / Revert 丢弃、指派写实体字段 + 可撤销、
  无选中实体时双击材质不凭空生成。
- `xgame-script`（24 条）：4 个 API 的 Lua 可达性；`set_material` 自动夹取
  metallic / roughness，`clear_material` 只清覆盖不动资产。
- `xgame-mcp`（31 条）：`project_check` 同时报缺材质、缺 albedo、坏 RON，且
  `material_refs` 只数实体引用。
- 全工作区：**599 条通过 / 7 条基准 ignore**；clippy `-D warnings` 零告警。
- 样例项目：`--check` 输出 `material_refs=7 assets=14` 且 `OK`；`--headless`
  跑 300 帧；`--screenshot` 出图；编辑器 `--ui-shot` 在 1280 / 1440 / 1920
  三档自检材质浮窗、详情区与 Inspector 选择器（另扫 1280×720 最小高度）。

### 五、已知限制（有意为之）

- 只有 albedo 一个贴图槽：无法接入法线 / 金属-粗糙 / 自发光贴图，法线贴图需要
  先给顶点着色器补切线（glTF 的 `TANGENT` 目前没读）；
- 没有材质变体 / 继承 / 实例参数；
- 没有材质缩略图与文件监听，刷新靠 ⟳，保存靠显式失效；
- 脚本不能逐字段清除覆盖（要么 `clear_material`，要么改用资产）；
- `audio_streaming` 仍是数据面：流式播放未落地。

## 三十六、粒子系统：CPU 模拟 + billboard 渲染 + 编辑器 / 脚本（2026-10-21）

### 一、为什么是粒子、为什么这样做

P2 清单里"粒子系统"一直挂着。选它的理由：它是**第一个"每帧全变的大量运行态"
**—— 音频是命令束、动画是时钟、UI 是静态树，都不同时具备"量大 + 每帧全变 + 要进
渲染"三个性质。这一轮把这条路走通，后面真要上 GPU 粒子 / 特效编辑器时，分层已经
在那儿了。

三个关键取舍（都有被否决的备选）：

1. **模拟住在新 crate `xgame-particle`（CPU），不住 runtime 也不住 render**。
   粒子位置 / 速度是运行态，量大且每帧全变，塞进 `Entity` 会让场景 diff、撤销
   快照、脏检查全部失真（UI 的 `state` 字段上一轮已经示范过这个坑）。但也不能
   塞进 render —— 无头运行（`--headless`）没有渲染器，却要在报告里报粒子数。
   runtime 持有 `ParticleSystem`（与 `AudioEngine` / `PhysicsWorld` 同级的"子系统
   字段"），渲染只是消费者。
2. **确定性优先**：每个发射器一把 xorshift RNG（种子 = 组件 `seed`），所有随机数
   只从这一把流里取。同种子 + 同 dt 序列 ⇒ 逐位相同的四边形序列。这是无头截图与
   回归测试的地基，所以**刻意不引入** `rand::thread_rng()` / `SystemTime` /
   `HashMap` 迭代序。代价是"换个机器结果也一致"这句只对同平台成立（f32 运算
   顺序由代码固定，跨编译器理论上仍可能差）—— 对截图回归够用。
3. **billboard 朝向从 `view_proj` 逆矩阵现算**，而不是让三个宿主各自传相机
   right / up。渲染入口只多一个 `quads` 参数；传相机基的话有三个会写错的地方。

### 二、core：`ParticleEmitter` 组件（纯数据层）

- `crates/xgame-core/src/particle.rs`：`ParticleEmitter`（emitting / rate /
  max_particles / duration / looping / lifetime / speed / direction / spread /
  shape / gravity / drag / start_size / end_size / start_color / end_color /
  blend / seed）+ `EmissionShape`（Point / Sphere / Box）+ `ParticleBlend`
  （Alpha / Additive）。`Entity::particle` 字段 + `with_particle()`。
- 预设：`fire()`（默认值，刻意是一份"挂上就看得见"的配置）/ `smoke()` /
  `sparks()` / `burst_of(count)`。
- `sanitized()`：NaN / inf / 逆序区间在**加载边界**钳制（`ProjectHandle::
  load_scene` / `load_prefab` 各一道，与 `sanitize_ui` 同款）。非有限值退回
  默认值而不是钳到上界 —— `inf` 几乎总是手改坏了，猜"作者想要极快"不如回到
  一份确定可用的配置。
- `burst` 是 `#[serde(skip)]` 的运行态字段（脚本"炸一下"的请求位，模拟层消费
  后清零）。`data_eq()` 忽略它，所以脚本写 burst **不会**让编辑器显示未保存。
- 默认重力**不取自物理世界**：火焰要往上飘、烟要慢慢散，与场景全局重力无关；
  粒子是纯视觉，不该被物理世界的参数牵着走。

### 三、`xgame-particle`：确定性 CPU 模拟

- xorshift RNG、发射累加（`rate*dt` 攒够 1 才生一颗，单帧封顶 256 颗防手滑）、
  寿命 / 重力 / 阻尼积分、体积采样（球 / 盒内均匀）、方向锥采样（`spread` 半角，
  180° = 全向）、burst、`max_particles` 硬顶（满了丢欠账不攒）、按实体分桶、
  实体删除即丢桶。
- 出生点在实体世界变换下（体积与方向随实体朝向转）；出生后活在世界空间，实体
  再动不拖着粒子走（与 Unity `Simulation Space = World` 一致）；实体缩放不参与
  尺寸（尺寸是绝对世界单位）。
- 稳态零分配：桶里 `Vec` 与输出四边形缓冲都跨帧复用；粒子死光后每帧只剩一次
  O(n) 扫表（与音频快路径同思路）。
- 输出 `ParticleQuad`（世界中心 / 世界半边长 / 线性 RGBA / blend）+ `ParticleStats`
  （alive / emitters / capacity —— capacity 是各发射器上限之和，"粒子数撞顶"
  是这个系统最主要的性能故障模式，状态栏要能看见）。
- `signature()`（内容指纹：静止时恒定、在动时每帧都变）与 `animating()`（还要
  不要继续出帧）分开：前者给渲染门控做静帧跳过，后者给帧调度防"视角不动、
  粒子在飞时被当成空闲停下来"。

### 四、render：实例化 billboard pass

- `crates/xgame-render/src/particles.rs`：一次实例化 draw 画完同一种混合方式的
  所有粒子；软圆片 SDF 片元；Alpha（`One / OneMinusSrcAlpha`，片元输出**预乘**，
  用 `SrcAlpha` 会把 alpha 乘两遍、烟雾薄一半）/ Additive（`One / One`）两条管线；
  Alpha 远→近排序、Additive 不排（加法可交换）。
- **画在场景 pass 内部**（不透明与天空之后）：粒子是场景里的东西 —— 要被几何
  遮住（深度测试开、深度写关，粒子之间不互相遮挡）、要参与曝光 / 色调映射 /
  Bloom。UI 相反，见 `ui_renderer` 模块头。
- 批次数进 `FrameStats`（`particle_draws` / `particle_instances`）。

### 五、runtime + 三宿主

- `World::particles: ParticleSystem`，在 `update` 之后、音频之前推进（脚本这帧写
  的 burst 当帧就能炸出来）；用**渲染帧 dt**（与动画时钟同一取舍，无头 dt 固定
  所以仍逐位可复现）。
- 编辑器：`GameRuntime` 转发 + `SceneRenderer::render` 多收 `particles` 与
  `particle_signature`（指纹进渲染门控，否则"只有粒子在动"会被静帧跳过冻住）；
  帧调度的 `animating` 并入 `particles_animating()`；状态栏 Play 态显示
  `particles 活着/上限 (发射器数)`，撞上限转警告色。
- 独立游戏 / 无头截图同一条 pass；`render_scene_with_particles` 单开（既有调用点
  与全部静态取景测试传 `&[]`，没有噪音）。
- Inspector：`Particle` 区块（勾选才有 + Fire/Smoke/Sparks 预设 + 全字段），
  与 Audio 同款写法；没有"预览播放"按钮 —— Play 就是那条编辑器内模拟回路。
- 脚本：`particle_emitting` / `set_particle_emitting` / `particle_burst` /
  `set_particle_rate`。burst 可累积（连按两帧 = 两次爆发相加），非有限 rate 钳 0。
- `SampleScene`：柱顶 Torch（火焰）+ 地面 Campfire.Smoke（炊烟），两种混合方式
  并排演示；固定种子保证截图逐位可复现。

### 六、验证

- `xgame-core`（新增 7 条）：默认值是一份能看见的配置、RON 往返 / burst 不落盘 /
  缺字段回落、`sanitized` 钳 NaN 与逆序区间且幂等、零方向退回 +Y、burst 预设。
- `xgame-particle`（14 条）：同种子逐位一致、各语义（寿命到头一颗不剩、上限硬顶、
  阻尼衰减、burst、duration/looping、实体删除丢桶、形状/锥采样方向正确）。
- `xgame-render`（新增 2 条 GPU 像素钉子）：`particle_quads_light_up_the_frame`
  （HDR 白粒子点亮中心、纯红粒子红通道压过绿蓝、取走粒子逐位回基线）、
  `particle_quads_respect_depth`（被立方体挡住的粒子逐位等于无粒子、前面的
  加色提亮冒出来）。
- `xgame-script`（新增 2 条）：GameCtx 直驱（开关/rate/burst 累积/没挂发射器时
  静默 no-op 不凭空创建组件）+ Lua 可达性（四个函数都落在组件上）。
- 全工作区测试与 clippy：见收尾 commit 信息（本节写于跑批前，数字以 commit 为准）。
- 样例项目 `--check`：`entities=23`（+2 发射器）且 `OK`。

### 七、已知限制（有意为之）

- CPU 模拟：万级以上粒子会吃满一个核；真要上万，该上 GPU 变换反馈 / compute
  （分层已经留好：模拟与渲染只靠 `ParticleQuad` 数组通信）。
- 只有"软圆片"一种片元形状：没有图集帧动画 / 网格粒子 / 拖尾。
- 没有编辑器内的发射器预览（编辑态不模拟，Play 才喷）—— 见第五条的理由。
- 没有粒子与场景的碰撞（出生即世界空间，飞出去就飞出世界）。
- `Sprite`/纹理粒子没有：颜色是纯色插值，要贴图粒子得先有 UI 图集之外的
  第二张粒子图集（UI 图集是字体 + 矢量图元，塞不进火焰序列帧）。

## 三十七、编辑器 Profiler 面板：帧时间滚动窗口 + 绘制读数（2026-10-21）

### 一、为什么是它

差距分析「推荐顺序」第 7 项的第一件：内容层收口之后，缺口集中在**编辑器产品化**。
三件（Profiler / Preferences / 布局持久化）里挑 Profiler 先做，理由是它的**数据面
早就存在**——`frame_probe`（`XGAME_FRAME_LOG`）与 `ScenePipeline::FrameStats` 一直在
采，缺的只是"在编辑器里看得见"。让作者为了看一帧花在哪而去设环境变量、跑脚本，
是这一层最不合理的成本。

一次只做一件：Preferences 与布局持久化留作下一刀（`egui_tiles` 的树序列化 + 一份
`editor.ron`，与帧读数无关，硬凑在一起只会让两边都不完整）。

### 二、数据面：`crates/xgame-editor/src/profiler.rs`

- `FrameSample`：`dt_us`（帧间隔，与状态栏 FPS 同源 `stable_dt`）、`work_us`
  （`update()` 进编辑器界面到返回）、`runtime_us`（物理 / 脚本 / 音频推进那一段）、
  `ui_us`（停靠区：各面板排版 + **视口渲染提交**）、`live`（帧调度是否继续出帧）。
- `Profiler`：**定长环形缓冲 600 帧**（60fps 下 10 秒），写满即覆盖最旧；
  `record` 只写一个预分配槽位，稳态零分配；`paused` 时直接丢样本（边看边滚的图
  没法读某一帧），`clear` 重置窗口与计数但保留暂停位。
- `Summary`：frames / live / idle / fps / avg / min / max / **p50** / **p99** /
  avg-work / max-work / avg-runtime / avg-ui。p50 给图的纵轴定标用，p99 是
  "最差的 1% 帧有多慢"。

两个口径是硬约定（详见决策记录 §十九）：

1. **FPS 只统计 `live` 帧**。帧调度在静止界面上会停帧，停帧期间没有 `update()`，
   所以"停帧后重新出帧的那一帧"的 `dt` 是整段空闲时长。把它算进平均值，打字停几秒
   再动一下，FPS 就塌到个位数 —— 那是测量假象，不是卡顿。空闲帧单独计数并压暗显示。
2. **纵轴按中位帧 ×3 定标**（下限 6ms、上限 100ms），不是按最大值 / p99。一次
   80ms 的唤醒间隔会把其余 599 帧压成贴着底的一条线；反过来固定按 33ms 画，
   300fps 的构建里所有柱子都只有 9% 高，等于没有分辨率。

### 三、面板：`panels/profiler.rs`

- 与 Output Log **同一页签组**（右下列），默认激活仍是 Output Log —— 老的截图与
  手感逐像素不变，要看帧分析就点页签（截图走 `--ui-profiler`）。
  `dock::activate()` 按 pane 谓词激活，祖先链由 `egui_tiles` 自己补，布局再改也不会失配。
- 三段式：顶部操作行（Pause / Clear + 累计帧数）、KPI 行（fps / frame / 1% low /
  work / runtime / viewport）、帧时间图（每帧一根竖条，底部深一档的一段是主循环工作）、
  两行脚注（图例 / 绘制读数）。
- 绘制读数：`draw` / 不透明批次与实例 / 半透明项 / 剔除比例，Play 态再挂粒子数。
  这些是"渲染侧有没有退化"的第一现场（实例化拆批、剔除失效、粒子撞顶），与帧时间
  放在同一块面板里，"慢了"和"为什么慢"才不用来回切面板。

### 四、顺带修的两处（都是截图后才发现的）

1. **`CentralPanel` 之前画状态栏**。此前顺序反了：停靠区先进 `CentralPanel` 把剩余
   矩形全吃掉，之后加的 `TopBottomPanel` 只是**叠在它上面**，于是停靠区底部有 24px
   永远压在状态栏底下。以前没人察觉（Console 是滚动列表，少 24px 看不出来），
   Profiler 的绘制读数整行正好落在那条缝里 —— 截图里"有一行被切掉半截"。这也正是
   egui 文档里写明的那条规则：`CentralPanel` 必须最后加。
2. **脚注高度按字体行高算，且在分配图之前扣掉**。第一版写死 14px（图例被切半行），
   第二版补了行距但仍少算 —— 图占掉 `available_height - 常量`，两行脚注在流式布局里
   是"画到哪算哪"，剩多少算多少。现在用
   `fonts(|f| f.row_height(10.5pt)) + item_spacing.y` 现算两行的真实高度。
3. `xgame-core/particle.rs` 的两个 `impl Default` 改成 `#[derive(Default)] + #[default]`
   —— 新 toolchain 的 `clippy::derivable_impls` 会报，`-D warnings` 下直接失败。

### 五、验证

- `xgame-editor` 单元测试：`profiler` 8 条（环形环绕与时间序、暂停不采也不计数、
  统计口径含 p50/p99、全空闲帧 FPS 报 0 且有限、空窗口全零、单样本分位不越界、
  清空、微秒夹取）、`dock` 2 条（每个 `PanelTab` 变体都有 pane；Profiler 与
  Output Log 同组且默认激活是后者、`activate` 能切过去）、`main` 的截图开关
  组合解析（新增 `--ui-profiler`）。
- clippy `-D warnings` 零告警（顺带修了 `xgame-core` 两条）。
- 截图自检：`--ui-profiler` 在 1280×800 / 1440×900 / 1920×1080 三档 + Play 态出图，
  读图确认三段都在可视区内、读数行没被裁（1280 那档正是发现 24px 缝的那张）。
- 无头与全量测试见收尾 commit 信息（本节写于跑批前，数字以 commit 为准）。

### 六、已知限制（有意为之）

- 只有"帧级"读数，没有函数级采样（要采样栈就得引 profiler crate 或做 `#[profiling]`
  埋点；对"是不是渲染 / 物理慢"这个问题，现有两段已经够定位到模块）。
- 没有帧快照 / 回看（停帧靠 Pause 手动冻结）。
- 图不可点选（不能点某根柱子看那一帧的明细）。
- 单进程内窗口只有 600 帧；要长时段趋势看 `XGAME_FRAME_LOG`（探针仍在，分工见
  `profiler` 模块头）。
- **启动头几帧会拉高 1% low**（实测 release 版首帧 83ms：管线创建 + 字体图集 +
  窗口落定）。这是真实的初始化成本，不做"预热丢样本"来掩盖 —— 窗口滚过去就消失；
  要看稳态数字，等两三秒或按 Clear。

## 三十八、相机实体：`Entity::camera` + 三宿主同源（2026-10-21）

### 一、为什么是它

`crates/xgame-game/src/main.rs` 里那句注释挂了很久：

```rust
// 还没有"相机实体"这个概念，先用固定的轨道相机看向原点。
let eye = Vec3::new(5.0, 5.0, 5.0);
```

后果比"机位不好看"严重得多：**任何玩法都做不出自己的镜头**。第三人称跟随、
第一人称、过肩、切机位、过场摇镜 —— 全都要求"镜头是一个能被脚本推动的物体"。
内容层四件套（角色 / UI / 材质 / 粒子）已经齐了，缺的正是这一块把"能动的东西"
和"能看见它"连起来的地基，所以它是本轮最高优先级。

一次只做一件：**相机实体 + 三宿主接线 + 最小脚本面**。正交投影、多机位叠画、
相机后处理绑定都不做（见"刻意的取舍"）。

### 二、数据层：`xgame-core/src/camera.rs`

- `Camera { fov, near, far, active }`，随场景落盘（`Entity::camera`）。
  `fov` 是**垂直**视场角（度），水平视角由宽高比推出来 —— 宽高比属于视口，
  不属于场景：同一个相机在编辑器视口 / 游戏窗口 / 截图里尺寸各不相同。
- `Camera::sanitized()`：fov 钳 1..179（0°/180° 会让投影矩阵退化）、
  near ≥ 0.001（0 会让 1/near 变 inf）、far ≥ near + 0.001。
  为什么在 core 钳而不是渲染层：这些值是场景数据（人手改 RON / 脚本写 / MCP 写），
  坏值会直接变成"整屏黑"，而**在那之前没有任何地方会报错**。
- `look_rotation(forward, up_hint)`：由"看向哪"反解四元数。退化情形全部返回
  有限值（零向量 / NaN / 与前方向平行的 up 提示），绝不把 NaN 送进 `look_at`。
- **主相机解析只有一处**：`Scene::active_camera_index()` —— 第一个 `active` 的
  相机；全都不 active 就退回场景里第一台；一台都没有 → `None`。
  退回这一步是刻意的：把唯一的相机误点成 inactive 不该让画面变成"没有相机"。

### 三、运行时：`xgame-runtime/src/camera_view.rs`

`CameraView { entity, position, forward, up, fov_deg, near, far }` +
`view_matrix()` / `projection_matrix(aspect)` / `view_projection(aspect)`。

- 位姿取实体的**世界变换**（`Scene::world_transform`）：相机能被父节点带着走 ——
  肩视角挂在角色下面、摇臂挂在上层节点、脚本每帧推着跟随，都是这一条的直接结果。
- 视图矩阵走 `projection::look_at(eye, eye + forward, up)`，**不自己拼**：
  右手系视图空间的镜头朝向是 −Z，而引擎实体本地朝向是 +Z，这一层换算由
  `look_at` 天然给出，`up` 还带着实体滚转（相机歪着看也正确）。
- 每帧一次线性扫描、**不缓存**：脚本可以在任意一帧改相机的 `active` / 变换，
  缓存只会引入"切了机位但画面还是上一台"的时序 bug。

### 四、三宿主接线（同源是本轮的核心纪律）

| 宿主 | 改动 |
|---|---|
| 独立游戏 `xgame-game` | 机位从 `world.camera_view()` 取；场景没有相机才退回旧的 (5,5,5) 兜底 |
| 编辑器 Play `scene_view` | **Play 态视口用游戏相机**（Unity 的 Game 视图语义）；自由相机让位（右键拖拽早退），HUD 角标 `Cam` → `GameCam`，渲染的相机位置统一分量也换成游戏相机 |
| 无头 `--screenshot` | 优先级 **显式 `--camera-pos` → 场景相机 → 自动取景**；报告新增 `screenshot.camera.mode`（`explicit` / `scene` / `auto`） |

无头那一路的动机是"截图能当验收证据"：场景里已经有相机，截图却拿自动取景顶替它，
人和智能体都看不出玩家实际看到什么。

### 五、脚本面（6 个）

`look_at` / `look_at_entity`（朝向，任何实体都能用 —— 炮塔转向、NPC 盯人）、
`active_camera` / `set_active_camera`（切机位）、`camera_fov` / `set_camera_fov`（变焦）。
外加 `entity_forward(ctx, id)`：跟随相机"绕到背后"要拿别人的朝向。

两个语义决定：

1. **`look_at` 只改旋转、不改位置**。跟随相机是"先摆位置、再看向目标"两步；
   合成一个方法会让相机每帧被拽到目标旁边。
2. **`set_active_camera` 是独占式的**（其它相机全关）。解析规则是"第一个 active"，
   只置 true 的话第二台永远轮不到 —— 作者会以为切换 API 坏了。

### 六、刻意的取舍

- **不做正交投影**：等真出现塔防 / 等距玩法再加 `projection` 枚举。现在加只会让
  Inspector 多一个永远默认的字段、视图矩阵多一个分支。
- **不做多机位叠画 / 分屏**：单帧只渲染一台主相机。Unity 那种"多相机按 depth 叠画"
  要改 ScenePipeline 的渲染循环与清屏策略，量级是另一刀。
- **相机不进 `Scene::lights` 那种场景级数据**：光属于环境，相机是物体。
  做成场景级就丢了 Transform 层级（`Light` 已经付过这个代价：点光位置只能手填）。
- **样例相机不做"绕到背后"**：实测从正后方看，场景正中的箱塔正好挡住狐狸
  （截图为证）。样例改用固定世界偏移，并在脚本注释里给出两行改法 ——
  把"为什么不用更漂亮的写法"写在内容里，比让人自己猜好。
- **近 / 远裁剪面在截图路径不生效**：截图按场景包围盒推裁剪面，避免机位参数把
  整场裁掉；fov 与朝向照用。

### 七、顺手修的两处（都是做这一刀时暴露的）

1. **Inspector 的旋转用 XYZ 分解，脚本用 YXZ**。同一台相机面板读 153°/−46°/160°，
   `rot(ctx)` 读 −130.6°/18° —— 两边都是"对的"，但没人能对上。统一到 **YXZ**
   （与 `rot` / `set_rot` 一致），相机这类物体因此显示人类可读的 yaw / pitch。
   顺带把 −0.000 抹成 0（四元数分解的负零，看着像算错）。
2. **新建项目的 `Main Camera` 只是个名字，没有 `camera` 组件**。于是新项目一按
   Play 就退回兜底机位，用户会以为"相机没生效"。`default_scene()` 现在带上组件。

### 八、验证

- 新增 **24 条回归测试**（总数 640 → **664 passed / 7 ignored**，全绿）：
  - core 13 条：`Camera` 默认值 / `sanitized` 钳制（含 NaN / inf）/ `look_rotation`
    四方向 + 垂直俯视退化 + 零向量与 NaN；场景层 RON 往返、无相机不写行、
    旧场景无此字段照常加载、`camera` 参与相等判定；主相机解析（active 优先 /
    全 inactive 退回 / 一台都没有）。
  - runtime 9 条：沿本地 +Z 看（含"面朝 +Z 时世界 +X 落在画面左侧"这条方向约定）、
    绕 Y 转 90° 后朝向、**父节点带动的肩视角**、inactive 跳过、fov 变宽点向中心靠、
    坏参数不产出 NaN 矩阵、正下方俯视矩阵有限、aspect 0 / NaN 兜底。
  - script 1 条：Lua 侧 `look_at_entity` 真的把 +Z 转向目标、
    `set_active_camera` 独占、`camera_fov` 读回。
  - editor 1 条：截图机位优先级（场景相机 vs `--camera-pos`）与 `camera.mode` 回写。
- 端到端截图：`--headless --frames 60 --screenshot` 的报告 `camera.mode == "scene"`、
  `eye == (4.2, 2.8, 5.4)`、画面就是编辑器 Play 里看到的那一帧。
- **"脚本真的在驱动相机"的独立证明**（`eye` 与静态变换刚好重合时，上面那条看不出
  脚本到底跑没跑）：把场景里 Main Camera 的 `position` 临时改成 `(0,0,0)` 再截一次，
  报告 `eye` 仍然是 `(4.2, 2.8, 5.4)` = 狐狸世界坐标 `(0,0,1.8)` + 脚本偏移
  `(4.2,2.8,3.6)`，`target` 落在 `eye + forward` 上。改完立刻还原（`md5sum` 对过）。
  两张截图**逐字节相同**（md5 `27fe26a8…`）—— 静态 `position` 对最终画面零影响，
  正是"脚本每帧覆写"的直接结果。
  这一步同时验证了 `guid://` 形式的脚本引用能被解析（场景里手写实体走的是 GUID）。
- 发布包复检：`dist\xgame-editor.exe --check dist\projects\SampleScene` 通过；
  `dist` 版 `--headless --screenshot` 同样报 `mode == "scene"`。
- clippy `-D warnings` 零告警。
- UI 自检：`--ui-select "Main Camera" --ui-size 1920x1240` 读到 Camera 区块
  （FOV 60 / Near 0.100 / Far 1000 / Active）与 Rotation 18.025°/−130.602°/0°；
  `--ui-play --ui-size 1280x800` 读到 HUD `GameCam 4.2 2.8 5.4` + "Scene locked" 横幅。
- clippy `-D warnings` 零告警。

### 九、遗留（明确不做，不是遗漏）

- 正交投影、多机位分屏 / 叠画、相机抖动 / 后处理体积绑定、`Camera` 的层遮罩（culling mask）。
- 编辑器里画相机视锥体 gizmo（选中相机时看得见范围）—— 便宜的体验改进，但与机位无关，
  留给编辑器产品化那几刀。
- 无头截图忽略场景相机的 `near/far`（见上）。

## 三十九、编辑器偏好 + 面板布局持久化：一份 `editor.ron`（2026-10-22）

### 一、为什么是它

差距分析 P1 #8，也是上一轮 Profiler 明确留下的另一半：内容层与相机收口之后，
编辑器产品化最刺眼的两件事就是"每次启动布局都没了"和"相机手感 / 网格开关
只能改代码"。这两件都是纯编辑器侧的落盘，与帧读数无关，所以上一轮没有硬塞进
Profiler 那一刀。

本轮一次只做这一件：**Preferences（相机速度 / 灵敏度 / 网格）+ 完整停靠布局**。
主题选择、`gizmo_snap`、多窗口都不做（见"刻意的取舍"）。

### 二、配置目录与 `editor.ron`

新增 `crates/xgame-editor/src/config.rs`：统一解析用户配置目录
（Windows `%APPDATA%\XGameEngine\`，其它平台 `XDG_CONFIG_HOME` / `~/.config`），
并提供 `XGAME_CONFIG_DIR` 整体覆盖。`recents.rs` 改为复用它，行为不变。

`EditorPreferences`（`prefs.rs`）只有四个字段：

- `camera_speed`（自由相机平移速度，世界单位/秒）；
- `camera_sensitivity`（右键转视角灵敏度）；
- `show_grid`（视口网格默认开关）；
- `layout: Option<egui_tiles::Tree<PanelTab>>`（`None` = 从没保存过）。

全部 `#[serde(default)]`，以后加字段时老文件照常能读。读入后先 `sanitized()`：
非有限值（NaN / Inf）直接换默认值，有限值钳到相机常量定义的区间 —— 这些值会
直接进相机每帧计算，坏 f32 的表现是"相机不动 / 速度显示 NaN"，在数据入口挡住
是唯一能保证坏值不出现在界面上的位置。

**为什么单独一份文件**：最近项目只是历史记录，坏了丢了无感；布局坏了会让人
一打开编辑器就找不到面板。合并成一份后，一次布局序列化失败会连最近项目一起丢。

### 三、布局校验与自愈

`dock::is_usable_layout` 在恢复前检查四项：

1. 根存在，且从根能走到的每个 tile 都真实存在；
2. 没有环 / 重复引用（同一个 tile 不能走到两次）；
3. 没有不可达的孤儿 tile；
4. 每个 `PanelTab::ALL` 变体恰好出现一次。

少一个面板就永远打不开它；多一个 / 悬空 / 重复则可能让 `egui_tiles::Tree::ui`
panic。校验不过时记 warning、本次用 `dock::create_tile_tree()` 继续跑，**不阻塞
启动**；`layout_for_startup` 不修改 `self.layout`，下一次保存触发会用实际使用的
布局覆盖它，文件自愈。

恢复后 `dock::normalize_layout` 用固定树 id `"editor_tree"` 重建 `Tree`：
`egui_tiles` 把这个 id 当页签 / 分隔条状态的 key，旧文件里的 id 可能不同或被人
手改过。重建只花几个分配，换来状态一定挂在当前 id 下。

### 四、窗口与保存时机

- 工具栏新增滑块图标（`Icon::Sliders`，与项目设置的齿轮区分），点开 Preferences。
- 数值走**草稿 + Apply / Cancel**：拖数值框时不改运行时状态、不写盘。
- 「恢复默认布局」立即重建出厂布局并写盘；窗口不关，用户可继续调数值。
- Apply 时布局以**当前树**为准（窗口打开期间用户可能又拖了页签）。
- 保存触发点：指针松手、Apply、Reset、返回枢纽 / 关闭项目、`on_exit`。
  `sync_layout` 相同就返回 `false`，调用方不写盘 —— 同一棵树反复同步不重复写；
  第一次运行时布局是 `None`，第一次松手 / 关闭会把实际使用的布局写下来。
- 写盘先 `editor.ron.tmp` 再 rename；占用导致 rename 失败时退回直接写。

### 五、截图开关

`--ui-shot` 默认**只读**：不读也不写 `editor.ron`，永远截出厂界面（老截图逐像素
不变这条验收才成立）。新增两个显式开关：

- `--ui-restore-prefs`：截图时读真实 `editor.ron`（验证恢复 / 网格关闭）；
- `--ui-preferences`：截图前打开 Preferences 窗口。

### 六、验证

- 新增 **15 条回归测试**（总数 664 → **679 passed / 7 ignored**，全绿）：
  - `prefs` 10 条：默认值对齐相机常量、数值钳制含 NaN/Inf、布局 RON 往返后
    七面板齐全、少 Profiler 的老布局回退出厂、坏文件 / 缺文件回默认、保存后读回、
    坏数值读入即夹、`sync_layout` 首次变化第二次无操作、`PanelTab` 变体名即磁盘格式。
  - `dock` 4 条：缺一个面板拒绝、空树 / 悬空根拒绝、同一 pane 重复引用拒绝、
    不可达孤儿 tile 拒绝。
  - `panels::preferences` 1 条：打开窗口时草稿复制当前偏好。
  - `main` 的截图开关解析测试补 `--ui-preferences` / `--ui-restore-prefs`。
- 端到端（`XGAME_CONFIG_DIR` 指向临时目录，避免碰真实配置）：
  - 首次 `--ui-restore-prefs` 退出后生成 `editor.ron`，布局含七个面板；
  - 手改 `camera_speed: 12.5` / `show_grid: false` / 根分隔条 `0.65 / 0.35`，
    恢复运行的截图里左栏变宽、网格消失；出厂截图仍是默认布局 + 网格；
  - 把根 tile 改成不存在的 id `(999)`：恢复运行退出码 0、无 panic、用出厂布局，
    文件被自愈回七个面板。
- 截图自检：`--ui-preferences` 在 1280×900 / 1440×900 / 1920×1080 三档出图，
  窗口完整、无溢出；工具栏滑块图标与齿轮可区分。
- clippy `-D warnings` 零告警。

### 七、刻意的取舍

- **主题选择不做**：当前只有固定深色主题，加一个只有一项的下拉框没有意义。
- **`gizmo_snap` 不进偏好**：字段存在但还没有任何逻辑消费，存一个永远不生效的
  值只会误导人。
- **布局全局共享，不按项目存**：同一台机器换项目仍用同一套面板位置；按项目存
  会让"我习惯的布局"在每个新项目里重新丢一次。
- **不做关闭 / 打开面板**：七个面板是产品定义的一部分，校验要求全部存在。
  真要做可关闭面板，必须先放宽校验并定义"面板关闭后布局怎么表示"。
- **Cancel 不回滚布局**：布局是直接拖动产生的，回滚需要额外维护快照；
  「恢复默认布局」已覆盖"弄乱了想重来"。数值才走草稿 + Cancel。

### 八、遗留

- 主题选择、快捷键自定义、`gizmo_snap` 接线。
- 布局按项目 / 工作区保存（当前刻意全局）。
- 多窗口 / 面板独立成窗（egui 单窗口，成本高）。
- 偏好里还没有视口 FOV / 正交预设 / 自动保存间隔等后来新增的编辑器设置。

## 四十、多场景编辑：当前场景 / 启动场景分离 + 全场景 `--check`（2026-10-22）

### 一、为什么是它

差距分析 P1 #9。上一轮 Preferences 收口后，编辑器产品化最堵的一处就是
**项目只有一张场景**：`save_scene` 永远写 `entry_scene`，打开第二关再按 Ctrl+S
会把入口场景覆盖掉。这不是"少一个菜单"，是**会静默毁内容**的语义缺陷；只要项目
里出现第二张地图，它就会咬人。

本轮一次只做这一件：**多场景编辑的最小闭环**。运行时 `load_scene` 脚本 API、
多场景同时打开、构建场景清单 / 排序 / 平台预设都不做（见"刻意的取舍"）。

### 二、语义：`current_scene` 与 `entry_scene` 拆开

| 动作 | 写哪个文件 | 改启动场景 |
|---|---|---|
| Save / Ctrl+S | `current_scene` | 否 |
| Save As | 新文件，并切过去 | **否**（旧语义会改，本轮刻意改掉） |
| New Scene | 新建 `Scenes/<name>.ron`，并切过去 | 否 |
| Set Current as Startup | `project.ron` | 是 |
| 打开另一张场景 | 只读 | 否 |

- `current_scene` 是编辑器状态（项目相对路径，不进 `project.ron`）；
- `entry_scene` 是发布配置：构建、独立游戏、下次打开项目都读它；
- **另存为不再等于"设为启动场景"**：旧语义把"做副本"和"改发布配置"绑在一起，
  用户想备份关卡时会悄悄改掉游戏入口。要在新场景启动，必须显式点菜单项。

新建场景写 `default_scene()`（主相机 + 方向光），立刻落盘并切换；同名文件已存在
时**拒绝而不是覆盖**。启动场景只由 `set_startup_scene` 修改，写盘失败会回滚内存值，
不让界面显示一个磁盘上不存在的启动场景。

### 三、场景列表与路径约束

场景列表**现扫磁盘** `Scenes/*.ron`（复用 `ProjectHandle::list_scenes()`），不新增
`project.ron` 字段：用户在资源管理器里加 / 删关卡，菜单下次打开就能看到，不存在
"清单和磁盘两份真相源"。入口场景与当前场景即使不在 `Scenes/` 下也会补进列表。

所有场景路径统一过 `normalize_scene_relative`：

- 拒绝绝对路径（`/abs`、`C:`、UNC）与任何 `..`；
- 只收 `.ron`，排除 `.prefab.ron`；
- 新建 / 另存为的名字再拒绝路径分隔符与 Windows 非法字符。

设置窗口的 `entry_scene` 也走同一套校验（此前只检查非空）。清单是手改文本，
不能让一个 `../` 把构建启动路径指到项目外。

### 四、UI：Scenes 菜单 + 两个对话框

- 工具栏 `Scenes` 菜单列出全部场景：显示名去路径与 `.ron`，后缀标
  `· current` / `· startup`，悬停给完整路径；当前场景那一项禁用（点了只会白清撤销栈）；
- `New Scene…` 与 `Set Current as Startup` 在同一菜单；播放中整组禁用；
- 有未保存改动时，切换 / 新建先弹「未保存的更改」：保存 / 不保存 / 取消。
  保存失败弹窗保留，错误进 Console；
- 新建场景有命名对话框，预览路径实时校验；非法名 / 同名时窗口不关，改名重试；
- Save As 对话框文案明确"启动场景不变"。

`adopt_scene` 换场景后清空撤销栈、选中、折叠集合、Edit 组件副本、Gizmo 与资源
拖拽状态、Play 快照、脏检查 memo —— 这些状态都属于上一张场景，留着会串味。

### 五、`--check` 覆盖全部场景

旧实现只 `load_entry_scene()`：第二关的断引用 / 坏文件完全看不见。现在：

1. 入口场景先加载；失败仍返回 `Err`（退出码 1，启动契约断了）；
2. 遍历 `list_scenes()`（去重入口）：非入口场景加载失败记
   `invalid_scenes`，检查继续跑完（退出码 2）；
3. 每张场景的模型 / 脚本 / 音频 / UI 图片 / 预制体 / 材质引用都检查；
4. 问题消息统一加 `<场景路径>: ` 前缀；材质按路径**跨场景**去重；
5. `entities` / `lights` 改为全部场景合计，新增 `scenes` 字段；MCP
   `project_check` JSON 同步增加 `scenes` / `invalid_scenes`。

退出码契约不变：`0` 干净 / `1` 加载失败 / `2` 有问题。非入口坏场景属于"有问题"，
不是工具失败。

### 六、验证

- **净增 7 条回归测试**（总数 679 → **686 passed / 7 ignored**，全绿）：
  - editor 6 条（净 +5，替换旧 `save_as` 测试）：切换后 Save 只写当前场景、
    Save As 不改启动场景 / 旧文件不动 / 拒绝同名、新建场景不设启动 / 同名拒绝、
    `set_startup_scene` 落盘后重开仍生效、路径与名字校验、脏时切换只挂起；
  - core 2 条：非入口场景断引用被报且带场景前缀、非入口坏场景记
    `invalid_scenes` 而入口坏场景仍 `Err`。
- `cargo test --workspace --all-targets -j 2`：**686 passed / 0 failed / 7 ignored**。
  （默认并行度在本机触发过页面文件耗尽，`-j 2` 是环境限制，不是测试问题。）
- `cargo clippy --workspace --all-targets -j 2 -- -D warnings`：0 告警。
- `--check projects/SampleScene`：`scenes=1 entities=24 ...`，OK。
- 端到端多场景检查（临时副本，不碰仓库）：加一张坏 `Level2.ron` → 退出码 2、
  `invalid scene: Scenes/Level2.ron (...)`；换成可解析但引用缺失模型的副本 →
  `scenes=2 entities=48`，8 条 `missing model: Scenes/Level2.ron: ...`（带场景前缀）。
- `--headless --frames 60 projects/SampleScene`：`ok=true`、24 实体、60 固定步、
  0 非有限变换（`frame_us` p50 27.2 / p99 1394.2）。
- UI 自检：`--ui-shot` 在 1280×800 / 1440×900 / 1920×1080 三档出图，
  `Scenes` 菜单与 Save / Save As 同排不溢出；截图用于确认工具栏改动没有破坏布局。

### 七、刻意的取舍

- **Save As 不改启动场景**：见第二节。要改必须显式 `Set Current as Startup`。
- **新建场景不自动设为启动场景**：新建关卡 ≠ 游戏从这里开始。
- **场景列表不进 `project.ron`**：磁盘是唯一真相源，外部增删立刻可见；
  构建清单 / 排序留给构建设置。
- **同名拒绝覆盖**：一次手滑不该毁掉一张关卡；要覆盖请先删文件（或以后做显式确认）。
- **播放中禁用场景操作**：`self.scene` 是运行时副本，切场景会丢 `play_snapshot`。
- **不做多开 / additive 编辑**：当前模型是一张文件一张场景；多开要重做保存与
  场景所有权语义，不是加个页签的事。
- **不做运行时 `load_scene` 脚本 API**：编辑器换文件与运行时换关卡是两件事，
  资源驻留 / 实体生命周期都要单独设计。

### 八、遗留

- 运行时 `load_scene` 脚本 API（关卡切换 / 过场）。
- 构建场景清单与排序、平台预设、图标 / 版本号。
- 场景重命名 / 删除的专用 UI（当前交给资源管理器）。
- 多场景同时打开 / additive 编辑。
- 自动保存 / 崩溃恢复（P1 #14）。

## 四十一、级联阴影（CSM）：4 层深度数组 + 视线深度选层（2026-10-22）

差距分析 P1 #11。上一轮的单级拟合按**整场景包围盒**定视锥，相机一拉远，每个像素
对到的纹素就少一大截，影子边缘糊成噪点。这一轮把它换成 4 级级联。

### 一、切分：视线方向的线性深度 + practical split

- `splits[i]` = 第 i 层远平面距相机的**线性**距离（沿视线），不是 NDC z ——
  NDC z 在透视下被非线性压缩，按它切出来的段在世界空间里不成比例。
  选层口径与 CPU 拟合口径是同一个标量：`dot(world_pos - camera_pos, forward)`。
- 切分用 practical split（λ = 0.7，对数项与均匀项混合）：纯对数第一层装不下
  脚下地面，纯均匀浪费近处精度。
- **覆盖距离 = min(相机远平面, 场景在视线方向的延伸, 120)**。编辑器远平面常是
  1000，照它切等于把 99% 的纹素花在看不见的地方；120 是硬上限，防"物体飞出
  视野"把最后一级拉糊。最后一级的远平面恰好等于覆盖距离（少一点点会留一圈
  "没影子"的环）。

### 二、每层拟合：反推视锥 + 紧致包围 + 纹素吸附

- 从 `inv_view_proj` 反推视锥 8 角点，**不额外传 near/far**：三套宿主（编辑器
  自由相机 / 游戏相机 / 无头截图）各自组装矩阵，多传一份就有"传的与用的不是
  同一组"的风险，错了只表现为阴影错位。
- 子视锥 = 近角点与远角点的线性插值（透视下与解析式恒等，正交下棱边平行）。
- 逐轴求**紧致包围**（不用外接球：子视锥是梯形，球比它胖）；x/y 取同一尺寸
  （正方形范围），PCF 的 3×3 核才不会被拉长。
- 宽度向上取整到整数纹素、中心吸附到纹素网格 —— 否则相机一动影子边缘就抖
  （shimmer）。
- 深度范围的符号是这一轮唯一一次"写对了才不黑屏"的坑：`look_at_rh` 视空间前方
  是 -z，而 glam 的 `orthographic_rh` 要正距离，所以 `near = -max.z - pad`。
  写反的后果是整段深度落在 `[0,1]` 外 —— 全屏没有影子。这条由
  `shadow_cascades_cover_their_own_slab`（断言 8 角点 NDC 落在 `[-1,1]×[0,1]`）
  钉住。

### 三、GPU 侧

- 阴影图从单张 `texture_depth_2d` 改成**4 层深度数组**，一层一级联；
  主 pass 用 `texture_depth_2d_array` + `textureSampleCompareLevel(..., layer, ...)`。
- 阴影 pass **每层一个 render pass**：wgpu 不允许把数组视图当单层的渲染目标，
  必须另建单层 D2 视图（采样用数组视图、渲染用单层视图）。
- 每层矩阵走**动态偏移**（`SHADOW_PASS_STRIDE = 256`）切段，而不是四个绑定组：
  省三次绑定组创建，也不依赖 push constant 特性。绑定必须**显式写 size = 64**，
  `as_entire_binding()` 会把绑定钉成整条缓冲、只允许偏移 0（四层全画进同一层，
  表现为影子错位，不报错）。
- 主 pass 的 uniform 结构：`array<mat4x4,4>` + `splits/forward/params` 三个 vec4
  = 304 B；切分表用 `vec4` 而不是 `array<f32,4>`（uniform 里后者的元素步长是 16）。
- 阴影视锥剔除改用**最后一级**（覆盖最广），否则会漏掉"近处看不见、影子落在
  远处"的物体。

### 四、验证

- **xgame-render 全量单测 152 passed / 0 failed**（含 3 条阴影 GPU 回归：
  `instanced_draws_still_cast_shadows` / `offscreen_casters_still_cast_shadows` /
  `instancing_matches_per_draw_rendering`）。
- **全工作区 `cargo test --workspace --all-targets -j 2 -- --test-threads=4`：
  696 passed / 0 failed / 7 ignored**（上一轮 686，净增 10）。
  环境备注：本机沙箱不允许往仓库内 `target/` 写代码生成产物（OS Error 5），
  本轮改用 `CARGO_TARGET_DIR=D:\xgame-target` 构建；`plugin_loading` 那 4 条
  按设计去 `<repo>/target/{debug,release}` 找 cdylib，把两个插件 DLL 复制过去后
  全绿 —— 是构建目录约定，不是代码问题。
- **新增 9 条**（纯 CPU，无需 GPU）：
  - `cascade_splits`：单调、末项恰好 = far、近处一级明显短于均匀切分、
    near ≥ far 的坏相机不 panic；
  - `shadow_cascades_cover_their_own_slab`：每层子视锥的 8 角点必须落在该层的
    光源裁剪空间内（NDC xy ∈ [-1,1]、z ∈ [0,1]）—— 这是"影子能出现"的前提，
    也是上面那个符号坑的守门员；
  - `shadow_cascades_sharpen_the_near_field`：第 0 层纹素必须细于单级拟合的一半，
    且远端层确实更粗（切分与拟合没接反）；
  - `shadow_cascades_are_deterministic`：同输入两次拟合逐位相同（无头截图
    "同机逐字节一致"的地基）；
  - `shadow_cascades_reject_degenerate_input`：空场景 / 层数 0 / 退化逆矩阵给
    `None` 而不是 NaN 矩阵；
  - `shadow_cascades_survive_a_zero_direction`：零方向退化成垂直向下；
  - `shadow_coverage_follows_the_scene_not_the_far_plane`：40 单位的地面不被切到
    200 单位的远平面上，同时仍罩住整个场景；
  - `shadow_uniform_matches_wgsl_layout`：304 B 且 16 对齐、动态偏移步长与层数
    一致性（ABI 错了只会表现为"影子在奇怪的位置"）。
- clippy `-p xgame-render --all-targets -- -D warnings`：0 告警。
- **无头截图对比**（`--headless --frames 150 projects/SampleScene`）：与改动前
  基线逐区域一致，无接缝、无黑带、无消失的影子；另外跑了 `BehaviorsDemo` /
  `CharacterDemo` 两个项目，均正常出图、`ok=true`。

### 五、刻意的限制 / 遗留

- 不做级间混合（blend band）：边界是"锐度突变"而非接缝，混合要一次额外采样，
  留给软阴影那一刀。
- 不按光源方向扩展剔除视锥去抓"画面外投影物"（盒外物体的影子本来也落不进这张图）。
- 层数写死 4（常量 + WGSL 数组长度 + uniform 布局三处一致，有测试钉住）；
  还没接到质量档位（P2 #23）。
- 单级拟合 `fit_directional_shadow` 保留：关掉 CSM 的等价退路 + 精度账的基准。
- 点光阴影 / 软阴影（PCSS、VSM）仍在 P2 #20。

## 四十二、自动保存 + 崩溃恢复：`.autosave/` 副本 + 恢复框（2026-10-22）

差距分析 P1 #14，也是「编辑器产品化」这一条线剩下的最后一处硬缺口（Profiler /
Preferences / 布局持久化 / 多场景编辑都已落地）。它不是能力差距，而是**风险**：
崩溃即丢全部未保存工作，`Ctrl+S` 是唯一防线。

### 一、存哪儿：难点不在"写"，在"不污染"

第一反应是在 `Scenes/` 下写 `*.autosave.ron`，但 `ProjectHandle::list_scenes` 是
**递归扫描**这个目录的。副本一旦落在那里，会同时污染三处：

| 消费点 | 后果 |
|---|---|
| Scenes 菜单 | 多出一条"未命名副本"，点一下就打开它 |
| `--check` | 把副本当场景校验 —— 一次崩溃残留就能让 CI 变红 |
| `--migrate-assets` | 把副本当场景改写引用 |

所以最终落在 `<项目根>/.autosave/<场景相对路径>`：项目根的点目录，场景 / 预制体 /
资产三条扫描都碰不到。为此专门写了两条测试钉住（`autosave_stays_out_of_the_scene_list`
与 core 侧 `autosave_roundtrips_and_stays_out_of_the_scene_list`）——**只看"文件写了"
是不够的**，还得断言"场景清单没多东西"。

顺带补了新建项目的 `.gitignore`（`Build/` / `Plugins/` / `.autosave/`）。原本
`ProjectHandle::create` 什么都不写，用户第一次 `git add .` 就会把 DLL 和副本一起提交。

### 二、"自动保存 ≠ 保存"是这一轮最重要的一条

写副本之后 `saved_snapshot` 一个字节都不动：星号还在、关窗照样要确认、
`save_scene` 语义不变。反过来做（写副本就当已保存）会在两个地方立刻出问题：

1. 用户以为落盘了，实际磁盘上是两小时前的版本；
2. 关窗时被放行，而那份"已保存"的内容其实还躺在 `.autosave/` 里。

同理，**手动保存成功后要删副本**。这不是省磁盘：留着它，下次启动会问"要不要恢复
两小时前那一份"，用户点"恢复"就把刚存的东西盖回去了。

### 三、调度：时间注入 + 空闲唤醒

`AutoSave`（`crates/xgame-editor/src/autosave.rs`）只做计时，不碰 IO：

- `new(interval, now)` / `due_at(now)` / `mark_attempt(now)` / `mark_success(now)`；
- 时间全部**由参数注入**，单测能把时钟拨快三分钟验证间隔语义，不用真等；
- 干净时到点也 `mark_attempt` 推进计时器（"改一笔之后最多等一个间隔"），
  `scene_dirty()` 拦在写盘那一步。

真正容易漏的是**空闲停帧**：界面静止后 `frame_policy` 会停帧，`update()` 不再被调用，
`autosave_tick` 也就没人问。所以 `AutoSave::wait(now)` 把剩余时间交给主循环，在
`!live` 时 `request_repaint_after(wait)` —— 准点醒一帧再睡。没有这一步，自动保存
只在"用户一直拖着鼠标"时触发，恰好是最不需要它的时候。

### 四、恢复：探测 ≠ 自动载入

`EditorState::new` 只探测入口场景有没有副本（`pending_autosave`），首帧由
`dialogs::show_autosave_recovery_dialog` 弹三选一：恢复 / 丢弃 / 稍后。
自动恢复被刻意排除 —— 在用户没同意的情况下把场景换成一份旧现场，比丢改动更让人困惑。

恢复路径有个细节：`adopt_scene` 会把 `saved_snapshot` 设成刚载入的场景（= 干净），
所以之后要**单独覆盖回磁盘版本**，否则恢复完的场景会被标成"已保存"，用户直接关窗
就真丢了。恢复后还 `schedule_now()` —— 恢复出来的内容尚未进任何副本，中间再崩一次
就白恢复。

### 五、验证

新增 21 条测试（core 3 + editor 18）。**新模块没有"旧实现"可对照**（类型还不存在，
编译即红），所以这里的"红"是初版断言真的失败过三次，逐条修的是断言而不是代码：
  - 第一条草稿让"干净时到点"和"脏时到点"共用一个时刻，被"干净也推进计时器"的语义
    打败 —— 断言写错而不是代码写错，改成按真实时刻序列 `t0+60 / t0+100 / t0+121`；
  - `enter_play_mode()` **不设** `play_mode`（那是工具栏的活，主循环补调），测试要自己
    推上去，否则"Play 态不写"这条测的是编辑态；
  - `ProjectHandle::create(base, name)` 的 root 是 `base/name`，重开项目要用
    `state.project.root()`，不是 `base`。
- 实机：造副本后 `--check` 仍 `scenes=1 / exit 0`；`--headless --frames 30` 正常；
  `--ui-shot` 截图确认恢复框（文案 / 三个按钮 / 相对时间"17 秒前"），截图后副本
  字节数未变 —— 只读约束成立。
- 全工作区 `cargo clippy --all-targets` 0 告警；全量测试
  **717 passed / 0 failed / 7 ignored**（上一轮 696，净增 21），
  `-j 2 -- --test-threads=4`。

### 六、刻意的限制

- 只探测**入口场景**；切场景不弹框（副本仍在盘上，Console 提一句，等它作为启动场景
  打开时再问）。
- 单一副本，不做时间线 / 多版本。
- 不做"自动保存后自动清空撤销栈"之类的联动。

## 四十三、运行期场景切换：`load_scene` + `current_scene` + 三宿主接线（2026-10-22）

差距分析 P1 #9 的剩余项（"无运行时 `load_scene`"）与 #13 的一个脚本 API 缺口。
挑它而不是 TAA 的理由很直接：**渲染只差一刀观感，而缺了它，游戏连"菜单 → 关卡"
都写不出来** —— 一张图跑到底不是引擎能力的边界问题，是"能不能做出一款游戏"的问题。
成本也可控：`ProjectHandle::load_scene`（含 GUID 解码）早就存在，缺的是"什么时候换"
与"换的时候清什么"。

### 一、装载来源：注入，而不是让 `World` 认识项目

`World` 的既有契约是"一份场景 + 一个根目录"，测试 / 无头 / 独立游戏都按这个契约
构造它。让 runtime 直接持有 `ProjectHandle` 会把"项目布局"焊进运行时；而让
`World` 自己读 RON 又会漏掉 GUID 解码（磁盘存 `guid://`、内存存路径，边界在
`ProjectHandle`）。所以是一条 trait：

```rust
pub trait SceneLoader { fn load_scene(&self, relative: &str) -> Result<Scene, String>; }
impl SceneLoader for ProjectHandle { .. }        // 真实路径
impl<T: SceneLoader + ?Sized> SceneLoader for Arc<T> { .. }
```

宿主 `with_scene_loader(project.clone())` —— 句柄共享资产索引，clone 不重扫盘。
**没接装载器时返回错误而不是静默 no-op**：静默会让作者以为脚本写错了。

### 二、切换时机：只有一处，且在帧末

脚本 `load_scene()` → `ScriptCommand::LoadScene` → `LuaHost::take_commands` 聚合
（一帧多次只留最后一个）→ `World::pending_scene_load` → `apply_pending_scene_load`
在 `update` 里"消费脚本通知"之后、`sync_bodies` 之前应用。

三条理由写进了代码注释与决策记录 §二十五：钩子期间场景被可变借用；固定步长循环
中途换场景会让 `pull_dynamic_bodies` 把旧刚体位姿写进新场景；再往后挪就赶不上本帧
的粒子 / 音频结算。固定步长循环里也会调到 `consume_script_commands`，所以那里
**只登记不应用**。

### 三、清什么：按"是否与实体绑定"划线

清：物理刚体与 `body_of_entity`、`character_contacts`、脚本 `started` / `states`、
粒子、`audio_autoplayed` 与扫描计数、UI 布局与交互、组件实例。
不清：脚本编译缓存、路径驻留表、**日志与命令缓冲**、脚本模块级全局变量。

第三条踩了一次：第一版直接调 `LuaHost::reset_runtime_state()`，它连日志缓冲一起清 ——
切换那一帧的日志（含 A 场景的 `start:Scenes/A.ron`）全没了，而那恰好是最该留下的
诊断。测试 `script_can_switch_scene_and_new_scene_starts_fresh` 就是被这条绊倒的，
于是新增 `LuaHost::reset_scene_state()`（只清 `started` + `states`）。
**保留全局变量是刻意的**：Unity 也不在场景加载时重载 domain。

### 四、编辑器 Play：切换不写回项目

`GameRuntime::start` 改为收 `&ProjectHandle`，并 `set_current_scene(当前编辑的场景)`。
Play 里切场景只动运行时那份，Stop 时 `play_snapshot` 整个覆盖回来；Console 会在
Stop 时补一句"Play 期间场景切到了 X"，否则用户会以为切过去的那关丢了。
编辑器与运行时每帧 `mem::swap` 场景，所以换过之后两边**下一帧自动收敛**到新内容 ——
不用额外同步代码，但这条必须在 `game_runtime` 的文档里写清（谁改动帧序时要留意）。

### 五、验证

- **新增 5 条运行时回归测试**（`crates/xgame-runtime/src/world.rs`）：
  实体与刚体确实换掉（3 实体 / 1 刚体 → 3 实体 / 2 刚体）、失败时旧场景原封不动、
  没装载器时留下可诊断错误、脚本 `load_scene` 端到端（帧末生效 + 新场景 `start`
  看到新路径）、来回切换时粒子清空且 `start` 重跑。
- **全量**：`cargo test --workspace --all-targets -j 2 -- --test-threads=4`
  → **722 passed / 0 failed / 7 ignored**（上一轮 717，净增 5）；
  `cargo clippy --workspace --all-targets` → **0 警告**。
- **CLI 实跑**（临时双场景项目：从 `projects/SampleScene` 复制，加一张
  `Scenes/Level2.ron` 与一个"第 5 帧切过去"的脚本；验证完即删，不进仓库）：
  - `--check` → `scenes=2 entities=48 ... check: OK`，退出码 0（副本场景不会被
    误当资产 / 不污染清单）；
  - `--headless --frames 20` → `scene_loads: 1`、`scene_current: "Scenes/Level2.ron"`、
    `logs: {info: 3, warn: 0, error: 0}`、`ok: true`、退出码 0 ——
    三条 info 分别是 `switch-start:` / `switch-request:` / `场景已切换 →`；
  - `cargo run -p xgame-game -- <项目>`（独立游戏宿主，窗口化实跑）：
    stderr 依次出现 `switch-start:Scenes/scene.ron` → `switch-request:Scenes/scene.ron`
    → `运行时: 场景已切换 → Scenes/Level2.ron（实体 24 → 24）`，之后继续正常渲染
    （手动结束进程，未观察到崩溃或串味）。
  - **编辑器 Play 也验过**（`dist\xgame-editor.exe --ui-play --ui-frames 30 --ui-shot`）：
    截图里工具栏右侧显示 `Level 2 · 24 entities`、Console 有
    `[运行时] 场景已切换 → Scenes/Level2.ron（实体 24 → 24）`，视口正常出图；
    把脚本的触发帧改成永不触发再截一张，两张图 **md5 不同**（729 266 B vs 708 262 B）——
    说明视口渲染的确实是**切换后**的场景，不是"只改了状态栏"。
    （Level2 特意把地面抬高了 3 米，否则两份拷贝的截图看不出差别。）
  - 把脚本改成加载不存在的场景 → `ok: false`、退出码 **2**、
    `error_samples` 里是 `切换场景 'Scenes/Missing.ron' 失败：无法读取 '...'（os error 2）`，
    且实体数仍是 24（旧场景照常跑）。
- **契约**：无头报告新增 `scene_current` / `scene_loads`，`json_contract_has_stable_keys`
  同步（新增字段允许，改名 / 删除不行）。

### 六、刻意的限制

- **不做 `load_scene` 的"带参数"版本**（如把数据传给下一关）：状态传递该走全局
  单例脚本 / 存档，那是另一个设计。
- **不做异步 / 流式加载**：一次同步读盘，大场景会掉一帧 —— 与场景流式加载
  （P3 #37）一起做才有意义。
- **不做场景栈 / 回到上一关**：`load_scene` 是"换掉"，不是"压栈"。
- **构建仍按 `entry_scene` 启动**：场景清单 / 排序属"构建设置"那一刀（P1 #9 剩余）。
  好在 `build` 本来就随 `content_dirs()` 打包整个 `Scenes/`，所以运行期切到任意
  场景在打包后的游戏里同样成立。**独立游戏宿主已实跑验证**（`cargo run -p xgame-game
  -- target/e2e_switch`，日志里 `switch-start:` → `switch-request:` → `场景已切换 →
  Scenes/Level2.ron（实体 24 → 24）`，进程继续正常渲染直到手动结束）；
  **`dist/` 里的 release 产物没跑双场景**（本轮只重新打包，没有窗口化验收）。
- **没给样例项目加第二关**：`projects/SampleScene` 被大量测试与截图钉着，
  往入口场景塞实体 / 加场景会牵动它们的基线。想做演示的话，建议单独加一对
  `Menu.ron` + `Level1.ron`，再配一个"按 E 切场景"的脚本 —— 下一轮的事。

## 四十四、构建设置：关卡顺序即索引 + 版本号 + 构建前逐关试装（2026-10-22）

上一刀（四十三）让游戏能换场景，但"下一关"还得在每张关卡的脚本里写死路径。这一刀
把**关卡编排**变成数据：`project.ron` 多一个 `build` 节（版本号 + 有序场景清单），
顺序就是关卡索引。差距分析 P1 #9 的"构建设置"到此补掉大半。

挑它而不是 TAA：TAA 是观感最后一刀（一周量级、验证只能靠肉眼/指标），而"发布路径"
缺这一环时，"加一关要改两处、删一关直接断链"是每天都会遇到的麻烦，且这一刀不引入
任何新依赖。

### 一、数据形状

```ron
(
    build: (
        version: "1.2.3",
        scenes: ["Scenes/Menu.ron", "Scenes/Level1.ron"],
    ),
    entry_scene: "Scenes/Menu.ron",
    ...
)
```

`#[serde(default)]`：老清单没有这一节也能读，空清单 = **未配置**（有效顺序为空）。

### 二、为什么是"顺序即索引"

因为脚本要能问"我是第几关"。三个 API（`scene_count` / `scene_index` /
`load_scene_at`）都读这份清单，`load_scene_at(scene_index(ctx) + 1)` 就是"进下一关"。
下标 **0 起**（与 `load_scene_at` 直接配对，不折 Lua 的 1 起）。

`entry_scene` 仍是唯一启动来源，**没有**让 `build.scenes[0]` 兼任（Unity 那么做是
因为它在清单之前没有别的地方记启动场景；本引擎早就有 `entry_scene`，再造一个必然
出现"改了清单第一项、启动图变了但没人知道"）。代价：入口可以不在清单里，此时
`scene_index()` 返回 `nil`，`--check` 只提示。

### 三、`--check` 分两级

| 情况 | 字段 | 退出码 |
|---|---|---|
| 清单指向的关卡不存在 / 路径越界 | `build_missing_scenes` | 2 |
| 盘上有、清单没列 | `build_unlisted_scenes` | 0（提示） |

第二级刻意不算问题：否则每个配了清单的项目在 CI 里永远红，警报就没人看了。
清单里的关卡会被**真正装载**（可以在 `Scenes/` 之外，如 `Levels/L1.ron`），
这样它们的悬空引用也进检查视野；输出多一行
`check: build scenes=2 order=[Scenes/Menu.ron, Scenes/Level1.ron]`。

### 四、构建器：逐关试装 + `--build` CLI

`build()` 在动手前按清单把每一关读一遍，**一次列全**失败项再中止（不留半成品目录）。
理由：构建只是复制目录，清单里的关卡少个文件时构建不会报错，玩家却在切关卡时撞黑屏。

新增 `xgame-editor --build <项目>`（同一个 `builder::build`，不起窗口）：
退出码 0/1，stdout 打 `summary` 与 `scene_count: N`，失败项原样进 stderr。
没有加 MCP 工具 —— `run_headless` 已能验证运行时，"产文件"的动作交给 CLI 更容易
看清产物路径。

### 五、编辑器：⚙ 项目设置 → 构建

版本号输入框 + 关卡列表（`上移` / `下移` / `✕` 移除 + `添加场景…` 下拉）。
用列表而不是逗号分隔的文本框：**顺序本身是语义**，让用户手打顺序等于把 `index + 1`
的 bug 挪进配置。

- Apply 时逐项 `normalize_scene_relative`（越界 / 非 `.ron` 直接挡下）、去重、丢空行；
  入口不在清单里只 `log_warning`。
- 空行不报错：手改清单留空串很常见，报错只会挡住整次 Apply。
- 校验失败的返回从闭包改成关联函数 `EditorState::reject_settings` —— 闭包捕获
  `&mut self` 会把借用锁到最后一个使用点，中间任何 `self.log_warning` 都是 E0501。

### 六、路径规则收进 core

`normalize_scene_relative`（从编辑器搬来）与 `same_scene_path`（0 分配、忽略分隔符）
现在都在 `xgame_core::project`。消费方四个：编辑器改清单、`--check` 校验、构建器试装、
运行时 / 脚本的 `scene_index`。四份拷贝必然漂移成"检查通过、构建成功、运行时打不开"。

### 七、验证

- **新增 13 条回归测试**：core 8（顺序解析 / 旧清单兼容 / 路径规则 / 清单装载与顺序 /
  缺场景与越界 / 未列入提示 / 未配置静默）、runtime 4（脚本按索引推进 / 越界报错 /
  无清单返回 `nil` / 宿主 `load_scene_at`）、editor 1（草稿规范化去重 + 越界在 Apply 挡住）。
- **全量**：`cargo test --workspace --all-targets -j 2 -- --test-threads=4`
  → **735 passed / 0 failed / 7 ignored**（上一轮 722，净增 13）；
  `cargo clippy --workspace --all-targets` → **0 警告**。
- **CLI 实跑**（临时双关卡项目 `target/e2e_build`，从 SampleScene 复制 + `Level2.ron`，
  脚本第 5 帧 `load_scene_at(scene_index(ctx) + 1)`）：
  - `--check` → `check: OK`、`check: build scenes=2 order=[Scenes/scene.ron, Scenes/Level2.ron]`，
    exit 0；加一张未列入的 `Level3.ron` 后多一行 `not in build list`，退出码仍是 **0**；
  - `--headless --frames 20` → `scene_loads: 1`、`scene_current: "Scenes/Level2.ron"`、
    `ok: true`、exit 0（脚本靠**索引**推进，不是路径）；
  - `--build` → `scene_count: 2`、产物含 `Scenes/`，exit 0；
  - 清单加 `Scenes/Missing.ron` → `--check` exit **2**（`build scene missing` 带 OS 错误）、
    `--build` exit **1** 并列出该关，`Build/` 目录**没有**被创建（无半成品）；
  - `projects/SampleScene`（无 `build` 节）→ 不打印 build 行、`--build` 报 `scene_count: 0`。
- **设置面板截图**：`--ui-settings --ui-shot` 两张（全部列入 / 有未列入），
  确认版本号与两行关卡的 ↑↓ 首末项置灰、`添加场景…` 下拉出现。

### 八、刻意没做

- **平台预设**（只有 Windows 一种产物）与**图标**（要图像导入管线，属资产侧）。
- **"禁用某个场景"开关**（Unity 那种勾选框）：当前语义是"列入 = 发布"，删掉即禁用；
  真需要时再加，别提前给清单加三种状态。
- **构建产物裁剪**：`content_dirs()` 仍然整目录打包，没按清单只打需要的资产。
- **MCP 的 build 工具**：见 §四，走 CLI。

---

## 四十五、运行时预制体实例化：`spawn_prefab` / `spawn_prefab_under`（2026-10-22）

### 一、为什么是这一刀

差距分析 P1 #13 的缺口里，"预制体实例化"是唯一一个**卡住玩法闭环**的：
编辑器里能把预制体摆进场景，但脚本不能实例化它 —— 子弹、敌人、掉落物、
技能特效一条都写不出来，玩法永远停在"场景里预先摆好的东西"。相比之下
同列的"手柄"要硬件才能验证、"时间缩放"是几十行的小补面。选它。

### 二、事实基础（动手前确认）

- `Prefab::instantiate()` / `Scene::instantiate_prefab()` 已存在且带测试
  （分配新 ID、重映射父引用、贴 `PrefabInstanceRef`），`ProjectHandle::load_prefab`
  也已存在（读盘 + GUID 解码 + UI / 粒子钳制）—— **缺的只是把两者接到脚本轨**。
- 渲染侧不拦路：编辑器与独立游戏每帧都按 `entity.model` 懒加载网格，
  新实体当帧就能画出来。
- 物理 / 粒子 / 音频都按帧扫或按命令补：新实体不需要额外的"注册"步骤。
- 依赖方向是硬约束：`script ← runtime`，所以脚本要的装载 trait 必须在 core。

### 三、做了什么

1. **`SceneLoader` 从 runtime 搬到 core**（`xgame_core::loader`），新增
   `load_prefab`（默认实现返回 `Err`）；runtime 改 `pub use` 转出，宿主零改动。
2. `FrameCtx` / `GameCtx` 加 `loader` 字段 + `with_loader` 构造器；
   `LuaHost::run_hook` 与 runtime 的四处派发全部注入同一份装载器。
3. `GameCtx::spawn_prefab` / `spawn_prefab_under`：读盘 → 实例化 → 设位置 /
   挂父 → **逐实体**推 `ScriptCommand::EntitySpawned`（运行时据此补刚体）。
4. Lua 注册两个函数，注册名 145 → **147**；失败返回 `nil` + 一条分级日志。
5. 顺带修掉 `dispatch_collision_to` 脚本轨上下文缺 `scene_name` / `scene_order`
   的不一致（此前 `on_collision_enter` 里 `current_scene()` 恒为空串）。

### 四、关键取舍

- **位置是"设置"不是"叠加"**：`spawn_prefab` 覆盖根的世界位置、
  `spawn_prefab_under` 是局部偏移；子节点相对位置原样保留。没有第三种形态。
- **立即生成**（与 `spawn_entity` 同款理由）：`local e = spawn_prefab(...)`
  之后马上 `set_entity_pos(e, ...)` 必须生效。
- **不做缓存**：每次读盘约几十微秒；等"每秒上千发"再加，缓存要处理编辑器
  改文件的失效问题，提前做是拿正确性换一个未发生的性能问题。
- **失败不留半个实例**：三种失败（没装载器 / 读不到 / 空预制体）各自一条日志，
  都返回 `nil`。

### 五、验证（全部实跑）

- **全量测试**：`745 passed / 0 failed / 7 ignored`（新增 10 条：core 2、
  script 5 + Lua 1、runtime 2）；clippy 0 警告。
- **dist 版无头实跑**（`--headless --frames N`，SampleScene 临时挂一个生成器脚本）：
  基础场景 24 实体，每帧两次实例化各 +3（1 帧 → 30、14 帧 → 108），
  实例根 ID 连续不重复，`error: 0 / warn: 0`；
  路径改成不存在文件 → Error 日志带绝对路径、脚本拿到 `nil`、实体数不变、不崩。
- 恢复项目后 `--check` OK（exit 0）、30 帧冒烟 `entities: 24 / error: 0`；
  `dist/` 已用最终源码重新打包。

### 六、刻意没做

- 预制体缓存（见 §四）。
- 实例根传旋转 / 改名（Unity `Instantiate` 的第三、第四个参数）。
- MCP 侧的预制体生成工具（走 CLI / 编辑器）。

### 七、下一步候选

- **TAA**（渲染最后一刀，P1 #10）：历史缓冲 + 运动向量 + 重投影。
- **手柄输入**（P1 #15）：要硬件才能验证手感。
- **构建设置剩余项**：平台预设 / 图标。

## 四十六、时间缩放：`set_time_scale` 一处生效，四个子系统同源（2026-10-22）

### 一、为什么是这一刀

上一轮结尾把"时间缩放"记成"几十行的小补面"，动工前核对代码发现两个事实：

1. `xgame-core/src/time.rs` 里 `time_scale` 字段、`set_time_scale()`、`tick` 里的
   乘法**全都在**，但全仓库 `grep time_scale()` **零个读者** —— 能力存在却不可触达，
   是标准"半截接线"。
2. 它同时是**帧管线的公共分母**：真能设了，物理固定步、脚本 `delta` / `time`、
   粒子、动画钟必须一起跟手。当时游戏宿主的 `anim_driver.tick(dt)` 吃的是**原始 dt**，
   编辑器吃**墙钟** —— 一设缩放就会各走各的。

比起再补一个面板，这类"漏一个子系统就静默走偏"的活更值得先钉死。改动面可控，
且能用无头固定 dt 逐位验证（这正是本轮验证的基础）。

### 二、事实基础（动手前确认）

- `World::update` 已经把 `clock.delta_time()` 传给脚本 `update` 与
  `particles.update(...)`，固定步走 `consume_fixed_update()` 读同一个累计器 ——
  **管线本来就同源**，缺的只是"能设"与"宿主动画钟跟手"。
- `GameCtx` 已有 `loader` 这个 `Option<&dyn Trait>` 注入先例，`clock` 照抄同一形状即可。
- 注册名侧：`lua_api.rs` 123 处 + `ui_api.rs` 26 处 = 149（改前 147）。

### 三、做了什么

1. **core**：`set_time_scale` 补钳制（负→0、>100→100、NaN/∞ 忽略）与 `MAX_TIME_SCALE`
   常量；`tick` 的缩放语义补 4 条测试钉住。
2. **script**：`GameCtx` 加 `clock: Option<&mut GameClock>` + 构造器 `with_clock`；
   `delta` / `time` 文档标注"已缩放"；新增 `time_scale()` / `set_time_scale()`；
   Lua 注册 `time_scale` / `set_time_scale`（注册名 147 → **149**）。
3. **runtime**：`World` 的**五处** `GameCtx` / `FrameCtx` 构造点全部注入时钟
   （含 `dispatch_collision_to` —— 上一轮刚在这里吃过"上下文漏字段"的亏）；
   对外加 `time_scale` / `set_time_scale` / `frame_dt` / `game_time` 四个口。
4. **三宿主**：游戏侧动画钟改吃 `world.frame_dt()`；编辑器 Play 态每帧把
   `game_runtime.time_scale()` 同步给 `SceneRenderer::set_anim_time_scale`
   （编辑器 Pause 写 0、编辑态写 1.0）；`AnimationDriver` 加 `wall_time_scale`
   （`Default` 手写为 1.0，derive 会让预览动画卡在第一帧）；无头报告加 `time_scale`。
5. 文档：脚本API §4.6 扩成小节、无头运行加字段说明、差距分析 P2 #33 结项、
   ROADMAP / README 各一条、决策记录 §二十八。

### 四、关键取舍

- **缩放只在 `GameClock::tick` 一处消费**，不做 `dt * scale` 的二次分发 ——
  多一个分发点就多一次漏掉的机会。
- **`0` 停模拟不停引擎**：`update` 派发照跑、`delta` 为 0，暂停菜单靠这个活着；
  与编辑器 Play 的 Pause（完全不推进 `World`）是两件事。
- **音频与 UI 不缩放**（kira 自带时钟 / 宿主驱动），与 Unity 一致；暂停时 BGM 继续放，
  要不要停由作者决定。
- **不发未缩放时间 API**：多一份读数就多一次选错的机会；代价（暂停菜单的呼吸动画）
  写进决策记录，真需要时再补两个只读函数。
- **动画钟三宿主分别处理**：编辑器不让 `SceneRenderer` 看见 `runtime::World`，
  只同步一个 `f32`；`tick_wall` 先夹 `min(0.1)` 再乘缩放（否则高倍速会把
  "拖动窗口三秒"放大成长跳）。
- **生效在下一帧**：`clock.tick` 在 `update` 最开头，脚本写的缩放只能影响下一帧。

### 五、验证（全部实跑）

- **全量测试**：`755 passed / 0 failed / 7 ignored`（新增 10 条：core 4 / script 1 /
  runtime 3 / render 1 / editor 1）；clippy 0 警告。
- **无头实跑**（release exe + 临时项目：40m 高动态方块 + 三段式脚本）：
  - 三段式（第 5 帧 0.25 倍速 / 第 40 帧暂停 / 第 70 帧恢复实时），跑 140 帧 →
    `fixed_steps: 83`、`total_time: 1.3958`、`time_scale: 1.0`；
    手算 5 + 8 + 70 步、`5/60 + 35/240 + 70/60 = 1.395833` —— **逐位一致**。
  - `--dump-scene` 里方块停在 `39.86034`（自由落体 0.1667s 的解析值 39.864），
    暂停后再跑 110 帧位置不变。
  - 单段暂停版（第 10 帧停、跑 120 帧）→ `fixed_steps: 10`、`total_time: 0.16666667`。
  - `--check` 该临时项目 exit 0（先 `--migrate-assets` 补 meta）。
- **编辑器冒烟**：`--ui-shot --ui-play --ui-size 1280x800` 截图正常
  （GameCam 角标 / 视口 / 状态栏 24px / Profiler 页签都在），视口渲染路径无回归。

### 六、刻意没做

- `unscaled_delta` / `unscaled_time`（理由见决策记录 §二十八.4）。
- 单一实体的局部时间缩放（Unity 也没有；要做是"每实体时钟"，属于另一层语义）。
- 编辑器工具栏的"暂停 / 单步"保持不变 —— 它是调试器语义，不跟 `time_scale` 合并。

### 七、下一步候选

- **TAA**（P1 #10）：历史缓冲 + 运动向量 + 重投影，牵动剔除 / CSM 拟合 / 拾取三处。
- **手柄输入**（P1 #15）：要硬件才能验证手感。
- **动画剪辑切换的脚本 API**（P1 #13 剩下的唯一缺口）：`crossfade` 已存在，
  只差一层暴露。

## 四十七、TAA：抖动只进相机，历史被钳制（2026-10-23）

### 一、为什么是这一刀

`Docs/差距分析.md` 的"未来 10 刀"里只剩 TAA 没做（P1 #10），它之前的每一项都已落地。
动工前核对了三件事：

1. `postfx.rs` 的模块文档里已经写着"等出现 TAA 要历史缓冲的 pass 再抽 RenderGraph"
   —— 说明当时的判断是"TAA 会带来带依赖分叉的 pass"。实际做下来：**不需要抽图**，
   后处理链仍是线性的（TAA 插在场景 pass 与合成 pass 之间），只是多要一张深度纹理。
2. 场景 pass 的投影是宿主算好后传进 `ScenePipeline::render` 的 —— 抖动有地方插，
   但**不能直接改宿主的矩阵**（阴影拟合也读它）。
3. `HdrTargets` 的深度附件没有 `TEXTURE_BINDING` —— 这是唯一的硬阻塞，加一位即可。

### 二、做了什么

1. **core**：`TaaConfig { enabled, history_weight }`（默认关、`is_default` 不落盘、
   `sanitized` 夹 [0, 0.98]），进 `RenderConfig`；新增 `effective_fxaa()`
   （TAA 开着时 FXAA 让位，判定只有这一处）。
2. **render 新模块 `taa.rs`**：Halton(2,3) 抖动序列 / `jitter_to_ndc` / `jitter_to_uv`
   （y 取反）/ `jitter_projection`（行 0、行 1 += d · 行 3）；`TaaResolver`
   （两张 HDR 历史纹理乒乓、176 B uniform、resolve 管线、绑定组按代号缓存）；
   WGSL 里 3×3 邻域钳制 + 重投影 + 首帧直通。
3. **`ScenePipeline::render`** 加一个 `jitter_ndc` 参数：只叠在相机 uniform 上，
   剔除视锥与 CSM 拟合仍用未抖动的矩阵。
4. **`HdrTargets`** 的深度加 `TEXTURE_BINDING`；`PostFx` 的绑定组缓存从单槽改成
   有界多槽（容量 8，键仍含源代）—— 乒乓让源视图每帧都换，单槽等于每帧重建。
5. **三宿主接线**：编辑器（指纹用未抖动矩阵、`recreate_targets` 里 `reset`）、
   独立游戏（`resize` 里 `reset`）、无头截图；`--headless` 报告的 `render.taa`
   是契约字段。
6. ⚙ 项目设置加「TAA / TAA 历史权重」两行（区块标题同步）。

### 三、关键取舍

- **不做逐物体运动向量**（v1）：刚体要存上一帧模型矩阵、蒙皮要存上一帧骨骼矩阵，
  是另一刀的量级；先用 3×3 邻域钳制压住残影，限制写进 `TaaConfig` 的文档与决策记录。
- **直通优先于猜测**：历史为空 / uv 越界 / 点在上一帧相机背后 → `blend = 0`。
- **抖动是纯函数**，不是随机数：无头截图的"逐字节确定"契约在 TAA 下必须继续成立。
- **编辑器静止画面不收敛**（指纹门控不重绘）：不为此改帧调度 —— 为了几帧收敛让编辑器
  在空闲时全速重绘，代价远大于收益。

### 四、验证（全部实跑）

- **全量测试**：`769 passed / 0 failed / 7 ignored`（新增 14 条）；clippy `-D warnings`
  0 告警。
- **量化效果**（1× MSAA、斜边薄盒、12 帧）：硬台阶 672 → 541（-19%），
  介于前景 / 背景之间的中间调像素 0 → 675（FXAA 当年在同一画面上是硬台阶 -44%，
  见三十一节 —— 两者同一量级）。这是"TAA 真的在收敛"的证据，不是"图变了"。
- **确定性**：两个全新渲染器各跑 5 帧，PNG 逐字节一致。
- **抓到一个真 bug**：首帧整屏全黑（`resolve` 返回了刚读的那张而不是刚写的那张）。
  先补 `taa_first_frame_is_not_black`，再把它改回旧写法确认变红，然后修。
- **三宿主**：编辑器 `--headless --frames 30 --screenshot`（TAA 开 / 关两个项目）
  `ok:true` / `entities: 24` / `error: 0`；`--ui-shot --ui-settings` 1440 宽排版正常；
  编辑器视口 24 帧 A/B 平均亮度 166.08 → 166.25（差集中在边缘，不是整体变亮变暗）。

### 五、刻意没做

- 逐物体运动向量（见 §三）、TAA 的锐化（sharpen）后处理、历史权重自适应的动态分支
  （Unity 那套"运动越大越不信历史"要运动向量才能算）。
- 无头单帧截图的 TAA 预热（多渲几帧再出图）：会让"首帧直通"这条分支失去唯一的
  GPU 测试，收益不值。

### 六、下一步候选

- **动画 morph target**（P1 #12）：表情 / 面部，需要带 morph 的样例资产。
- **手柄输入**（P1 #15）：要硬件才能验证手感。
- **动画剪辑切换的脚本 API**（P1 #13 最后一条缺口）：`crossfade` 已存在，只差暴露
  （已在四十八落地）。
- **CI**（差距分析说"半小时、零风险"）：`.github/workflows` + `check` / `clippy` /
  `test` / `--check` / `--headless`。**本轮没做**：容器里跑不了 GitHub Actions，
  交付物无法自验（按纪律不写"已验证"）。

---

## 四十八、动画脚本控制：六个 API 补齐最后一块纯软件缺口（2026-10-23）

### 一、为什么是这一刀

`Docs/差距分析.md` 的 P1 #13 只剩两项：

- **手柄 / 触摸**：要硬件才能验证手感，本轮无法闭环；
- **动画剪辑切换**：`crossfade` 已在编辑器与 RON 里存在，脚本只能推状态机参数
  （`set_anim_param`）或调层权重，缺"直接换剪辑"的入口。

选后者的唯一理由是**可当场实跑验证**。动工前核对了三件事：

1. `xgame-core` 的 `Animator` 已经有 `clip / speed / looping / autoplay / fade_time /
   machine / additive / layers`，全是普通字段 —— 不需要新数据结构。
2. `xgame-render::AnimationDriver::tick` 已经把"剪辑名变化 → 时间归零 → 按
   `fade_time` 交叉淡入"做成唯一路径；状态机优先的解析也在这一个函数里
   （`active_machine()` 可用时读状态表，否则读 `Animator` 字段）。
3. 脚本层已有 `set_anim_param` / `set_anim_additive_weight` / `set_anim_layer_weight`，
   注册名 149 个；补的只是"剪辑 + 基础播放参数"这几个字段写入。

所以这一刀不新增依赖、不改架构、不新增第二份混合实现。

### 二、做了什么

1. **`xgame-script/src/ctx.rs`**：
   - 新增私有 `animator_mut()`（`entity_index` + `get_mut` + `as_mut` 收在一处）；
   - 新增 6 个 `GameCtx` 方法：`set_anim_clip` / `set_anim_speed` /
     `set_anim_looping` / `set_anim_fade_time` / `set_anim_additive_clip` /
     `set_anim_layer_clip`；
   - `speed` 钳 `[0, 100]`、`fade_time` 钳 `[0, 10]`，NaN / inf 忽略；
   - 没挂 animator / 没配叠加层 / 层名不存在 → 静默 no-op。
2. **`xgame-script/src/lua_api.rs`**：`register_animation` 注册 6 个 Lua 全局名
   （注册名 149 → 155；`lua_api` 129 + `ui_api` 26）。
3. **测试**：`lua_host::tests::script_controls_base_clip_additive_and_layers`
   （真实 Lua 钩子走完六个 API）+ `ctx::tests::animation_setters_clamp_and_apply_to_the_owning_entity_only`
   （钳制 / 忽略非有限值 / 只改自己实体 / 缺组件与缺层名 no-op）。
4. **文档**：`Docs/脚本API.md` 4.9 / 4.10、`README.md` 动画章节、`ROADMAP.md`
   第七阶段、`Docs/差距分析.md` P1 #13、`Docs/决策记录.md` §三十。

**没有改** `xgame-core` 的 `Animator` 与 `xgame-render` 的 `AnimationDriver` ——
它们已经是唯一真相源，这一刀只是把脚本接到字段上。

### 三、关键取舍

- **状态机优先，且不自动停用机器**：`tick` 原本就是"有可用机器读状态表、没有才读
  `Animator` 字段"。脚本直接 setter 在机器模式下只写字段、不改变实际播放 ——
  与 Inspector 语义一致。静默移除机器会让"为什么我的状态机没了"极难排查；
  要直接控制就明确删掉机器配置。
- **`set_anim_clip` 只换基础剪辑**：不碰 `autoplay` / `speed` / `looping`。否则
  "只想换动作"的调用会把"摆好第一帧再播"的 `autoplay = false` 悄悄改掉。
- **叠加层 / 层栈是硬切**：驱动器对它们只有独立时钟、没有 `Prev`；换剪辑只把该层
  时钟归零。想平滑就先拉权重到 0、换好再拉回 —— 不为这一刀扩多层混合栈。
- **钳制而不是报错**：脚本算错速度 / 淡入时不该把时钟推成 `inf`，也不该刷日志。
- **不做 getter / 逐调用 fade / 强制状态**：getter 要先定义"配置值还是实际值"；
  逐调用 fade 要改 `AnimationDriver` 的公开签名；强制状态要绕开"状态机优先"。
  三者都等真实需求。

### 四、验证（全部实跑）

- **先红后绿**：新增 Lua 测试在旧实现下报
  `attempt to call a nil value (global 'set_anim_clip')`，再补实现。
- **全量测试**：`771 passed / 0 failed / 7 ignored`（上一轮 769，净增 2 条）；
  clippy `-D warnings` 0 告警。
- **无头端到端**（`target/e2e_anim`，Fox 单剪辑、无状态机）：
  - `--check target/e2e_anim` 退出码 0；
  - `--headless --frames 5 --dump-scene target/e2e_anim/after.ron target/e2e_anim`
    退出码 0，报告 `ok: true`、`entities: 1`、`frames: 5`、`error: 0`、`warn: 0`、
    `time_scale: 1.0`；
  - dump 的 animator 六项都落地：`clip: "Run"`、`speed: 1.5`、`looping: false`、
    `fade_time: 0.05`、叠加层 `clip: "Survey"`、`upper` 层 `clip: "Walk"`
    （权重 / 速度 / 循环 / 遮罩保持原值）。
- **未验证**：状态机模式下"直接 setter 不生效"没有新增"真实 Lua + 状态机"组合
  测试；解析顺序由既有 `driver_uses_state_speed_and_looping_over_animator_fields`
  与 entry 覆盖测试钉住。逐调用 fade、getter、morph、手柄均未做。

### 五、刻意没做

- 逐调用淡入（每次 `set_anim_clip` 带一个时长参数）：要改 `AnimationDriver` 的输入，
  且 v1 的全局 `fade_time` 已经够用。
- 强制切状态 / 绕过状态机：直接 setter 的语义边界已经写清，不额外造一条"忽略机器"
  的后门。
- getter（`get_anim_clip` 等）：返回配置值还是驱动器实际值没有定论，先不加。
- morph target（P1 #12）与手柄（P1 #15）：前者缺带 morph 的样例资产，后者缺硬件。

### 六、下一步候选

- **CI**（差距分析说"半小时、零风险"）：`.github/workflows` + `check` / `clippy` /
  `test` / `--check` / `--headless`。仍需能跑 GitHub Actions 的环境才能自验。
- **动画 morph target**（P1 #12）：先找 / 造一个带 morph 的 glTF 样例。
- **手柄输入**（P1 #15）：要硬件。
- **动画 getter / `crossfade` 脚本化**：等出现"按当前剪辑做分支"的真实需求再定。

---

## 四十九、资产 / 脚本热重载：1 Hz 轮询 + 动作后 rebase（2026-10-23）

### 一、为什么是这一刀

`Docs/差距分析.md` 的 P1 里，资产热重载是**唯一不依赖外部条件的纯软件缺口**
（morph 缺带 morph 的样例资产、手柄缺硬件）。它也是编辑器日常使用里最刺眼的一处：

- 改 `.lua` 只能退出 Play、点工具栏 **⟳ Scripts**，或者重启项目；
- 新拷进来的模型 / 贴图 / 材质要手动点资源面板的 ⟳；
- 外部工具改完 `.meta` 后，渲染侧缓存不失效，画面纹丝不动；
- 反过来，如果粗暴地"检测到变化就重载场景"，用户未保存的编辑会被直接冲掉。

这一轮把"什么时候该重载、重载什么、什么绝对不能重载"做成一条可测试的规则。

### 二、做了什么

1. **`crates/xgame-editor/src/hot_reload.rs`（新文件）**
   - `ProjectWatcher`：持有上一轮文件快照（`项目相对路径 / mtime / 长度`）与下次扫描时刻；
   - `scan()`：只扫清单声明的目录（脚本 / 场景 / 预制体 / 模型 / 贴图 / 音频 / shader），
     重叠目录去重；只读 `metadata`，不读文件内容；
   - `classify()`：`.prefab.ron` → Prefab；Scenes 下的 `.ron` → Scene；Scripts 下的 `.lua`
     → Script；其余 → Asset。同一文件被多个根扫到时取更具体的类别；
   - `diff()`：按路径归并比较，产出新增 / 修改 / 删除；
   - `poll()` 到点才扫、返回差异并**推进基线**；`rebase()` 在动作跑完后重扫，把本轮
     自己写下的文件（主要是 `ensure_asset_metas` 补的 `.meta`）吞进基线；
   - `SKIP_DIRS`（`.autosave` / `target` / `Build` / `Plugins` / `.git`）、不跟随符号链接、
     递归深度上限 32。
2. **`EditorState` 接线**
   - `hot_reload` 字段 + 构造时以当前磁盘为基线（打开项目不该把已有文件当"刚变化"）；
   - `hot_reload_tick(now)`：脚本 → 编辑态刷清单 / Play 中 `reload_scripts()`；资产 →
     `refresh_assets()` + `renderer.invalidate_assets()`；预制体 → 只 `refresh_assets()`；
     场景 → 只写 debug 日志；随后 `rebase()`；
   - `reload_open_material()`：当前材质草稿干净 → 丢弃并等下次读盘；脏 → 保留 + warning；
   - `hot_reload_wait(now)`：给主循环排定时唤醒。
3. **主循环（`main.rs`）**
   - 与自动保存同在 `write_autosave` 分支：`--ui-shot` 等只读会话不驱动；
   - `!live` 时 `request_repaint_after(hot_reload_wait(now))`，空闲停帧也能在 1 秒内发现
     外部改动；不强制立即重绘。
4. **测试（12 条）**
   - watcher 6 条：增 / 改 / 删、没变化为空、资产与 `.meta`、预制体 / 场景分类、忽略自身
     产物目录、rebase 防自激；
   - `EditorState` 6 条：打开项目时自补的 `.meta` 不算外部改动、编辑态刷脚本清单、
     Play 中重编译、场景不被覆盖、干净材质草稿重载、脏材质草稿保留。

### 三、刻意的取舍

- **轮询而不是 `notify` / `ReadDirectoryChangesW`**：不引平台相关依赖，不需要一套事件去抖；
  内容目录是几棵小树，1 Hz 的 `metadata` 扫描成本远小于维护事件语义的成本。
- **判据不含哈希 / 文件内容**：多报一次只是多一次重载（加载器自己有缓存），漏报才是 bug；
  长度用来补 mtime 粒度粗的情况。
- **`poll` 推进基线 + 动作后 `rebase`**：少了 rebase，`ensure_asset_metas` 写下的 `.meta`
  会每秒被当成"外部新增"，形成自激循环。测试专门钉住这条。
- **场景文件永不自动覆盖**：磁盘变了只记 debug。Scenes 菜单现扫，新场景照样出现；
  "重载场景"必须是显式动作，自动做就是拿未保存编辑冒险。
- **预制体只刷清单，不热替换已有实例**：实例是普通实体，热替换要先定义字段保留规则
  （Unity 也要 Prefab Mode）。没有语义的自动替换比不换更危险。
- **`project.ron` 不热重载**：项目级配置需要重建运行时；重启编辑器是最可预期的语义。
- **已解码音频不换**：kira 没有公开的解码缓存失效口。
- **脏材质草稿不覆盖**：外部改动只提示，Revert 才是载入磁盘版的动作。
- **只读会话不启用**：`--ui-shot` 的截图不能被开发机上的临时文件影响。

### 四、验证（全部实跑）

- **全量测试**：`783 passed / 0 failed / 7 ignored`（上一轮 771，净增 12 条）；
  clippy `-D warnings` 0 告警。
- **无头端到端**：
  - `--check projects/SampleScene` 退出码 0（`scenes=1 entities=24 ...`，全部引用存在）；
  - `--headless --frames 30 projects/SampleScene` 退出码 0，`ok: true`、`entities: 24`、
    `error: 0`、`warn: 0`、`non_finite_transforms: 0`；
  - `--ui-shot ... --ui-size 1280x800` 两次都退出码 0，画面正常。
- **截图确定性说明**：两次 `--ui-shot` 的 PNG 哈希不同，差异来自状态栏 FPS 与场景里的
  动画 UI（血条）—— `--ui-shot` 本来就不承诺逐字节一致；逐字节契约属于无头
  `--screenshot`，本轮没有改动那条路径。
- **未验证**：没有用真实鼠标在 GUI 里走"外部保存脚本 → 控制台自动出现重编译日志"一遍
  （UI 层只在单元测试里驱动 `EditorState`）；资产缓存失效后的**画面变化**没有做像素级
  A/B（缓存失效 / 列表刷新有测试，渲染正确性由既有截图测试覆盖）。

### 五、刻意没做 / 已知边界

- 没有编辑器内的资产 rename / move / delete（仍属资产管线 P2）；
- 没有 shader 文件热重载（`.wgsl` 目前是内联字符串，见差距分析 P2 Shader 工具链）；
- 没有"重载当前场景"按钮（要先解决未保存编辑的确认语义）；
- 扫描只覆盖清单里声明的目录；用户把资产放到目录外不会被检测（这是清单的契约）；
- 1 Hz 是固定值，没有暴露成偏好项：先看真实使用里"太快 / 太慢"的反馈再决定是否可调。

### 六、下一步候选

- **CI**：`.github/workflows` + `check` / `clippy` / `test` / `--check` / `--headless`；
  仍需能跑 GitHub Actions 的环境才能自验。
- **动画 morph target**（P1 #12）：先找 / 造一个带 morph 的 glTF 样例。
- **手柄输入**（P1 #15）：要硬件。
- **资产管线剩余项**：编辑器内 rename / move / delete、音频流式播放。
- **Shader 工具链**：把内联 WGSL 抽成 `.wgsl` 文件后，热重载才有明确对象。

---

## 五十、场景 schema 导出（Agent-Native ③，2026-10-23）

### 一、为什么做这一轮

Agent-Native 的 ③ 是「机器可读契约」：智能体能读写 RON 不等于知道**该写什么**。
字段名、可选组件、serde 默认值、枚举变体、资产引用的两种形态散在十几个 Rust 类型里，
智能体只能靠 `--check` 的报错反推，试错成本高（一次坏 RON 就是一次加载失败）。
这一轮把「场景格式说明书」做成可导出的 JSON，并同时给 MCP 一个 `schema_read` ——
智能体在写第一个实体之前就能拿到完整契约。

它也是后续所有 Agent 能力的地基：`scene_write` / `entity_upsert` 的参数语义、资产路径
规则、默认值，都可以引用同一份目录，不必在工具描述里各写一份。

### 二、做了什么

1. **`crates/xgame-core/src/schema.rs`（唯一内容源）**
   - `scene_schema()` 返回 `SchemaDoc`：`format_version` / `engine_version` /
     `roots`（场景 `.ron` + 预制体 `.prefab.ron`）/ `conventions` / `primitives` /
     `asset_references` / `types`；
   - 36 个落盘类型：Scene / Entity / Transform / Light / 动画 / 物理 / 音频 / 材质 /
     角色 / 相机 / UI / 粒子 / 行为参数；每个类型带 `summary` / `kind` /
     `default_ron` / `example_ron`，字段带 `name` / `ty` / `required` /
     `default_ron` / `note`，枚举变体带形态与可解析示例；
   - 示例与默认值都由**真实 Rust 值序列化**得到（不是手写字符串）；实体示例把
     `EntityId` 固定为 1，避免全局原子计数器让两次导出不同；
   - 运行时字段（`Entity.components` / `UiElement.state` / `ParticleEmitter.burst`）
     明确排除：写进 RON 也不会生效。
2. **`crates/xgame-editor/src/schema_cli.rs`（CLI 出口）**
   - `--export-schema [--out <path>]`；无 `--out` 打 stdout（末尾一个换行），有则写文件
     并自动建父目录，stdout 只回一行确认；
   - `serde_json::to_string_pretty` + 确定性内容；参数错误退出码 1，成功 0，与
     `--check` 的 0/1/2 语义互不干扰。
3. **MCP `schema_read`（第 9 个工具）**
   - 与 CLI 调同一个 `scene_schema()`，只是走 MCP 文本通道；**不需要 `project`**；
   - 工具描述里直接写「写场景之前先读它」，并把闭环串成
     `schema_read → scene_read → scene_write / entity_upsert → project_check / run_headless`。
4. **防漂移测试（核心）**
   - 穷举解构每个结构体、穷举匹配每个枚举：新增 / 删除字段或变体会让测试**编译不过**；
   - 每个 `default_ron` 反解回真实类型并与 `Default` 比较；每个 `example_ron` 反解并
     断言关键字段；
   - 每个枚举变体示例必须解析成**那个**变体；
   - 字段 `ty` 里的类型引用必须能在目录或基元表里找到（防悬空文档）；
   - 新增项目往返测试：示例场景 / 预制体经 `ProjectHandle::write_scene` /
     `write_prefab` 落盘（资产引用变 `guid://`）再 `load_*` 读回（变回路径），
     逐字段一致，且同一份场景再写一次文本不变。

### 三、刻意的取舍

- **手写目录，而不是 derive / 反射**：serde 的运行期没有字段元数据；为此引入 proc
  macro 或新依赖，收益不如「穷举解构测试」直接 —— 漏写一个字段是编译错误，不是
  运行期才发现的文档漂移。
- **目录放 core，JSON 放 editor**：core 的落盘格式是 RON，刻意不依赖 `serde_json`；
  编辑器本来就带它（插件构建的 `cargo --message-format=json` 在用）。MCP 只依赖 core，
  所以 `schema_read` 直接序列化同一个结构体，不经过编辑器。
- **输出确定性是硬契约**：没有时间戳 / 随机 ID；示例 ID 固定；`BTreeMap` 保证参数表
  顺序稳定。这样 schema 可以进版本库做 diff，智能体也能用哈希判断引擎格式有没有变。
- **示例必须能走真实读写边界**：只测 `ron::from_str` 会漏掉 GUID 编码 / 解码与默认值
  补齐；项目往返测试专门钉这一层。
- **资产引用只描述、不转换**：schema 里的 `example_ron` 是**内存形态**（项目相对路径）；
  `guid://` 转换只发生在 `ProjectHandle` 的读写边界。不要把 GUID 写进 schema 示例，
  否则智能体会以为磁盘 / 内存是同一套字符串。
- **`UiElement` 用「省略字段的有效默认值」**：`UiElement::default()` 是面板预设
  （`interactable: false`），而省略字段反序列化得到透明样式 + `interactable: true`。
  目录描述的是后者（智能体写 RON 时的真实语义），并用测试把两者都钉住。
- **不做 JSON Schema**：这是引擎自己的领域目录（默认值是可解析的 RON 示例、枚举带
  变体形态），不是通用 JSON Schema；硬套会让「默认值 / 示例」失去可解析性。
- **CLI 退出码独立**：`--export-schema` 只有 0/1（成功 / 参数或序列化错误），不复用
  `--check` 的 2 —— 智能体不会把「schema 导出失败」误判成「项目有问题」。

### 四、验证（全部实跑）

- **全量测试**：`799 passed / 0 failed / 7 ignored`（上一轮 783，净增 16 条：
  core schema 10（含项目往返 1）+ CLI 5 + MCP 1）；clippy `-D warnings` 0 告警；
  `cargo check --workspace --all-targets` 通过。
- **schema 导出（二进制级）**：
  - `--export-schema --out a.json` 与 `--out b.json` 两次都退出码 0，文件 76,685 字节；
  - 直接跑 `xgame-editor.exe --export-schema` 重定向到 stdout，stderr 为空，退出码 0；
  - 三份内容 SHA256 全部为
    `4DC0B0A990436630D564F9DCDF530C6D80A277BAD55F82811F9A47C89B419207`；
  - 用 Python 按 UTF-8 解析通过：`format_version = 1`、`types = 36`、根文件为
    Scene / Prefab。
- **无头端到端**：
  - `--check projects/SampleScene` 退出码 0：
    `scenes=1 entities=24 lights=1 script_refs=5 audio_refs=1 material_refs=7 assets=15`，
    `OK (all referenced assets exist)`；
  - `--headless --frames 30 projects/SampleScene` 退出码 0：`ok: true`、`entities: 24`、
    `error: 0`、`warn: 0`、`non_finite_transforms: 0`、`total_time ≈ 0.5`。
- **测试覆盖**：`cargo test -p xgame-mcp` 32 条全绿；`xgame-core` 的 schema 测试
  10 条全绿；`xgame-editor` 的 `schema_cli` 5 条全绿。
- **MCP 进程级**：用 Python 起真实 `xgame-mcp.exe`，走
  `initialize → tools/list → tools/call schema_read`：工具数 9、首个是 `schema_read`，
  返回 JSON 可解析（`format_version = 1`、`types = 36`），进程正常退出。

### 五、刻意没做 / 已知边界

- **脚本 API 清单导出**还没做（本轮五十一已补）：支柱 8 的另一半，当时真相源仍是
  `Docs/脚本API.md`；
- **没有 schema 版本迁移器**：`SCHEMA_FORMAT_VERSION` 只在改语义 / 删字段 / 改默认值时
  递增，旧版本的 JSON 不提供自动升级；
- **目录是手写的**：加落盘字段时测试会编译不过，但 `summary` / `note` 的文案仍要人写；
- **不覆盖运行时字段**：`components` / `state` / `burst` 刻意不在目录里，智能体写进去
  也不会生效（工具描述已说明）；
- **没有在真实 MCP 客户端里走一遍**：`schema_read` 由协议层单元测试与共享目录测试覆盖，
  未接 Cursor / Claude 做端到端交互。

### 六、下一步候选

- **脚本 API 清单导出**：把 `GameCtx` 的注册名 / 参数 / 返回值导出成 JSON，补完支柱 8；
- **`asset_import`**：让智能体经 MCP 改 `.meta` 导入设置（当前只能编辑器里改）；
- **CI**：`.github/workflows` + `check` / `clippy` / `test` / `--check` / `--headless`；
- **schema 快照测试**：把导出的 JSON 存一份进仓库，格式变化时 diff 直接可见
  （当前用「两次导出相同 + 内容断言」防漂移，快照是更直观的补充）。

---

## 五十一、脚本 API 清单导出（Agent-Native ③ 收口，2026-10-23）

上一轮把「场景怎么写」变成了机器可读 JSON；这一轮补上另一半 ——「脚本怎么调」。
目标不是再写一份人读文档，而是让智能体在写第一个 `.lua` 之前拿到**可解析的契约**：
155 个注册名、签名、参数、返回值、6 个生命周期钩子、值类型与全局约定。

### 一、做了什么

1. **`xgame-core::script_api`（新模块）**
   - `ScriptApiDoc { format_version, engine_version, conventions, hooks,
     value_types, categories }`；
   - 17 个类别（`transform` / `entity` / `spawn` / `physics` / `character` /
     `input` / `time` / `state` / `math` / `animation` / `animation_layers` /
     `audio` / `material` / `particle` / `ui` / `camera` / `scene`）；
   - 155 个 `ScriptFunction { name, signature, params, returns, summary }`；
   - 6 个钩子、7 个值类型（`ctx` / `Vec3` / `Vec2` / `RayHit` / `EntityId` /
     `AssetPath` / `LuaValue`）、9 条全局约定；
   - 参数不单独存，构造时从签名解析 —— 避免「签名改了、params 没改」的两处漂移。
2. **CLI `xgame-editor --export-script-api [--out <path>]`**
   - 无 `--out` 打 stdout（末尾一个换行）；有则写文件、stdout 只回一行确认；
   - 退出码 0 成功 / 1 参数或序列化失败，不复用 `--check` 的 2；
   - JSON 确定性：没有时间戳 / 随机 ID，函数与类别按文档顺序。
3. **MCP `script_api_read`（第 10 个工具）**
   - 与 CLI 同源（`xgame_core::script_api::script_api`），不需要 `project`；
   - 追加为第 10 个工具，避免重排已有 9 个工具的文档编号。
4. **三层防漂移**
   - `xgame-script`：从 `lua_api.rs` / `ui_api.rs` 的 `globals.set("…")` 提取
     全部注册名，与清单逐名比对；再在真实 Lua VM 里确认每个名字都是函数；
   - `xgame-editor`：从 `Docs/脚本API.md` 的 API 表提取函数名，与清单逐名比对；
   - `xgame-core`：结构测试（类别引用、签名与函数名一致、名字唯一、数量）。
5. **文档**：`Docs/脚本API.md` 顶部加机器清单入口；`README` / `ROADMAP` /
   `Docs/差距分析.md` / `Docs/MCP.md` / `FLOW.md` / `AGENTS.md` 同步；决策记录
   新增 §三十三。

### 二、刻意的取舍

- **清单放 core，不放 `xgame-script`**：MCP 只依赖 `xgame-core`；如果把清单放进
  脚本 crate，`xgame-mcp` 会被迫依赖 mlua / vendored Lua。清单是纯数据，放 core
  让 CLI、MCP、测试共享同一份内容源。
- **手写元数据，不做运行时反射**：Lua C 函数没有可用的参数名 / 返回类型反射；
  强行从 Rust 闭包类型推导会把 `LuaCtx`、`f32`、`Option<T>` 等实现细节漏给脚本
  作者。手写签名 + 注册名测试比“自动但不准确”更有价值。
- **参数类型只放在签名字符串里**：`set_pos(ctx, x, y, z)` 对智能体已经足够；
  没有把每个参数拆成 `{name, ty}`。这是第一版的刻意收敛 —— 等出现“必须按类型
  过滤”的真实需求再升级，避免 155 × N 个字段的手写负担。
- **返回值是字符串，不是 JSON Schema**：`EntityId | nil` / `RayHit` 这类写法
  直接对应 Lua 语义；硬套 JSON Schema 会把 `nil`、Lua 表、userdata 表达得很别扭。
- **文档只钉函数名，不钉文案**：`Docs/脚本API.md` 继续承担叙述与示例，清单的
  `summary` 可以更精炼；名字集合必须完全一致（否则智能体会照着文档写出不存在的
  函数），文案允许各自演进。
- **MCP 工具追加为第 10 个**：`schema_read` 仍是首个；`script_api_read` 不需要
  `project`，`tools_list_shape` 对它单独放行。这样不用重排已有 9 个工具的章节。
- **格式版本从 1 开始**：加字段不涨；改语义 / 删字段 / 改返回值才涨。与 schema
  的版本纪律一致。

### 三、验证（全部实跑）

- **全量测试**：`813 passed / 0 failed / 7 ignored`（上一轮 799，净增 14：
  core 6 + editor 6 + script 1 + MCP 1）；`cargo check --workspace --all-targets`
  通过；`cargo clippy --workspace --all-targets -- -D warnings` 0 告警。
- **二进制级导出**：
  - `--export-script-api` 两次 stdout 逐字节相同，50,695 字节，SHA256
    `DBF83431BE494BB675734F8C18BA69EC57DF293AD1F432DAB0F5C6FEE7486CDC`；
  - 两次 `--out` 与 stdout 完全相同；stderr 为空、退出码 0；
  - Python 按 UTF-8 解析通过：`format_version=1`、17 类、155 函数、6 钩子、
    7 值类型、9 条约定；`set_pos(ctx, x, y, z)` 与
    `spawn_prefab -> EntityId | nil` 等抽样断言通过。
- **MCP 进程级**：Python 起真实 `xgame-mcp.exe`，`initialize → tools/list`
  返回 10 个工具（含 `script_api_read`，无必填 `project`）→
  `tools/call script_api_read` 返回同一份清单（155 函数 / 17 类），退出码 0；
  stderr 只有启动横幅。
- **文档防漂移**：`xgame-editor` 的 Markdown 解析测试从 `Docs/脚本API.md` 的
  API 表提取出 155 个函数名，与清单完全一致。

### 四、还没做 / 已知边界

- **没有真实 MCP 客户端端到端**：只验证了协议层与真实 server 进程；
  Cursor / Claude 的实际工具选择与上下文注入未测。
- **参数 / 返回值的结构粒度有限**：参数类型在签名字符串里；返回值是字符串。
  如果未来智能体需要按类型生成强类型桩代码，再升级为结构化参数。
- **文档 summary 不参与漂移检查**：函数名集合由测试钉住，但文档文案与清单
  `summary` 可以各自演进；改语义时仍要人工同步两处。
- **清单不含“自哪个版本加入”**：当前只有整体 `format_version` / `engine_version`；
  没有逐函数 `since`。等 API 开始做兼容性分层时再加。
- **UI / 音频等固定参数个数**：清单只描述签名，不描述“参数个数写错会当场报错”
  这类实现约束；这些仍在 `Docs/脚本API.md` 的叙述里。

### 五、下一步候选

- ~~**CI**~~ ✅ **已完成（PLAN 五十二）**：`.github/workflows` + `check` / `clippy` /
  `test` / `--check` / `--headless`；
- **`asset_import`**：让智能体经 MCP 改 `.meta` 导入设置；
- **脚本 API 结构化参数**：如果出现强类型桩代码需求，再把签名升级成
  `{ name, ty, optional }`；
- **两份 JSON 的快照测试**：schema + script_api 各存一份进仓库，格式变化时
  diff 直接可见。

## 五十二、CI（GitHub Actions，2026-10-23）

上一轮把脚本契约变成机器可读；这一轮让 813 条测试与两个无头入口（`--check` /
`--headless`）不再只靠手动跑。目标很小：push / PR 时自动跑与本地同一套命令，
先抓编译、lint、回归和示例项目损坏。

### 一、做了什么

1. **`.github/workflows/ci.yml`**
   - `windows-latest` + stable Rust + clippy + `Swatinem/rust-cache@v2`；
   - 触发：push `master` / pull request / manual dispatch；同 ref 并发取消旧任务；
   - 60 分钟超时，`--locked` 固定 `Cargo.lock`，`CARGO_INCREMENTAL=0` 缩小缓存。
2. **CI 步骤**
   - `cargo check --locked --workspace --all-targets`
   - `cargo clippy --locked --workspace --all-targets -- -D warnings`
   - `cargo test --locked --workspace --all-targets`
   - `cargo run --locked -p xgame-editor -- --check projects/SampleScene`
   - `cargo run --locked -p xgame-editor -- --headless --frames 30 projects/SampleScene`
   - `--export-schema` / `--export-script-api` 写到 `RUNNER_TEMP`，做二进制级冒烟。
3. **文档**：`Docs/差距分析.md` 把 CI 从"立即做"改成完成；`ROADMAP.md` 加状态；
   `README.md` / `FLOW.md` / `AGENTS.md` 同步入口与边界；决策记录新增 §三十四。

### 二、刻意的取舍

- **只跑 Windows**：引擎目前只有 Windows 实测；Linux 还要补 ALSA 等系统依赖，
  在没验证前加矩阵只会制造"红得没意义"的 CI。等跨平台真正开始做再扩。
- **不跑 `--screenshot`**：hosted runner 没有 GPU；`gpu_or_skip` 已让渲染断言在
  无适配器时跳过，截图要的是真实像素，不能拿 skip 冒充验证。留给带 GPU 的自托管
  runner。
- **不跑 7 条 ignored 基准**：性能基准受机器噪声影响，CI 里只保留 `#[ignore]` 的
  手动入口；要防回归得先有稳定的自托管 runner 与基线。
- **不加 `cargo fmt --check`**：当前工具链未安装 rustfmt，仓库也没有格式化门禁；
  先保证 check / clippy / test，等格式化一次性收口后再加。
- **单 job 顺序跑**：check → clippy → test → 冒烟共用同一份 target 缓存；
  拆成并行 job 会让每个 job 各自冷编译，反而更慢。
- **不跑 release build**：发布打包仍由 `一键构建.cmd` / `tools/build-release.ps1`
  负责；CI 先守住 debug 编译与测试，避免首版 CI 时间过长。
- **`--locked`**：`Cargo.lock` 是提交物；CI 不允许静默升级依赖。

### 三、验证（本地实跑）

- **YAML**：PyYAML 解析通过；`actionlint 1.7.12` 0 问题；10 个 step、`windows`
  job 结构正确。**未在 GitHub 托管 runner 上执行** —— 本地仓库尚未推送这版
  workflow，首次 push / PR 才是真正的 runner 验证；本地能证明的是命令与 YAML
  语法。
- **命令**（全部带 `--locked`）：
  - `--check projects/SampleScene` 退出码 0；
  - `--headless --frames 30` 退出码 0，报告 `frames=30`、`entities=24`、
    `logs.error=0`；
  - `--export-schema --out` 76,685 字节，`format_version=1`；
  - `--export-script-api --out` 50,695 字节，`format_version=1`、17 类、155 函数。
- **全量**：`cargo check --workspace --all-targets` 通过；`cargo clippy --workspace
  --all-targets -- -D warnings` 0 告警；`cargo test --workspace --all-targets`
  813 passed / 0 failed / 7 ignored（CI 只改 YAML 与文档，测试数不变）。
- **已知噪声**：`xgame-derive` 的 MSVC 链接器警告仍在（非致命）；CI 日志会出现，
  但不影响退出码。

### 四、还没做 / 已知边界

- **没有 GitHub 托管运行记录**：本地只验证 YAML 与命令；runner 上的 MSVC / WARP /
  缓存命中情况要等首次 push。
- **没有截图 / 基准 / release 构建**：见上面的取舍；`--screenshot` 需要带 GPU 的
  自托管 runner，性能基准需要稳定机器。
- **没有 Linux / macOS 矩阵**：跨平台仍是 P2；先别让未验证的平台拖慢主流程。
- **没有格式门禁**：`rustfmt` 未安装，代码格式还没一次性收口。
- **CI 不检查工作区是否干净**：导出写到 `RUNNER_TEMP`，但测试会生成临时文件；
  目前没有把"工作区必须 clean"作为门禁，避免测试自身产物误报。

### 五、下一步候选

- **`asset_import`**：让智能体经 MCP 改 `.meta` 导入设置；
- **自托管 GPU runner**：把 `--screenshot` 的逐字节确定性也纳入 CI；
- **性能基准 runner**：给 7 条 ignored 基准建立稳定基线后再进 CI；
- **跨平台矩阵**：Linux 先补系统依赖与实测，再加进 workflow。

## 五十三、MCP 资产侧闭环：`asset_import` / `asset_move`（2026-10-23）

上一轮的"下一步候选"第一条就是 `asset_import`。这一轮把它和它的孪生兄弟
`asset_move` 一起做完，让 Agent-Native 的资产侧也齐成闭环：
**看见（`asset_list`）→ 导入（`asset_import`）→ 整理（`asset_move`）→ 自证
（`project_check`）**。此前智能体能"看见"资产树，却改不动导入设置、也搬不动文件。

### 一、做了什么

1. **core（`xgame-core::asset`）** —— 三个新原语，编辑器与 MCP 共用同一条路径：
   - `normalize_asset_relative()`：把外部给的路径收敛成项目内相对路径。拒绝绝对
     路径 / 盘符 / `..` / 未知扩展名 / 构建缓存目录（`target` / `dist` / `.git` 等），
     也拒绝 Windows 会**静默吃掉**的结尾空格与点、`file:stream` 里的冒号、非法字符
     —— 否则"我写下的 `.meta`"和"扫描看到的资产"会对不上。
   - `ImportSettings::sanitized()`：`model_scale` 非有限值退回 1.0、有限值钳到
     `[0.001, 1000]`。**读盘（`AssetMeta::from_ron`）与写盘（`set_import`）两个边界
     都过一遍** —— 坏值既进不来也存不下去，不会出现"列表里是 1e30、画面里是 1000"。
   - `AssetDatabase::ensure_meta()`：**只给点名的那个资产**写 `.meta`。与
     `ensure_metas()`（编辑器打开项目时的批量补全）分工明确：一次点名就该是一次
     范围明确的写操作。已有合法 `.meta` 原样返回（幂等、不换 GUID），已有坏
     `.meta` 拒绝覆盖（那可能是用户手改到一半的内容）。
   - `AssetDatabase::move_asset()`：资产 + `.meta` 一起搬，**GUID 不变**。所有拒绝
     判定都在动盘之前（源未登记 / 目标已存在 / 改变资产类型 / 目标是场景 / 路径
     穿越）；`.meta` 搬失败时回滚资产文件；成功后 `refresh()` 全量重扫，换"索引
     绝不会半新半旧"。
2. **MCP（`xgame-mcp`）** —— 两个新工具，工具面从 10 个变成 12 个：
   - `asset_import`：不给参数 = 只补 `.meta`（幂等）；给参数 = **部分更新**
     （只覆盖传了的字段 —— 只调缩放不能顺手把用户改过的 `texture_srgb` 冲回默认）。
     返回 `created` / `changed` / 落盘后的 `import`；`model_scale` 被钳过时另带
     `model_scale_requested` + `model_scale_clamped`，让"我设的"和"生效的"两个事实
     都可见。非有限值**直接拒绝**而不是静默修正。
   - `asset_move`：搬移 + 返回 `references_preserved`；**刻意不"顺手改写引用"** ——
     磁盘上存的是 `guid://`，GUID 不变就天然不断链，那才是这条工具的价值。
3. **文档**：`Docs/MCP.md` 补 11 / 12 两节 + 修正"编辑器不监视文件"这句已经过时的
   说法（热重载上一轮已落地）；`ROADMAP.md` / `Docs/差距分析.md` / `README.md` /
   `FLOW.md` / `AGENTS.md` 同步工具数与测试数；决策记录新增 §三十五。

### 二、刻意的取舍

- **单资产而不是整目录**：`asset_import` 只碰点名的那个文件。智能体点名一个资产却
  顺手给整个资源目录 GUID 化，会把一次小操作变成一次范围外写入。
- **部分更新而不是整体替换**：`entity_upsert` 那套语义沿用到导入设置。整体替换会
  让"只调一个字段"变成"其余字段回默认"，用户手改的设置会在第二次调用时消失。
- **越界钳制 + 两个值都报**：钳制是对的（`1e30` 会把包围盒 / 剔除算爆），但静默
  钳制会让智能体以为设置生效了。返回值里同时给请求值与实际值。
- **非有限值拒绝而不是退回默认**：`NaN` 没有"钳到边界"的合理含义，core 退回 1.0
  是给"手写 `.meta` 的用户"留的兜底；MCP 是**智能体的入口**，那里应该报错。
- **场景不归资产库**：导入 / 移动都明确拒绝 `.ron`。场景被 `project.ron` 按**路径**
  引用（入口场景、发布清单），搬它要同步改清单，不是资产库能单方面决定的事。
- **移动后全量重扫**：移动同时改了 `by_path` / `by_guid` / 诊断三处，手工同步容易
  漏；这是低频交互操作（毫秒级扫盘），用一次全扫换"索引绝不会半新半旧"。
- **不碰编辑器 UI**：资源浏览器里的重命名 / 拖拽移动仍是空缺（本轮只做智能体通道）。
  UI 侧要处理选中项、Undo、已打开场景的引用刷新，是另一件事。

### 三、验证（本地实跑）

- **MCP 行为测试 3 条**（`cargo test -p xgame-mcp`）：
  - `asset_import_creates_meta_and_merges_settings`：只补 `.meta` → 幂等（反斜杠
    路径归一、GUID 不变、`changed=false`）→ 写 `model_scale=2.5` 并落盘 → 部分更新
    （改 `texture_srgb` 后 `model_scale` 仍是 2.5）→ `1e9` 钳到 1000 且
    `model_scale_clamped=true` → `1e39`（f64 有限、转 f32 溢出）被拒绝 → 场景 /
    不存在的资产 / `..` 全部拒绝。
  - `asset_move_keeps_guid_references_alive`：先断言盘上场景**确实存的是 `guid://`**
    （否则这条测试什么都没证明）→ 移动 → 资产与 `.meta` 都在新路径、源目录清空 →
    `project_check` 的 `has_issues=false`、`dangling_guids` 为空 → 场景读回来的
    `model` 直接是新路径。
  - `asset_move_rejects_unsafe_targets_without_touching_files`：目标已存在 / 改类型 /
    目标是场景 / 路径穿越 / 同路径 / 源缺 `.meta` 六种拒绝，之后源文件、`.meta`
    原文与索引全部原样。
- **core 测试 5 条**：路径归一（16 个拒绝样例）、导入设置读写两边界净化、
  单资产 `ensure_meta` 不碰别人 + 幂等 + 坏 `.meta` 不覆盖、移动保 GUID 且旧引用
  解析到新路径、六种拒绝不动盘。
- **全量**：`cargo check --workspace --all-targets` 通过；`cargo clippy --workspace
  --all-targets` 0 告警；`cargo test --workspace --all-targets` **821 passed /
  0 failed / 7 ignored**（上轮 813，本轮 +5 core +3 MCP）。

### 四、还没做 / 已知边界

- **编辑器 UI 的 rename / move**：资源浏览器仍不能重命名或拖拽移动资产（MCP 通了，
  UI 没通）。
- **`audio_streaming` 仍未被消费**：参数能写、能落盘，音频层还是全量解码（流式播放
  是 P2）。
- **移动不检查"谁在引用"**：不做引用方提示（Unity 会警告"移动会影响 N 个场景"）。
  引用不会断，但"这个资产被谁用着"仍是盲区。
- **大小写改名在 Windows 上被拒绝**：`Box.glb` → `box.glb` 会命中"目标已存在"。
  没去猜"用户是不是只想改大小写" —— 猜错的代价比多一条报错大。
- **编辑器里已打开的场景不跟随资产移动**：热重载只刷索引，内存中的场景仍是旧路径，
  重新打开再保存才对齐（已写进 MCP 文档的已知边界）。

### 五、下一步候选

- **资源浏览器的 rename / move UI**：接上 core 的 `move_asset`，处理选中项与 Undo；
- **`asset_delete`**：与 move 同族（要处理孤儿 `.meta` 与引用方提示）；
- **`project_write`**：改 `project.ron`（入口场景 / 渲染参数）目前只能直接编辑文件；
- **自托管 GPU runner**：把 `--screenshot` 的逐字节确定性纳入 CI。

## 五十四、层与碰撞矩阵：让"碰不碰"成为数据（2026-10-23）

### 一、为什么是它

`Docs/差距分析.md` 的 P2 表里，`标签与层 / 碰撞矩阵` 一直空着。它不是"少一个功能"
那么简单：没有它，"谁和谁碰"只能写在脚本里逐个判断（`if id ~= player then`），
而**物理层根本不认这个判断** —— 子弹照样撞队友、触发器照样推箱子。这属于"挡住
正确性"的一类问题，且被它挡住的还有：触发器 / 伤害判定 / 摄像机避让 / 分帧层
（UI 相机不该进物理世界）。

选它的第二个理由：它一口气打通 core → physics → runtime → script → editor → MCP →
schema → `--check` 八个环节，是"数据在底层、能力在各层"的典型样本，能顺带验证
几条老红线（默认值不改老行为、脚本只读场景、`sync_bodies` 早退）。

### 二、做了什么

**core（真相源）**

- 新增 `layers.rs`：`LAYER_COUNT = 32`、`Layers { names, masks }`（只存覆盖）、
  `collides(a,b)`（**两侧相与**）、`set_collides`（对称写）、`groups(layer)`、
  `label(layer)`（`"3  Enemy"` / `"7  Layer 7"`）、`sanitize_layer`（取模 32）。
- `Project.layers`（默认序列化成 `()`）+ `Entity.layer`（0 层不落盘）。
- `schema.rs` 导出 `layer` 字段与 `u8` 原语；`check.rs` 新增 `invalid_layers`
  （越界层号逐条报"场景: 实体 -> layer N"）。

**physics**

- `Collider.memberships / filter`（默认 `u32::MAX`，老场景逐位一致）。
- `add_collider` 落到 `InteractionGroups`；`set_body_groups()` 支持运行时改组，
  刻意不重建 query pipeline；`RaycastFilter.layer_mask` 用 `predicate` 实现
  （只看成员位，不看矩阵）；`move_character` 的 `QueryFilter.groups` 补上自身组。

**runtime**

- `WorldConfig.layers`；`World::set_entity_layer()`（写场景 + 立刻改 rapier）；
  `sync_bodies` 建体时按 `entity.layer` 取组；`consume_script_commands` 消费
  `Relayered`。`WorldConfig::from_project_runtime` 改名 `from_project`（吃整份清单）。

**script（Lua）**

- `ctx:layer(id)` / `ctx:set_layer(id, layer)` / `ctx:raycast_masked(...)`；
  改层走 `ScriptCommand::Relayered`。脚本 API 计数 155 → 158（`Docs/脚本API.md`、
  `script_api.rs`、`script_api_cli.rs`、MCP 工具描述、`lua_host.rs` 断言同步）。

**editor**

- Inspector 顶部加 `Layer` 下拉（32 项，显示层名，写 `entity.layer` + `touched`）。
- 设置面板新增「层与碰撞矩阵」：32 行层名编辑 + 32×32 矩阵。矩阵是**一张画布 +
  一个交互矩形 + 一次命中测试**（`Grid` 的行高把矩阵撑到 670~800px、把页脚按钮
  挤出屏幕 —— 见决策记录 §三十六.7）。
- `apply_settings_draft` 对 `layers` 做 `sanitized()` 并告警。

**MCP**

- `entity_upsert` 支持 `layer`（越界 / 非整数**拒绝**）；`project_check` 输出
  `invalid_layers`。

### 三、刻意的取舍

- **只做 32 层**：与 rapier 的 32 位组掩码一致，不多不少。层的"名字"只是 UI 糖，
  物理层只认 0..31。
- **矩阵对称写**：点一格改两侧。允许不对称（数据上能表达）只会制造"看得到但不算"
  的假象 —— rapier 要求两侧都同意。
- **查询掩码不参与碰撞矩阵**：Unity 语义照搬（`layerMask` 只筛成员）。这样"幽灵层"
  （不与任何层碰撞、只给射线用）才是可表达的状态。
- **改层立刻生效**：`set_entity_layer` 不等 `sync_bodies`。等重同步 = 永远不生效
  （早退判据里没有层）。
- **越界取模 + 报错**：取模保证不崩，`invalid_layers` 保证不骗人。
- **不做"层预设 / 禁用场景"**：Unity 的 `Layer Collision Matrix` 之上的那些
  （预设、按场景禁用）是效率工具，不是正确性工具，留给 P2 剩下的部分。

### 四、验证（本地实跑）

- 各 crate（分别 `cargo test -p …` 实跑）：core **242 passed / 5 ignored**（基准）、
  physics **37**、runtime **54 passed / 4 ignored**、script **38**、editor **189**、
  mcp **37**。本轮新增 17 条：
  - core `layers.rs` 9 条：默认全通 + 默认落盘 `()`、对称关闭、手改不对称两侧相与、
    0 掩码不被修、sanitize、label 回退、越界取模、只存覆盖的 roundtrip、
    `layer` 只在非 0 时落盘；
  - physics 4 条：互不碰撞的两层穿过彼此 / `set_body_groups` 立刻生效 /
    射线掩码无视碰撞矩阵 / 角色控制器认自己的层；
  - runtime 2 条：建体时按层过滤 / `set_entity_layer` 不等重同步；
  - script 1 条：Lua `set_layer` 写场景并上报 `Relayered`；
  - mcp 1 条：`entity_upsert` 写层并拒绝越界。
- **先证红**：把 `move_character` 的 `QueryFilter.groups` 改回 `None`，
  `character_respects_its_own_layer` 立刻红（`0 passed; 1 failed`），恢复后绿。
- **UI 自检**：`--ui-shot --ui-settings --ui-size …` 四档（1280×900 / 1440×900 /
  1920×1080 / 1280×1400）。判据是像素而不是肉眼：每行 **32 个 9px 蓝格**、
  横跨 380px（右边界离窗口边 76px，无横向溢出）；`Apply` 那块 80×26 纯蓝矩形在
  四档里都能找到（y=838 / 838 / 968 / 1128）—— 也就是说页脚没有被挤出窗口。
- **全量**：`cargo test --workspace --all-targets` → 见下方"验证结果"。
- `cargo clippy --workspace --all-targets`：0 告警。

### 四·五、UI 上踩的两个坑（都写进了决策记录 §三十六.7 / §三十六.7.1）

- `Grid` 的行高不可控（`ui.label` 25px、定尺控件 21px，都不是 `LAYER_CELL + 1`），
  32 行把矩阵撑到 670~800px；`Grid.min_col_width` 默认 40px，32 列把窗口撑到半屏宽。
  → 矩阵改成一张画布 + 一个交互矩形 + 一次命中测试。
- 设置窗口高度靠 `screen_rect - 190` 算：`egui::Window` 自身上限只有 1005，
  1400 高的屏上滚动区吃掉全部可用高度，**页脚的 Apply 被挤出窗口外**。
  改用 `ui.available_height()` 又掉进反馈环（面板塌成 400px 高）。
  → 固定窗口高度，滚动区高度 = 常量 - 常量 - 常量。

### 五、还没做 / 已知边界

- **层预设与"按场景禁用层"**：只做了矩阵本身。
- **`only_dynamic` 与层掩码的组合**没单独测（各自都有测试，组合没测）。
- **编辑器资产 rename / move UI** 仍是空缺（上轮遗留，与本轮无关）。
- **矩阵没做键盘操作**：只能点，不能方向键移动 + 空格切换（无障碍后面统一做）。
- **`RaycastFilter.layer_mask` 只看成员位**这个语义只写在决策记录里 + 一条测试钉住，
  脚本 API 文档里也写了，但没有"与 Unity 对照"的说明（怕读者以为它等价于矩阵）。

### 六、下一步候选

- **触发器 / 事件**（`on_trigger_enter`）：层的下一站，`QueryFilter` 已经齐了；
- **质量设置（P2 #23）**：与层同属"项目级物理配置"，可以共用设置面板的节；
- **层预设**：把常用的"角色 / 环境 / 子弹 / UI"做成预设，一键铺矩阵；
- **编辑器资产 rename / move UI**（上轮遗留）。

### 验证结果

- `cargo test --workspace --all-targets -j 2 -- --test-threads=4`：**838 passed /
  0 failed / 7 ignored**（上一轮 821，本轮 +17）。
- `cargo clippy --workspace --all-targets`：**0 告警**（首轮 6 条 `identity_op`
  是测试里写 `u32::MAX & !(1 << 2)` 的冗余，已删）。
- 无头冒烟：`--check` / `--headless` 对 `projects/SampleScene` 退出码 0，
  且 `project.ron` **没有**被写入 `layers` 节（默认值不落盘这条真的成立）。

## 五十五、形变目标（morph target）：CPU 变形 + 顶点缓冲按需重写（2026-10-24）

### 一、为什么是它

`Docs/差距分析.md` 的 P1 #12 里，morph target 是动画系统最后一块**内容面**缺口。
它和别处的缺口不同 —— 不是"少了功能"，而是**静默丢数据**：
`gltf_loader.rs` 遇到 `targets[]` 时不报错、不崩，只打一条 `warn` 就跳过；动画里
的 `MorphTargetWeights` 通道同样直接 `continue`。导入一个带表情的角色，引擎照
常渲染，只是那张脸永远是基准形状，而没有第二个地方会提醒你。

选它的第二个理由：它一口气串起 **core（组件 / schema）→ render（加载 / 变形 /
顶点缓冲）→ animation（权重轨道）→ script（2 个 API）→ editor（Inspector 滑条）→
docs** 六层，正好把"数据在 core、能力在各层"这条主线再压一遍。

### 二、做了什么

**加载（`xgame-render/src/gltf_loader.rs`）**

- 新 `parse_morph_targets(mesh, primitive, buffers, base, scale)`：读
  `primitive.morph_targets()` 的 POSITION / NORMAL 增量，按 **target-major** 平铺
  （`positions[t * vertex_count + v]`）。
- **增量乘 `model_scale`**（法线增量不乘：均匀缩放不改方向，长度由归一化收回）。
  不乘的话，把导入缩放从 1 调到 2，模型大一倍而表情幅度纹丝不动。
- 目标名读 `mesh.extras.targetNames`（非标准但全行业导出器都写这里）；为此给
  gltf crate 打开 `extras` 特性，并加 `serde_json` 依赖 —— `extras` 在 gltf 里是
  **未解析的 JSON 原文**（`Box<RawValue>`），要自己反序列化一次。缺名字退回
  `Target N`。
- 默认权重读 `mesh.weights()`，按目标数补齐。
- `GltfPrimitive` 新增 `morph: Option<MorphTargets>`；带 morph 的图元顶点缓冲多一个
  `COPY_DST` 用途（其余图元的 usage 一位不变）。
- 动画解析：`GltfProperty::MorphTargetWeights` 不再跳过，落成
  `TrackPath::MorphWeights` 轨道（标量流按 `times.len()` 分组，`CUBICSPLINE`
  复用同一套"取中段值"降采样）。

**变形（新文件 `xgame-render/src/morph.rs`）**

- `MorphTargets`：增量 + 基准顶点 + 一个 `Mutex<MorphState>`（已应用权重 / 复用
  缓冲）。**纯函数 `morph_vertices`** 单独拎出来 —— 这是唯一能在无显卡 CI 上
  逐位断言的部件。
- `apply_source(queue, buffer, source)`：解析权重 → 与上次比对 → 变了才
  `morph_vertices` + `write_buffer`。**权重没变就不写**（静止的表情零开销）。
- 权重解析三层：实体短表用模型默认补齐、长表截断（换模型不错位到别的表情上）。
- 法线**只在真的加过增量之后**才归一化 —— 权重全 0 时连最后一位都不该动，
  否则"关掉表情"与"从来没有表情"在截图里会差出浮点级噪声。

**动画 / 渲染接线（`animation.rs`、`scene_pipeline.rs`）**

- `Track::stride()` 现算（`MorphWeights` 的 stride 是逐轨道的目标数，不能像
  变换通道写死 3/4）；`sample_clip_into` 显式跳过权重轨道。
- 新 `sample_morph_weights(clip, t, out)`：剪辑没有权重轨道时返回 `false` 且
  **不动**输出缓冲 —— 调用方据此退回实体权重，而不是把脸清零。
- `ScenePipeline::render` 里：先采剪辑权重（**不要求模型有蒙皮**，静态网格也能带
  morph），再按"剪辑 > 实体 `morph_weights`"选来源，逐图元 `apply_morph`。
  **顺序必须是形变 → 蒙皮**：形变改的是基础顶点，蒙皮矩阵随后才把它们搬到骨骼空间。

**core / 脚本 / 编辑器**

- `Entity.morph_weights: Vec<f32>`（`skip_serializing_if = "Vec::is_empty"`，
  旧场景文件 → 空表）；`schema.rs` 加目录项（穷举解构测试当场逼着改）。
- `GameCtx::morph` / `set_morph`：钳 `[0,1]`、拒绝 NaN、**允许稀疏写**、下标上限
  256、**立刻生效**（渲染每帧读场景，不走命令队列 —— 与 `set_layer` 不同，
  换层要同步物理侧）。脚本 API 160 个。
- Inspector 新增 Morph 区块：滑块逐目标，标题用 `targetNames`；
  模型没有 morph 时整段不显示（**不给 "Add" 按钮** —— morph 是资产属性，
  不是能凭空挂上去的组件）。扫的时候不扩容、拖动才扩容：进面板不该把场景改脏。

### 三、刻意的取舍

- **CPU 变形，不是 GPU storage 累加**。商业引擎走 GPU；这里 v1 走 CPU，理由
  与 CPU 粒子同构：只有权重变化才重算、面部网格量级很小（几千顶点 × 76 字节）、
  换来**零着色器改动 / 零新增绑定 / 零槽位分配**，而代价那一侧是一个能被单测
  钉死的纯函数。增量本来就是 target-major 平铺的，将来搬进 storage buffer
  不需要改数据布局。
- **法线增量不做导入缩放**：均匀缩放不改变方向，乘了反而要再归一化回去。
- **长度不自洽整段放弃**：增量错位会把顶点挪到不存在的位置，画出来是一团谁都
  认不出的几何，比"没有表情"难查得多。
- **`Mutex` 而不是 `&mut self`**：图元挂在 `GltfMesh` 上、渲染期只有 `&GltfMesh`
  （模型缓存共享给多个实体），改成可变借用会把"多实体共用一份网格"这条结构推翻。
  锁只在单线程渲染里出现，零竞争。
- **动画优先于手动权重**：Unity 同款语义 —— 被动画驱动的属性不听手动覆盖。
  想做"表情叠加"，就做成单独的剪辑。
- **不做 tangent 增量**：引擎的 PBR 不算法线贴图 TBN，收了没有消费者。

### 四、验证

- **先证红**：`morph_vertices` / 解析 / 采样各有断言（见下）；把
  `morph_deltas_are_baked_with_model_scale` 里的 `* scale` 去掉，
  它立刻红在 `[0.1, 0.2, 0.3] != [0.2, 0.4, 0.6]`。
- **合成 glTF**（不碰磁盘、不需要显卡）：测试里手写一个带两个目标、带
  `targetNames` / `weights` 的三角形，用 `gltf::import_slice` + 内嵌 base64
  缓冲跑通"解析 → 名字 / 默认权重 / 增量"整条 CPU 链路。仓库里的 `Fox.glb`
  没有 morph 目标，靠它只能测到"没有 morph 时返回 None"。
- **新增 20 条**：morph.rs 9（零权重逐位不变 / 满权重到目标 / 半权重 / 多目标累加 /
  钳位与 NaN / 法线归一化 / 越界忽略 / 补齐截断 / 错位增量被拒）、
  animation.rs 4（逐目标采样 / stride 现算 / 无轨道返回 false 且不清表 /
  权重轨道不碰姿态）、gltf_loader.rs 3（名字默认值目标数 / 增量烘焙缩放 /
  无 targets → None）、core 3（RON 往返 / 空表不落盘 / 旧文件兼容）、
  script 1（钳位 + 稀疏写 + 越界与非有限值拒绝 + 不进命令队列）。

### 五、还没做 / 已知边界

- **多网格角色的权重轨道只取第一条**：glTF 把权重轨道挂在节点上，一个角色理论上
  可以有多个带 morph 的网格（头 / 身体分开）各自一条轨道，当前取第一条并在
  文档里写明。
- **Inspector 显示的是第一个带 morph 图元的目标名**，目标数取所有图元的最大值
  （保证每一条都调得到）。
- **没有可视化的端到端验证**：仓库里没有带 morph 的样例资产，所以"改权重 → 画面
  真的变了"这条只能靠单测 + 代码审查保证；补一份带表情的样例资产要连 GUID /
  `.meta` 一起进，留作后续。
- **GPU 变形**：万级顶点 / 几十个角色同屏时再换（分层已留好）。

### 六、下一步候选

- **P1 剩下的手柄 / 触摸输入**（改动集中在 `xgame-input`，风险最小）；
- **动画 Phase B**（分层状态机 / 根运动 / 逐条转移时长）—— 动画阶段收口的最后一块；
- **带 morph 的样例资产**：把"改权重 → 画面变了"变成可截图的回归；
- **存档 / 读档**（P2 #32）—— 玩法侧最缺的一块。

### 验证结果

- `cargo test --workspace --all-targets -j 2 -- --test-threads=4`：**858 passed /
  0 failed / 7 ignored**（上一轮 838，本轮 +20）。
- `cargo clippy --workspace --all-targets`：**0 告警**（首轮 1 条
  `needless_range_loop` 在 `morph_vertices` 的权重循环上，改成 `iter().take().enumerate()`）。
- 先证红实跑：把 `parse_morph_targets` 里的 `* scale` 去掉，
  `morph_deltas_are_baked_with_model_scale` 立刻红（`0 passed; 1 failed`，
  `[0.1, 0.2, 0.3] != [0.2, 0.4, 0.6]`），恢复后绿。
- 无头冒烟：`--check projects/SampleScene` 退出码 0（scenes=1 / entities=24 /
  assets=15，全部引用可解析）。
- **一次偶发**：首轮全量跑里 `xgame-editor --bin` 进程崩过一次
  （`STATUS_ACCESS_VIOLATION`，4 线程并行跑 GPU 相关测试时）；单线程与复跑 4 线程
  均 189/189 绿，判定为环境偶发，未改动任何代码。若后续复现要查 wgpu 侧的并发建
  设备路径（`acquire_gpu_test_permit` 已串行化，但设备销毁时机不受它管）。

---

## 六十七、G0 资产引用迁移单文件写盘保护（2026-10-27）

`ProjectHandle::rewrite_literals` 之前直接 `fs::write` 覆写场景、预制体和材质：中途写入失败会截断旧内容；Windows 上目标允许写却禁止删除时，直接覆写还会绕过预期的替换失败。迁移继续保留原文注释和格式，只把写回收敛到主保存已有的 `write_atomic`，沿用同目录独占暂存、同步和替换。迁移报告仍按文件计数并收集错误；失败文件可在解除占用后重试。

Windows 回归 `migration_locked_target_preserves_original_and_reports_error` 以不共享删除的读句柄锁住老场景，验证逐文件报错、原字节及旧版暂存残留不变，再解除占用验证迁移成功。旧实现下会在错误断言处失败，因为 `fs::write` 在该句柄下可以直接覆写。此处只保证单文件替换；多个文件间仍可能部分完成，目录同步和残留回收另行处理。

验证：原实现定向回归 0 passed / 1 failed，报告误把锁定目标记为 `rewritten_scenes: 1`；改后定向回归通过。全工作区 `check --all-targets --locked`、`clippy --all-targets --locked -- -D warnings` 通过；冷构建时插件嵌套编译超出 240 秒门限，先明确预建同 profile 的两个 DLL，重跑 `test --workspace --all-targets --locked` 全绿。初次使用独立 `CARGO_TARGET_DIR` 是因为原 `target/debug/.cargo-build-lock` 拒绝访问；不将该环境故障计入功能回归。release workspace 在独立目录构建成功；三份 EXE 已逐一复制到 `dist` 并核对哈希。`dist/xgame-editor.exe` 对 SampleScene 的 `--check` 返回 0（1 场景 / 24 实体 / 15 资产），`--headless --frames 30` 返回 0（`ok=true`，30 固定步、0 错误、0 非有限变换）。此次无 UI 变更，未执行 UI 截图；不据此关闭 G0 的多文件事务与断电验收。

## 六十六、G0 `input.ron` 保存保护（2026-10-27）

### 事实与选择

`InputMap::save` 原来直接 `std::fs::write(path, text)`；编辑器 Play、无头与独立游戏从 `input.ron` 加载键位映射，损坏会导致解析失败。上一轮 `.meta` 写入保护完成，输入配置是 G0 剩余的单文件写入风险；输入 crate 不依赖 core，本轮不新增依赖。

### 红绿与实现

Windows 测试先打开目标并允许读写、禁止替换：旧实现竟返回成功，新断言 1 failed。新保存用同目录独占暂存（pid + 序号），写入同步后替换目标；替换失败返回错误，旧字节保持不变。解锁后再保存，`InputMap::load` 读回新绑定；旧 `.ron.tmp` 残留在失败和成功保存后仍保留，不阻断写入。输入库 14 项测试通过。

### 边界

本轮只修改 `InputMap::save`。全工作区 `check` / 严格 `clippy` 已通过，release 构建刷新 dist；dist 编辑器 `--check projects/SampleScene` 返回 OK，三 EXE 与 release 哈希一致。首次全量测试期间仍在写源码与并行构建 release，三个插件加载测试因宿主/DLL 构建指纹不匹配失败；随后单独复验插件集成测试 6/6 通过。复验时同一工作区另有迁移写入代码的并行修改；全量测试退出 0（包含本轮 1 项及对方 1 项新增测试，合计 921 passed / 7 ignored），不将迁移变更归入本轮输入修复。异常退出的暂存文件清理与断电目录持久化未验证，资产迁移的多文件事务仍需专项处理；不据此关闭 G0。

## 六十五、G0 资产 `.meta` 保存可靠性（2026-10-27）

### 事实与选择

G0 的主文件暂存冲突已处理，但 `AssetDatabase::ensure_meta`、`ensure_metas` 和 `set_import` 仍直接写目标 `.meta`。这是资产 GUID 与导入设置的持久身份：直接截断可能损坏旧设置；批量扫描后若另一写入者抢先生成，旧实现还会覆盖其身份。此次只处理资产元数据写侧，不新增依赖。

### 实施与红绿验证

Windows 下先新增 `import_update_failure_preserves_meta_and_index`：打开旧文件并允许读写、禁止替换；旧 `set_import` 返回成功，测试断言报错失败（1 failed）。新实现更新走现有 `project::write_atomic`，失败旧字节与索引均不变，解除占用后更新成功。首次生成使用同目录暂存 + `hard_link` 原子创建目标；目标已存在时拒绝覆盖，批量生成保留错误并等待 `refresh` 读入后来创建的 GUID。新冲突测试检查这条路径；core 263 项测试通过。

### 尚存边界

失败或异常中止后的暂存清理、目录断电持久化、批量生成跨资产事务以及 `input.ron` / 迁移文件写入仍待处理；本轮结果不表示 G0 全部关闭。本轮全工作区 `check --all-targets --locked -j 2`、`clippy --all-targets --locked -D warnings`、`test --all-targets --locked` 均通过（原有 917 项 + 新增 2 项 = 919 passed，7 ignored）；release workspace 构建成功，dist 三个程序与 release 哈希一致；dist 编辑器对 SampleScene 实际 `--check` 返回 0（1 场景、24 实体、15 资产）。本轮未复跑桌面窗口 GPU 组合验收。

## 六十六、G0 打包拒绝旧引擎播放器（2026-10-27）

接 173「通用播放器选择的新鲜度仍未关闭」。`builder::locate_player` 只判"文件存在、
取第一个候选"，于是"改完引擎只重编编辑器"就会把旧引擎的 `xgame-game.exe` 打进成品，
没有任何征兆。

### 实现

- `crates/xgame/build.rs` 增加第二份指纹 `xgame-src-v1:…`：与插件用的
  `xgame-build-v1:…` 同一套输入，但**排除** `PROFILE` / `OPT_LEVEL` / `DEBUG`，
  所以 debug 编辑器配 release 播放器仍算同一份源码。
- `xgame::plugin::SOURCE_FINGERPRINT` + `embeds_source_fingerprint(bytes)`；
  常量用 `#[used]` 锚点保住 —— 实测没有锚点时链接器会把没人引用的常量整段丢掉
  （`xgame-game.exe` 里搜不到 `xgame-src-v1:`），插件的 `@BUILD_FINGERPRINT` 之所以
  健在是因为握手代码真的读它。
- `builder::pick_player`：顺序不变，但跳过源码不一致的候选；跳过时写一条
  `BuildReport::warnings`，全部不一致则 `BuildError::PlayerStale`（含重编命令与
  已查过的路径）。`build_with_player` 的闭包类型从 `PathBuf` 换成 `PlayerPick`，
  测试用 `PlayerPick::plain` 绕过指纹判据。

### 红绿与端到端

新增能力，旧代码没有对应分支可先红；改为**真实二进制负对照**，比单测更硬：

- 阳性：`dist/xgame-editor.exe` + `dist/xgame-game.exe`（同批）→ `--build` 退出 0。
- 负对照 1（旧引擎）：把包内播放器的 `xgame-src-v1:` 后一个十六进制字符改掉 → 同一命令退出 **1**，错误里带"不是同一份引擎源码…cargo build -p xgame-game"。
- 负对照 2（跳过旧的用新的）：把改过的播放器放回编辑器同目录、另放一份正确播放器到 `outputs/release/` → 退出 **0**，warning 明确写出"跳过了 …（用的是 …）"。
- 单测 4 条：指纹格式与两份不相等、整段相等才命中（少一字节不命中）、旧候选在前时仍选新的并留提醒、第一个就对时无噪音。证据目录 `outputs/freshness/`。

### 全量验证与发布

- check / clippy `-D warnings` / test 全绿：27 套件 **942 passed / 0 failed / 8 ignored**。
- release workspace 同批重建、`一键构建.cmd` 刷新 dist（最后一次按提交后的 HEAD 源码重建）；dist 两个可执行文件与示例插件 DLL 的两份指纹都一致（`xgame-src-v1:6961134d…`、`xgame-build-v1:bbdd032f…`）。示例插件 DLL 刷新为 `12F8A994…3AB1`（引擎源码变了，指纹门禁要求同批），BehaviorsDemo 无头 60 帧零 error，SampleScene / BehaviorsDemo `--check` 均退出 0。

### 边界

只比源码指纹，不做内容签名，不防篡改。指纹的输入是**各 lib crate 的 `src`、各
`Cargo.toml` / `Cargo.lock`、工具链与 flags**（编辑器自己的 bin 代码不在其中 ——
它不会进播放器），所以动了这些就得先重编播放器再打包；`一键构建.cmd` 本身满足。
完整发布端到端仍只有本地脚本验收，没有接进 hosted CI。G0 不整体关闭。

## 六十五、G0 写盘实现统一与遗留暂存回收（2026-10-27）

从 8b9e14a 继续，沿 G0「保存可靠性」主线只处理写盘与暂存残留。前置事实有两条：同一段「同目录独占暂存 + `sync_all` + 替换」在 `core::project`、`editor::prefs`、`input::action` 各有一份，而且三份的差错点各不相同（一份补建父目录、一份支持"只发布到空位"、一份两者都没有）；差距分析与 PLAN 六十四 都记着"历史暂存不自动回收"，项目目录会一直攒垃圾。

### 实现

- 新增 `xgame_core::fsx`，三个入口：`write_atomic`（替换发布）、`write_atomic_new`（`hard_link` 只发布到空位，保护首次分配的 GUID）、`sweep_staging`（超龄暂存回收）；发布成功后追加父目录 fsync（仅 POSIX 有效）。三处副本改成委派，`ProjectHandle` 的各个保存入口与 `.meta` 写入的调用点不变。
- 命名即判据：暂存叫 `<主名>.<扩展名>.tmp.<进程号>.<序号>`，回收只认收尾**恰好两段数字**的名字。单段数字（`notes.tmp.5`）与旧版固定名（`scene.ron.tmp`）跟用户文件无法区分，猜错就是删用户文件，所以一律不动。阈值 600 秒，防止误删并发写入者正在用的那份。
- 回收挂在发布成功之后，**不在** `ProjectHandle::open`：后者是 `--check` 的入口，检查必须只读（§十四）。
- `xgame-input` 因此新增 `xgame-core` 依赖；§四十五 里"输入 crate 不引入 core 依赖"那条边界由决策记录 §四十九 明确取消，方向仍是向下。

### 红绿

- 先写 `project::tests::main_save_reclaims_abandoned_staging`，在旧实现实跑失败：`crates/xgame-core/src/project.rs:1646: 超龄暂存应被回收`（0 passed / 1 failed / 266 filtered）。同一条用例同时钉住三件事：超龄的回收、新写下的不动、不匹配命名的用户文件不动。
- 实现后补 `fsx` 的 9 条：命名匹配表（含 `notes.tmp.5` / `scene.ron.tmp` / `scene.ron.tmp..7` 等反例）、只回收超龄、零阈值全收、写盘不留暂存且补建父目录、`write_atomic_new` 拒绝已存在目标、保存顺手回收、覆盖替换、Windows 锁定目标时保留旧字节且失败路径不留暂存，以及**四写一读并发**（读到的永远是某一次完整写入；替换瞬间的共享冲突不算问题，只对读成功的结果断言）。并发那条第一版自己等自己死锁过，改成"写者全退读者才退"的计数式退出。

### 成本

release 实测（本机，`fsx::tests::sweep_cost`，默认 ignore）：扫描 100 个条目的目录 82.5 µs（1000 次 82.5 ms），一次写盘 4.26 ms（200 次 852 ms，`fsync` 主导）。回收约占一次保存的 2%，不做额外节流 —— 加节流就要引入进程级状态与"这次到底扫没扫"的不确定性，不值。

### 全量验证与发布

- workspace `check --all-targets --locked`、`clippy -D warnings`、`test --all-targets --locked -- --test-threads=4` 全部退出 0：27 套件 **942 passed / 0 failed / 8 ignored**（本轮新增 9 条，含 1 条默认 ignore 的成本测量）。
- release workspace 构建成功；`一键构建.cmd`（`tools/build-release.ps1`）退出 0，dist 三个 EXE 刷新。
- 冒烟：dist 版 `--check` 对 SampleScene 退出 0（1 场景 / 24 实体 / 15 资产）、对 BehaviorsDemo 退出 0；`--headless --frames 120 --screenshot`（BehaviorsDemo）与 60 帧 SampleScene 均退出 0，截图已查看（地面 + 方块 + 球，阴影正常）。
- 插件指纹：本轮改的是引擎源码，示例 DLL 的构建指纹随之失效。先做负对照 —— 把 §六十一 之前的旧 DLL 换回去，无头实跑打出 `插件加载失败，已跳过：… 缺少构建指纹符号 xgame_plugin_descriptor_v1`；换回同批 release DLL 后同一命令零 error。`projects/BehaviorsDemo/Plugins/behaviors_demo.dll` 已刷新，SHA-256 `36532B20…C1E1A`。

### 边界

同一目标的多进程同时保存仍是最后完成者胜出；多文件迁移不是整体事务；Windows 上没有目录 fsync（只剩 NTFS 元数据日志），断电后的目录持久性仍未验证。旧固定 `.tmp` 与打包临时目录 `.xgame-staging-*` 不回收 —— 前者与用户文件无法区分，后者内含 `project.ron`，已被扫描器当成"另一个项目"跳过，不会污染资产库，只是占盘。G0 不整体关闭。

## 六十四、G0 主保存残留暂存文件阻塞（2026-10-27）

本轮只处理主保存暂存冲突。此前 `ProjectHandle::write_atomic` 固定用 `<目标>.tmp` 且 `create_new`；进程异常退出遗留暂存文件后，清单、场景、预制体、材质和自动保存均可能持续报 `AlreadyExists`。回归 `stale_staging_file_does_not_block_main_saves` 已在旧实现实际运行失败（Windows code 80，project.ron 暂存已存在）。

现改为目标同目录的 `<扩展名>.tmp.<进程号>.<原子序号>`，`create_new` 独占；名字碰撞继续取下一序号，仅清理本次创建且写入/替换失败的暂存文件，不碰旧残留。沿用 `sync_all` 后 rename 的原子替换，禁止直接覆写目标。旧版暂存目录与文件均不阻断新保存；编辑器 Apply 原测试的固定 tmp 注入改为 Windows 禁止替换的目标文件句柄，以稳定验证失败时实例节点不绑定、目标字节不变。

**已验证**：旧实现回归先红；修改后 core 261 项通过；workspace `check --all-targets --locked -j 2`、clippy `-D warnings`、`test --all-targets --locked -j 2 -- --test-threads=4` 均退出 0；SampleScene debug 与 dist release `--check` 均退出 0（1 场景、24 实体、15 资产）。release workspace 构建成功，dist 三个 EXE 与 target/release 对应文件哈希相同。`cargo fmt --all -- --check` 对仓库大量历史文件报差异，未批量格式化，也不宣称格式门禁通过。

**遗留与下一步**：未自动回收历史暂存文件，其他写入入口及断电目录持久化未专项验收；同一目标的多进程同时保存仍是最后完成者胜出。G0 保存可靠性下一步可选单独核验 meta/input/迁移写入的失败保护。本轮未复跑桌面 GPU 发布包三关验收，不能用此前历史结果冒充本轮结果。

## 六十三、G3 跨平台构建的实施顺序（2026-10-27，计划，尚未实施）

用户要求先把其他平台构建纳入计划，之后再实现。本节记录实施取舍；当前没有修改跨平台代码、CI 或发布脚本，也没有运行 Linux/macOS/Web 的构建和验证。正式阶段和排序以差距分析「跨平台构建计划」为准，ROADMAP 仅记待做状态。

先做 Linux 桌面原生闭环：核对目标机器的 Rust 与系统图形/音频依赖 → 同平台 check/clippy/test → 示例项目无头检查和运行 → 编辑器与独立播放器窗口启动 → 同平台程序、项目资源及插件一起打成包，移走源码路径后验实际输入、切关、插件和正常退出。发现平台假设时用该平台的编译错误与最小复现逐个修，不在 Windows 上猜 Linux API。首轮实机通过之后再加 Linux CI；无 GPU 的 CI 只证明构建/无头流程，不能冒充渲染/窗口已通过。

Linux 有可重复证据后，选 macOS 主机重复原生闭环；重点检查 Metal 后端、资源查找、动态库后缀及分发目录形式。macOS 的 CI 与桌面 GPU 门禁按各自环境能力分开记录。跨平台导出预设要基于实际成功的平台产物，不凭重命名 Windows 可执行文件提供选项。Web 与桌面插件架构差异明显，先核对目标功能范围和依赖/文件系统/音频/脚本/插件限制，待专项立项再定义验收标准。

停止条件：单个平台完成真实构建、启动、验证核心流程及独立包的本机可运行状态，或出现目标机器/依赖等不可绕过的外部阻塞；每次只交付一个已验证平台切片并记录未覆盖项。当前 Windows 仍是唯一实测平台，本节的任何步骤均不计为已完成。

## 六十二、G0 真实发布包自定义输入 / 切关 / 插件（2026-10-27）

从干净的 9226714 继续，只补真实发布组合验收。已读 ROADMAP、最新差距、插件/场景/CI 决策、PLAN、builder、game 输入桥及现有测试。现有 Rust 测试没有桌面发布驱动；采用 Windows PowerShell 验收脚本调用真实 --build 和播放器，复用 BehaviorsDemo 的模型场景、Spinner DLL，未修改引擎 Rust 源码、依赖、架构或 CI。

### 实测与证据

- 新脚本 `tools/test-published-player.ps1` 生成三关和自定义 Gameplay 脚本目录、J 键 Advance 动作/Travel 轴，原字节打包验证。包移出源目录、源目录改名保留，以非项目 cwd 无参数运行；路径定位不靠源项目。每关 5 个唯一日志断言（START/PLUGIN/PRESS/RELEASE/DESTROY），插件证据为只有 Rust Spinner 会修改的实体 yaw 变化超过 5°。按下与松开分帧等待确认，松开才切关。无新引擎 bug，不伪造旧实现红灯。
- 正例 grxfz9 退出 0：`outputs/published-player-36723efff89f49bfaafd2d12ab5c4eb4/`。最终刷新 dist 后 a0ribz 再次退出 0：`outputs/published-player-ad75db134051426884e30ac0121f3e7e/`，result.json ok=true、scenes=3、assertions=15、exit_code=0；player.log 明确 Scenes/0→1→2，两次切关，三次插件旋转、三次按下/松开与三次销毁，零 ERROR/panic。
- 负对照 q48t9g：错绑为 K，`outputs/published-player-4603d232c980494a894aeb67652c1abb/`，超时 G0 PRESS 0，任务失败。g4olll：省略 DLL，`outputs/published-player-6daf03d153ad4866a918b0a7638fd96f/`，超时 G0 PLUGIN 0，任务失败。均正常发窗口关闭消息，不强杀，不删证据。
- 最终三张 scene-0.png/scene-1.png/scene-2.png 已由主代理查看；窗口有地面、两立方体、球体及阴影。日志确认 RTX 2060/Vulkan/610.88，HDR+ACES、MSAA x4。三关使用相同视觉夹具，截图相同不作为切关证据，身份依据脚本和运行时日志。保留 Vulkan present mode 警告，不称零 warning。
- 脚本开发期失败仅为夹具漏填必填字段、PowerShell 重复大小写代理键、日志共享读取、无 BOM 中文注释解析导致句柄赋值被吞；不是引擎缺陷。重复代理仅在调用进程删除小写键，不改系统配置。脚本最终 UTF-8 BOM，无 Rust 文件需要 rustfmt。

### 全量与发布

沿用 `.workbuddy/validate-g0.ps1` 的 LIB/INCLUDE/TEMP，全部 cargo 为 PowerShell 绝对路径、CARGO_INCREMENTAL=0、-j2 --locked，测试 --test-threads=4。任务 iqo8km check/clippy -D warnings/test 全部退出 0；27 套件 **917 passed / 0 failed / 7 ignored**，日志 `.workbuddy/logs/g0-published-{check,clippy,test}.log`。任务 wxyvwp release workspace 退出 0，日志 `g0-published-release.log`。PowerShell 将原生命令 stderr 包装成 NativeCommandError 的展示不等于 cargo 失败，最终以各命令 LASTEXITCODE 及 Finished/test result 判断。

仅复制刷新 dist 三 EXE，不清目录，SHA-256 与 target/release 一致且与上一轮相同：editor EA74558BFA5F9773C2222D6E4D7A69378153B1EE44A8DF1A8F1E8AB96D6DA2AD；game CA0055FBE4C9B8F95CA8D5CEAF81967713BDF3854A08B961B39121165B454C85；mcp 4E73645FE8DA2F4C666598B88D06EFC7F7F78BB9A65E168F032F2BB8381450F4。本轮未改指纹源码；Demo 现有 DLL 与同批 release 哈希均为 6F722DCCAEE062549718DF2D89431E90585A9823C4E81A5FB636BA0650544744，无需替换，上一轮备份保留。测试包复制同批 release DLL 并校验哈希。

### 边界

新增本地端到端验收（真实播放器/GPU/窗口消息链），不计入 Rust 单测数量。未验证物理键盘、系统焦点、IME、鼠标/手柄、任意项目依赖、跨平台、通用播放器新鲜度；现有 hosted CI 未接桌面 GPU 门禁。没有新依赖与播放器烟测特例，不复制 World 循环，不用 headless 充当真实播放器。G0 不整体关闭。ROADMAP/差距/决策/README 已同步，日记 append-only，提交推送不跳 hooks。

## 六十一、G0 插件注册前兼容检查（2026-10-27）

### 实现与契约

先纠正 plugin.rs、决策记录 §四及双轨计划 C4 中“版本一致即可推出同编译器/依赖/flags”的错误说明。新增标准库 build.rs，以固定摘要算法覆盖 rustc -vV、TARGET/HOST、PROFILE、OPT_LEVEL、DEBUG、panic、target features、xgame features、可见 rustflags/wrapper，以及引擎库源码、Cargo 清单/锁文件/构建脚本；输入排序，文件标识使用仓库相对路径，导出描述符不含绝对路径。无新增依赖。

export_plugin! 增加 xgame_plugin_descriptor_v1：固定 C ABI，字节缓冲区+容量，返回所需长度，空指针/容量不足不写。LoadedPlugin::register_into 首先检查符号与长度/字节，失败不调用原 entry；PluginHost 布局及原版本握手保留。测试 DLL 仅定位当前测试 profile，嵌套构建同 profile，不再回退另一 profile 的历史 DLL。

### 实跑红绿证据

- 修改加载器之前，任务 v7izur 执行 `cargo test -p xgame --test plugin_loading incompatible_plugin --locked -j2 -- --test-threads=4 --nocapture`：**0 passed / 2 failed / 4 filtered**。缺指纹和错误指纹的标准库真实 cdylib 均被旧加载器返回 Ok，入口计数均为 **1**（期望 0），并非编译或环境失败。
- 修改后 ben3ct：xgame 全目标 **16 passed / 0 failed**；补同长度错指纹及描述符空指针/容量/尾部哨兵检查后 boih2u：三个包定向 check/clippy 通过，**21 passed / 0 failed**。两个负例均 entry=0；spinner 与 BehaviorsDemo 真实 DLL 注册及 World 驱动通过。
- 本轮仅 rustfmt `crates/xgame/build.rs`、`crates/xgame/src/plugin.rs`、`crates/xgame/tests/plugin_loading.rs`。首次工具缺失，rustup component add rustfmt 成功后格式化；之后重新构建指纹，源码未再修改。

### 全量命令与数量

全部 cargo 在 PowerShell 执行，绝对路径 `C:/Users/Administrator/.cargo/bin/cargo.exe`，`CARGO_INCREMENTAL=0`、`-j2 --locked`、测试线程 4；环境来自已读取的 `.workbuddy/validate-g0.ps1`（MSVC onecore/Windows SDK LIB 与 INCLUDE，E 盘 TEMP/TMP），未猜路径。

```powershell
& $cargo check --locked --workspace --all-targets -j2
& $cargo clippy --locked --workspace --all-targets -j2 -- -D warnings
& $cargo test --locked --workspace --all-targets -j2 -- --test-threads=4
& $cargo build --locked --release --workspace -j2
```

任务 51vfu4 全量退出 0；Transcript 未捕获原生命令的输出，因此任务 1p7vzm 重跑并直接捕获输出，退出 0。最终日志 `.workbuddy/logs/g0-plugin-full-captured.log`：**27 套件、917 passed / 0 failed / 7 ignored**。7 项为默认忽略测试，不把返回 ok 的可跳过 GPU 用例一律视为 GPU 实跑。release 任务 1e01dn 退出 0（35.25 秒）；其真实工具输出记录 xgame、两个插件和 editor/game 编译及 Finished release，Transcript 本身不含原生输出，不能以其空内容作编译证据。

### 发布刷新与真实 GPU 证据

没有运行 build-release.ps1，没有清 dist 或删除缓存。复制 target/release 三 EXE 到 dist，并逐个校验 SHA-256 相同：

- editor：EA74558BFA5F9773C2222D6E4D7A69378153B1EE44A8DF1A8F1E8AB96D6DA2AD
- game：CA0055FBE4C9B8F95CA8D5CEAF81967713BDF3854A08B961B39121165B454C85
- mcp：4E73645FE8DA2F4C666598B88D06EFC7F7F78BB9A65E168F032F2BB8381450F4
- 同批 behaviors_demo.dll：6F722DCCAEE062549718DF2D89431E90585A9823C4E81A5FB636BA0650544744；复制到 projects/BehaviorsDemo/Plugins 后哈希相同。

旧 DLL 以禁止覆盖方式备份在 `projects/BehaviorsDemo/Plugins/behaviors_demo.dll.pre-g0-compat-f658ef94be2946f982e15b95f17297fa.bak`，旧哈希 C6C5112A61905CAD0071451C86245BC02B790348801F3231895CC891EF2C383D。

任务 gf1ar7 实跑：

```powershell
./dist/xgame-editor.exe --headless --frames 120 --screenshot outputs/plugin-compat-smoke.png --report .workbuddy/logs/g0-plugin-smoke.json --dump-scene .workbuddy/logs/g0-plugin-smoke-scene.ron projects/BehaviorsDemo
./dist/xgame-editor.exe --check projects/SampleScene
```

两者退出 **0**。GPU 日志确认 **NVIDIA GeForce RTX 2060 / Vulkan / driver 610.88**，并非无适配器跳过；PNG 已查看，地面、立方体、球体及阴影可见。报告 ok=true、120 frames/120 fixed_steps、4 entities、logs info=2/warn=0/error=0、non_finite_transforms=0、scene_loads=0；截图 1280×720、2 draw calls、4 opaque instances。场景转储 RustSpinner 四元数 `(0, 0.9659258, 0, 0.25881907)`，对应 150°，与 LuaSpinner 相同，证明真实插件已驱动实体。audio.available=false 为静音降级。

SampleScene 检查：scenes=1、entities=24、lights=1、script_refs=5、audio_refs=1、material_refs=7、assets=15，all referenced assets exist。完整日志 `.workbuddy/logs/g0-plugin-smoke.log`、`g0-plugin-smoke.json`、`g0-plugin-smoke-scene.ron`、`g0-plugin-sample-check.log`；截图 `outputs/plugin-compat-smoke.png`。

### 边界与交付

仅当前 workspace 的 xgame 构建单元保守检查，非完整 Rust ABI 证明；无法覆盖下游全部 flags、依赖 feature 合并、第三方源码状态，OS 动态库加载时的初始化发生在描述符校验之前。摘要不作密码学认证，匹配不证明任意插件可信；源码非 ABI 改动也可能触发保守拒绝。不宣称通用播放器新鲜度、完整包输入/切关/插件 CI 或全部 G0 已关闭。ROADMAP 顶端、差距分析顶端及 128/173、README 插件用法已同步；每日日志 append-only。按用户本轮授权提交推送，不跳 hooks。

## 六十、发布副本污染原项目资产扫描（2026-10-27）

从已推送的 4df0eae 继续。根因是资产库递归扫描项目根，真实 Build 包复制了源资产的 `.meta`，导致重复 GUID。先新增回归，在未改生产实现时任务 VxIaUW 实测 **0 passed / 1 failed**：`Build/Game/Resources/Box.glb` 与源文件冲突，包内 `Missing.png` 还污染缺失 meta 诊断。旧 dist 对实际 SampleScene（保留既有 Build 包 35 文件）复验同样 **15 个冲突，退出 2**，任务 wjxjsw。

### 设计审查

阅读 core 扫描/刷新、ContentConfig、BuildConfig 和 builder 的内容收集、路径解析、输出重叠拒绝后，确认当前没有可配置 output 字段。采用有效嵌套项目清单作为独立包边界，同时让显式内容根优先；完整约束只写在决策记录 §十四。没有增加 Build 名称黑名单，也没有缩窄为内容目录白名单。普通 Build 资产、散放资产、无效清单目录和显式声明的内容继续扫描；自定义存放位置的完整包也能隔离。没有修改构建器、运行时和渲染。

### 回归与全量

- 新增 2 项自动化测试：默认/自定义目录下发布副本隔离（含 refresh、GUID 解析、包内独立扫描和 ensure_metas 不写包），以及 9 个合法资产的内容根/祖先/子树保护。
- 定向扫描 **4 passed / 0 failed**（e61M6L），其中新增两项；随后补充内容根祖先场景由最终全量覆盖。
- `--locked --workspace --all-targets` check 与 clippy `-D warnings` 通过；全量任务 arnl0i **27 套件，914 passed / 0 failed / 7 ignored**。日志 `.workbuddy/logs/g0-build-scan-full.log`。
- 使用既有 `.workbuddy/validate-g0.ps1` 的工具链、E 盘 TEMP、MSVC/SDK 环境。首次裸 cargo 因 PATH 不含 cargo 未运行，改用既有安装绝对路径；不将环境失败当回归红灯。
- 没有单独重跑上一批双角色 GPU 专项；全量测试包含既有渲染测试。

### 发布实测

- release workspace 任务 eowc2h 退出 0，日志 `.workbuddy/logs/g0-build-scan-release.log`。dist 三 EXE 已刷新，与 target/release SHA-256 逐个相同：editor `58E1C9FB70E90A9E3251707B3B645014E460508EBCB355FB8F695E4287399DA4`；game `B0FF29FEA81595DFC640C72FC2809415F99D5FC6BB2A56622A8C39A0B8169559`；mcp `4E73645FE8DA2F4C666598B88D06EFC7F7F78BB9A65E168F032F2BB8381450F4`。
- 任务 kis5u7：不删除、不重建已有 Build 包，先用新 dist 检查原项目与旧包，**两次退出 0**。旧包原有 35 个文件完整保留到这次验证。
- 任务 khfhpu：真实 `--build projects/SampleScene` 退出 0，33 个资产文件，目录 Prefabs/Resources/Scenes/Scripts；包内合计 35 文件，播放器 SHA-256 与新 dist game 相同。之后原项目与新包 `--check` **再次均退出 0**。
- 四次成功检查计数一致：scenes=1、entities=24、lights=1、script_refs=5、audio_refs=1、material_refs=7、assets=15，零资产问题。最终实际 Build 目录仍在原位置，未通过删除掩盖。
- SampleScene 未配置发布关卡清单，scene_count=0；本轮不声称完成真实输入/切关/插件或 ABI 门禁。只读扫描与发布隔离问题已修复，其余 G0 边界沿用差距分析。
- 已更新既有 ROADMAP、差距分析、决策记录及本地日记 `.workbuddy/memory/2026-10-27.md`；日志、日记、dist 和实际 Build 按既有规则不入库。

## 五十九、G0 异骨数双角色 GPU 姿态与阴影验收（2026-10-27）

从 545406c 干净工作区继续，按正式优先级选择完整交付 R03/031 的 GPU 证据。既有固定槽距实现未发现新缺陷，本轮新增自动化回归而非声称再次修复运行时。其余包输入/切关/插件及 ABI 指纹继续保留。

### 验证设计与红绿证据

在现有 `screenshot.rs` 测试模块合成 2 骨、5 骨两个角色，顶点绑定各自最后一根骨，动画沿相反方向平移；同一个 ScreenshotRenderer 重用 GPU 缓冲，依次采样 0、0.5、1、0 秒并交换两角色实体顺序。参考画面直接平移 CPU 顶点，不复用骨骼采样/上传；两条路径使用同一保守盒隔离阴影拟合影响。主画面与阴影分别一项测试，要求逐像素完全一致、姿态改变确实改变像素、左右各有超过 50 个阴影变暗像素。测试断言三个实例参与绘制，避免空画面假绿。

- 当前实现初跑两项通过，日志 `.workbuddy/logs/g0-skin-initial.log`。
- 仅临时移除 `append_bone_slot` 的补齐逻辑，模拟 R03 历史上传行为；任务 `8id0rl` 实际 **0 passed / 2 failed**，t=0 主画面差异 **1224** 像素，含阴影差异 **1629** 像素；日志 `g0-skin-red.log`。这是历史缺陷补证，不是 HEAD 新缺陷。
- Edit 恢复补齐后任务 `t7lp2u` **2 passed / 0 failed**，四姿态、双顺序均执行，日志 `g0-skin-green.log`。生产文件 `scene_pipeline.rs` 最终无 diff。
- `XGAME_GPU_EVIDENCE_DIR` 可指向已存在目录，保存八张 `dual-skin-shadow-{true,false}-{0..3}.png`；指定证据目录却拿不到 GPU 时必须失败，普通 CI 沿用无 GPU 跳过规则。
- 全量首遍 clippy 拒绝新增测试的多余借用及 chunks_exact 用法，已修正；此编译/lint 失败不算功能红灯。rustfmt 当前工具链未安装，未宣称格式化通过。

### 全量与发布结果

沿用项目 PowerShell 环境脚本，cargo 绝对路径、CARGO_INCREMENTAL=0、-j2、测试线程 4，TEMP/TMP 在 E 盘项目内，LIB/INCLUDE 使用既有 MSVC onecore 与 SDK。

- 最终任务 `0daw80`：check、clippy `-D warnings` 均退出 0；27 套件 **912 passed / 0 failed / 7 ignored**，新增两项 GPU 测试通过且指定证据目录，不允许无 GPU 假跳过。日志 `.workbuddy/logs/g0-skin-full-retry.log`。
- 前一遍 `cu1daq` 在 spinner-plugin 测试链接报 LNK1104，目标 EXE 不存在且没有编译残留进程，C/E 空间充足；未删除或修改权限，原命令重试通过。失败日志 `g0-skin-full-final.log` 保留，不将瞬态失败说成永久解决。
- release workspace 任务 `j61ik6` 成功，日志 `g0-skin-release.log`。dist 三 EXE 已刷新，SHA-256 与 target/release 逐个相同：editor `4D53183B977020EFF91686DDE21BCEF546EC7D2BEE9C7585EF2FCF0D16998F57`；game `002FBDF0120D120CFA335AA6CF144386BFB4A4FBC4A12C99001F250076A12D11`；mcp `A442A350C519FACC36CFC2EC6743B5A9AC336925CD7E75ADDE0EC6DE22F82270`。
- 原项目 SampleScene `--check` **退出 2**：上轮 Build/SampleScene 的 15 份资产被递归扫描导致重复 GUID。证据 `g0-skin-check.json`（实际为文本）；未删除旧包掩盖问题，登记为后续 G0。
- 原项目 120 帧 GPU 截图、真实 `--build`、新包目录 `--check` 与 120 帧 headless 全部退出 0（任务 `g6pp25`）。两份运行报告均 ok=true、120 fixed_steps、error/warn=0、non_finite_transforms=0；音频 available=false 属静音降级。日志 `g0-skin-smoke.json`、`g0-skin-build.json`、`g0-skin-package-check.log`、`g0-skin-package.json`。
- 新包 `projects/SampleScene/Build/SampleScene`；未启动包内玩家窗口或注入输入，scene_loads=0，不当作完整发布端到端验收。
- 截图 `.workbuddy/shots/dual-skin-shadow-{true,false}-{0..3}.png` 共八张与 `g0-skin-SampleScene.png`；已查看初态/末态阴影图及 SampleScene，场景、狐狸、阴影、UI 可见。
- `git diff --check` 通过；本地项目日记 `.workbuddy/memory/2026-10-27.md` 已更新，按既有忽略规则不纳入 Git。

### 交付边界

这是合成 GPU 槽位回归：覆盖异骨数、末关节索引、动态平移、槽位重排、缓冲复用和方向光阴影。不等于真实 glTF 导入、多关节混合权重、旋转/层级/非单位逆绑定、任意骨骼位移的保守 bounds、动态 TAA 或跨 GPU 逐位一致性。未新增第二份运行时或更改 crate 依赖方向。当前工具没有独立代理入口，使用并行只读检查；未清理个人目录、修改系统权限。

## 五十八、G0 morph 保守包围盒与发布复验（2026-10-27）

从 e19df1f 干净工作区继续，按正式 G0 优先级处理 029。现有 glTF bounds 只并原始顶点，而视锥剔除早于 morph 采样，导致原始姿态在视野外、形变后进入视野的实体整帧消失。导入时按每顶点全部目标的正负增量计算 `[0,1]` 全权重组合保守盒，合并到共享 mesh bounds；相机剔除、阴影拟合、自动取景沿用同一数据源。无 morph 路径不变，不引入每帧扫描。

### 先红后绿

- 新增真实 glTF 导入测试 `morph_import_bounds_cover_combined_targets_and_scale` 与 GPU 测试 `morph_culling_matches_unculled_pixels`。旧实现任务 `8sa61b`：7 passed / 2 failed，分别失败于“X 形变必须进入包围盒”和“形变后剔除开关必须逐像素一致”。GPU 测试先确认形变确实进入画面，避免空画面假绿。
- 初次测试的私有模块引用编译错误、临时文件同名导致 PermissionDenied 不计有效红灯；修正为公开 re-export 与进程内唯一临时名后才获得上段双红。
- 修改后任务 `k46x73`：10 passed / 0 failed，包含上述两项 GPU 测试及新增纯 CPU 正负增量/分数权重测试。GPU 测试未 skip，逐次切换权重后开关剔除逐字节一致；导入测试覆盖 scale 1/2 及同时启用多个目标。

### 全量与交付验证

使用 PowerShell cargo 绝对路径、CARGO_INCREMENTAL=0、`-j2`、`--test-threads=4`，LIB/INCLUDE 按构建脚本选择 onecore 与 Windows SDK，TEMP/TMP 指向项目 `.workbuddy/tmp`。

- `cargo check --locked --workspace --all-targets -j2`：退出 0。
- `cargo clippy --locked --workspace --all-targets -j2 -- -D warnings`：退出 0。
- `cargo test --locked --workspace --all-targets -j2 -- --test-threads=4`：27 套件，**910 passed / 0 failed / 7 ignored**（新增 3 条）。任务 `ky7j4c`，日志 `.workbuddy/logs/g0-morph-full.log`（PowerShell UTF16）。
- `cargo build --locked --release --workspace -j2`：退出 0，任务 `9qeto1`，日志 `.workbuddy/logs/g0-morph-release.log`。刷新 dist 的 editor/game/mcp 三个 EXE，逐个 SHA-256 与 release 相同；未删除 dist 其他内容。
- 新 dist `--check projects/SampleScene`：退出 0，1 scene / 24 entities / 15 assets。
- 新 dist `--headless --frames 120 --screenshot .workbuddy/shots/g0-morph-round-SampleScene.png projects/SampleScene`：退出 0，ok=true，120 fixed_steps，error/warn=0，non_finite_transforms=0。截图已查看，三维场景、阴影、UI 正常可见；这是普通冒烟截图，不作为 morph 专项截图。
- 新 dist `--build projects/SampleScene`：退出 0，33 个资产文件，产物 `projects/SampleScene/Build/SampleScene`。该项目未配置 build.scenes，报告 scene_count=0，不等于切关已验证。
- 用新 editor 对该真实产物目录运行 headless 120 帧：退出 0，ok=true，error/warn=0，non_finite_transforms=0。未运行包内玩家窗口与键鼠输入，不把该项称为完整独立玩家端到端验收。
- `git diff --check`：通过。rustfmt 未安装，格式化命令失败；未将其记为已执行成功。

初始只读空间检查 C 盘约 11.3 GiB、E 盘约 172.5 GiB，无 cargo/rustc/link 占用；本轮 release 与全量测试未复现上一轮环境阻塞，未清理个人目录或改权限。当前工具未提供 Agent/子代理入口，采用分项检查与串行修改，未宣称完成独立子代理审查。

### 剩余边界

保守盒覆盖有限合法资产与现行钳位权重组合，会牺牲部分剔除精度；不宣称任意骨骼位移、异常非有限资产、容量超限策略已完成。双角色异骨数 GPU、动态 TAA、包自定义输入/切遍关卡、插件 ABI 指纹、其他主写入与复杂生命周期仍按正式优先级继续。G0 未全部关闭。文档职责仍以差距分析为唯一优先级入口，决策约束见 §四十二。

## 五十七、G0 追加正确性审计与修复（2026-10-27）

本轮从已有未提交修复继续推进，保留全部改动，不创建 commit。R01—R12 与额外 G0 的逐项状态集中到 `Docs/差距分析.md` 顶部；既有报告保持快照，不将 200 项标为完成。

### 新增回归先红证据与实现

所有测试使用 PowerShell、指定 cargo.exe、CARGO_INCREMENTAL=0、`-j 2` 与 `--test-threads=4`，LIB/INCLUDE 参照构建脚本配置。

| 测试 | 旧实现失败证据 | 修改 |
|---|---|---|
| `config_and_material_saves_preserve_locked_targets` | 任务 q9y4vm：0 passed / 1 failed，project.ron 拒绝=false、保留=false | 配置与材质复用 write_atomic；同一测试覆盖两个路径 |
| `top_level_budget_is_independent_after_an_exhausted_script` | uc56g3：正常第二脚本加载失败，0/1 | 顶层 exec 前独立重置预算 |
| `saved_context_is_rejected_outside_hook_window` | 1utnnn：钩子结束后的句柄仍转换成功，0/1 | VM 活动槽地址校验，RAII 关闭窗口；测试不解引用过期 GameCtx |
| `removed_entity_does_not_run_queued_update` | 6ijbau：删除已成功，但仍执行旧快照 update，0/1 | 非 Destroy 钩子执行前检查实体仍在场 |
| `removing_current_behavior_does_not_skip_successor` | bauu0u 的 Rust 测试：后继位置未更新，0/1 | Rust 按阶段入口实体身份快照派发，每组件前复查实体 |
| `zero_viewport_clears_previous_ui_state` | 9q8llj：hovered/pressed/clicked 全为 true，预期全 false，0/1 | 零视口清空布局并走统一状态清理 |

Lua 删除测试第一次使用了不存在的 `find_entity`，实体未删除，不计有效红灯；改成真实 API `entity_id` 后重跑获得上表证据。先前模块绿灯 qkf15i 为 core 258 + script 41，不包含之后新增的两个派发回归和 UI 回归。

### 验证与产物（环境阻塞，未完成刷新）

- 首遍 check/clippy 通过；首遍 test 包含新增零视口回归并失败，之后修复。
- 最终代码任务 `np4c2g` 的 workspace/all-targets check、clippy `-D warnings`、test 均通过，包含 runtime 54、script 42、render 188 条；未据截断输出重新统计全局数量。随后同任务 release 失败，所以整个任务状态为失败，不能把它写成构建通过。
- release 先报 `target/release/xgame_mcp.pdb` 拒绝访问与 editor LNK1104，重试又报 C 盘临时文件 LNK1108。检查 C 盘 Free=0，E 盘约 183GB；未发现引擎进程，相关输出非只读。迁移 TEMP/TMP 到已有 `.workbuddy/tmp` 后仍访问失败，隔离 `target/g0-20261027` 构建也在新依赖对象文件报拒绝访问。未删除文件、终止进程或绕过权限。
- 为持久保存测试输出而补跑任务 `2dsyz3`（E 盘 TEMP）失败：core 242 passed / 16 failed，失败集中在文件替换/清理的 PermissionDenied；另有 Cargo 全局缓存 disk full。该失败不抹去此前同代码通过事实，但说明当前环境下验证不可重复，不能仅报全绿。
- 日志：`.workbuddy/logs/g0-validation-20261027.log`、`g0-final-20261027.log` 是 PowerShell transcript，工具模式下不保证收齐 native 输出；完整补录失败输出为 `g0-tests-final-20261027.log` / `.stderr.log`，隔离构建为 `g0-release-isolated-20261027.log` / `.stderr.log`（PowerShell UTF16）。完整成功输出保留在工具任务 np4c2g。
- **dist 未刷新，新增 release GPU/发布冒烟未执行**。原有 dist 和旧证据保留，不当成本轮产物。需恢复 C 盘空间并排查 E 盘文件访问限制后，再完成 release、适用 GPU 截图、真实打包运行与 dist 更新。

### 保留边界

配置/材质主写入已扩大保护范围，但 input/meta/迁移写入、固定 tmp 残留、并发编辑冲突和断电持久性仍未解决。ctx 采用“当前钩子上下文”语义，旧句柄在下一有效窗口内仍映到当前 ctx；未新增 generation 语义，未运行 Miri。结构派发的存在检查采用线性查询，新增大规模行为负载开销未量化，不宣称性能优化。递归销毁/卸载中新增、多角色动态 GPU、morph bounds、真实发布包输入切关与 ABI 指纹门禁仍未关闭。

## 五十六、G0 首轮修复与正式优先级采纳（2026-10-26）

### 实施依据与范围

正式采纳《引擎全面对标与差距深度分析-2026-10-26》的 G0—G4 阶段，排序回写 `Docs/差距分析.md`：**G0 优先于 G1—G4**。报告原文保持 afe30cc 基线的审计快照，其“未运行 / 候选”不改成事后实施结论；本节之前的 P0—P3 推荐与数量均是历史记录。

本次文档同步逐项核对了工作区 14 个 Rust 文件的 `git diff`，R01—R12 均有代码改动。选择先修保存、发布与组合正确性，是因为功能存在不能证明多个实例、卸载、异常写入或独立包中也正确。没有以新效果或架构重写代替这些验证。

| 风险 | 本轮实施与取舍 |
|---|---|
| R01/R02 | 构建枚举 scripts_dir（含自定义目录），复制可选根 input.ron；在 staging 完成复制后发布，失败回滚，回滚失败保留旧包；拒绝源/输出相互包含及相关链接冲突。未宣称完整资源依赖构建系统或断电事务。 |
| R03/R04 | 每角色骨骼槽固定 256、空位补单位矩阵；CUBICSPLINE 按每关键帧入切线/值/出切线取值，morph 先按目标数分组。仍降级线性插值，没有实现 Hermite。 |
| R05 | 独立游戏 Surface 持有 Arc<Window>，删除借用生命周期 transmute，不再靠字段析构顺序保证安全。 |
| R06 | scene/prefab 主保存接原子替换：独占 tmp、写入、sync_all、rename；失败不退回直接覆盖。范围不扩展为配置/材质等全部主写入；已有 tmp 阻止保存，需确认写入者结束后处理残留。 |
| R07 | Apply 保留同来源 source_id、重映射父引用；新增节点在写盘成功后绑定来源并记录场景 Undo。不等于字段 override、嵌套或 Revert 保留实例 ID。 |
| R08 | 暂停冻结既有固定步积压，恢复继续消费；保留单帧步数上限，不清空或丢弃积压。 |
| R09 | range 传入 GPU，最后 20% 平滑收尾，范围外零贡献；非有限/非正范围禁用，近处沿用旧衰减。 |
| R10 | World 帧末按更新后场景相机姿态同步 listener，场景相机优先于本帧脚本/宿主手动设置；无相机保留已有 listener。 |
| R11 | unload_scene 幂等；切关、编辑器 Stop、headless 正常完成、独立游戏正常退出接入。旧状态供钩子读取，卸载结构命令不进入下一场景；Stop 先交换最新运行场景，headless 先冻结终点统计/截图再收卸载日志。 |
| R12 | 场景正有限 mass 按形状体积换算密度实现总质量，附加质量归零；零值等沿用自动密度。 |

稳定契约集中记录于 `Docs/决策记录.md` §四十，本表记录本轮选择及范围。

### 已有验证与证据边界

本轮已完成 `cargo check --workspace --all-targets`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace --all-targets`，结果 **901 passed / 0 failed / 7 ignored**；dist 已刷新。SampleScene / CharacterDemo / MyGame 的 headless 截图通过；独立包真实窗口启动、退出通过。既有日志位于 `.workbuddy/logs/g0-validation-20261026.log`、`g0-tests-20261026.log`、`g0-headless-*.json`、`g0-package-headless.json`；真实窗口启动/退出结果由本轮交接确认，空 stdout/stderr 本身不能证明退出成功。本次文档同步未重新执行编译、测试或窗口验证。

**R01/R02 已补跑旧行为红灯并恢复转绿（2026-10-26）**。仅用 Edit 临时删除 `content_dirs()` 的 `content.scripts_dir.as_str()` 行及根 `input.ron` 复制块，保留新测试、staging、路径检查及其他修复；没有回退整个 builder。PowerShell 设置 `$env:CARGO_INCREMENTAL='0'`，补入 `$env:USERPROFILE/.cargo/bin` 到当前进程 PATH 后，分别执行：

```powershell
cargo test -p xgame-editor -j2 --bin xgame-editor builder::tests::package_includes_scripts_and_input -- --exact --nocapture
cargo test -p xgame-editor -j2 --bin xgame-editor builder::tests::package_preserves_custom_input_bytes -- --exact --nocapture
```

- R01：`0 passed / 1 failed / 204 filtered out`，退出码 101；临时版本 `builder.rs:641:95` 读取 `Scripts/nested/main.lua` 时 `NotFound`（系统错误 3，路径不存在）。首个默认目录分支已失败，自定义目录分支未执行。
- R02：`0 passed / 1 failed / 204 filtered out`，退出码 101；临时版本 `builder.rs:654:71` 读取产物 `input.ron` 时 `NotFound`（系统错误 2，文件不存在）。
- 用 Edit 原位恢复上述两处后，原命令各得 `1 passed / 0 failed / 204 filtered out`，退出码均 0；R01 此时完整通过默认 `Scripts` 与自定义 `Logic/Lua` 两个分支。
- 首次调用因 PowerShell PATH 未包含 Cargo 而未运行测试，不计作红灯；补齐 PATH 后才得到以上实际测试失败。红灯/绿灯工具任务分别为 `idjr0t` / `s10rd8`。builder 恢复后 SHA-256 为 `0C5F5420731CA92D3B972D28F50A1D6B806778958F714478F64155E1AD6BDDD6`。

本次是保留其他修复的最小旧行为复现，补齐失败证据，不追溯宣称原实施时已先证红；没有重跑全量、更新 dist 或执行完整发布包端到端验收。未逐项核实的其他红灯也不补写为已跑。

### BehaviorsDemo 旧插件刷新与原项目复验

用户已验证旧 DLL 导致退出 `0xC0000374`，同批 release DLL 在隔离副本通过。本次先确认备份父目录存在，再将原 `projects/BehaviorsDemo/Plugins/behaviors_demo.dll` 备份到 `.workbuddy/artifacts/g0-validation-20261026/old-plugin/behaviors_demo.dll`，以 `target/release/behaviors_demo.dll` 刷新原项目插件，并逐字节核对。旧 DLL SHA-256 为 `1c8d8decbcabebc807ccf0962213c1c89b4ba388a092a7e1ecb89faa4c6507bd`，新 DLL 与 release 源均为 `c6c5112a61905cad0071451c86245bc02b790348801f3231895cc891ef2c383d`。

执行 `dist/xgame-editor.exe --headless --frames 300 --screenshot .workbuddy/shots/g0-BehaviorsDemo-updated.png projects/BehaviorsDemo`：首次后台运行报告成功，但工具未显示数值退出码；同命令再运行并显式捕获进程退出码 **0**。报告 `ok=true`、frames/fixed_steps 均为 300、error/warn 均为 0、non_finite_transforms=0，截图 1280×720。含命令、退出码与完整 stdout/stderr 的报告保存在 `.workbuddy/artifacts/g0-validation-20261026/old-plugin/updated-validation.json`。本次仅刷新插件和执行该项验证，未重跑全量测试。

### 未关闭事项

R01—R12 已改不等于 G0 全关。原项目旧插件已刷新并复验通过；ABI 兼容拒绝门禁仍待做，继续保留 ABI/旧 DLL 风险、复杂生命周期与 ctx 调用窗口、双角色蒙皮/morph/动态 TAA 的 GPU 验证、其他主文件写入、完整发布包输入/切关/插件端到端 CI 等待办。普通示例截图和窗口冒烟不能代替这些定向用例。仅同步文档与追加工作日志，不创建代码提交。
