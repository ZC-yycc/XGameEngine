//! 微基准：质量档位那几颗螺丝各值多少钱。
//!
//! 默认 `#[ignore]`（耗时波动大、需要显卡，不该进 CI 门禁）。手动跑：
//!
//! ```sh
//! cargo test -p xgame-render --release --test quality_cost -- --ignored --nocapture
//! ```
//!
//! # 比什么
//!
//! 同一个场景（2000 个方块 + 一盏投影方向光）、同一台相机，只动质量档位
//! 消费的两个参数（见 `xgame_core::quality`）：
//!
//! | 档位 | MSAA | 阴影贴图 |
//! |---|---|---|
//! | 低 | 1× | 1024 |
//! | 中 | 4× | 2048 |
//! | 高 | 4× | 4096 |
//!
//! （采样数只有 1× / 4× 两档：2× / 8× 不是 wgpu 保证档，实测 RTX 2060 上
//! `Rgba16Float` 开 2× 直接管线创建失败。）
//!
//! 读数口径与 `lod_cost` / `culling_cost` 同一套：时间戳取**场景段**
//! （含阴影 pass 与主 pass，后处理只做色调映射合成），两段分开报。
//! 每档 `ROUNDS` 轮取中位数，档位之间**交替**跑，抵消 GPU 升温偏差。
//!
//! ⚠ 绝对值随机器状态浮动，**跨版本比较必须同机背靠背**。

use std::collections::HashMap;

use xgame_core::{BloomConfig, Entity, Light, RenderConfig, Scene};
use xgame_math::glam::{Mat4, Vec3};
use xgame_render::animation::AnimationDriver;
use xgame_render::gltf_loader::GltfMesh;
use xgame_render::mesh::BuiltinMesh;
use xgame_render::postfx::{HdrTargets, PostFx, HDR_FORMAT};
use xgame_render::scene_pipeline::{FrameStats, SceneLighting, ScenePipeline};

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
/// 三档质量（MSAA / 阴影贴图边长），与 `QualityPreset::base()` 同款取值。
const QUALITY_CASES: [(u32, u32); 3] = [(1, 1024), (4, 2048), (4, 4096)];

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

/// 45×45 方阵 + 一盏**投影**方向光（阴影 pass 的成本是本基准的一半读数）。
fn actor_scene() -> Scene {
    let mut scene = Scene::new("quality-cost");
    scene.ambient = [0.03, 0.03, 0.05];
    scene.lights = vec![Light::directional(
        "sun",
        [0.5, -1.0, -0.4],
        [1.0, 0.98, 0.92],
        2.2,
    )];
    scene.lights[0].casts_shadow = true;
    // 灯不写 shadow_resolution：让质量档位的默认值生效（正是本基准要量的）。
    scene.lights[0].shadow_resolution = 0;

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
    let mut cache = HashMap::with_capacity(1);
    cache.insert(
        "builtin://cube".to_owned(),
        GltfMesh::from_mesh_data(device, queue, &BuiltinMesh::Cube.build(), "bench cube"),
    );
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


struct BenchPath {
    pipeline: ScenePipeline,
    hdr: HdrTargets,
    _ldr: wgpu::Texture,
    ldr_view: wgpu::TextureView,
    postfx: PostFx,
}

impl BenchPath {
    /// 每档建一条新的管线：**MSAA 是管线创建参数**，改档位只能重建（这正是
    /// 质量档位被定义成"启动期"决策的原因，见 `xgame_core::quality`）。
    fn new(device: &wgpu::Device, sample_count: u32, shadow_resolution: u32) -> Self {
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let ldr = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bench_quality_ldr"),
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
        let mut pipeline = ScenePipeline::new(device, HDR_FORMAT, sample_count);
        pipeline.set_default_shadow_resolution(shadow_resolution);
        let hdr = HdrTargets::new(device, "bench_quality", WIDTH, HEIGHT, sample_count);
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
    ) -> FrameStats {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench_quality_encoder"),
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
    let mut stats = FrameStats::default();
    for i in 0..FRAMES {
        stats = frame(i as u32 * STAMPS_PER_FRAME as u32);
    }

    let byte_len = (FRAMES * STAMPS_PER_FRAME * 8) as u64;
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench_quality_resolve"),
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
        },
        stats,
    )
}

#[test]
#[ignore = "GPU 时间基准：耗时波动大、需要显卡，不进 CI 门禁"]
fn quality_cost() {
    let Some((device, queue, adapter)) = gpu() else {
        return;
    };
    eprintln!("适配器：{}", adapter.get_info().name);

    let scene = actor_scene();
    let cache = actor_cache(&device, &queue);
    let matrices = scene.world_matrices();
    let (eye, view_proj) = actor_camera();
    let anim = AnimationDriver::new();

    // 每档建一条新管线（MSAA 是创建参数），三档交替跑一轮再取中位。
    let mut paths: Vec<(u32, u32, BenchPath)> = QUALITY_CASES
        .iter()
        .map(|&(msaa, shadow)| {
            let path = BenchPath::new(&device, msaa, shadow);
            (msaa, shadow, path)
        })
        .collect();

    // 每档先预热（管线刚建，第一帧不作数）。
    for (_, _, path) in paths.iter_mut() {
        let warmup = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("bench_quality_warmup"),
            ty: wgpu::QueryType::Timestamp,
            count: (FRAMES * STAMPS_PER_FRAME) as u32,
        });
        let warmup_start = std::time::Instant::now();
        while warmup_start.elapsed() < WARMUP_TIME {
            path.frame(
                &device, &queue, &warmup, 0, &scene, &matrices, &cache, &anim, &view_proj, &eye,
            );
        }
        device.poll(wgpu::Maintain::Wait);
    }

    // 三档**交替**跑 ROUNDS 轮，逐档收齐再取中位（抵消 GPU 升温偏差）。
    let mut rounds: Vec<Vec<Round>> = vec![Vec::with_capacity(ROUNDS); QUALITY_CASES.len()];
    for round in 0..ROUNDS {
        for (index, (msaa, shadow, path)) in paths.iter_mut().enumerate() {
            let stamps = device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("bench_quality_stamps"),
                ty: wgpu::QueryType::Timestamp,
                count: (FRAMES * STAMPS_PER_FRAME) as u32,
            });
            let byte_len = (FRAMES * STAMPS_PER_FRAME * 8) as u64;
            let resolve = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bench_quality_stamps_resolve"),
                size: byte_len,
                usage: wgpu::BufferUsages::QUERY_RESOLVE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bench_quality_stamps_readback"),
                size: byte_len,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let (r, _) = run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
                path.frame(
                    &device, &queue, &stamps, base, &scene, &matrices, &cache, &anim, &view_proj, &eye,
                )
            });
            rounds[index].push(r);
            eprintln!(
                "  [{msaa}x / shadow {shadow}] round {round}: scene {:.1} µs / post {:.1} µs",
                r.scene_us, r.post_us
            );
        }
    }

    eprintln!();
    let mut medians = Vec::with_capacity(QUALITY_CASES.len());
    for (index, (msaa, shadow, _)) in paths.iter().enumerate() {
        let mut scene_us: Vec<f64> = rounds[index].iter().map(|r| r.scene_us).collect();
        let mut post_us: Vec<f64> = rounds[index].iter().map(|r| r.post_us).collect();
        let r = Round {
            scene_us: median(&mut scene_us),
            post_us: median(&mut post_us),
        };
        medians.push(r);
        eprintln!(
            "MSAA {msaa}x / 阴影 {shadow}：scene {:.1} µs / post {:.1} µs",
            r.scene_us, r.post_us
        );
    }

    // 断言只钉"读数必须为正"：档位之间的大小关系随 GPU / 场景变化，
    // 基准不该假装它是一条跨机器的定律。
    for (index, (msaa, shadow, _)) in paths.iter().enumerate() {
        assert!(
            medians[index].scene_us > 0.0,
            "MSAA {msaa}x / 阴影 {shadow} 的场景段读数必须为正"
        );
    }
}
