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

use xgc_sim::{AabbCollisionSystem, AnimationLayerSystem, Schedule, Simulation};
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
    // 这个固定场景实测约 7.3 次/帧。给刚体后端与 trace 扩容留余量，
    // 调度或碰撞收集恢复每帧分配时仍会由下方的零分配用例检出。
    let budget = 60 * 12;
    assert!(
        allocations <= budget,
        "稳态帧分配超预算：60 帧分配了 {allocations} 次（{bytes} 字节），预算 {budget} 次"
    );
}

#[test]
fn mixed_serial_batch_and_aabb_reuse_working_storage() {
    let mut world = World::new();
    // 给排序施加足够的行数压力；静态体之间不会产生接触。
    for index in 0..256 {
        world.spawn((
            Transform {
                position: [index as f32 * 2.0, 0.0, 0.0],
                ..Transform::default()
            },
            PhysicsBody {
                kind: "static".into(),
                ..PhysicsBody::default()
            },
        ));
    }
    let dynamic = world.spawn((Transform::default(), PhysicsBody::default()));
    let mut schedule = Schedule::new();
    schedule.add(AnimationLayerSystem);
    schedule.add(AabbCollisionSystem::default());
    let plan = schedule.plan();
    let [xgc_sim::PlanStage::Host, xgc_sim::PlanStage::Batch(indices)] = plan.stages() else {
        panic!("两个系统应位于同一批：{:?}", plan.stages());
    };
    assert_eq!(indices, &[0, 1]);
    let step = |schedule: &mut Schedule, world: &mut World| {
        // 每帧重叠一次，持续走接触求解和位姿回写。
        world.get_mut::<Transform>(dynamic).unwrap().position[0] = 0.25;
        schedule.run_plan(&plan, world, 1.0 / 60.0, |_| {});
        assert_eq!(world.get::<Transform>(dynamic).unwrap().position[0], 1.0);
    };
    for _ in 0..2 {
        step(&mut schedule, &mut world);
    }

    reset_counter();
    for _ in 0..60 {
        step(&mut schedule, &mut world);
    }
    let (allocations, bytes) = snapshot().expect("量测线程的计数器");
    assert_eq!(
        allocations, 0,
        "串行混合批和 AABB 预热后必须零分配：60 帧 {allocations} 次 / {bytes} 字节"
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

#[test]
fn kinematic_parent_chain_reuses_working_storage() {
    use xgc_sim::{RapierWorld, FIXED_DT};
    use xgc_world::Parent;

    let mut world = World::new();
    let root = world.spawn((Transform::default(),));
    let mut parent = world.entity(root).unwrap();
    // 八层链会超过临时 Vec 首次分配的容量，覆盖扩容后的复用。
    for _ in 0..6 {
        let node = world.spawn((Transform::default(), Parent { entity: parent }));
        parent = world.entity(node).unwrap();
    }
    world.spawn((
        Transform::default(),
        Parent { entity: parent },
        PhysicsBody {
            kind: "kinematic".into(),
            ..PhysicsBody::default()
        },
    ));

    let mut physics = RapierWorld::new();
    let mut step = |frame| {
        let y = 2.0 + (frame % 4) as f32 * 0.5;
        world.get_mut::<Transform>(root).unwrap().position[1] = y;
        physics.step(&mut world, FIXED_DT);
        // 每一步都从碰撞世界读父节点的新位置，避免建体后跳过同步时假绿。
        assert_eq!(physics.ground_under(0.0, 0.0, 16.0), Some(y + 0.5));
    };
    for frame in 0..8 {
        step(frame);
    }

    reset_counter();
    for frame in 8..68 {
        step(frame);
    }
    let (allocations, bytes) = snapshot().expect("量测线程的计数器");
    eprintln!("kinematic parent chain: 60 帧 {allocations} 次分配 / {bytes} 字节");
    assert_eq!(physics.rebuild_count(), 1);
    assert_eq!(physics.body_patch_count(), 0);
    // 为固定负载保留 Rapier 单步的分配预算；父链扩容只需在预热阶段发生。
    assert!(
        allocations <= 60 * 3,
        "运动学刚体父链超预算：60 帧 {allocations} 次分配 / {bytes} 字节"
    );
}

#[test]
fn ui_layout_reuses_working_storage() {
    use xgc_sim::{System, UiLayoutSystem, FIXED_DT};
    use xgc_world::{Parent, UiElement, UiLayout};

    let mut world = World::new();
    let hud = world.spawn((
        UiElement {
            anchor_min: [0.5, 0.0],
            anchor_max: [0.5, 0.0],
            ..UiElement::default()
        },
        UiLayout::default(),
    ));
    let hud_id = world.entity(hud).unwrap();
    let panel = world.spawn((
        Parent { entity: hud_id },
        UiElement {
            offset_min: [32.0, 16.0],
            offset_max: [416.0, 144.0],
            ..UiElement::default()
        },
        UiLayout::default(),
    ));
    let panel_id = world.entity(panel).unwrap();
    // 让 HUD 的元素数超过标准库稳定排序的栈缓冲，覆盖大型布局的稳态开销。
    let tile = (0..1024)
        .map(|index| {
            let x = (index % 32) as f32 * 12.0;
            let y = (index / 32) as f32 * 4.0;
            world.spawn((
                Parent { entity: panel_id },
                UiElement {
                    offset_min: [x, y],
                    offset_max: [x + 8.0, y + 8.0],
                    ..UiElement::default()
                },
                UiLayout::default(),
            ))
        })
        .last()
        .unwrap();

    let mut system = UiLayoutSystem::default();
    let mut step = |frame: u32| {
        let slide = (frame % 8) as f32 * 4.0;
        let visible = frame % 2 == 0;
        let element = world.get_mut::<UiElement>(hud).unwrap();
        element.offset_min = [slide, 0.0];
        element.offset_max = [slide + 320.0, 240.0];
        element.visible = visible;
        system.run(&mut world, FIXED_DT);
        // 末端元素随 HUD 移动，并落在两级裁剪后的矩形之外。
        let hud_left = 640.0 + slide;
        let layout = world.get::<UiLayout>(tile).unwrap();
        assert_eq!(layout.rect, [hud_left + 404.0, 140.0, 8.0, 8.0]);
        assert_eq!(layout.clip, [hud_left + 32.0, 16.0, 288.0, 128.0]);
        assert_eq!(layout.visible, visible);
    };
    for frame in 0..8 {
        step(frame);
    }

    let start = Instant::now();
    reset_counter();
    for frame in 8..68 {
        step(frame);
    }
    let (allocations, bytes) = snapshot().expect("量测线程的计数器");
    eprintln!(
        "UI layout: 60 帧 / {} 个元素 / {allocations} 次分配 / {bytes} 字节 / {:?}",
        world.len(),
        start.elapsed()
    );
    assert_eq!(allocations, 0, "UI 布局稳态不应分配：{bytes} 字节");
}

#[test]
fn ui_layout_reuses_buffers_across_worlds() {
    use xgc_sim::{System, UiLayoutSystem, FIXED_DT};
    use xgc_world::{Parent, UiElement, UiLayout};

    let mut world = World::new();
    let spare = world.spawn((Parent::default(), UiElement::default(), UiLayout::default()));
    let first = world.spawn((
        Parent::default(),
        UiElement {
            offset_min: [10.0, 0.0],
            offset_max: [50.0, 40.0],
            visible: false,
            ..UiElement::default()
        },
        UiLayout::default(),
    ));
    let first_id = world.entity(first).unwrap();
    let second = world.spawn((
        Parent { entity: first_id },
        UiElement {
            offset_min: [20.0, 0.0],
            offset_max: [80.0, 50.0],
            ..UiElement::default()
        },
        UiLayout::default(),
    ));
    world.get_mut::<Parent>(first).unwrap().entity = world.entity(second).unwrap();
    // swap_remove 将较大的 ID 挪到首行，循环仍应从较小的 ID 开始求解。
    assert!(world.despawn(spare));
    UiLayoutSystem {
        viewport: [200.0, 100.0],
    }
    .run(&mut world, FIXED_DT);
    let first_layout = world.get::<UiLayout>(first).unwrap();
    assert_eq!(first_layout.rect, [30.0, 0.0, 40.0, 40.0]);
    assert_eq!(first_layout.clip, [20.0, 0.0, 60.0, 50.0]);
    assert!(!first_layout.visible);
    let second_layout = world.get::<UiLayout>(second).unwrap();
    assert_eq!(second_layout.rect, [20.0, 0.0, 60.0, 50.0]);
    assert_eq!(second_layout.clip, [0.0, 0.0, 200.0, 100.0]);
    assert!(second_layout.visible);

    let mut other = World::new();
    let child = other.spawn((
        Parent { entity: first_id },
        UiElement {
            anchor_min: [1.0, 0.0],
            anchor_max: [1.0, 0.0],
            offset_min: [-45.0, 10.0],
            offset_max: [-25.0, 20.0],
            ..UiElement::default()
        },
        UiLayout::default(),
    ));
    reset_counter();
    UiLayoutSystem {
        viewport: [300.0, 200.0],
    }
    .run(&mut other, FIXED_DT);
    let (allocations, _) = snapshot().expect("量测线程的计数器");
    assert_eq!(allocations, 0, "同一线程换世界后应继续复用布局缓冲");
    // 第二个世界缺少该父节点，必须按自己的视口求解。
    let layout = other.get::<UiLayout>(child).unwrap();
    assert_eq!(layout.rect, [255.0, 10.0, 20.0, 10.0]);
    assert_eq!(layout.clip, [0.0, 0.0, 300.0, 200.0]);
    assert!(layout.visible);
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
