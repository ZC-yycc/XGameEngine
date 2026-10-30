//! 微基准：级联阴影 pass 的每帧成本，以及"逐级剔除"能省下多少。
//!
//! 默认 `#[ignore]`（耗时波动大、需要显卡，不该进 CI 门禁）。手动跑：
//!
//! ```sh
//! cargo test -p xgame-render --release --test shadow_cost -- --ignored --nocapture
//! ```
//!
//! # 比什么
//!
//! 同一个场景、同一台相机、同一套管线，只切四件事：
//!
//! | 档位 | 阴影 pass | 片元侧 | 每级提交的实例 |
//! |---|---|---|---|
//! | `shadows = off` | 整段跳过 | 3×3 PCF | 0 |
//! | `shadows = on`，逐级剔除关 | 4 级各画一遍**全部**相机可见实例 | 3×3 PCF | 4 × 相机可见 |
//! | `shadows = on`，逐级剔除开 | 每级只画落在该级视锥里的实例 | 3×3 PCF | Σ 命中数 |
//! | `shadows = on`，剔除开 + **软阴影**（softness = 1.0） | 同上 | PCSS | Σ 命中数 |
//!
//! 中档（逐级剔除关）就是引入剔除前的行为，所以这张表本身就是 A/B 对照：
//! 第三行与第二行的差 = 剔除的收益，与第一行的差 = 阴影本身的成本，
//! 第四行与第三行的差 = **PCSS 软阴影的片元侧成本**（只多采样，不碰阴影 pass）。
//!
//! # 场景
//!
//! 45×45 的方块阵、纵深 264 个单位；相机站在阵南侧、45° 视场，看向阵心。
//! 阴影覆盖距离封顶 120（见 `MAX_SHADOW_DISTANCE`），所以远场方块不投影；
//! 但**近场**每级的覆盖范围都比整个阵小得多 —— 这正是逐级剔除该发挥作用的
//! 形态。若把方块全部挤在一小片区域，"每一级都能看见所有人"会让剔除毫无收益，
//! 那样的场景测出来的 0% 不能拿来否定这个功能。
//!
//! # 读数口径
//!
//! 与 `lod_cost` / `culling_cost` 同一套：GPU 时间戳取**场景段**（阴影 pass 在
//! 场景段里，后处理只做色调映射合成），CPU 段记"编码 + 提交"。各档交替跑
//! `ROUNDS` 轮取中位数。
//!
//! ⚠ 绝对值随机器状态浮动，**跨版本比较必须同机背靠背**。

use std::collections::HashMap;
use std::time::Instant;

use xgame_core::{BloomConfig, Entity, Light, RenderConfig, Scene};
use xgame_math::glam::{Mat4, Vec3};
use xgame_render::animation::AnimationDriver;
use xgame_render::gltf_loader::GltfMesh;
use xgame_render::mesh::BuiltinMesh;
use xgame_render::postfx::{HdrTargets, PostFx, HDR_FORMAT};
use xgame_render::scene_pipeline::{
    supported_sample_count, FrameStats, SceneLighting, ScenePipeline,
};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
/// 每帧 3 个时间戳：帧头 / 场景 pass 末 / 帧末。
const STAMPS_PER_FRAME: usize = 3;
const FRAMES: usize = 64;
const ROUNDS: usize = 6;
const WARMUP_TIME: std::time::Duration = std::time::Duration::from_millis(1200);

/// 演员数量：与 `culling_cost` / `lod_cost` / `instancing_cost` 同一口径。
const ACTORS: usize = 2000;
/// 方阵边长（2000 ≤ 45² = 2025）。
const GRID: usize = 45;
/// 方块间距。
const SPACING: f32 = 6.0;

const _: () = assert!((FRAMES * STAMPS_PER_FRAME * 8).is_multiple_of(256));

fn gpu() -> Option<(wgpu::Device, wgpu::Queue, wgpu::Adapter)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        flags: wgpu::InstanceFlags::empty(),
        ..Default::default()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;
    let required =
        wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    if !adapter.features().contains(required) {
        eprintln!("跳过 shadow_cost：适配器不支持时间戳查询");
        return None;
    }
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("shadow_cost device"),
            required_features: required,
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        },
        None,
    ))
    .ok()?;
    Some((device, queue, adapter))
}

/// 45×45 方块阵；`cast_shadow` 决定这盏太阳灯投不投影。
fn actor_scene(cast_shadow: bool) -> Scene {
    let mut scene = Scene::new("shadow-cost");
    scene.ambient = [0.03, 0.03, 0.05];
    scene.lights = vec![Light::directional(
        "sun",
        [0.5, -1.0, -0.4],
        [1.0, 0.98, 0.92],
        2.2,
    )];
    scene.lights[0].casts_shadow = cast_shadow;

    for i in 0..ACTORS {
        let mut e = Entity::new(format!("c{i}"));
        e.model = "builtin://cube".to_owned();
        let (gx, gz) = ((i % GRID) as f32, (i / GRID) as f32);
        e.transform.position = Vec3::new(
            (gx - (GRID as f32 - 1.0) * 0.5) * SPACING,
            0.5,
            -(gz + 1.0) * SPACING,
        );
        scene.add_entity(e);
    }
    scene
}

fn actor_cache(device: &wgpu::Device, queue: &wgpu::Queue) -> HashMap<String, GltfMesh> {
    let cube = GltfMesh::from_mesh_data(device, queue, &BuiltinMesh::Cube.build(), "bench cube");
    let mut cache = HashMap::with_capacity(1);
    cache.insert("builtin://cube".to_owned(), cube);
    cache
}

/// 相机站在方阵南侧（+Z），朝阵心看 —— 与 `lod_cost` / `culling_cost` 同机位。
fn actor_camera() -> (Vec3, Mat4) {
    let eye = Vec3::new(0.0, 6.0, 0.0);
    let view = Mat4::look_at_rh(eye, Vec3::new(0.0, 0.0, -150.0), Vec3::Y);
    let proj = Mat4::perspective_rh(
        45.0_f32.to_radians(),
        WIDTH as f32 / HEIGHT as f32,
        0.1,
        800.0,
    );
    (eye, proj * view)
}

struct BenchPath {
    pipeline: ScenePipeline,
    hdr: HdrTargets,
    _ldr: wgpu::Texture,
    ldr_view: wgpu::TextureView,
    postfx: PostFx,
    format: wgpu::TextureFormat,
}

impl BenchPath {
    fn new(device: &wgpu::Device, adapter: &wgpu::Adapter) -> Self {
        let sample_count = supported_sample_count(adapter, HDR_FORMAT);
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let ldr = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bench_shadow_ldr"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let ldr_view = ldr.create_view(&wgpu::TextureViewDescriptor::default());
        let pipeline = ScenePipeline::new(device, HDR_FORMAT, sample_count);
        let hdr = HdrTargets::new(device, "bench_shadow", WIDTH, HEIGHT, sample_count);
        let postfx = PostFx::new(device, format);
        Self {
            pipeline,
            hdr,
            _ldr: ldr,
            ldr_view,
            postfx,
            format,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        stamps: &wgpu::QuerySet,
        base: u32,
        scene: &Scene,
        matrices: &[Mat4],
        cache: &HashMap<String, GltfMesh>,
        anim: &AnimationDriver,
        view_proj: &Mat4,
        eye: &Vec3,
        shadow: xgame_core::ShadowConfig,
    ) -> FrameStats {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench_shadow_encoder"),
        });
        encoder.write_timestamp(stamps, base);
        self.pipeline.render(
            device,
            queue,
            &mut encoder,
            &self.hdr.color_view,
            self.hdr.msaa_view.as_ref(),
            &self.hdr.depth_view,
            &scene.entities,
            matrices,
            cache,
            anim,
            view_proj,
            [0.0, 0.0],
            eye,
            SceneLighting::new(scene.ambient, &scene.lights),
            RenderConfig::default().sky,
            1.0,
            shadow,
            &[],
        );
        encoder.write_timestamp(stamps, base + 1);
        self.postfx.render(
            device,
            queue,
            &mut encoder,
            &self.hdr.color_view,
            0,
            self.hdr.size,
            &self.ldr_view,
            RenderConfig {
                bloom: BloomConfig {
                    intensity: 0.0,
                    ..Default::default()
                },
                ..RenderConfig::default()
            },
        );
        encoder.write_timestamp(stamps, base + 2);
        queue.submit(std::iter::once(encoder.finish()));
        self.pipeline.frame_stats()
    }
}

#[derive(Default, Clone, Copy, Debug)]
struct Round {
    scene_us: f64,
    cpu_us: f64,
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn run_round(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    stamps: &wgpu::QuerySet,
    resolve: &wgpu::Buffer,
    readback: &wgpu::Buffer,
    mut frame: impl FnMut(u32) -> FrameStats,
) -> (Round, FrameStats) {
    let mut cpu = Vec::with_capacity(FRAMES);
    let mut stats = FrameStats::default();
    for i in 0..FRAMES {
        let start = Instant::now();
        stats = frame(i as u32 * STAMPS_PER_FRAME as u32);
        cpu.push(start.elapsed().as_secs_f64() * 1e6);
    }

    let byte_len = (FRAMES * STAMPS_PER_FRAME * 8) as u64;
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench_shadow_resolve"),
    });
    encoder.resolve_query_set(stamps, 0..(FRAMES * STAMPS_PER_FRAME) as u32, resolve, 0);
    encoder.copy_buffer_to_buffer(resolve, 0, readback, 0, byte_len);
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv()
        .expect("读回通道断开")
        .expect("映射时间戳缓冲失败");
    let ticks: Vec<u64> = {
        let view = slice.get_mapped_range();
        view.as_chunks::<8>()
            .0
            .iter()
            .map(|c| u64::from_le_bytes(*c))
            .collect()
    };
    readback.unmap();

    let period = queue.get_timestamp_period() as f64;
    let mut scene = Vec::with_capacity(FRAMES);
    for f in 0..FRAMES {
        let base = f * STAMPS_PER_FRAME;
        scene.push((ticks[base + 1] - ticks[base]) as f64 * period / 1000.0);
    }
    (
        Round {
            scene_us: median(&mut scene),
            cpu_us: median(&mut cpu),
        },
        stats,
    )
}

#[test]
#[ignore = "需要显卡与时间戳查询；`--ignored --nocapture` 手动跑"]
fn shadow_pass_cost_and_cascaded_culling() {
    let Some((device, queue, adapter)) = gpu() else {
        return;
    };
    let cache = actor_cache(&device, &queue);
    let (eye, view_proj) = actor_camera();
    let anim = AnimationDriver::default();

    let mut path = BenchPath::new(&device, &adapter);
    let _ = path.format;

    let stamps = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("bench_shadow_stamps"),
        ty: wgpu::QueryType::Timestamp,
        count: (FRAMES * STAMPS_PER_FRAME) as u32,
    });
    let resolve = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bench_shadow_resolve"),
        size: (FRAMES * STAMPS_PER_FRAME * 8) as u64,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bench_shadow_readback"),
        size: (FRAMES * STAMPS_PER_FRAME * 8) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // 两级阴影（自阴影贴图）都要重建分辨率以外的资源：同一个 pipeline 复用，
    // 只切灯的 `casts_shadow`。矩阵每框架一次（层级没动，这里直接复用）。
    let scene_off = actor_scene(false);
    let scene_on = actor_scene(true);
    let matrices: Vec<Mat4> = scene_on
        .entities
        .iter()
        .map(|e| e.transform.local_matrix())
        .collect();

    // 预热：第一帧要建阴影贴图 / 材质绑定组，别把它算进读数。
    let warm_start = Instant::now();
    while warm_start.elapsed() < WARMUP_TIME {
        path.frame(
            &device,
            &queue,
            &stamps,
            0,
            &scene_on,
            &matrices,
            &cache,
            &anim,
            &view_proj,
            &eye,
            xgame_core::ShadowConfig::default(),
        );
        device.poll(wgpu::Maintain::Poll);
    }

    // 四档交替：A = 阴影关，B = 阴影开 + 逐级剔除关，C = 阴影开 + 逐级剔除开，
    // D = C + 软阴影（PCSS，softness = 1.0）。D − C 就是 PCSS 的片元侧成本。
    const HARD: xgame_core::ShadowConfig = xgame_core::ShadowConfig {
        softness: 0.0,
        cascade_blend: 0.0,
    };
    const SOFT: xgame_core::ShadowConfig = xgame_core::ShadowConfig {
        softness: 1.0,
        cascade_blend: 0.0,
    };
    let mut a_scene = Vec::new();
    let mut b_scene = Vec::new();
    let mut c_scene = Vec::new();
    let mut d_scene = Vec::new();
    let mut a_cpu = Vec::new();
    let mut b_cpu = Vec::new();
    let mut c_cpu = Vec::new();
    let mut d_cpu = Vec::new();
    let mut stats_off = FrameStats::default();
    let mut stats_plain = FrameStats::default();
    let mut stats_culled = FrameStats::default();
    let mut stats_soft = FrameStats::default();

    for _ in 0..ROUNDS {
        path.pipeline.set_cascaded_shadow_culling(false);
        let (r, s) = run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
            path.frame(
                &device,
                &queue,
                &stamps,
                base,
                &scene_off,
                &matrices,
                &cache,
                &anim,
                &view_proj,
                &eye,
                HARD,
            )
        });
        a_scene.push(r.scene_us);
        a_cpu.push(r.cpu_us);
        stats_off = s;

        let (r, s) = run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
            path.frame(
                &device,
                &queue,
                &stamps,
                base,
                &scene_on,
                &matrices,
                &cache,
                &anim,
                &view_proj,
                &eye,
                HARD,
            )
        });
        b_scene.push(r.scene_us);
        b_cpu.push(r.cpu_us);
        stats_plain = s;

        path.pipeline.set_cascaded_shadow_culling(true);
        let (r, s) = run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
            path.frame(
                &device,
                &queue,
                &stamps,
                base,
                &scene_on,
                &matrices,
                &cache,
                &anim,
                &view_proj,
                &eye,
                HARD,
            )
        });
        c_scene.push(r.scene_us);
        c_cpu.push(r.cpu_us);
        stats_culled = s;

        // 软阴影只改片元侧采样，不碰阴影 pass —— 与 C 同一条阴影收集路径。
        let (r, s) = run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
            path.frame(
                &device,
                &queue,
                &stamps,
                base,
                &scene_on,
                &matrices,
                &cache,
                &anim,
                &view_proj,
                &eye,
                SOFT,
            )
        });
        d_scene.push(r.scene_us);
        d_cpu.push(r.cpu_us);
        stats_soft = s;
    }

    let a = median(&mut a_scene);
    let b = median(&mut b_scene);
    let c = median(&mut c_scene);
    let d = median(&mut d_scene);
    let a_c = median(&mut a_cpu);
    let b_c = median(&mut b_cpu);
    let c_c = median(&mut c_cpu);
    let d_c = median(&mut d_cpu);

    println!("\n=== shadow_cost （{WIDTH}x{HEIGHT}，{ACTORS} 方块，{ROUNDS} 轮中位）===");
    println!("时段      场景段 GPU µs   CPU µs   阴影实例提交");
    println!(
        "阴影关        {a:9.2} {a_c:9.2} {:12}",
        stats_off.shadow_instances_submitted
    );
    println!(
        "阴影开·免剔   {b:9.2} {b_c:9.2} {:12}",
        stats_plain.shadow_instances_submitted
    );
    println!(
        "阴影开·剔除   {c:9.2} {c_c:9.2} {:12}",
        stats_culled.shadow_instances_submitted
    );
    println!(
        "阴影开·软阴影 {d:9.2} {d_c:9.2} {:12}",
        stats_soft.shadow_instances_submitted
    );
    println!(
        "\n阴影本身成本 : {:+.2} µs (+{:.1}%)",
        b - a,
        (b - a) / a * 100.0
    );
    println!(
        "PCSS 软阴影  : {:+.2} µs ({:+.1}% 相对硬阴影)",
        d - c,
        (d - c) / c * 100.0
    );
    println!(
        "逐级剔除收益 : {:+.2} µs ({:+.1}% 相对免剔)",
        c - b,
        (c - b) / b * 100.0
    );
    if stats_plain.shadow_instances_submitted > 0 {
        println!(
            "阴影实例提交 : {} → {} （{:+.1}%）",
            stats_plain.shadow_instances_submitted,
            stats_culled.shadow_instances_submitted,
            (stats_culled.shadow_instances_submitted as f64
                / stats_plain.shadow_instances_submitted as f64
                - 1.0)
                * 100.0
        );
    }
    println!(
        "批次读数     : 免剔 {}/{} 剔除 {}/{}（opaque/shadow_only）",
        stats_plain.opaque_batches,
        stats_plain.shadow_only_batches,
        stats_culled.opaque_batches,
        stats_culled.shadow_only_batches
    );
}

// ---------------------------------------------------------------------------
// 点光立方体阴影：六面各画一遍全部投影物的成本
// ---------------------------------------------------------------------------

/// 同一片方块阵，灯换成**吊在阵心上方的点光**；`cast_shadow` 决定它投不投影。
///
/// 点光阴影的成本结构完全不同：方向光是"4 级各画一遍"，点光是"6 面各画一遍
/// **全部**投影物"（v1 不做逐面剔除）。用同一片阵子量，两套数字才能直接对表。
fn point_scene(cast_shadow: bool) -> Scene {
    let mut scene = actor_scene(false);
    scene.lights = vec![Light::point(
        "lamp",
        [0.0, 40.0, -130.0],
        [1.0, 0.98, 0.92],
        60.0,
        400.0,
    )];
    scene.lights[0].casts_shadow = cast_shadow;
    scene
}

#[test]
#[ignore = "需要显卡与时间戳查询；`--ignored --nocapture` 手动跑"]
fn point_light_shadow_cost() {
    let Some((device, queue, adapter)) = gpu() else {
        return;
    };
    let cache = actor_cache(&device, &queue);
    let (eye, view_proj) = actor_camera();
    let anim = AnimationDriver::default();
    let mut path = BenchPath::new(&device, &adapter);
    let _ = path.format;

    let stamps = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("bench_point_shadow_stamps"),
        ty: wgpu::QueryType::Timestamp,
        count: (FRAMES * STAMPS_PER_FRAME) as u32,
    });
    let resolve = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bench_point_shadow_resolve"),
        size: (FRAMES * STAMPS_PER_FRAME * 8) as u64,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bench_point_shadow_readback"),
        size: (FRAMES * STAMPS_PER_FRAME * 8) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let scene_off = point_scene(false);
    let scene_on = point_scene(true);
    let matrices: Vec<Mat4> = scene_on
        .entities
        .iter()
        .map(|e| e.transform.local_matrix())
        .collect();

    let warm_start = Instant::now();
    while warm_start.elapsed() < WARMUP_TIME {
        path.frame(
            &device,
            &queue,
            &stamps,
            0,
            &scene_on,
            &matrices,
            &cache,
            &anim,
            &view_proj,
            &eye,
            xgame_core::ShadowConfig::default(),
        );
        device.poll(wgpu::Maintain::Poll);
    }

    let mut off_scene_us = Vec::new();
    let mut plain_scene_us = Vec::new();
    let mut on_scene_us = Vec::new();
    let mut off_cpu_us = Vec::new();
    let mut plain_cpu_us = Vec::new();
    let mut on_cpu_us = Vec::new();
    let mut stats_off = FrameStats::default();
    let mut stats_plain = FrameStats::default();
    let mut stats_on = FrameStats::default();

    for _ in 0..ROUNDS {
        let (r, s) = run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
            path.frame(
                &device,
                &queue,
                &stamps,
                base,
                &scene_off,
                &matrices,
                &cache,
                &anim,
                &view_proj,
                &eye,
                xgame_core::ShadowConfig::default(),
            )
        });
        off_scene_us.push(r.scene_us);
        off_cpu_us.push(r.cpu_us);
        stats_off = s;

        // 逐面剔除**关**：六面各画一遍全部投影物（引入前的行为）。
        path.pipeline.set_point_shadow_culling(false);
        let (r, s) = run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
            path.frame(
                &device,
                &queue,
                &stamps,
                base,
                &scene_on,
                &matrices,
                &cache,
                &anim,
                &view_proj,
                &eye,
                xgame_core::ShadowConfig::default(),
            )
        });
        plain_scene_us.push(r.scene_us);
        plain_cpu_us.push(r.cpu_us);
        stats_plain = s;

        path.pipeline.set_point_shadow_culling(true);
        let (r, s) = run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
            path.frame(
                &device,
                &queue,
                &stamps,
                base,
                &scene_on,
                &matrices,
                &cache,
                &anim,
                &view_proj,
                &eye,
                xgame_core::ShadowConfig::default(),
            )
        });
        on_scene_us.push(r.scene_us);
        on_cpu_us.push(r.cpu_us);
        stats_on = s;
    }

    let a = median(&mut off_scene_us);
    let p = median(&mut plain_scene_us);
    let b = median(&mut on_scene_us);
    let a_c = median(&mut off_cpu_us);
    let p_c = median(&mut plain_cpu_us);
    let b_c = median(&mut on_cpu_us);

    println!("\n== 点光立方体阴影成本（{WIDTH}x{HEIGHT} / {ACTORS} 方块 / 6 面）==");
    println!("档位              GPU µs    CPU µs   六面实例  draw");
    println!(
        "点光阴影关      {a:9.2} {a_c:9.2} {:10} {:5}",
        stats_off.point_shadow_instances, stats_off.point_shadow_draw_calls
    );
    println!(
        "阴影开·免剔     {p:9.2} {p_c:9.2} {:10} {:5}",
        stats_plain.point_shadow_instances, stats_plain.point_shadow_draw_calls
    );
    println!(
        "阴影开·逐面剔   {b:9.2} {b_c:9.2} {:10} {:5}",
        stats_on.point_shadow_instances, stats_on.point_shadow_draw_calls
    );
    println!("\n六面成本（免剔）  : {:+.2} µs (+{:.1}%)", p - a, (p - a) / a * 100.0);
    println!("六面成本（逐面剔）: {:+.2} µs (+{:.1}%)", b - a, (b - a) / a * 100.0);
    println!(
        "逐面剔除收益      : {:+.2} µs ({:+.1}% 相对免剔)",
        b - p,
        (b - p) / p * 100.0
    );
    if stats_plain.point_shadow_instances > 0 {
        println!(
            "六面实例提交      : {} → {} （{:+.1}%）",
            stats_plain.point_shadow_instances,
            stats_on.point_shadow_instances,
            (stats_on.point_shadow_instances as f64
                / stats_plain.point_shadow_instances as f64
                - 1.0)
                * 100.0
        );
    }
    println!(
        "六面 draw 数      : 免剔 {} → 剔 {}",
        stats_plain.point_shadow_draw_calls, stats_on.point_shadow_draw_calls
    );
}
