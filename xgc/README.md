# XGame Core（XGC）

这是 `Docs/引擎框架设计` 提案的**独立实现 workspace**，与根仓库的
`xgame-*` crate 不互相依赖，也不被根 workspace 收录。旧引擎仍是唯一
产品路径；XGC 在 M0–M6 完成并通过割接门禁之前不替换它。

## 当前阶段

**M0 已完成；M1 前十刀已落地**：`xgc-base`（持久 ID / 句柄 / 诊断）、
`xgc-reflect`（组件注册表 / 确定性 schema / 动态字段读写 / schema 语义校验）、
`xgc-world`（archetype 列式存储、单 / 双组件可变查询、按 archetype 分片的并行
遍历、生成回收、`Command` 事务 plan / apply / revert、结构化 `Spawn` / `Despawn` +
实体快照撤销、可序列化 `EntityData` / 组件工厂、实体级 revision、世界快照 / 恢复、
事务日志 JSON 往返与重放、状态哈希）、`xgc-store`（版本化 JSON 包络 + 原子写）、
`xgc-sim`（Scenario / Trace / 固定步 `Schedule` / `SystemAccess` 冲突判定 /
`Schedule::batches` / `VelocitySystem` / `GravitySystem` / 逐帧 state hash /
回放校验）、`xgc-agent`（JSON-RPC：describe / query / get / digest / stats /
load / save / plan / apply / revert；查询支持字段投影 + 标量过滤 + 稳定分页）、
`xgc-cli`（`schema` / `check` / `bench` / `export` / `import` / `run` / `serve`）、
`xgc-xtask`（文件预算 / 依赖方向 / schema 确定性门禁）。

实测（i7-10700，release，10 000 个 `(Transform, Velocity)`）：

| 操作 | 时间 |
|---|---|
| spawn 10 000 | 1.89 ms（189 ns/实体） |
| 单组件查询 10 000 | 22.5 µs（2.25 ns/实体） |
| despawn 10 000 | 334 µs |

命令事务（debug，100 条 `SetField`）：plan + apply **77.8 µs**，revert **69.6 µs**；
apply / revert 各只推进一个 revision，回滚后状态哈希回到原值。`Command` 可
JSON 往返：`Spawn { data: EntityData }` 反序列化后由组件工厂重建真实组件列；
销毁回滚恢复**原 EntityId 与全部组件**。`WorldData` 可把整个世界落盘再恢复，
`state_hash` / 实体 ID / revision 一致；`Journal` 可从初始快照重放并校验 revision。
`xgc run 60` 已跑通固定步帧循环：命令 → 系统（`position += linear * dt`）→ trace；
`VelocitySystem` 按 archetype 分片并行，1 / 2 / 8 线程 state hash 逐位一致。
Agent 协议可完成 `describe → plan（含 before 预览）→ apply → query / get /
digest / stats → save/load → revert` 闭环；查询 IR 支持
`select: ["core.transform:position"]` 字段投影、`filter` 的 `eq/ne/lt/le/gt/ge`
标量比较（向量用 `axis: x/y/z/w` 取分量）与稳定分页。所有 64 位实体 ID 在 JSON
里是十进制字符串，避免 2^53 精度丢失。

`cargo test --workspace` 全绿（50 条），`clippy -D warnings` / `fmt --check` 通过；
`xgc-xtask check` 全绿（25 个 Rust 文件最大 588 行）；schema 两次导出逐字节相同
（`Value` 带 `kind` 标签，`quat` / `vec4` 不再歧义）。

**尚未实现**：跨系统并行执行（批次计划已就绪，世界所有权拆分未做）、物理 /
脚本 / 动画 / 音频、渲染、编辑器；Agent 协议尚未覆盖能力清单与视觉验证。

## 命令

```sh
cd xgc
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p xgc-xtask -- check
cargo run -p xgc-cli -- schema
cargo run -p xgc-cli -- check
cargo run -p xgc-cli -- bench
cargo run -p xgc-cli -- export world.json
cargo run -p xgc-cli -- import world.json
cargo run -p xgc-cli -- run 60
cargo run -p xgc-cli -- serve
```

本机 cargo 不在 PATH 时，用 `C:\Users\Administrator\.cargo\bin\cargo.exe`；
网络不可用时追加 `--offline`（依赖已在本地缓存）。

## 纪律

- 一个概念一个类型；不写 `V2` / `legacy` / 兼容别名。
- 组件是纯数据；系统只通过查询 / 命令改世界。
- 10k 实体路径先保证正确，再谈零分配与并行；性能数字必须来自实跑。
- 核心 crate 默认 `#![deny(unsafe_code)]`；需要 unsafe 时单独评审。
- 文档与代码冲突时以 [主文档](../Docs/引擎框架设计.md) 的决策为准。
