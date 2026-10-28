//! 确定性：**同样的初始场景 + 同样的每帧 dt**，两次运行必须走出同一条轨迹。
//!
//! # 为什么值得单独钉
//!
//! 这条性质是"回放 / 复核 bug / 将来做网络同步"的地基，也是"引擎行为可解释"的
//! 底线：如果两次运行的结果不同，用户报的 bug 就永远复现不了。
//!
//! 具体抓的是**迭代顺序**这一类隐患 —— `HashMap` 的 `RandomState` 每个实例都用
//! 不同种子，所以"两次运行"在同一个进程里就足以暴露：凡是"按哈希顺序决定谁先谁后"
//! 的地方（建体、删体、派发），都会让轨迹分叉。浮点还要加上一条：加法不满足结合律，
//! 顺序一变，结果就变。
//!
//! # 记录什么
//!
//! 每帧把**所有实体**的世界位置 / 旋转按 `f32::to_bits` 拼成 `u32` 序列。用位表示
//! 而不是近似比较：真值轨道的差异就该是 0，容差比较会把"微小分叉"吞掉，而微小
//! 分叉正是这类 bug 的第一形态。

use xgame_core::{Entity, PhysicsBody, Scene};
use xgame_math::glam::Vec3;
use xgame_runtime::{World, WorldConfig};

const FRAMES: usize = 90;

/// 6 个动态方块叠成两列 + 一个地面：要接触、要碰撞、要堆叠，顺序敏感。
fn build_scene() -> Scene {
    let mut scene = Scene::new("determinism");
    let mut ground = Entity::new("ground");
    ground.physics = Some(PhysicsBody::static_box([8.0, 0.5, 8.0]));
    ground.transform.position = Vec3::new(0.0, -0.5, 0.0);
    scene.add_entity(ground);

    for i in 0..6 {
        let mut e = Entity::new(format!("box{i}"));
        e.physics = Some(PhysicsBody::dynamic_box([0.5, 0.5, 0.5]));
        // 两列、每列三个，留一点点水平偏移让接触不退化。
        e.transform.position = Vec3::new(
            (i / 3) as f32 * 0.02,
            0.55 + (i % 3) as f32 * 1.02,
            (i % 2) as f32 * 0.01,
        );
        scene.add_entity(e);
    }
    scene
}

/// 跑一遍，返回逐帧的位级轨迹。
fn trace() -> Vec<u32> {
    let mut world = World::new(build_scene(), std::env::temp_dir(), WorldConfig::default());
    let mut out = Vec::with_capacity(FRAMES * 9 * 7);
    for _ in 0..FRAMES {
        world.update(1.0 / 60.0);
        for entity in &world.scene().entities {
            let m = entity.transform;
            for v in [
                m.position.x,
                m.position.y,
                m.position.z,
                m.rotation.x,
                m.rotation.y,
                m.rotation.z,
                m.rotation.w,
            ] {
                out.push(v.to_bits());
            }
        }
    }
    out
}

/// 两次独立运行（各自新建 `World`，于是各自的 `HashMap` 种子不同）轨迹必须一致。
#[test]
fn identical_input_produces_identical_trajectory() {
    let a = trace();
    let b = trace();
    if a != b {
        // 找出第一处分叉，报出帧号与实体，别让调试从"两个大数组不同"开始。
        let first = a
            .iter()
            .zip(&b)
            .position(|(x, y)| x != y)
            .expect("长度不同说明实体数变了");
        let per_frame = a.len() / FRAMES;
        let frame = first / per_frame;
        let slot = first % per_frame;
        panic!(
            "两次运行的轨迹在第 {frame} 帧分叉（实体槽 {slot}，共 {} 个值）：\
             {:08x} != {:08x} —— 说明某处顺序依赖了哈希 / 并行调度",
            a.len(),
            a[first],
            b[first]
        );
    }
}

/// 同一帧内**多个**物理实体被删除时，删除顺序也必须确定。
///
/// `body_of_entity` 是 `HashMap`：直接从 `keys()` 收"待删列表"，多个实体同时消失时
/// `remove_body` 的顺序就随机了。rapier 会回收 body handle，回收顺序不同 → 下一次
/// 建体拿到的 handle 不同 → 求解器内部顺序不同 → 浮点结果分叉。
///
/// 所以这里跑**两遍完全相同的逻辑**（同样按 id 升序调 `remove_entity`），只有
/// `World` 内部的哈希种子不同。轨迹一旦分叉，就说明清理路径的顺序泄漏到了物理。
fn trace_with_removals() -> Vec<u32> {
    let scene = build_scene();
    let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default());
    // 先跑几帧让刚体真的建出来。
    for _ in 0..5 {
        world.update(1.0 / 60.0);
    }
    // 待删的三个按 id 升序（调用方顺序固定，变量只有内部哈希种子）。
    let mut victims: Vec<_> = world
        .scene()
        .entities
        .iter()
        .filter(|e| e.name == "box0" || e.name == "box1" || e.name == "box2")
        .map(|e| e.id)
        .collect();
    victims.sort_unstable_by_key(|id| id.raw());
    assert_eq!(victims.len(), 3, "场景里应当有三个待删方块");
    for id in &victims {
        world.scene_mut().remove_entity(*id);
    }

    let mut out = Vec::new();
    for _ in 0..30 {
        world.update(1.0 / 60.0);
        for entity in &world.scene().entities {
            for v in [
                entity.transform.position.x,
                entity.transform.position.y,
                entity.transform.position.z,
            ] {
                out.push(v.to_bits());
            }
        }
    }
    out
}

/// 一次性删掉多个物理实体之后，轨迹仍然必须逐位一致。
#[test]
fn removing_several_bodies_at_once_is_order_stable() {
    let a = trace_with_removals();
    let b = trace_with_removals();
    assert!(!a.is_empty(), "轨迹不能为空（否则这条测试等于没跑）");
    if a != b {
        let first = a
            .iter()
            .zip(&b)
            .position(|(x, y)| x != y)
            .expect("长度不同说明实体数变了");
        panic!(
            "批量删除后轨迹在第 {first} 个值分叉：{:08x} != {:08x} —— \
             清理刚体的顺序依赖了 HashMap 迭代",
            a[first], b[first]
        );
    }
}
