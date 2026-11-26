//! 测量带真实结构化组件的世界哈希，只计预热后的调用。

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::time::Instant;

use xgc_world::{
    AnimLayer, AnimationLayers, AnimationState, LayerMode, LodGroup, LodLevel, StateDef,
    StateMachine, TransitionCondition, TransitionDef, World,
};

thread_local! {
    static COUNTER: Cell<(u64, usize)> = const { Cell::new((0, 0)) };
}

struct CountingAllocator;

// SAFETY: 每项操作保留原指针和布局约定，计数器只记录成功的系统分配。
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        record_allocation(ptr, layout.size());
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let ptr = unsafe { System.realloc(ptr, layout, size) };
        record_allocation(ptr, size);
        ptr
    }
}

fn record_allocation(ptr: *mut u8, size: usize) {
    if !ptr.is_null() {
        let _ = COUNTER.try_with(|counter| {
            let (calls, bytes) = counter.get();
            counter.set((calls + 1, bytes + size));
        });
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn machine() -> StateMachine {
    StateMachine {
        initial: "idle".into(),
        states: vec![
            StateDef {
                name: "idle".into(),
                clip: "Idle".into(),
                ..StateDef::default()
            },
            StateDef {
                name: "run".into(),
                clip: "Run".into(),
                speed: 1.25,
                ..StateDef::default()
            },
        ],
        transitions: vec![
            TransitionDef {
                from: Some("idle".into()),
                to: "run".into(),
                when: Some(TransitionCondition::AtLeast {
                    parameter: "speed".into(),
                    value: 0.75,
                }),
                duration: 0.2,
            },
            TransitionDef {
                from: None,
                to: "idle".into(),
                when: Some(TransitionCondition::Below {
                    parameter: "speed".into(),
                    value: 0.25,
                }),
                duration: 0.1,
            },
            TransitionDef {
                to: "idle".into(),
                ..TransitionDef::default()
            },
        ],
    }
}

fn animation_layers() -> World {
    let mut world = World::new();
    for i in 0..128 {
        world.spawn((AnimationLayers {
            layers: vec![
                AnimLayer {
                    name: format!("骨骼层{i}"),
                    mode: LayerMode::Override,
                    weight: 0.6,
                    speed: 1.25,
                    looping: false,
                    mask: vec!["根骨骼".into(), "hip".into()],
                    machine: Some(machine()),
                    state: "run".into(),
                    time: 0.75,
                    parameters: [("speed".into(), 1.5), ("斜率".into(), -0.0)].into(),
                    fade_from: "Idle".into(),
                    fade_from_time: 0.5,
                    fade_from_speed: 1.25,
                    fade_from_looping: false,
                    fade_elapsed: 0.125,
                    fade_duration: 0.25,
                    ..AnimLayer::default()
                },
                AnimLayer {
                    name: "breathing".into(),
                    clip: "Breath".into(),
                    ..AnimLayer::default()
                },
            ],
        },));
    }
    world
}

fn animation_states() -> World {
    let mut world = World::new();
    for i in 0..128 {
        world.spawn((AnimationState {
            machine: machine(),
            state: "run".into(),
            time: i as f32 * 0.125,
            parameters: [("speed".into(), 1.5), ("斜率".into(), -0.0)].into(),
        },));
    }
    world
}

fn lods() -> World {
    let mut world = World::new();
    for i in 0..128 {
        world.spawn((LodGroup {
            levels: vec![
                LodLevel {
                    model: "builtin://sphere".into(),
                    max_distance: 10.0 + i as f32,
                },
                LodLevel {
                    model: "builtin://cube".into(),
                    max_distance: 50.0,
                },
                LodLevel {
                    model: "builtin://plane".into(),
                    max_distance: -0.0,
                },
            ],
        },));
    }
    world
}

fn measure(label: &str, world: &World, expected: u64) {
    const HASHES: u64 = 60;
    assert_eq!(world.state_hash(), expected, "{label} 的旧哈希应保持不变");
    COUNTER.set((0, 0));
    let start = Instant::now();
    let mut hashes = 0u64;
    for _ in 0..HASHES {
        hashes = hashes.wrapping_add(black_box(world).state_hash());
    }
    let elapsed = start.elapsed();
    let (allocations, bytes) = COUNTER.get();
    eprintln!("{label}: hash={expected:016x}, {} entities × {HASHES} hashes, {allocations} allocations, {bytes} bytes, {elapsed:?}", world.len());
    assert_eq!(hashes, expected.wrapping_mul(HASHES));
    assert_eq!((allocations, bytes), (0, 0), "{label} 的稳态哈希分配");
}

#[test]
fn hash_animation_layers() {
    measure("animation.layers", &animation_layers(), 0x7ad25120368a8169);
}

#[test]
fn hash_animation_states() {
    measure("animation.state", &animation_states(), 0x95a1837ce6c4a925);
}

#[test]
fn hash_lod_levels() {
    measure("render.lod", &lods(), 0xe5b155b5f041e325);
}
