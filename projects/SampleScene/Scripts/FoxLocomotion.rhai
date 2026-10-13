// Fox 状态机 + 叠加层演示：
// 1) 按时间分段推 `speed` 参数，引擎侧的状态机（见 scene.ron 里 Fox 的
//    animator.machine）自己在 Idle/Walk/Run 间切换；
// 2) 振荡叠加层权重 —— Run 剪辑的**增量**（相对它第 0 帧的奔跑弹跳）按
//    sin 波叠在当前基础姿势上，"呼吸/弹跳幅度"看得见地起伏。
//
// 状态机阈值（含滞回）：
//   Idle(Survey) → Walk 需 speed ≥ 0.5；退回 Idle 需 < 0.3
//   Walk         → Run  需 speed ≥ 2.0；退回 Walk 需 < 1.8
//
// 参数表是 0 → 1 → 3 循环（每段 2.5 秒）：站立 → 走 → 跑 → 站立……
// 换状态复用换剪辑的交叉淡入（fade_time），所以看不到硬跳。

const CYCLE = 7.5; // 一轮 3 段 × 2.5s

fn update(ctx) {
    // ── 状态机参数 ──
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

    // ── 叠加层权重 ──
    // sin ∈ [-1,1] → 0.3 + 0.3·sin ∈ [0, 0.6]：增量从无到明显来回起伏。
    // 没配叠加层的实体这里静默 no-op，脚本不用探测组件。
    set_anim_additive_weight(ctx, 0.3 + 0.3 * sin(time(ctx) * 4.0));
}
