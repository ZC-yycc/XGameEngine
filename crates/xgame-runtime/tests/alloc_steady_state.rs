//! 稳态分配门禁：`World::update` 在**预热之后**不该每帧分配。
//!
//! # 为什么要有这条测试
//!
//! 帧时间基准（`update_throughput.rs`）只能看出"慢了"，看不出"为什么慢"。每帧
//! 申请几块临时 `Vec` 在 60 FPS 下不会体现在均值里，但会让帧时间抖动、让
//! 分配器成为热点，而且在 Web / 主机这类分配器较弱的平台上代价更大。项目纪律
//! 是"稳态热路径零分配"，这条测试把那句话变成一个可执行的判据。
//!
//! # 怎么数
//!
//! `#[global_allocator]` 包一层 `System`，用原子开关控制计数窗口：预热阶段
//! 允许分配（建刚体、扩容查询结构都是**一次性**的），窗口只覆盖预热之后的
//! N 帧。测试框架自身的分配在同一个进程里，但都发生在开关之外。
//!
//! # 为什么放在 `tests/` 而不是 `src`
//!
//! 自定义全局分配器是**进程级**的，只能在独立的 test binary 里用。这个文件
//! 只有一条测试，所以不存在同 binary 内并行测试互相污染计数的问题
//! （`--test-threads` 再大，单个测试也是单线程跑的）。

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use xgame_core::animation_runtime::{AnimClip, AnimNode, Interpolation, Track, TrackPath};
use xgame_core::{
    AnimationAsset, AnimationLoader, Animator, Entity, PhysicsBody, RootMotion, Scene, UiElement,
};
use xgame_math::glam::{Quat, Vec3};
use xgame_runtime::{World, WorldConfig};

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

struct CountingAlloc;

impl CountingAlloc {
    fn record(size: usize) {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(size, Ordering::Relaxed);
        }
    }
}

// SAFETY: 每一项都是"先记账、再原样转交给 `System`"，没有改变分配语义；
// 记账只用无锁原子操作，不分配、不 panic、不递归进分配器。
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::record(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Self::record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Self::record(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

const ENTITIES: usize = 200;
const WARMUP_FRAMES: usize = 30;
const MEASURED_FRAMES: usize = 60;

/// 200 实体、一半静态碰撞体：`sync_bodies` / 层级解算 / 拾取索引全都真的跑起来，
/// 但没有任何活动脚本 —— 脚本宿主每帧的分配属于另一条路径，混进来会让这条
/// 门禁的数字说不清是谁的。
fn build_scene() -> Scene {
    let mut scene = Scene::new("alloc");
    for i in 0..ENTITIES {
        let mut e = Entity::new(format!("e{i}"));
        if i % 2 == 0 {
            e.physics = Some(PhysicsBody::static_box([0.5, 0.5, 0.5]));
        }
        e.transform.position = Vec3::new((i % 20) as f32, 0.5, (i / 20) as f32);
        scene.add_entity(e);
    }
    scene
}

/// 12 个 UI 按钮，全部落在 1280×720 视口内：让布局 / 命中测试真的跑起来
/// （UI pass 每帧都要重算）。
///
/// `at(anchor, x, y, w, h)` 的第一个参数是**归一化锚点**，不是位置 —— 第一版
/// 误传了 `[10.0, i * 50.0]` 当锚点，锚点越界会让布局走异常分支，"零分配"
/// 就成了"没跑"的假绿。
fn build_ui_scene() -> Scene {
    let mut scene = Scene::new("alloc-ui");
    for i in 0..12 {
        let mut e = Entity::new(format!("btn{i}"));
        e.ui =
            Some(UiElement::button("OK").at([0.0, 0.0], 10.0, 10.0 + i as f32 * 50.0, 120.0, 40.0));
        scene.add_entity(e);
    }
    scene
}

/// 跑 `MEASURED_FRAMES` 帧，返回 `(分配次数, 分配字节数, 计数窗口内的世界时长增量)`。
///
/// 第三个返回值是**活跃性证明**：如果 `update` 因为某种原因没真的跑，分配次数
/// 一样会是 0，那这条测试就成了永远为绿的空转。用**增量**而不是绝对值，是为了
/// 不把预热帧算进去（第一版就是这么写错的：断言 1.0s，实际 1.5s）。
fn measure_steady_state(mut world: World, viewport: Option<(u32, u32)>) -> (usize, usize, f32) {
    if let Some((w, h)) = viewport {
        world.set_ui_viewport((w, h));
    }
    for _ in 0..WARMUP_FRAMES {
        world.update(1.0 / 60.0);
    }

    let time_before = world.clock().total_time();
    ALLOC_COUNT.store(0, Ordering::Relaxed);
    ALLOC_BYTES.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::Relaxed);
    for _ in 0..MEASURED_FRAMES {
        world.update(1.0 / 60.0);
    }
    COUNTING.store(false, Ordering::Relaxed);
    let elapsed = world.clock().total_time() - time_before;

    (
        ALLOC_COUNT.load(Ordering::Relaxed),
        ALLOC_BYTES.load(Ordering::Relaxed),
        elapsed,
    )
}

/// 断言一个场景在稳态下零分配（并顺带证明它真的在推进）。
fn assert_zero_alloc(label: &str, world: World, viewport: Option<(u32, u32)>) {
    let (count, bytes, elapsed) = measure_steady_state(world, viewport);

    // 先证明这 N 帧真的推进了世界：否则"零分配"可能只是"什么都没跑"。
    let expected = MEASURED_FRAMES as f32 / 60.0;
    assert!(
        (elapsed - expected).abs() < 1e-2,
        "[{label}] 世界时间应当推进约 {expected}s，实际 {elapsed}s —— 计数窗口里 update 没真的跑"
    );

    assert_eq!(
        count,
        0,
        "[{label}] {MEASURED_FRAMES} 帧稳态里分配了 {count} 次 / {bytes} 字节\
         （每帧 {:.1} 次）；热路径应复用缓冲，别每帧新建",
        count as f64 / MEASURED_FRAMES as f64
    );
}

#[derive(Clone)]
struct MemoryAnimationLoader;

impl AnimationLoader for MemoryAnimationLoader {
    fn load_animation(&self, _model: &str) -> Result<AnimationAsset, String> {
        Ok(AnimationAsset {
            nodes: vec![AnimNode {
                parent: None,
                name: "MotionRoot".into(),
                translation: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            }],
            clips: vec![AnimClip {
                name: "Walk".into(),
                duration: 1.0,
                tracks: vec![Track {
                    node: 0,
                    path: TrackPath::Translation,
                    interpolation: Interpolation::Linear,
                    times: vec![0.0, 1.0],
                    values: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                }],
            }],
        })
    }
}

/// 动画时钟现在住在 `World::update`，因此带根运动的常规游戏帧也必须守住零分配。
fn root_motion_world() -> World {
    let mut scene = Scene::new("alloc-root-motion");
    let mut entity = Entity::new("walker");
    entity.model = "Resources/Walker.gltf".into();
    entity.animator = Some(Animator {
        clip: "Walk".into(),
        root_motion: Some(RootMotion {
            node: "MotionRoot".into(),
            axes: [true, false, false],
        }),
        ..Animator::default()
    });
    scene.add_entity(entity);
    World::new(scene, std::env::temp_dir(), WorldConfig::default())
        .with_animation_loader(MemoryAnimationLoader)
}

/// 稳态每帧零分配。**这是门禁，不是基准** —— 数字一旦不再是 0，说明有人
/// 在热路径里加了 `Vec` / `collect` / `format!`，帧时间基准看不出来。
///
/// 覆盖两条最常走的稳态路径：纯实体 + 物理，和 UI 布局 + 命中。
///
/// # 为什么两条变体写在**一个**测试函数里
///
/// 全局分配器是**进程级**的，同一个 test binary 里的测试默认并行。两条测试
/// 各自开/关计数窗口的话，对方线程的分配会被记进自己的账，出现随机假红。
/// 合成一条（单线程执行）比加锁简单，也比加锁可靠 —— 锁对象自身的分配同样
/// 会落进窗口。
#[test]
fn steady_state_update_does_not_allocate() {
    // 元检查：UI 变体必须真的跑到布局 / 命中，否则它的"零分配"是假绿
    // （锚点写错、视口没设、命中表为空都能造成这种假绿）。
    {
        let mut world = World::new(
            build_ui_scene(),
            std::env::temp_dir(),
            WorldConfig::default(),
        );
        world.set_ui_viewport((1280, 720));
        // 第一个按钮：offset (10,10)、尺寸 120×40 → 中心是 (70,30)。
        world.set_ui_pointer(Some(xgame_math::glam::Vec2::new(70.0, 30.0)));
        world.update(1.0 / 60.0);
        assert!(
            world.ui_hovered().is_some(),
            "UI 命中测试没跑起来，那么 UI 变体的\"零分配\"不说明任何问题"
        );
    }

    assert_zero_alloc(
        "entities+physics",
        World::new(build_scene(), std::env::temp_dir(), WorldConfig::default()),
        None,
    );
    assert_zero_alloc(
        "ui",
        World::new(
            build_ui_scene(),
            std::env::temp_dir(),
            WorldConfig::default(),
        ),
        Some((1280, 720)),
    );

    let mut root_motion = root_motion_world();
    root_motion.update(1.0 / 60.0);
    assert!(
        root_motion.scene().entities[0].transform.position.x > 0.0,
        "根运动变体必须先证明动画资产、采样与实体位移整条链真的跑起来"
    );
    assert_zero_alloc("animation+root-motion", root_motion, None);
}
