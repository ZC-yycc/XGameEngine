//! S3 Crowd 中档基线。Release 下独占 CPU / GPU，单测试线程运行。
//! `cargo test -p xgc-host --release --test crowd -- --ignored --nocapture --test-threads=1`
//!
//! 500 个同屏蒙皮角色共享一个导入资产。2,000 个播放实例指每角色一条基础剪辑和
//! 三条覆盖层。骨骼、剪辑和权重都进正常采样与蒙皮路径。角色几何由测试生成。

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::time::{Duration, Instant};

use xgc_host::{asset_paths, ScriptedRunner};
use xgc_render::{
    build_draw_list, camera_view_projection, transform_point, AnimationPath, DrawList, GpuContext,
    Mesh, MeshCache, PlayedClip, RenderStats, Renderer, ResourceDesc, ResourceUsage,
};
use xgc_sim::{AnimationLayerSystem, AnimatorSystem, System as SimSystem};
use xgc_world::{AnimationLayers, Animator, LayerMode, World, WorldData};

#[path = "crowd/fixture.rs"]
mod fixture;

use fixture::{
    fixture_assets, fixture_world, phase, restore, speed, FixtureDir, ANIMATION_INSTANCES,
    CAMERA_Z, CELL_SIZE, CHARACTERS, CLIPS, DURATION, FOV_DEG, HEIGHT, JOINTS, LAYER_WEIGHTS,
    MODEL, TRIANGLES, VERTICES, WIDTH,
};

const WARMUP: usize = 24;
const SAMPLES: usize = 120;
const ALLOC_SAMPLES: usize = 8;

#[derive(Clone, Copy, Debug, Default)]
struct Allocations {
    calls: u64,
    bytes: u64,
}

thread_local! {
    static ALLOCATIONS: Cell<Option<Allocations>> = const { Cell::new(None) };
}

struct CountingAllocator;

fn count(pointer: *mut u8, bytes: usize) -> *mut u8 {
    if !pointer.is_null() {
        let _ = ALLOCATIONS.try_with(|counter| {
            if let Some(mut sample) = counter.get() {
                sample.calls += 1;
                sample.bytes += bytes as u64;
                counter.set(Some(sample));
            }
        });
    }
    pointer
}

// SAFETY: 原样转发指针与布局给 System。线程本地计数不分配，也不解引用指针。
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

fn allocations<T>(work: impl FnOnce() -> T) -> (T, Allocations) {
    ALLOCATIONS.set(Some(Allocations::default()));
    let result = work();
    (result, ALLOCATIONS.take().unwrap())
}

fn assert_mesh(mesh: &Mesh) {
    assert_eq!(mesh.positions.len(), VERTICES);
    assert_eq!(mesh.indices.len(), TRIANGLES * 3);
    let skin = mesh.skin.as_ref().expect("导入网格必须带蒙皮");
    assert_eq!(skin.joint_nodes.len(), JOINTS);
    assert_eq!(skin.inverse_bind.len(), JOINTS);
    assert!(skin
        .inverse_bind
        .iter()
        .any(|matrix| matrix[12] != 0.0 || matrix[13] != 0.0));
    assert_eq!(skin.joints.len(), VERTICES);
    assert_eq!(skin.weights.len(), VERTICES);
    let mut covered = [false; JOINTS];
    for (joints, weights) in skin.joints.iter().zip(&skin.weights) {
        for (&joint, &weight) in joints.iter().zip(weights) {
            if weight > 0.0 {
                covered[usize::from(joint)] = true;
            }
        }
    }
    assert!(covered.into_iter().all(|used| used));
    let set = mesh.animations.as_ref().unwrap();
    assert_eq!(set.clips.len(), CLIPS.len());
    assert_eq!(set.nodes.len(), JOINTS + 1);
    for name in CLIPS {
        let clip = set.clip(name).unwrap();
        assert_eq!(clip.duration, DURATION);
        assert_eq!(clip.channels.len(), JOINTS);
        for (joint, channel) in clip.channels.iter().enumerate() {
            assert_eq!(channel.node, joint);
            assert_eq!(channel.path, AnimationPath::Translation);
            assert_eq!(channel.times.len(), 3);
        }
        let rest = set.joint_pose(name, 0.0, false, &skin.joint_nodes).unwrap();
        let peak = set
            .joint_pose(name, DURATION * 0.5, false, &skin.joint_nodes)
            .unwrap();
        assert_eq!(rest.len(), JOINTS);
        assert_eq!(peak.len(), JOINTS);
        assert!(
            rest.iter().zip(&peak).all(|(a, b)| a != b),
            "剪辑 {name} 必须驱动每个关节"
        );
    }
}

fn assert_playback(world: &World, steps: usize, dt: f32) -> DrawList {
    let list = build_draw_list(world);
    assert_eq!(world.len(), CHARACTERS + 1);
    assert_eq!(list.len(), CHARACTERS);
    let mut playing = 0;
    for (character, item) in list.items.iter().enumerate() {
        assert_eq!(item.model, MODEL);
        assert!(item.pose.is_none(), "基准只走模型动画入口");
        let expected_time = |clip: usize| {
            (0..steps).fold(phase(character, clip), |time, _| {
                let next = time + dt * speed(character, clip);
                if clip == 0 {
                    next.rem_euclid(DURATION)
                } else {
                    next
                }
            })
        };
        let base = item.animation.as_ref().unwrap();
        assert_eq!(base.clip, CLIPS[0]);
        assert_eq!(base.time, expected_time(0));
        assert!(base.looping && base.blend_from.is_empty() && base.additive_clip.is_empty());
        playing += 1;
        assert_eq!(item.layers.len(), LAYER_WEIGHTS.len());
        for (layer_index, layer) in item.layers.iter().enumerate() {
            let clip = layer_index + 1;
            assert_eq!(layer.clip, CLIPS[clip]);
            assert_eq!(layer.mode, LayerMode::Override);
            assert_eq!(layer.weight, LAYER_WEIGHTS[layer_index]);
            assert_eq!(layer.time, expected_time(clip));
            assert!(layer.looping && layer.fade.is_none() && layer.mask.is_empty());
            playing += 1;
        }
    }
    assert_eq!(playing, ANIMATION_INSTANCES);
    list
}

fn assert_frame(stats: RenderStats, cold: bool) {
    assert_eq!(stats.draws, CHARACTERS);
    assert_eq!(stats.draw_calls, CHARACTERS);
    // 现行 GPU 蒙皮首次分配 palette 和 model 两张竞技场，稳态应复用。
    assert_eq!(stats.skin_uniform_buffers, if cold { 2 } else { 0 });
    assert_eq!(stats.point_shadow_draw_calls, 0);
    assert_eq!(stats.shadow_skin_uniform_buffers, 0);
    assert_eq!(stats.ui_draws, 0);
    assert_eq!(stats.passes, 2, "HDR 主场景与 tone-map");
}

fn frame_hash(pixels: &[u8]) -> u64 {
    pixels.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
    })
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn latency(label: &str, samples: &mut [Duration]) {
    samples.sort_unstable();
    let rank = |p: usize| ms(samples[(samples.len() * p).div_ceil(100) - 1]);
    println!(
        "S3 {label} samples={} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} min_ms={:.3} max_ms={:.3}",
        samples.len(),
        rank(50),
        rank(95),
        rank(99),
        ms(samples[0]),
        rank(100),
    );
}

#[cfg(windows)]
fn peak_working_set() -> Option<u64> {
    use std::process;

    let query = format!("(Get-Process -Id {}).PeakWorkingSet64", process::id());
    let output = process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &query])
        .output()
        .ok()?;
    output.status.success().then_some(())?;
    std::str::from_utf8(&output.stdout)
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(not(windows))]
fn peak_working_set() -> Option<u64> {
    None
}

fn verify_pixels(reference: &[u8], comparison: &[u8], world: &World, list: &DrawList, label: &str) {
    let vp = camera_view_projection(world, WIDTH, HEIGHT).expect("必须使用基准相机");
    let size = (WIDTH * HEIGHT * 4) as usize;
    assert_eq!(reference.len(), size);
    assert_eq!(comparison.len(), size);
    // 同一个深度平面，每个角色对应互不重叠的屏幕格。逐格核对，防止单个角色替全场通过。
    let half =
        ((HEIGHT as f32 * 0.5) / (CAMERA_Z * (FOV_DEG.to_radians() * 0.5).tan()) * CELL_SIZE * 0.5)
            .floor() as usize;
    let mut changed = 0;
    for item in &list.items {
        let [x, y, z, w] = transform_point(vp, [item.transform[12], item.transform[13], 0.0, 1.0]);
        assert!(w > 0.0 && x.abs() < w && y.abs() < w && z > 0.0 && z < w);
        let cx = ((x / w + 1.0) * WIDTH as f32 * 0.5).round() as usize;
        let cy = ((1.0 - y / w) * HEIGHT as f32 * 0.5).round() as usize;
        assert!(
            cx > half && cy > half && cx + half < WIDTH as usize && cy + half < HEIGHT as usize
        );
        let mut differences = 0;
        let mut lit = 0;
        for row in cy - half..cy + half {
            let start = (row * WIDTH as usize + cx - half) * 4;
            let end = start + half * 2 * 4;
            for (a, b) in reference[start..end]
                .chunks_exact(4)
                .zip(comparison[start..end].chunks_exact(4))
            {
                lit += usize::from(a[..3] != [0, 0, 0]);
                differences += usize::from(a[..3] != b[..3]);
            }
        }
        assert!(
            lit > 0 && differences > 0,
            "{label} 的角色 {} 没有有效像素变化",
            item.entity
        );
        changed += differences;
    }
    println!("S3 image_check={label} checked_characters={} changed_rgb_pixels={changed} rgba8_hash={:016x}", list.len(), frame_hash(comparison));
}

fn sample_animation_cpu(initial: &WorldData, mesh: &Mesh, dt: f32, checkpoints: [u64; 2]) {
    let mut world = restore(initial);
    let mut base = AnimatorSystem::default();
    let mut layers = AnimationLayerSystem;
    let set = mesh.animations.as_ref().unwrap();
    let joints = &mesh.skin.as_ref().unwrap().joint_nodes;
    let mut clocks = Vec::with_capacity(SAMPLES);
    let mut extraction = Vec::with_capacity(SAMPLES);
    let mut pose = Vec::with_capacity(SAMPLES);
    for frame in 0..WARMUP + SAMPLES {
        // 复用生产系统和生产采样函数。这里省去其余宿主系统、材质解析、palette 转码与提交。
        let start = Instant::now();
        base.run(&mut world, dt);
        layers.run(&mut world, dt);
        let advanced = Instant::now();
        let list = build_draw_list(&world);
        let extracted = Instant::now();
        let mut joints_sampled = 0;
        for item in &list.items {
            let sample = item.animation.as_ref().unwrap();
            let pose = set
                .joint_pose_played(
                    PlayedClip {
                        clip: &sample.clip,
                        time: sample.time,
                        looping: sample.looping,
                        root_motion: None,
                    },
                    None,
                    None,
                    &item.layers,
                    joints,
                )
                .unwrap();
            joints_sampled += black_box(pose).len();
        }
        let sampled = Instant::now();
        assert_eq!(joints_sampled, CHARACTERS * JOINTS);
        if frame + 1 == WARMUP {
            assert_eq!(world.state_hash(), checkpoints[0]);
        }
        if frame >= WARMUP {
            clocks.push(advanced - start);
            extraction.push(extracted - advanced);
            pose.push(sampled - extracted);
        }
    }
    assert_eq!(world.state_hash(), checkpoints[1]);
    latency("cpu_animation_clocks_isolated", &mut clocks);
    latency("cpu_draw_list_extract_isolated", &mut extraction);
    latency("cpu_joint_pose_sampling_isolated", &mut pose);
}

#[test]
#[ignore = "S3 Release 中档真机基线需要独占 CPU / GPU"]
fn crowd_500x200() {
    if cfg!(debug_assertions) {
        panic!("S3 基线须使用 --release");
    }
    let fixture = FixtureDir::new();
    let started = Instant::now();
    let asset_bytes = fixture_assets(&fixture.0);
    let authored = fixture_world();
    let initial_hash = authored.state_hash();
    let snapshot = authored.world_data();
    let fixture_ms = ms(started.elapsed());
    drop(authored);

    let started = Instant::now();
    let context = GpuContext::headless().expect("S3 需要可用的 wgpu 设备");
    let resource = context
        .create_resource(&ResourceDesc::texture2d(
            "xgc-s3-display",
            "rgba8unorm-srgb",
            WIDTH,
            HEIGHT,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let target = resource.as_texture().unwrap();
    let device_ms = ms(started.elapsed());
    let adapter = context.adapter_info();
    assert!(
        matches!(
            format!("{:?}", adapter.device_type).as_str(),
            "DiscreteGpu" | "IntegratedGpu"
        ),
        "S3 需要硬件 GPU"
    );
    assert_eq!((target.width(), target.height()), (WIDTH, HEIGHT));

    let init = Instant::now();
    let world = restore(&snapshot);
    let restored = Instant::now();
    let (mut runner, meshes) =
        ScriptedRunner::with_assets(world, asset_paths(Some(&fixture.0), &[]));
    let host_ready = Instant::now();
    assert!(meshes.is_empty());
    let mut renderer = Renderer::new(meshes);
    // 固定单采样、环境光与默认色调映射，将负载集中在采样、蒙皮和上传。
    renderer.set_sample_count(1);
    let render_start = Instant::now();
    // 网格缓存留空，由首帧的正常渲染路径触发实际 glTF 读取和导入。
    let cold = renderer.render(&context, target, runner.world()).unwrap();
    let cold_render = render_start.elapsed();
    // 复制同一队列的 1×1 区域并等映射，以固定单帧在途。计时单列，含读回与主机同步。
    let fence = || assert_eq!(context.read_texture_rgba8(target, 1, 1).unwrap().len(), 4);
    fence();
    let first_complete = Instant::now();
    assert_eq!(runner.world().state_hash(), initial_hash);
    let dt = runner.fixed_dt();
    assert_eq!(dt, 1.0 / 60.0);
    assert_frame(cold, true);
    assert_eq!(renderer.meshes().len(), 1);
    assert_eq!(renderer.gpu_skin_mesh_count(), 1);
    assert_eq!(renderer.gpu_mesh_count(), 0);
    println!("S3 characters={CHARACTERS} joints_per_character={JOINTS} animation_instances={ANIMATION_INSTANCES} vertices_per_character={VERTICES} triangles_per_character={TRIANGLES} asset_bytes={asset_bytes} resolution={WIDTH}x{HEIGHT} msaa=1 fixed_dt_s={dt:.9} warmup={WARMUP} samples={SAMPLES} adapter={adapter:?} features={:?}", context.device().features());
    println!("S3 fixture_ms={fixture_ms:.3} device_and_target_ms={device_ms:.3} restore_world_ms={:.3} host_setup_ms={:.3} renderer_setup_ms={:.3} first_cpu_render_with_import_ms={:.3} first_render_with_import_through_fence_ms={:.3} in_memory_world_to_first_frame_with_import_and_fence_ms={:.3} load_budget_ms=300.000", ms(restored - init), ms(host_ready - restored), ms(render_start - host_ready), ms(cold_render), ms(first_complete - render_start), ms(first_complete - init));

    for frame in 0..WARMUP {
        assert_eq!(runner.step().unwrap(), frame as u64);
        assert_frame(
            renderer.render(&context, target, runner.world()).unwrap(),
            false,
        );
        fence();
    }
    let warm_list = assert_playback(runner.world(), WARMUP, dt);
    let warm_state = runner.world().world_data();
    let warm_hash = runner.world().state_hash();
    assert_ne!(warm_hash, initial_hash);

    let mut host = Vec::with_capacity(SAMPLES);
    let mut render = Vec::with_capacity(SAMPLES);
    let mut cpu = Vec::with_capacity(SAMPLES);
    let mut wait = Vec::with_capacity(SAMPLES);
    let mut completion = Vec::with_capacity(SAMPLES);
    for frame in WARMUP..WARMUP + SAMPLES {
        let start = Instant::now();
        let actual = runner.step().unwrap();
        let advanced = Instant::now();
        let stats = renderer.render(&context, target, runner.world()).unwrap();
        let submitted = Instant::now();
        fence();
        let finished = Instant::now();
        host.push(advanced - start);
        render.push(submitted - advanced);
        cpu.push(submitted - start);
        wait.push(finished - submitted);
        completion.push(finished - start);
        assert_eq!(actual, frame as u64);
        assert_frame(stats, false);
    }
    let steps = WARMUP + SAMPLES;
    let end_list = assert_playback(runner.world(), steps, dt);
    let end_hash = runner.world().state_hash();
    assert_ne!(end_hash, warm_hash);
    latency("cpu_host_step", &mut host);
    latency("cpu_renderer_entry_through_submit", &mut render);
    latency("cpu_step_and_renderer_through_submit", &mut cpu);
    latency("host_post_submit_to_readback_fence", &mut wait);
    latency("wall_step_through_readback_fence", &mut completion);

    // 同一路径另开分配采样段，以免统计本身改动计时。只计测试线程 Rust 成功请求。
    let mut host_alloc = Vec::with_capacity(ALLOC_SAMPLES);
    let mut render_alloc = Vec::with_capacity(ALLOC_SAMPLES);
    for frame in steps..steps + ALLOC_SAMPLES {
        let (actual, sample) = allocations(|| runner.step().unwrap());
        host_alloc.push(sample);
        let (stats, sample) =
            allocations(|| renderer.render(&context, target, runner.world()).unwrap());
        render_alloc.push(sample);
        fence();
        assert_eq!(actual, frame as u64);
        assert_frame(stats, false);
    }
    assert_playback(runner.world(), steps + ALLOC_SAMPLES, dt);
    let rss = peak_working_set();
    println!("S3 rust_test_thread_alloc frames={ALLOC_SAMPLES} host={host_alloc:?} renderer={render_alloc:?} peak_process_working_set_bytes={rss:?}");

    let pixels = |renderer: &mut Renderer, world: &World| {
        assert_frame(renderer.render(&context, target, world).unwrap(), false);
        context.read_texture_rgba8(target, WIDTH, HEIGHT).unwrap()
    };
    let warm = restore(&warm_state);
    assert_eq!(warm.state_hash(), warm_hash);
    let reference = pixels(&mut renderer, &warm);
    assert!(
        reference == pixels(&mut renderer, &warm),
        "固定状态重复渲染须逐字节相同"
    );
    println!("S3 warm_rgba8_hash={:016x}", frame_hash(&reference));
    for (slot, name) in CLIPS.into_iter().enumerate() {
        let mut control = restore(&warm_state);
        if slot == 0 {
            control.for_each_mut::<Animator>(|_, animator| animator.clip.clear());
        } else {
            control
                .for_each_mut::<AnimationLayers>(|_, layers| layers.layers[slot - 1].weight = 0.0);
        }
        verify_pixels(
            &reference,
            &pixels(&mut renderer, &control),
            &warm,
            &warm_list,
            name,
        );
    }
    let after_alloc = pixels(&mut renderer, runner.world());
    verify_pixels(
        &reference,
        &after_alloc,
        runner.world(),
        &end_list,
        "after_steady_state",
    );
    assert_eq!(renderer.gpu_skin_mesh_count(), 1);
    assert_eq!(renderer.gpu_mesh_count(), 0);
    drop(renderer);
    drop(runner);
    drop(resource);
    drop(context);

    // 再用空网格缓存单量模型导入。文件已由本测试写出并读过，属于热文件系统口径。
    let mut meshes = MeshCache::with_assets(asset_paths(Some(&fixture.0), &[]));
    let import_start = Instant::now();
    let mesh = meshes.get(MODEL).unwrap().unwrap();
    let import = import_start.elapsed();
    assert_mesh(mesh);
    println!("S3 gltf_cache_fill_warm_filesystem_ms={:.3}", ms(import));
    sample_animation_cpu(&snapshot, mesh, dt, [warm_hash, end_hash]);
    // 按现行 256×mat4 palette 上传口径推导主场景字节，RenderStats 没有实际流量计数。
    println!("S3 verified characters={CHARACTERS} sampled_joints_per_frame={} animation_instances={ANIMATION_INSTANCES} main_skin_draws={} logical_passes={} palette_bytes_per_frame_from_source={} initial_state_hash={initial_hash:016x} warm_state_hash={warm_hash:016x} steady_end_state_hash={end_hash:016x}", CHARACTERS * JOINTS, cold.draw_calls, cold.passes, CHARACTERS * 256 * 64);
    println!("S3 budgets cpu_ms=6.7 animation_ms=2.0 gpu_ms=10.0 memory_gb=2.0 load_ms=300.0");
    println!("S3 missing=GPU 帧及逐 pass 时间戳、独立 palette 转码/上传计时、GPU 显存峰值、冷文件系统及完整项目加载；主机读回围栏时间含复制及同步；素材为共享合成角色，单采样 1080p 环境光，无阴影、状态机转移与根运动");
}
