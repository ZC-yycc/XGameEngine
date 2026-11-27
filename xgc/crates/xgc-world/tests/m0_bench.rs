//! M0 组件列读取。用 Release 运行忽略项：`cargo test -p xgc-world --release --test m0_bench -- --ignored --nocapture --test-threads=1`。

use std::hint::black_box;
use std::time::{Duration, Instant};

use xgc_world::{Transform, Velocity, World};

#[test]
#[ignore = "M0 计时需使用 Release 并串行运行"]
fn query_1m_reads() {
    const READS: usize = 1_000_000;
    const SAMPLES: usize = 9;
    let mut world = World::new();
    for index in 0..READS {
        world.spawn((
            Transform {
                position: [index as f32, 0.0, 0.0],
                ..Transform::default()
            },
            Velocity::default(),
        ));
    }
    assert_eq!(world.len(), READS);
    let expected = (READS as f64 - 1.0) * READS as f64 / 2.0;

    let read_fields = || {
        let mut visits = 0;
        let mut sum = 0.0;
        // 每行读取一个分量。用可精确比较的和检验实际读取结果，避免只计迭代开销。
        black_box(&world).for_each::<Transform>(|_, transform| {
            visits += 1;
            sum += f64::from(transform.position[0]);
        });
        (visits, sum)
    };
    for _ in 0..2 {
        assert_eq!(read_fields(), (READS, expected));
    }

    let mut elapsed = [Duration::ZERO; SAMPLES];
    for duration in &mut elapsed {
        let start = Instant::now();
        let result = read_fields();
        *duration = start.elapsed();
        assert_eq!(result, (READS, expected));
    }
    elapsed.sort_unstable();
    eprintln!(
        "M0 query_1m_reads reads={READS} sum={expected} median={:?} min={:?} max={:?} samples={elapsed:?}",
        elapsed[SAMPLES / 2],
        elapsed[0],
        elapsed[SAMPLES - 1],
    );
}
