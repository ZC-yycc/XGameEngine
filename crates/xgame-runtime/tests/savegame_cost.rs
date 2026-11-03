//! 存档 / 读档的**成本**（默认 `#[ignore]`，手动跑）。
//!
//! 存档是一条会插进玩家帧里的路径（"走到存档点"、"菜单里点保存"），所以它不能
//! 是"几百毫秒的卡顿"。这条基准分段量：采集 / 序列化 / 解析 / 套用。
//!
//! ```text
//! cargo test -p xgame-runtime --release --test savegame_cost -- --ignored --nocapture
//! ```

use std::time::Instant;

use xgame_core::{Entity, PhysicsBody, Scene};
use xgame_math::glam::Vec3;
use xgame_runtime::{World, WorldConfig};

/// `n` 个实体：一半动态方块（带刚体），一半纯视觉。
fn scene_with(n: usize) -> Scene {
    let mut scene = Scene::new("save-cost");
    for i in 0..n {
        let mut e = Entity::new(format!("e{i}"));
        e.transform.position = Vec3::new(i as f32 * 0.01, 1.0, 0.0);
        if i % 2 == 0 {
            e.physics = Some(PhysicsBody::dynamic_box([0.5, 0.5, 0.5]));
        }
        scene.add_entity(e);
    }
    scene
}

fn bench(n: usize, rounds: usize) {
    let root = std::env::temp_dir().join(format!("xgame_savebench_{n}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut world = World::new(scene_with(n), &root, WorldConfig::default());
    // 跑几帧让状态稳定（也让物理体建出来）。
    for _ in 0..10 {
        world.update(1.0 / 60.0);
    }

    let start = Instant::now();
    let mut captured = 0usize;
    for _ in 0..rounds {
        captured += world.capture_save().entities.len();
    }
    let capture_us = start.elapsed().as_secs_f64() * 1e6 / rounds as f64;

    let save = world.capture_save();
    let start = Instant::now();
    for _ in 0..rounds {
        xgame_core::write_save(&root, "bench", &save).unwrap();
    }
    let write_us = start.elapsed().as_secs_f64() * 1e6 / rounds as f64;
    let bytes = std::fs::metadata(root.join("Saves/bench.sav")).unwrap().len();

    let start = Instant::now();
    for _ in 0..rounds {
        std::hint::black_box(xgame_core::read_save(&root, "bench").unwrap());
    }
    let read_us = start.elapsed().as_secs_f64() * 1e6 / rounds as f64;

    let loaded = xgame_core::read_save(&root, "bench").unwrap();
    let start = Instant::now();
    let mut applied = 0usize;
    for _ in 0..rounds {
        applied += world.apply_save(&loaded).applied;
    }
    let apply_us = start.elapsed().as_secs_f64() * 1e6 / rounds as f64;

    println!(
        "存档成本[{n} 实体]: 采集 {capture_us:7.1} µs | 序列化+写盘 {write_us:7.1} µs | 解析 {read_us:7.1} µs | 套用 {apply_us:7.1} µs | 文件 {bytes} B（采集 {captured} / 套用 {applied} 条）"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
#[ignore = "基准：手动跑，数据写进 PLAN"]
fn save_load_cost() {
    // 小 / 中 / 大：中档是"典型关卡"，大档是"读档会卡吗"的上界。
    bench(200, 20);
    bench(2000, 10);
    bench(8000, 3);
}

/// 分开测平铺与父链，避免只测无父节点的快路径而漏掉逐条找父的二次复杂度。
#[test]
#[ignore = "手动运行的读档层级成本对照，不进 CI"]
fn save_apply_hierarchy_cost() {
    for n in [200, 2000, 8000] {
        for parented in [false, true] {
            let mut scene = Scene::new("save-hierarchy-cost");
            for i in 0..n {
                let mut entity = Entity::new(format!("body{i}"));
                entity.transform.position = Vec3::new(i as f32 * 4.0, 1.0, 0.0);
                entity.physics = Some(PhysicsBody::dynamic_box([0.1; 3]));
                scene.entities.push(entity);
            }
            if parented {
                // 每组 32 个子体共用一个视觉父体；父体排在末尾，查找不能靠首项命中作弊。
                for first in (0..n).step_by(32) {
                    let parent = Entity::new(format!("parent{first}"));
                    for child in &mut scene.entities[first..(first + 32).min(n)] {
                        child.parent = Some(parent.id);
                    }
                    scene.entities.push(parent);
                }
            }
            let mut world = World::new(scene, std::env::temp_dir(), WorldConfig::default());
            assert_eq!(world.physics().stats().0, n);
            let mut save = world.capture_save();
            for entity in &mut save.entities {
                entity.position[1] += 2.0;
            }
            for _ in 0..5 {
                world.apply_save(&save);
            }
            let rounds = 30;
            let start = Instant::now();
            let mut applied = 0;
            for _ in 0..rounds {
                applied += world.apply_save(std::hint::black_box(&save)).applied;
            }
            let us = start.elapsed().as_secs_f64() * 1e6 / rounds as f64;
            assert_eq!(applied, rounds * save.entities.len());
            assert_eq!(world.scene().entities[0].transform.position.y, 3.0);
            let body = world.scene().entities[0]
                .component::<xgame_core::PhysicsHandle>()
                .unwrap()
                .0;
            let expected_y = if parented { 5.0 } else { 3.0 };
            assert_eq!(
                world.physics().body_transform(body).unwrap().position.y,
                expected_y
            );
            println!("save_apply_hierarchy_cost[n={n},parented={parented}]: {us:.2} us/apply");
        }
    }
}
