# XGameEngine — Agent 指南

Rust 实现的类 Unity 游戏引擎（wgpu 渲染 + egui 编辑器 + rapier 物理 + Rhai 脚本）。

## 必读文档（按顺序）

1. `ROADMAP.md` — 阶段状态总表（先看「整体路线图」）
2. `PLAN.md` — 历史各轮的实施记录（含每个决策的理由与取舍，改代码前先查这里有没有已定结论）
3. `Docs/差距分析.md` — 与 Unity/UE/Godot 的能力差距及优先级
4. `Docs/双轨语言架构计划.md` — **当前进行中的大方向**：Rust（UE 插件模式）+ Lua（Unity 模式 + 热更）双轨，P0–P3 分期。开工前通读其「决策记录」一节

## 常用命令

```sh
cargo check --workspace --all-targets        # 检查（先跑这个）
cargo clippy --workspace --all-targets       # lint（要求 0 警告）
cargo test --workspace --all-targets         # 测试（要求全绿）
cargo run -p xgame-editor                    # 编辑器（先弹项目枢纽）
cargo run -p xgame-game -- projects/SampleScene  # 独立游戏
cargo test -p xgame-runtime --release --test update_throughput -- --ignored --nocapture  # 分档性能基准（默认 ignore）
```

发布打包：双击 `一键构建.cmd`（产物在 `dist/`）。

## 架构速记

- **crate 依赖方向**（不许反向依赖）：`core ← math ← {render, physics}`；`input`/`script` 在中层；`runtime`（World 编排）在上；`editor`/`game` 是两个宿主。
- **编辑器 Play 与独立游戏跑同一段 `World::update`**（`xgame-runtime`）——改运行时行为两端自动一致；不要在 editor/game 里复制运行时逻辑。
- 脚本层（`xgame-script`）：`ScriptHost` 管编译缓存/生命周期派发；`api.rs` 是脚本 API 真相源（双轨计划 P0 会把它平移到 `GameCtx` 方法层）；`ScriptCtx` 是地址束（unsafe 有严格调用窗口约定，见 `context.rs` 模块文档，别破坏该约定）。
- 场景格式 RON，纯文本、可 diff；新字段一律 `#[serde(default, skip_serializing_if = ...)]` 保证旧文件兼容。

## 代码风格约定

- **注释与文档写中文**，且遵循本仓库的风格：解释"为什么"而不是"是什么"；重要的取舍、别推翻的决策要写明理由（参考 `PLAN.md` 各轮的「刻意的取舍」小节）。
- 回归测试必须先证明它在旧实现下会红（仓库惯例，见 PLAN.md 第十节）。
- 性能改动"先量后改"，量数据写进记录。
- clippy 0 警告是硬要求。
- `一键构建.cmd` 保持 ASCII-only（cmd.exe GBK 解码问题）。

## 项目布局

- `crates/` — 引擎与工具（11 个 crate，名字见 workspace Cargo.toml）
- `projects/` — 游戏项目（`project.ron` + `Scenes/` + `Resources/` + `Scripts/` + `Prefabs/` + `input.ron`）
- `Docs/` — 脚本 API、差距分析、双轨语言架构计划
- `dist/` — 发布产物（构建输出，不入库改动）
- `tools/` — 构建脚本（build-release.ps1 等）

## 当前工作上下文（2026-10）

- 双轨语言架构已决策收敛为 **Rust + Lua**（C#/Verse/多语言中立层已否决，理由见计划文档决策记录 D1）。
- **P0 已完成**（2026-10-14，实施记录见 PLAN.md 十五）：`xgame` 门面 crate + `GameCtx` 方法层（api.rs 平移为转发壳）+ `Lifecycle` trait + 组件分发 + 注册表 + 插件边界（`export_plugin!` + 版本握手）+ 示例插件 `examples/spinner-plugin` 全链路。落地偏离已回写计划文档 D6。
- 下一步是 **P0.5 编辑器 PIE**（探测 Rust 插件项目 → 后台 cargo build → 加载进 Play 会话），随后 P1 `#[derive(Inspect)]` / P2 Lua 宿主，见计划文档 §四。
- Rhai 暂保留，P2.5 基准闸门数据决定去留——不要提前删。当前对照数据：1000 no-op 实体组件轨分发 37.6 µs/帧 vs Rhai 493.9 µs（13.1×），微基准 `behavior_dispatch_throughput`。
