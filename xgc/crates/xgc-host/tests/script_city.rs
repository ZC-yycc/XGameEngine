//! S4 中档脚本场景。逐实体钩子和模块回调共用场景、写入结果、哈希与采样窗口。
//! 以 Release 串行运行，预算按设计文档 §13.2 报告。

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use xgc_asset::AssetPaths;
use xgc_host::{asset_paths, ScriptedRunner};
use xgc_script::ScriptRuntime;
use xgc_world::{Script, Transform, World, WorldData};

const ENTITIES: usize = 10_000;
const WARMUP: usize = 32;
const SAMPLES: usize = 240;
const ALLOC_SAMPLES: usize = 30;
const CPU_MS: f64 = 6.7;
const SCRIPT_MS: f64 = 0.5;
const LOAD_MS: f64 = 200.0;
const SCRIPT_PATH: &str = "city.lua";
const SCRIPT_SOURCE: &str = r#"
local id
function start(ctx)
    id = self_id(ctx)
    state_set(ctx, "updates", 0)
end

function update(ctx)
    local updates = state_get(ctx, "updates") + 1
    state_set(ctx, "updates", updates)
    set_pos(ctx, id, updates, id % 17 + updates * 2)
end
"#;
const BATCH_SOURCE: &str = r#"
local M = {}
local updates = 0
local field = "core.transform.position"

function M.update(batch)
    updates = updates + 1
    local offset = updates * 2
    for i = 1, batch:len() do
        local id = i - 1
        batch:entity(i):set(field, {x = id, y = updates, z = id % 17 + offset})
    end
end

return M
"#;

thread_local! {
    static ALLOCATIONS: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
}

struct CountingAllocator;

fn count(pointer: *mut u8, bytes: usize) -> *mut u8 {
    if !pointer.is_null() {
        let _ = ALLOCATIONS.try_with(|counter| {
            if let Some((calls, total)) = counter.get() {
                counter.set(Some((calls + 1, total + bytes as u64)));
            }
        });
    }
    pointer
}

// SAFETY: 转发 System 的原始指针和布局。线程本地计数不分配，也不接触指针指向的内存。
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(unsafe { System.alloc(layout) }, layout.size())
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(unsafe { System.alloc_zeroed(layout) }, layout.size())
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count(unsafe { System.realloc(pointer, layout, size) }, size)
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct ScriptCity {
    root: PathBuf,
    world_data: WorldData,
    initial_hash: u64,
}

impl ScriptCity {
    fn new(source: &str) -> Self {
        let mut world = World::new();
        for _ in 0..ENTITIES {
            world.spawn((
                Transform::default(),
                Script {
                    paths: vec![SCRIPT_PATH.into()],
                },
            ));
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("xgc_s4_{}_{nonce}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let city = Self {
            root,
            world_data: world.world_data(),
            initial_hash: world.state_hash(),
        };
        std::fs::write(city.root.join(SCRIPT_PATH), source).unwrap();
        city
    }

    fn restore_world(&self) -> World {
        let mut world = World::new();
        world.restore_world_data(&self.world_data).unwrap();
        world
    }

    fn assets(&self) -> AssetPaths {
        asset_paths(Some(&self.root), &[])
    }

    fn assert_batch(&self, world: &World, updates: usize) {
        // 只有脚本能写该场景，单步应只有一次非空事务，所有实体得到同一个修订号。
        let revision = self.world_data.revision + updates as u64;
        assert_eq!(world.revision(), revision, "第 {updates} 步事务数");
        assert_eq!(world.len(), ENTITIES);
        let mut checked = 0;
        world.for_each::<Transform>(|key, transform| {
            let id = world.entity(key).unwrap();
            let count = updates as f32;
            let expected = [id.raw() as f32, count, (id.raw() % 17) as f32 + count * 2.0];
            assert_eq!(transform.position, expected, "第 {updates} 步实体 {id}");
            assert_eq!(
                world.entity_revision(id),
                Some(revision),
                "实体 {id} 修订号"
            );
            checked += 1;
        });
        assert_eq!(checked, ENTITIES);
    }
}

impl Drop for ScriptCity {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn report_latency(label: &str, samples: &mut [Duration], budget_ms: f64) {
    samples.sort_unstable();
    let percentile = |rank: usize| ms(samples[(samples.len() * rank).div_ceil(100) - 1]);
    println!(
        "S4 {label} samples={} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} budget_ms={budget_ms:.3}",
        samples.len(),
        percentile(50),
        percentile(95),
        percentile(99),
    );
}

#[test]
#[ignore = "S4 真机基线，使用 Release 和单测试线程运行"]
fn script_city_10k() {
    measure_script_city(SCRIPT_SOURCE);
}

#[test]
#[ignore = "S4 真机批模块计量，使用 Release 和单测试线程运行"]
fn script_city_batch_10k() {
    measure_script_city(BATCH_SOURCE);
}

fn measure_script_city(source: &str) {
    let city = ScriptCity::new(source);
    println!(
        "S4 entities={ENTITIES} warmup={WARMUP} debug_assertions={} initial_hash={:016x}",
        cfg!(debug_assertions),
        city.initial_hash,
    );

    // 输入为已生成的 WorldData；首步会读 Lua 文件、编译、挂载并执行首批写入。
    let start = Instant::now();
    let (mut runner, _) = ScriptedRunner::with_assets(city.restore_world(), city.assets());
    let setup = start.elapsed();
    let frame = runner.step().unwrap();
    let load = start.elapsed();
    let dt = runner.fixed_dt();
    assert_eq!(frame, 0);
    assert_eq!(runner.script_count(), ENTITIES);
    city.assert_batch(runner.world(), 1);
    println!(
        "S4 mounted={} fixed_dt_s={dt:.9} setup_ms={:.3} first_step_ms={:.3} load_to_first_step_ms={:.3} load_budget_ms={LOAD_MS:.3}",
        runner.script_count(),
        ms(setup),
        ms(load - setup),
        ms(load),
    );

    let steps = WARMUP + SAMPLES;
    let mut samples = Vec::with_capacity(SAMPLES);
    for frame in 1..steps {
        let start = Instant::now();
        let actual = runner.step().unwrap();
        let elapsed = start.elapsed();
        if frame >= WARMUP {
            samples.push(elapsed);
        }
        assert_eq!(actual, frame as u64);
        assert_eq!(
            runner.world().revision(),
            city.world_data.revision + actual + 1
        );
    }
    city.assert_batch(runner.world(), steps);
    let host_hash = runner.world().state_hash();
    assert_ne!(host_hash, city.initial_hash);
    report_latency("host_step", &mut samples, CPU_MS);

    // 分配单开采样段，统计 Rust 当前线程成功的 alloc / alloc_zeroed / realloc
    // 调用及请求字节。Lua C 堆由进程峰值内存覆盖，realloc 按新请求的整块大小计。
    ALLOCATIONS.with(|counter| counter.set(Some((0, 0))));
    for frame in steps..steps + ALLOC_SAMPLES {
        assert_eq!(runner.step().unwrap(), frame as u64);
    }
    let (calls, bytes) = ALLOCATIONS.with(|counter| counter.take().unwrap());
    city.assert_batch(runner.world(), steps + ALLOC_SAMPLES);
    let alloc_end_hash = runner.world().state_hash();
    println!(
        "S4 rust_alloc frames={ALLOC_SAMPLES} calls={calls} bytes={bytes} calls_per_frame={:.2} bytes_per_frame={:.2} end_hash={alloc_end_hash:016x}",
        calls as f64 / ALLOC_SAMPLES as f64,
        bytes as f64 / ALLOC_SAMPLES as f64,
    );
    drop(runner);

    // 宿主未公开阶段计时。用同一场景测宿主回调里的生产函数，终态须与固定步路径相等。
    let mut world = city.restore_world();
    let mut runtime = ScriptRuntime::with_assets(city.assets());
    samples.clear();
    for frame in 0..steps {
        let start = Instant::now();
        runtime.run_frame(&mut world, dt);
        let elapsed = start.elapsed();
        assert_eq!(runtime.take_error(), None, "脚本阶段第 {} 步", frame + 1);
        if frame >= WARMUP {
            samples.push(elapsed);
        }
    }
    assert_eq!(runtime.script_count(), ENTITIES);
    city.assert_batch(&world, steps);
    assert_eq!(world.state_hash(), host_hash);
    report_latency("script_phase", &mut samples, SCRIPT_MS);
    println!("S4 verified entities={ENTITIES} steps={steps} host_script_hash={host_hash:016x}");
}
