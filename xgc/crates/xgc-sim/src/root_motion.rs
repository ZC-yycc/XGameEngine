//! 根运动落地：把基础剪辑里根节点的平移增量写进实体世界位置。
//!
//! 旧端在 `crates/xgame-runtime/src/world.rs::apply_root_motion` 里做同一件事：
//! 本帧根位移（模型局部空间）乘实体世界矩阵的线性部分得到世界位移，再写回实体
//! 位置；渲染侧（`xgc-render/src/glb/root_motion.rs`）同时把蒙皮里的根节点平移
//! 改回绑定姿态，两者合起来才是旧端「实体前进 + 髋骨不再重复前进」的效果。
//!
//! 为什么是宿主显式调用而不是默认调度系统：根运动需要按**模型**查剪辑轨道表，
//! 而轨道表要由宿主（CLI / 将来的编辑器）从资产加载；塞进 `Schedule` 会把资产
//! 依赖带进 sim 的默认系统，破坏「默认调度零资产依赖」这条约定。
//!
//! 调用时机：`Simulation::step_with` 返回之后、宿主做渲染 / 落盘之前。此时
//! `Animator.time` 与 `blend_*` 都已推进到本帧末，`HierarchySystem` 也刚算完
//! `GlobalTransform`。

use xgc_world::{
    root_motion_delta, to_local_position, to_world_delta, Animator, GlobalTransform, MotionFade,
    MotionSpan, Parent, RootMotion, RootMotionLibrary, Transform, World,
};

/// 逐实体应用根运动；没有配置 / 没有轨道 / 数据非法的实体原样不动。
pub fn apply_root_motion(world: &mut World, dt: f32, library: &RootMotionLibrary) {
    if library.is_empty() || !dt.is_finite() || dt <= 0.0 {
        return;
    }
    let mut moves = Vec::new();
    world.for_each::<Animator>(|key, animator| {
        let Some(motion) = RootMotion::parse(&animator.root_motion) else {
            return;
        };
        let Some(set) = world
            .primary_model(key)
            .and_then(|model| library.set(model))
        else {
            return;
        };
        let Some(delta) = frame_delta(animator, &motion, set, dt) else {
            return;
        };
        if delta.iter().all(|value| *value == 0.0) {
            return;
        }
        moves.push((key, delta));
    });
    for (key, delta) in moves {
        apply(world, key, delta);
    }
}

/// 本帧的模型局部根位移。
fn frame_delta(
    animator: &Animator,
    motion: &RootMotion,
    set: &xgc_world::RootMotionSet,
    dt: f32,
) -> Option<[f32; 3]> {
    if !animator.autoplay || animator.clip.is_empty() {
        return None;
    }
    if !animator.speed.is_finite() || animator.speed <= 0.0 {
        return None;
    }
    // `time` 已是本帧末；反推本帧起点即上一帧末。
    let to = animator.time;
    let from = to - dt * animator.speed;
    let current = MotionSpan {
        clip: animator.clip.as_str(),
        from,
        to,
        looping: animator.looping,
    };
    let fade =
        (!animator.blend_from.is_empty() && animator.blend_duration > 0.0).then(|| MotionFade {
            clip: animator.blend_from.as_str(),
            from: animator.blend_from_time - dt * animator.blend_from_speed,
            to: animator.blend_from_time,
            looping: animator.blend_from_looping,
            alpha: (animator.blend_elapsed / animator.blend_duration).clamp(0.0, 1.0),
        });
    root_motion_delta(set, motion, current, fade)
}

fn apply(world: &mut World, key: xgc_base::EntityKey, local_delta: [f32; 3]) {
    let Some(global) = world.get::<GlobalTransform>(key).copied() else {
        return;
    };
    let world_delta = to_world_delta(&global, local_delta);
    let target = [
        global.position[0] + world_delta[0],
        global.position[1] + world_delta[1],
        global.position[2] + world_delta[2],
    ];
    let local_target = match world
        .get::<Parent>(key)
        .and_then(|parent| world.key(parent.entity))
        .and_then(|parent_key| world.get::<GlobalTransform>(parent_key).copied())
    {
        Some(parent) => to_local_position(&parent, target),
        None => target,
    };
    if let Some(transform) = world.get_mut::<Transform>(key) {
        transform.position = local_target;
    }
    // 本帧稍后的渲染 / 落盘直接读 `GlobalTransform`，它必须同步；下一帧
    // `HierarchySystem` 会用同一份局部变换重算（两者一致，不是双写语义）。
    if let Some(global) = world.get_mut::<GlobalTransform>(key) {
        global.position = target;
    }
}
