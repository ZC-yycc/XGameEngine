// Fox 状态机演示：按时间分段推 `speed` 参数，引擎侧的动画状态机
// （见 scene.ron 里 Fox 的 animator.machine）自己在 Idle/Walk/Run 间切换。
//
// 状态机阈值（含滞回）：
//   Idle(Survey) → Walk 需 speed ≥ 0.5；退回 Idle 需 < 0.3
//   Walk         → Run  需 speed ≥ 2.0；退回 Walk 需 < 1.8
//
// 参数表是 0 → 1 → 3 循环（每段 2.5 秒）：站立 → 走 → 跑 → 站立……
// 换状态复用换剪辑的交叉淡入（fade_time），所以看不到硬跳。

const CYCLE = 7.5; // 一轮 3 段 × 2.5s

fn update(ctx) {
    // 取模把时间折回 0..7.5，之后按区间给不同的 speed。
    let t = time(ctx) % CYCLE;
    let speed = 0.0;
    if t >= 5.0 {
        speed = 3.0; // Run：跨过 2.0 阈值
    } else if t >= 2.5 {
        speed = 1.0; // Walk：在 0.5~2.0 之间
    }
    // speed = 0 → 低于 0.3 → 退回 Idle（滞回让 0.3~0.5 区间不会来回抖）。
    set_anim_param(ctx, "speed", speed);
}
