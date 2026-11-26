//! 热路径门禁：稳态帧的堆分配次数与帧耗时。
//!
//! 为什么要有它：`World::update` 级别的每帧分配会让 GC 压力与帧时间抖动都跟着
//! 实体数线性涨；这类回归在功能测试里完全看不见，只能靠计数分配器钉住。
//! 判据分两半：
//! - 分配次数是**硬门禁**（计数分配器 + 线程本地计数器：每个测试线程只看自己
//!   的分配，不依赖机器速度，也不会被并行跑的其他用例污染）；
//! - 帧耗时只**打印**，不设阈值（CI 机器与开发机差异太大，写死阈值等于给机器
//!   调验收线）。想拿稳态数字时用 `--release --ignored` 跑 `steady_state_throughput`。
//!
//! 计数器**必须**按线程隔离：libtest 在独立工作线程里跑每个用例，如果计数是进程
//! 全局量，release 下量测窗口短到微秒级，别的线程收尾时的分配就会被算进来
//! （实测 `--release` 下两个量测用例同跑 20/20 假红）；互斥锁也挡不住——锁释放
//! 后另一线程立刻开始分配。线程本地计数器让每条用例只对自己负责。

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

use xgc_sim::Simulation;
use xgc_world::{Name, PhysicsBody, Transform, Velocity, World};

thread_local! {
    /// 本线程累计的（分配次数, 增长字节数）。用 `const` 初始化：惰性初始化会在
    /// 首次分配时自己申请堆，把计数器算进自己的账上。
    static COUNTER: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
}

/// 读本线程的计数；线程本地存储已进入析构时返回 `None`（那时分配不再计数）。
fn snapshot() -> Option<(u64, u64)> {
    COUNTER.try_with(Cell::get).ok()
}

fn reset_counter() {
    let _ = COUNTER.try_with(|counter| counter.set((0, 0)));
}

struct CountingAllocator;

// SAFETY: 只是把每次分配 / 释放转发给系统分配器，并在分配成功后累加本线程计数；
// 不做任何指针运算，也不改变布局与对齐语义。
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let _ = COUNTER.try_with(|counter| {
                let (count, bytes) = counter.get();
                counter.set((count + 1, bytes + layout.size() as u64));
            });
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    // 注：字节数只累加新块大小、不减旧块，所以是增长量而不是净占用；门禁只
    // 断言次数，字节数仅供打印参考。
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            let _ = COUNTER.try_with(|counter| {
                let (count, bytes) = counter.get();
                counter.set((count + 1, bytes + new_size as u64));
            });
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// 与演示世界同量级的稳态负载：200 个带速度的实体 + 10 个动态刚体。
///
/// 刻意不用证据世界（`showcase-particles.json`）：那份世界是场景资产，改资产
/// 就改门禁判据；这里用代码构造，判据只随引擎代码动。
fn demo_world() -> World {
    let mut world = World::new();
    for index in 0..200 {
        world.spawn((
            Transform {
                position: [index as f32 * 0.5, 0.0, 0.0],
                ..Transform::default()
            },
            Velocity::default(),
            Name {
                value: format!("walker_{index}"),
            },
        ));
    }
    for index in 0..10 {
        world.spawn((
            Transform {
                position: [index as f32, 5.0, 0.0],
                ..Transform::default()
            },
            PhysicsBody::default(),
        ));
    }
    world
}

/// 预热：第一帧要建刚体表 / 申请临时缓冲，那些钱不该记到稳态账上。
fn warmed() -> Simulation {
    let mut simulation = Simulation::new(demo_world());
    for _ in 0..8 {
        simulation.step(Vec::new()).expect("预热帧必须成功");
    }
    simulation
}

fn measure_frames(simulation: &mut Simulation, frames: u64) {
    for _ in 0..frames {
        simulation.step(Vec::new()).expect("稳态帧必须成功");
    }
}

#[test]
fn steady_state_frames_stay_within_the_allocation_budget() {
    let mut simulation = warmed();
    // 预热之后才清零：预热帧的分配不属于稳态账。
    reset_counter();
    measure_frames(&mut simulation, 60);
    let (allocations, bytes) = snapshot().expect("量测线程的计数器");
    eprintln!(
        "steady-state: 60 帧 {allocations} 次分配 / {bytes} 字节，实体 {}",
        simulation.world().len()
    );
    // 这个固定场景实测约 23 次/帧。给刚体后端和 trace 扩容留余量，同时让
    // 每实体分配重新出现时越过门禁。
    let budget = 60 * 32;
    assert!(
        allocations <= budget,
        "稳态帧分配超预算：60 帧分配了 {allocations} 次（{bytes} 字节），预算 {budget} 次"
    );
}

#[test]
fn hashing_numeric_and_string_fields_does_not_allocate() {
    let world = demo_world();
    const EXPECTED_HASH: u64 = 0xcda5_ee4b_28a9_f49e;
    assert_eq!(world.state_hash(), EXPECTED_HASH);
    reset_counter();
    let mut hash = 0u64;
    for _ in 0..60 {
        hash = hash.wrapping_add(std::hint::black_box(&world).state_hash());
    }
    let (allocations, _) = snapshot().expect("量测线程的计数器");
    assert_eq!(hash, EXPECTED_HASH.wrapping_mul(60));
    assert_eq!(
        allocations, 0,
        "含名字和刚体的世界哈希必须零分配：60 次共分配 {allocations} 次"
    );
}

#[test]
fn hashing_many_entities_reuses_the_sort_buffer() {
    let mut world = World::new();
    let removed: Vec<_> = (0..5_000)
        .map(|_| world.spawn((Transform::default(),)))
        .collect();
    for _ in 0..5_000 {
        world.spawn((Transform::default(),));
    }
    for key in removed {
        assert!(world.despawn(key));
    }
    // 复用句柄槽会让实体 ID 的遍历顺序与排序后的顺序不同。
    for _ in 0..5_000 {
        world.spawn((Transform::default(),));
    }
    let expected = world.state_hash();
    reset_counter();
    let actual = std::hint::black_box(&world).state_hash();
    let (allocations, _) = snapshot().expect("量测线程的计数器");
    assert_eq!(actual, expected);
    assert_eq!(allocations, 0, "大世界哈希的排序不能分配临时缓冲");
}

/// 默认忽略：帧耗时只在 `--release` 下有意义。
#[test]
#[ignore = "计时只在 release 下有意义：cargo test -p xgc-sim --release --test steady_state -- --ignored --nocapture"]
fn steady_state_throughput() {
    let mut simulation = warmed();
    let frames = 600;
    let start = Instant::now();
    measure_frames(&mut simulation, frames);
    let elapsed = start.elapsed();
    let per_frame = elapsed.as_secs_f64() / frames as f64;
    eprintln!(
        "throughput: {frames} 帧 {:?}，{:.3} ms/帧，实体 {}",
        elapsed,
        per_frame * 1000.0,
        simulation.world().len()
    );
    assert!(per_frame > 0.0 && per_frame.is_finite(), "帧耗时必须有限");
}
