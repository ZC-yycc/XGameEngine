# XGC Agent 入口

## 范围

`xgc/` 是 XGame Core 的独立 workspace；**不要**把它加进根 `Cargo.toml` 的
members，也不要在 `xgc` 里依赖根仓库的 `xgame-*` crate。旧引擎与 XGC 在割接前
是两套产品路径，禁止在同一个运行时里混用。

## 命令

```sh
cd xgc
cargo test --workspace --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo run -q --release -p xgc-cli --offline -- bench
```

本机 cargo 在 `C:\Users\Administrator\.cargo\bin\cargo.exe`，不在 PATH 时用全路径。
网络不可用时一律加 `--offline`（serde / serde_json 已缓存）。

## 入口文件

| 文件 | 职责 |
|---|---|
| `crates/xgc-base/src/lib.rs` | `EntityId` / `EntityKey` / `Tick` / `Diagnostic` |
| `crates/xgc-reflect/src/lib.rs` | `Component`、`Registry`、`SchemaDoc`、`Value` |
| `crates/xgc-world/src/lib.rs` | `World`、`Bundle`、`Archetype`、内建组件 |
| `crates/xgc-cli/src/main.rs` | `xgc schema` / `xgc bench` |

## 不变量（改代码前先看）

- 一个组件一个稳定 `ComponentId`；注册表按 ID 排序输出，schema 必须确定性。
- `EntityId` 是持久身份，`EntityKey` 是运行时句柄；句柄复用必须换 generation。
- archetype 的每一行列长度必须相等；`despawn` 用 `swap_remove` 并修正被搬实体行号。
- `World::spawn` 只接受元组 `Bundle`；重复组件在 debug 断言里拒绝。
- 热路径先保证正确；性能数字必须写进 README / 基准，不许凭感觉写"更快"。
- 核心 crate 默认 `#![deny(unsafe_code)]`；加 unsafe 必须单独说明调用窗口。

## 当前下一步（按顺序）

1. M0 收尾：`xgc-xtask`（size-check / dep-check / schema 确定性）、10k 基准固化、
   双线程/确定性门禁。
2. M1：`Command` 事务（plan/apply/revert）、世界 revision、快照/回放、
   多组件查询、`xgc check`、Agent 协议最小集。
3. 再往后的渲染 / 物理 / 脚本 / 编辑器见主文档 §14，别提前开分支。

## Token 纪律

- 先读本文件 + README；不要读 `target/` 或整份主文档。
- 每轮只动一个可验证的垂直切片；先跑测试再写总结。
- 长任务交给子智能体时，给它**文件路径 + 不变量 + 验收命令**，不要复制整份设计。
