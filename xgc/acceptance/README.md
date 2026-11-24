# XGC 割接验收参考（CUTOVER 门禁）

这个目录是 `xgc/CUTOVER.md` 里两条门禁的**参考数据**：

- **逐帧 trace / 状态哈希对照**：`trace.txt`
- **截图阈值判定**：`frame-<帧号>.png`

判定命令是同一条（在 `xgc/` 下跑）：

```sh
cargo run -q -p xgc-cli -- gate ../tmp/showcase.json ../projects/XGame acceptance
```

退出码：`0` 全部通过；`1` 有检查未过（门禁不通过）；`2` 用法 / IO / GPU 错误。

截图判定分两条线：

- **结构性（单通道 ≥8）始终按 `0.5%` 硬判**；
- **总差异 `2%` 只在参考可信时硬判**：参考目录带 `env.json` 且与本机 GPU / 后端 / 驱动一致、或显式给第 4 个参数、或加 `--strict-total`。否则只打印诊断——跨 GPU 的整片 ±1 编码舍入差不是回归。

记录本机验收指纹（写 `<参考目录>/env.json`，缺省 `acceptance`）：

```sh
cargo run -q -p xgc-cli -- gpu-env acceptance
```

换机器 / 换 GPU / 换驱动后，重生成参考图并重写 `env.json`。显式覆盖阈值：

```sh
cargo run -q -p xgc-cli -- gate ../tmp/showcase.json ../projects/XGame acceptance 1.5 0.4
```

`trace.txt` 的第 0 帧是「装载后、未步进」的状态；帧号必须从 0 连续递增，
否则门禁直接按配置错误退出（空目录也不能算通过）。

## 参考数据怎么来的

`trace.txt` 由 XGC 自己生成（`xgc trace` 与门禁重放走同一条 `ScriptedRunner` 路径）：

```sh
cargo run -q -p xgc-cli -- trace ../tmp/showcase.json 150 ../projects/XGame > acceptance/trace.txt
```

`frame-<帧号>.png` 必须由**旧引擎在验收机**生成（旧端 `--headless` 没有逐帧输出，
只有终态 dump；但截图口径就是验收口径）：

```sh
# 在仓库根目录
cargo run -q -p xgame-editor --offline -- \
  --headless --frames 150 --screenshot tmp/old-150-now.png projects/XGame
# 再拷到 xgc/acceptance/frame-150.png
```

**换机器 / 换 GPU / 换驱动必须重新生成截图参考。** 旧端参考跨 GPU 环境会有整片
±1 的编码舍入差（本仓库历史上用另一台机器的参考得到过 18% 的假回归）；不重生
成就等于把门禁变成噪声。`env.json` 只记录参考机的 GPU / 后端 / 驱动，不参与像素
比较；没有它时 `xgc gate` 把总差异降级为诊断，结构性仍硬判。`trace.txt` 则相反：它只依赖 CPU 侧确定性，换机器不必重跑，
但**代码改动确实影响了状态轨迹时必须与改动同一次提交更新**，否则门禁会红。

## 为什么不在 CI 跑这整条

截图判定要建 wgpu 设备，hosted runner 没有显卡。CI 只跑 `cargo test -p xgc-cli`
里的 `tests/gate.rs`（用 `trace` 参考做通过 / 不通过 / 配置错误三条断言，不碰 GPU）；
完整门禁在带显卡的验收机上本地跑。
