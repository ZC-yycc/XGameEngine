//! M0 层级传播。用 Release 运行忽略项：`cargo test -p xgc-sim --release --test m0_bench -- --ignored --nocapture --test-threads=1`。

use std::time::{Duration, Instant};

use xgc_sim::{HierarchySystem, System, FIXED_DT};
use xgc_world::{GlobalTransform, Parent, Transform, World};

#[test]
#[ignore = "M0 计时需使用 Release 并串行运行"]
fn transform_100k() {
    const ENTITIES: usize = 100_000;
    const SAMPLES: usize = 9;
    const ROOT_ROTATION: [f32; 4] = [0.0, 0.0, 1.0, 0.0];
    let mut world = World::new();
    let root = world.spawn((
        Transform {
            rotation: ROOT_ROTATION,
            scale: [2.0; 3],
            ..Transform::default()
        },
        GlobalTransform::default(),
    ));
    let mut nodes = Vec::with_capacity(ENTITIES);
    nodes.push(root);
    // 按层建二叉树，让每个样本都包含多级传播，且输入顺序可复现。
    for index in 1..ENTITIES {
        let parent = world.entity(nodes[(index - 1) / 2]).unwrap();
        nodes.push(world.spawn((
            Transform {
                position: [1.0, 0.0, 0.0],
                ..Transform::default()
            },
            Parent { entity: parent },
            GlobalTransform::default(),
        )));
    }
    assert_eq!(world.len(), ENTITIES);

    let mut hierarchy = HierarchySystem::default();
    for _ in 0..2 {
        hierarchy.run(&mut world, FIXED_DT);
    }

    let mut elapsed = [Duration::ZERO; SAMPLES];
    for (index, duration) in elapsed.iter_mut().enumerate() {
        // 每轮改根部位置，避免缓存旧结果也通过；改动和逐实体校验都在计时外。
        let root_x = (index + 1) as f32;
        world.get_mut::<Transform>(root).unwrap().position[0] = root_x;
        let start = Instant::now();
        hierarchy.run(&mut world, FIXED_DT);
        *duration = start.elapsed();

        for (index, &key) in nodes.iter().enumerate() {
            let depth = (index + 1).ilog2();
            let expected = GlobalTransform {
                position: [root_x - 2.0 * depth as f32, 0.0, 0.0],
                rotation: ROOT_ROTATION,
                scale: [2.0; 3],
            };
            assert_eq!(world.get::<GlobalTransform>(key), Some(&expected));
        }
    }
    elapsed.sort_unstable();
    eprintln!(
        "M0 transform_100k entities={ENTITIES} max_depth={} threads=1 median={:?} min={:?} max={:?} samples={elapsed:?}",
        ENTITIES.ilog2(),
        elapsed[SAMPLES / 2],
        elapsed[0],
        elapsed[SAMPLES - 1],
    );
}
