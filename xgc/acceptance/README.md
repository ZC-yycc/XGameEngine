# XGC 割接验收参考（CUTOVER 门禁）

这个目录是 `xgc/CUTOVER.md` 里两条门禁的**参考数据**：

- **逐帧 trace / 状态哈希对照**：`trace.txt`
- **截图阈值判定**：`frame-<帧号>.png`

在 `xgc/` 下迁移当前 showcase，再用仓库参考运行门禁：

```sh
cargo run -q -p xgc-cli -- migrate ../projects/XGame/Scenes/showcase.ron ../tmp/showcase.json
cargo run -q -p xgc-cli -- gate ../tmp/showcase.json ../projects/XGame acceptance
```

退出码：`0` 全部通过；`1` 有检查未过（门禁不通过）；`2` 用法 / IO / GPU 错误。

截图判定分两条线：

- **结构性（单通道 ≥8）始终按 `0.5%` 硬判**；
- **总差异默认阈值为 `2%`**。参考指纹与本机完整指纹一致、显式给出总差异阈值或加 `--strict-total` 时，超阈值判为失败。跨 GPU 或驱动版本时，整片 ±1 的舍入差会推高总差异比例，此时默认仅报告该比例。

`env.json` 记录**生成参考图**的 GPU、后端、驱动名称和版本信息。缺少驱动信息或指纹不一致时，总差异仅作诊断。`xgc gpu-env` 可探测 XGC 当前适配器，旧端截图的指纹由下方脚本从实际产图报告写入。

显式覆盖阈值：

```sh
cargo run -q -p xgc-cli -- gate ../tmp/showcase.json ../projects/XGame acceptance 1.5 0.4
```

`trace.txt` 的第 0 帧是「装载后、未步进」的状态；帧号必须从 0 连续递增，
否则门禁直接按配置错误退出（空目录也不能算通过）。

## 在验收机重生成截图

在仓库根目录执行以下命令，指定一个尚不存在的输出目录：

```powershell
powershell -File tools/regenerate-xgc-acceptance.ps1 -OutDir tmp/xgc-acceptance-local
```

脚本迁移当前 showcase 场景、用旧引擎渲染第 1、150、200、300 帧（1280×720），从四次截图报告确认并记录实际适配器。随后使用仓库里的 `trace.txt` 和 `xgc gate --strict-total` 校验。门禁通过后才发布 `showcase.json`、`trace.txt`、`env.json` 和四张 PNG。换机器、GPU 或驱动后，在新目录重跑。

状态轨迹确实发生预期变更时，先用 `xgc trace <世界.json> 150 <资产根>` 生成候选轨迹，核对变化后随代码提交更新 `trace.txt`。

可用这条命令复查本机生成的参考：

```powershell
cargo run --locked --manifest-path xgc/Cargo.toml -p xgc-cli -- gate tmp/xgc-acceptance-local/showcase.json projects/XGame tmp/xgc-acceptance-local --strict-total
```

## 为什么不在 CI 跑这整条

截图判定要建 wgpu 设备，hosted runner 没有显卡。CI 只跑 `cargo test -p xgc-cli`
里的 `tests/gate.rs`（用 `trace` 参考做通过 / 不通过 / 配置错误三条断言，不碰 GPU）；
完整门禁在带显卡的验收机上本地跑。
