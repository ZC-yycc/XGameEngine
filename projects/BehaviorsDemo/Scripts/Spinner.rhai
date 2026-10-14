// 绕 Y 轴匀速自转。
//
// 本项目里它与 Rust 插件的 `Spinner` 组件**同值同款**（75°/s）——
// 场景里两个实体各挂一个，跑起来应当完全一致。这是双轨架构最直观的验收方式：
// 同一个行为，两条轨道，一个运行时。
//
// ⚠️ Rhai 的一个坑：**全局 `let` 变量在函数里看不见**。
//    想给函数共享一个常量，必须写成 `const`（全局 const 会被编进 AST，
//    函数里可以直接引用）。写 `let` 的话函数体里会报
//    "Variable not found"，而脚本又不会崩，只是整帧静默失效 —— 很容易找半天。

const SPIN_SPEED = 75.0; // 度 / 秒

fn update(ctx) {
    // 帧率无关：乘以 delta 才是"每秒 75 度"，否则机器越快转得越快。
    let r = rot(ctx);
    set_rot(ctx, r.x, r.y + SPIN_SPEED * delta(ctx), r.z);
}
