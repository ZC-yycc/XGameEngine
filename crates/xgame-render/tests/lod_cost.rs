//! 微基准：LOD 的每帧成本与收益。
//!
//! 默认 `#[ignore]`（耗时波动大、需要显卡，不该进 CI 门禁）。手动跑：
//!
//! ```sh
//! cargo test -p xgame-render --release --test lod_cost -- --ignored --nocapture
//! ```
//!
//! # 比什么
//!
//! 同一个场景、同一台相机、同一套管线，只切 [`RenderConfig::lod_bias`]：
//!
//! | 档位 | 行为 | 每个实体的顶点数 |
//! |---|---|---|
//! | `lod_bias = 0`（= 关掉 LOD） | 永远第 0 级（球，24×32 ≈ 793 顶点） | ≈ 793 |
//! | `lod_bias = 1`（作者阈值） | 60 米内第 0 级、之外第 1 级（立方体，24 顶点） | 远处 ≈ 24 |
//!
//! 场景是 45×45 的方阵、纵深 264 个单位：相机站在阵南侧，约三分之二的
//! 实体在 60 米阈值之外。这不是"全部降级"的极端摆拍 —— 近处仍然画高模、
//! 远处画低模，正是 LOD 的常态；收益（省掉的顶点 / 光栅工作）与成本
//! （每实体一次开方 + 级判定）都在真实量级上。
//!
//! # 为什么量 GPU 时间而不是帧时间
//!
//! LOD 省的是**GPU 顶点与光栅**工作；CPU 侧只多了一次开方与两次比较，
//! 在帧时间里被调度噪声淹没。与 `culling_cost` 同一套读数口径：时间戳取
//! **场景段**（后处理只做色调映射合成），CPU 段另记。两档交替跑 `ROUNDS`
//! 轮取中位数。
//!
//! ⚠ 绝对值随机器状态浮动，**跨版本比较必须同机背靠背**。

use std::collections::HashMap;
use std::time::Instant;

use xgame_core::{BloomConfig, Entity, Light, LodGroup, LodLevel, RenderConfig, Scene};
use xgame_math::glam::{Mat4, Vec3};
use xgame_render::animation::AnimationDriver;
use xgame_render::gltf_loader::GltfMesh;
use xgame_render::mesh::{BuiltinMesh, MeshData};
use xgame_render::postfx::{HdrTargets, PostFx, HDR_FORMAT};
use xgame_render::scene_pipeline::{
    supported_sample_count, FrameStats, SceneLighting, ScenePipeline,
};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
/// 每帧 3 个时间戳：帧头 / 场景 pass 末 / 帧末。
const STAMPS_PER_FRAME: usize = 3;
const FRAMES: usize = 64;
const ROUNDS: usize = 8;
const WARMUP_TIME: std::time::Duration = std::time::Duration::from_millis(1500);

/// 演员数量：与 `culling_cost` / `instancing_cost` 同一口径。
const ACTORS: usize = 2000;
/// 方阵边长（2000 ≤ 45² = 2025）。
const GRID: usize = 45;
/// 方块间距。
const SPACING: f32 = 6.0;
/// 第 0 级（高模）生效到多远；之外换第 1 级（低模）。
const NEAR_LIMIT: f32 = 60.0;

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
        eprintln!("跳过 lod_cost：适配器不支持时间戳查询");
        return None;
    }
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("lod_cost device"),
            required_features: required,
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        },
        None,
    ))
    .ok()?;
    Some((device, queue, adapter))
}

/// 45×45 方阵、纵深 264 个单位，每个实体带两级 LOD。
fn actor_scene() -> Scene {
    let mut scene = Scene::new("lod-cost");
    scene.ambient = [0.03, 0.03, 0.05];
    scene.lights = vec![Light::directional(
        "sun",
        [0.5, -1.0, -0.4],
        [1.0, 0.98, 0.92],
        2.2,
    )];
    // 阴影关掉：阴影 pass 的成本与 LOD 无关，混进来会糊掉读数。
    scene.lights[0].casts_shadow = false;

    for i in 0..ACTORS {
        let mut e = Entity::new(format!("c{i}"));
        e.lod = LodGroup {
            levels: vec![
                LodLevel::new("bench://hi", NEAR_LIMIT),
                LodLevel::new("bench://lo", 0.0),
            ],
        };
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

/// 高模 = 球（24×32 段 ≈ 793 顶点），低模 = 立方体（24 顶点）。
///
/// 顶点数差 30 倍以上，LOD 的收益才不会被材质 / 光栅的固定成本盖掉。
fn actor_cache(device: &wgpu::Device, queue: &wgpu::Queue) -> HashMap<String, GltfMesh> {
    let hi = GltfMesh::from_mesh_data(device, queue, &BuiltinMesh::Sphere.build(), "bench hi");
    let lo = GltfMesh::from_mesh_data(device, queue, &BuiltinMesh::Cube.build(), "bench lo");
    let mut cache = HashMap::with_capacity(2);
    cache.insert("bench://hi".to_owned(), hi);
    cache.insert("bench://lo".to_owned(), lo);
    cache
}

/// 相机站在方阵南侧（+Z），朝阵心看：近处几排是高模，其余全部降级。
fn actor_camera() -> (Vec3, Mat4) {
    let eye = Vec3::new(0.0, 6.0, 0.0);
    // 目标必须**在阵里**（远处），写 (0,0,-1) 会让相机几乎垂直俯视，
    // 整个方阵都在视锥外 —— 量出来的就是"剔除 100%"的假读数。
    let view = Mat4::look_at_rh(eye, Vec3::new(0.0, 0.0, -150.0), Vec3::Y);
    let proj = Mat4::perspective_rh(
        45.0_f32.to_radians(),
        WIDTH as f32 / HEIGHT as f32,
        0.1,
        800.0,
    );
    (eye, proj * view)
}

fn vertex_count(data: &MeshData) -> usize {
    data.vertices.len()
}

struct BenchPath {
    pipeline: ScenePipeline,
    hdr: HdrTargets,
    _ldr: wgpu::Texture,
    ldr_view: wgpu::TextureView,
    postfx: PostFx,
}

impl BenchPath {
    fn new(device: &wgpu::Device, adapter: &wgpu::Adapter) -> Self {
        let sample_count = supported_sample_count(adapter, HDR_FORMAT);
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let ldr = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bench_lod_ldr"),
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
        let hdr = HdrTargets::new(device, "bench_lod", WIDTH, HEIGHT, sample_count);
        let postfx = PostFx::new(device, format);
        Self {
            pipeline,
            hdr,
            _ldr: ldr,
            ldr_view,
            postfx,
        }
    }

    /// 编码一帧并提交；返回本帧的绘制统计。
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
        lod_bias: f32,
    ) -> FrameStats {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench_lod_encoder"),
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
            lod_bias,
            xgame_core::ShadowConfig::default(),
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
    post_us: f64,
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
        label: Some("bench_lod_resolve"),
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
    let mut post = Vec::with_capacity(FRAMES);
    for f in 0..FRAMES {
        let base = f * STAMPS_PER_FRAME;
        scene.push((ticks[base + 1] - ticks[base]) as f64 * period / 1000.0);
        post.push((ticks[base + 2] - ticks[base + 1]) as f64 * period / 1000.0);
    }
    (
        Round {
            scene_us: median(&mut scene),
            post_us: median(&mut post),
            cpu_us: median(&mut cpu),
        },
        stats,
    )
}

/// 一档（固定 `lod_bias`）的完整测量：返回 `ROUNDS` 轮的中位数与末帧统计。
#[allow(clippy::too_many_arguments)]
fn measure(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    path: &mut BenchPath,
    scene: &Scene,
    matrices: &[Mat4],
    cache: &HashMap<String, GltfMesh>,
    anim: &AnimationDriver,
    eye: &Vec3,
    view_proj: &Mat4,
    lod_bias: f32,
) -> (Round, FrameStats) {
    let warmup = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("bench_lod_warmup"),
        ty: wgpu::QueryType::Timestamp,
        count: (FRAMES * STAMPS_PER_FRAME) as u32,
    });
    let warmup_start = Instant::now();
    while warmup_start.elapsed() < WARMUP_TIME {
        path.frame(
            device, queue, &warmup, 0, scene, matrices, cache, anim, view_proj, eye, lod_bias,
        );
    }
    device.poll(wgpu::Maintain::Wait);

    let byte_len = (FRAMES * STAMPS_PER_FRAME * 8) as u64;
    let mut rounds = Vec::with_capacity(ROUNDS);
    let mut stats = FrameStats::default();
    for round in 0..ROUNDS {
        let stamps = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("bench_lod_stamps"),
            ty: wgpu::QueryType::Timestamp,
            count: (FRAMES * STAMPS_PER_FRAME) as u32,
        });
        let (resolve, readback) = (
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bench_lod_stamps_resolve"),
                size: byte_len,
                usage: wgpu::BufferUsages::QUERY_RESOLVE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bench_lod_stamps_readback"),
                size: byte_len,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        );
        let (r, s) = run_round(device, queue, &stamps, &resolve, &readback, |base| {
            path.frame(
                device, queue, &stamps, base, scene, matrices, cache, anim, view_proj, eye, lod_bias,
            )
        });
        rounds.push(r);
        stats = s;
        eprintln!("  round {round}: scene {:.1} µs / post {:.1} µs / cpu {:.1} µs", r.scene_us, r.post_us, r.cpu_us);
    }

    let mut scene = rounds.iter().map(|r| r.scene_us).collect::<Vec<_>>();
    let mut post = rounds.iter().map(|r| r.post_us).collect::<Vec<_>>();
    let mut cpu = rounds.iter().map(|r| r.cpu_us).collect::<Vec<_>>();
    (
        Round {
            scene_us: median(&mut scene),
            post_us: median(&mut post),
            cpu_us: median(&mut cpu),
        },
        stats,
    )
}

#[test]
#[ignore = "GPU 时间基准：耗时波动大、需要显卡，不进 CI 门禁"]
fn lod_cost() {
    let Some((device, queue, adapter)) = gpu() else {
        return;
    };
    eprintln!("适配器：{}", adapter.get_info().name);

    let scene = actor_scene();
    let cache = actor_cache(&device, &queue);
    let hi_verts = vertex_count(&BuiltinMesh::Sphere.build());
    let lo_verts = vertex_count(&BuiltinMesh::Cube.build());
    eprintln!(
        "网格：高模 {hi_verts} 顶点 / 低模 {lo_verts} 顶点；{ACTORS} 个实体，切换阈值 {NEAR_LIMIT} 米"
    );

    let matrices = scene.world_matrices();
    let (eye, view_proj) = actor_camera();
    let anim = AnimationDriver::new();
    let mut path = BenchPath::new(&device, &adapter);

    // 交替顺序：先无 LOD 后有 LOD（每档内部自己也预热），减少系统偏差。
    eprintln!("[无 LOD：lod_bias = 0，永远高模]");
    let (no_lod, no_stats) = measure(
        &device,
        &queue,
        &mut path,
        &scene,
        &matrices,
        &cache,
        &anim,
        &eye,
        &view_proj,
        0.0,
    );
    eprintln!("[有 LOD：lod_bias = 1，作者阈值]");
    let (lod, lod_stats) = measure(
        &device,
        &queue,
        &mut path,
        &scene,
        &matrices,
        &cache,
        &anim,
        &eye,
        &view_proj,
        1.0,
    );

    let no_verts = ACTORS * hi_verts;
    let lod_verts = lod_stats.low_lod_instances * lo_verts
        + (lod_stats.opaque_instances - lod_stats.low_lod_instances) * hi_verts;
    eprintln!(
        "\n无 LOD：scene {:.1} µs / post {:.1} µs / cpu {:.1} µs；实例 {}（低模 {}，距离剔除 {}）",
        no_lod.scene_us,
        no_lod.post_us,
        no_lod.cpu_us,
        no_stats.opaque_instances,
        no_stats.low_lod_instances,
        no_stats.distance_culled_entities
    );
    eprintln!(
        "有 LOD：scene {:.1} µs / post {:.1} µs / cpu {:.1} µs；实例 {}（低模 {}，距离剔除 {}）",
        lod.scene_us,
        lod.post_us,
        lod.cpu_us,
        lod_stats.opaque_instances,
        lod_stats.low_lod_instances,
        lod_stats.distance_culled_entities
    );
    eprintln!(
        "顶点量：{} → {}（-{:.1}%）",
        no_verts,
        lod_verts,
        (1.0 - lod_verts as f64 / no_verts as f64) * 100.0
    );
    eprintln!(
        "场景段：{:.1} → {:.1} µs（{:+.1}%）",
        no_lod.scene_us,
        lod.scene_us,
        (lod.scene_us / no_lod.scene_us - 1.0) * 100.0
    );

    assert!(
        lod_stats.low_lod_instances > 0,
        "这个摆位下必须有一部分实体已经降级，否则量的不是 LOD"
    );
    assert!(
        lod_stats.low_lod_instances < lod_stats.opaque_instances,
        "也必须有一部分还是高模，否则只剩'低模 vs 高模'的极端对照"
    );
}
