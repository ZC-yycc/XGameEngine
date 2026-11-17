# XGC 割接验收（未完成）

> 结论：**XGameEngine → XGame Core 的完整割接尚未达成。**
> 本文件只记录可复现证据与未满足项；能力状态见 [STATUS.md](STATUS.md)。

## 门槛矩阵

| 门槛 | 状态 | 证据 / 缺口 |
|---|---|---|
| 场景 + 资源 + 脚本 + 预制体端到端迁移 | 通过 | showcase 24 实体；character 2 脚本；Pillar 3 实体；`verify-migration` 一致 |
| 迁移世界可运行 | 通过 | `run-world` / `run-cooked` / `xgc-player` 60 帧同哈希 |
| 旧 / 新 headless 统计一致 | 通过 | 24 / 10 / 10 / 2 / 1（`compare-headless`） |
| 视觉可渲染 | 通过（基础） | `render-frame` 第 150 帧 hash `3e0b371b20aef59b`，1280×720，14 个场景 draw / 5 call / 7 个 UI draw |
| 视觉对等 | **未通过** | 旧 / 新均为第 150 固定帧，仍有 915191/921600 像素不同（99.30%），mean_abs 27.87、max 221；帧号已统一，渲染能力尚未对等 |
| 逐帧 trace / 状态哈希对照 | **未通过** | XGC 重放自身确定；无旧引擎逐帧 trace |
| 正式 System 调度 / 并行 | **未通过** | `batches()` 计划已有，执行顺序 |
| 性能基准 + 热路径分配门禁 | **未通过** | 无 release CI 门禁 |
| 推送 / CI | **未通过** | 无 GitHub 凭据，全部 commit 仅本地 |

## 可复现命令

```sh
# 迁移 + 数据对照
cargo run -q -p xgc-cli --offline -- migrate ..\projects\XGame\Scenes\showcase.ron ..\tmp\showcase.json
cargo run -q -p xgc-cli --offline -- verify-migration ..\projects\XGame\Scenes\showcase.ron ..\tmp\showcase.json

# 统计对照（旧 headless JSON 由旧 xgame-editor 生成）
xgame-editor --headless --frames 60 projects/XGame > old.json
cargo run -q -p xgc-cli --offline -- compare-headless old.json ..\tmp\showcase.json

# 视觉指标：先在根 workspace 生成旧引擎第 150 帧，再回 xgc 生成同帧
cd ..
cargo run -q -p xgame-editor --offline -- --headless --frames 150 --screenshot tmp\old-150.png projects\XGame
cd xgc
cargo run -q -p xgc-cli --offline -- render-frame ..\tmp\showcase.json ..\tmp\new-150.ppm ..\projects\XGame 1280 720 150
cargo run -q -p xgc-cli --offline -- image-diff ..\tmp\old-150.png ..\tmp\new-150.ppm

# cooked 播放
cargo run -q -p xgc-cli --offline -- pack ..\tmp\showcase.json ..\projects\XGame ..\tmp\cooked
cargo run -q -p xgc-player --offline -- ..\tmp\cooked 60
```

> `render-frame` 缺省 0 帧，输出与此前层级修正图逐像素一致；显式 150 帧后 XGC 自身改变
> 306914/921600 像素（33.30%，mean_abs 15.76、max 205），证明系统与脚本正式帧确实推进。
> 本轮只统一固定帧号，不代表行为 / 渲染能力已对等。旧端本机运行还报告项目 Rust 插件构建指纹不匹配并跳过，
> XGC 也尚未迁移旧 Animator 状态机与完整动画素材；这些都必须在最终逐帧门禁前补齐。
> 当前 `ScriptRuntime::run_frame` 不返回错误，脚本诊断仍只写 stderr；正式调度切片需要把失败传播纳入宿主契约。

## 建议顺序

1. 渲染：HUD / 层级世界变换 / 第 150 帧同 tick 已通；下一步做 PBR / 网格纹理 / 阴影。
2. 正式调度：脚本与系统统一访问声明。
3. 旧新逐帧 trace + 截图像素门禁。
4. 密码学签名 / 跨机器增量。
5. 推送 + CI。
