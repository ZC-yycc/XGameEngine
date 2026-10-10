//! 微基准：大场景下每帧 `World::update` 的 CPU 耗时。
//!
//! 默认 `#[ignore]`（耗时波动大，不该进 CI 门禁）。手动跑：
//!
//! ```sh
//! cargo test -p xgame-runtime --release --test update_throughput -- --ignored --nocapture
//! ```
//!
//! # 为什么要分四档
//!
//! 只跑一档很容易把 rapier 的耗时误记到引擎头上。四档的差额才说明问题：
//!
//! | 档位 | 量的是什么 |
//! |---|---|
//! | 纯视觉 | 引擎自身每帧开销（`sync_bodies` 扫描、层级解算、脚本派发）—— 物理完全不参与 |
//! | 全静态体 | 上述 + 建体 / 清理扫描的完整路径 |
//! | 稀疏动态 | 上述 + 少量互不接触的动态体（接近真实玩法） |
//! | 密集接触 | 1000 个**互相挤压**的动态体（最坏情况，纯 rapier 求解器成本） |
//!
//! 写成 `tests/` 下的集成测试而不是 `src` 里的单元测试，是为了让它能独立于
//! 被测源码的改动而存在 —— 这样对同一个基准跑「改动前 / 改动后」两份代码才公平。

use xgame_core::{Entity, PhysicsBody, Scene};
use xgame_math::glam::Vec3;
use xgame_runtime::{World, WorldConfig};

const N: usize = 2000;
const FRAMES: usize = 300;

/// 造一个 `N` 实体的场景。`mode` 决定物理配置（见模块头表格）。
fn build_scene(mode: &str) -> Scene {
    let mut scene = Scene::new("bench");
    for i in 0..N {
        let mut e = Entity::new(format!("e{i}"));
        match mode {
            // 不参与物理：`sync_bodies` 应当整段跳过。
            "visual" => {}
            // 全静态体：走完建体 / 清理扫描，没有动力学积分。
            "static" => {
                e.physics = Some(PhysicsBody::static_box([0.5, 0.5, 0.5]));
                e.transform.position = Vec3::new((i % 40) as f32, -0.5, (i / 40) as f32);
            }
            // 少量动态体：每个之间留足间距，互不接触 → 没有接触岛求解成本。
            "sparse" => {
                if i < 64 {
                    e.physics = Some(PhysicsBody::dynamic_box([0.5, 0.5, 0.5]));
                    e.transform.position = Vec3::new((i % 8) as f32 * 3.0, 2.0, (i / 8) as f32 * 3.0);
                } else {
                    e.physics = Some(PhysicsBody::static_box([0.5, 0.5, 0.5]));
                    e.transform.position = Vec3::new((i % 40) as f32, -0.5, (i / 40) as f32);
                }
            }
            // 密集接触：1000 个 1×1 的方块按 1.0 间距挤成一张 40×25 的网，
            // 全部落在同一平面 → 形成一个巨大的接触岛。这是纯 rapier 的最坏情况。
            _ => {
                if i % 2 == 0 {
                    e.physics = Some(PhysicsBody::dynamic_box([0.5, 0.5, 0.5]));
                    e.transform.position =
                        Vec3::new((i % 40) as f32, 1.0 + (i / 40) as f32, 0.0);
                } else {
                    e.physics = Some(PhysicsBody::static_box([0.5, 0.5, 0.5]));
                    e.transform.position = Vec3::new((i % 40) as f32, -0.5, 0.0);
                }
            }
        }
        scene.add_entity(e);
    }
    scene
}

fn measure(mode: &str, label: &str) {
    let mut world = World::new(
        build_scene(mode),
        std::env::temp_dir(),
        WorldConfig::default(),
    );
    // 预热：让 lazily 建好的刚体、查询结构、接触岛稳定下来。
    for _ in 0..30 {
        world.update(1.0 / 60.0);
    }

    let start = std::time::Instant::now();
    for _ in 0..FRAMES {
        world.update(1.0 / 60.0);
    }
    let elapsed = start.elapsed();

    let per_frame_us = elapsed.as_secs_f64() * 1e6 / FRAMES as f64;
    println!(
        "update_throughput[{label}]: {N} 实体 × {FRAMES} 帧 = {:.3} ms 总，\
         {per_frame_us:.1} us/帧（{:.0} FPS 上限）",
        elapsed.as_secs_f64() * 1e3,
        1e6 / per_frame_us,
    );
}

#[test]
#[ignore = "手动运行的微基准，不进 CI"]
fn update_throughput() {
    measure("visual", "纯视觉");
    measure("static", "全静态体");
    measure("sparse", "稀疏动态 64");
    measure("dense", "密集接触 1000");
}
