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
    // 预算按"每实体 + 固定预算"给：字符串字段经 `Value` 反射读取时每实体克隆一次
    // 是当前反射模型的固有成本（见 STATUS 的未闭环），真正的回归是"每实体上千次"
    // 这种数量级；上限取 实体数 + 64 在测得的 230 次/帧附近留了余量，又把
    // `state_hash` 里按实体重排组件 / 字段的那类分配挡在外面。
    let entities = simulation.world().len() as u64;
    let budget = 60 * (entities + 64);
    assert!(
        allocations <= budget,
        "稳态帧分配必须随实体数线性：60 帧分配了 {allocations} 次（{bytes} 字节），预算 {budget} 次"
    );
}

/// `state_hash` 的排序 / 字段布局必须一次性算完：数值型组件世界里连一次分配都不许有。
/// 这条把 `hash.rs` 里"每帧逐实体收集并排序组件 ID / 每次调用 `field_names()`"的
/// 回归挡在外面（那种写法 60 帧会分配上千次）。
#[test]
fn hashing_numeric_components_does_not_allocate() {
    reset_counter();
    let mut world = World::default();
    for index in 0..200 {
        let position = index as f32 * 0.5;
        world.spawn((
            Transform {
                position: [position, 0.0, 0.0],
                ..Transform::default()
            },
            Velocity::default(),
        ));
    }
    let _ = world.state_hash();
    reset_counter();
    let mut hash = 0u64;
    for _ in 0..60 {
        hash = hash.wrapping_add(world.state_hash());
    }
    let (allocations, _) = snapshot().expect("量测线程的计数器");
    assert!(hash != 0);
    assert_eq!(
        allocations, 0,
        "数值型组件的 state_hash 必须零分配：60 次共分配 {allocations} 次"
    );
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
