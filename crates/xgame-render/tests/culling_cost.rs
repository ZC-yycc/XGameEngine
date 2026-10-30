//! 微基准：视锥剔除的每帧成本与收益。
//!
//! 默认 `#[ignore]`（耗时波动大、需要显卡，不该进 CI 门禁）。手动跑：
//!
//! ```sh
//! cargo test -p xgame-render --release --test culling_cost -- --ignored --nocapture
//! ```
//!
//! # 比什么
//!
//! 同一个场景、同一台相机、同一套管线，只切 [`ScenePipeline::set_frustum_culling`]：
//!
//! | 档位 | 收集阶段 | 主 pass 实例 |
//! |---|---|---|
//! | 关 | 全部实体都算世界矩阵、写实例数据、预热材质组 | 2000 |
//! | 开 | 视锥外的实体在写实例数据之前就跳过 | 只剩视锥内 |
//!
//! 相机刻意放在 45×45 方块阵的南侧、用 30° 窄视场：约四成方块在左右两侧
//! 与相机背后。这不是"全部剔除"的极端摆拍 —— 画面里仍有近九百个方块，
//! 剔除的收益（省掉实例写 / draw call / GPU 顶点处理）与成本（每实体一次
//! 包围盒变换 + 6 次平面测试）都在真实量级上。
//!
//! # 为什么要跑两种批次结构
//!
//! 剔除省下的东西**取决于合批**：
//!
//! - **共享网格**（2000 个实体全用 `builtin://cube`，压成 1 批）：省的是
//!   逐实例数据写入与 GPU 顶点处理，量小但稳定。
//! - **各自网格**（2000 个实体各持一份网格，2000 批）：省的是整条绘制项
//!   收集与逐 draw 的 `set_bind_group` / `set_vertex_buffer` / `draw_indexed`，
//!   是 CPU 侧的大头。
//!
//! 只报共享网格的数字会严重低估剔除在"素材没做合批"项目里的价值，只报
//! 各自网格又会把合批的收益算到剔除头上。两张表都给，读者自己对照。
//!
//! # 阴影为什么关掉
//!
//! 相机外的**投影物**必须保留（见 `offscreen_casters_still_cast_shadows`），
//! 它们会进阴影专用批次，主 pass 省下的实例数远小于剔除读数。开阴影测的是
//! "剔除 + 阴影补投"的组合，而不是剔除本身；两者混在一张表里会让"剔除省了
//! 多少"永远说不清。阴影的开销另有 `scene_pipeline` 的 GPU 测试守正确性。
//!
//! # 读数口径
//!
//! 与 `instancing_cost` / `postfx_cost` 同一套：GPU 时间戳取**场景段**
//! （后处理关 bloom，只做色调映射合成），CPU 段记"编码 + 提交"。两档交替
//! 跑 `ROUNDS` 轮取中位数。
//!
//! ⚠ 绝对值随机器状态浮动（与 `update_throughput` 同款警告），**跨版本
//! 比较必须同机背靠背**。

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
const ROUNDS: usize = 8;
const WARMUP_TIME: std::time::Duration = std::time::Duration::from_millis(1500);

/// 演员数量：与 `instancing_cost` / 差距分析同一口径。
const ACTORS: usize = 2000;
/// 方阵边长（2000 ≤ 45² = 2025）。
const GRID: usize = 45;
/// 方块间距（单位立方体，留出缝隙便于数数）。
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
        eprintln!("跳过 culling_cost：适配器不支持时间戳查询");
        return None;
    }
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("culling_cost device"),
            required_features: required,
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        },
        None,
    ))
    .ok()?;
    Some((device, queue, adapter))
}

/// 演员阵：`ACTORS` 个方块铺成方阵，无地面（剔除读数不被一块巨大地面污染
/// —— 地面的包围盒横跨整个场景，永远"相交"）。
///
/// `unique_meshes = false` 时全部实体共用 `builtin://cube`（合批成 1 批）；
/// `true` 时每个实体各持一份网格（2000 批）。几何与材质参数完全一致。
fn actor_scene(unique_meshes: bool) -> Scene {
    let mut scene = Scene::new(if unique_meshes {
        "culling-cost-unique"
    } else {
        "culling-cost-shared"
    });
    scene.ambient = [0.03, 0.03, 0.05];
    scene.lights = vec![Light::directional(
        "sun",
        [0.5, -1.0, -0.4],
        [1.0, 0.98, 0.92],
        2.2,
    )];
    // 阴影关掉：相机外的投影物按设计必须保留，混进来会糊掉剔除读数。
    scene.lights[0].casts_shadow = false;

    for i in 0..ACTORS {
        let mut e = Entity::new(format!("c{i}"));
        e.model = if unique_meshes {
            format!("bench://mesh{i}")
        } else {
            "builtin://cube".to_owned()
        };
        let (gx, gz) = ((i % GRID) as f32, (i / GRID) as f32);
        e.transform.position = Vec3::new(
            (gx - (GRID as f32 - 1.0) * 0.5) * SPACING,
            0.5,
            (gz - (GRID as f32 - 1.0) * 0.5) * SPACING,
        );
        scene.add_entity(e);
    }
    scene
}

fn actor_cache(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    unique: bool,
) -> HashMap<String, GltfMesh> {
    let data = BuiltinMesh::Cube.build();
    let mut cache = HashMap::with_capacity(if unique { ACTORS + 1 } else { 1 });
    if unique {
        for i in 0..ACTORS {
            let mesh = GltfMesh::from_mesh_data(device, queue, &data, "bench cube");
            cache.insert(format!("bench://mesh{i}"), mesh);
        }
    } else {
        let mesh = GltfMesh::from_mesh_data(device, queue, &data, BuiltinMesh::Cube.label());
        cache.insert(BuiltinMesh::Cube.uri(), mesh);
    }
    cache
}

/// 窄视场相机：站在方阵南侧（+Z），朝原点看，30° 视场。
///
/// 45 格 × 6 单位 = 264 单位见方的方阵，从 120 单位外只框进中间一条：
/// 左右两侧在水平视锥外，最后几排在相机背后。可见比例约四成。
fn actor_camera() -> (Vec3, Mat4) {
    let eye = Vec3::new(0.0, 6.0, 120.0);
    let view = Mat4::look_at_rh(eye, Vec3::new(0.0, 0.0, 0.0), Vec3::Y);
    let proj = Mat4::perspective_rh(
        30.0_f32.to_radians(),
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
    fn new(device: &wgpu::Device, adapter: &wgpu::Adapter) -> Self {
        let sample_count = supported_sample_count(adapter, HDR_FORMAT);
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let ldr = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bench_cull_ldr"),
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
        let hdr = HdrTargets::new(device, "bench_cull", WIDTH, HEIGHT, sample_count);
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
        culling: bool,
    ) -> FrameStats {
        self.pipeline.set_frustum_culling(culling);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench_cull_encoder"),
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
            // TAA 的亚像素抖动：基准里恒定不抖（关掉 TAA 的等价路径）。
            [0.0, 0.0],
            eye,
            SceneLighting::new(scene.ambient, &scene.lights),
            RenderConfig::default().sky,
            1.0,
            xgame_core::ShadowConfig::default(),
            &[], // 无粒子：这条 bench 量的是剔除 / 实例化成本
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

#[derive(Default, Clone, Copy)]
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
        label: Some("bench_cull_resolve"),
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

/// 一个场景的完整 A/B：返回（关剔除、开剔除）两档的中位数与末帧统计。
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
    label: &str,
) -> ((Round, FrameStats), (Round, FrameStats)) {
    let warmup = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("bench_cull_warmup"),
        ty: wgpu::QueryType::Timestamp,
        count: (FRAMES * STAMPS_PER_FRAME) as u32,
    });
    let warmup_start = Instant::now();
    while warmup_start.elapsed() < WARMUP_TIME {
        for culling in [false, true] {
            path.frame(
                device, queue, &warmup, 0, scene, matrices, cache, anim, view_proj, eye, culling,
            );
        }
    }
    device.poll(wgpu::Maintain::Wait);

    let byte_len = (FRAMES * STAMPS_PER_FRAME * 8) as u64;
    let make_buffers = || {
        (
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bench_cull_stamps_resolve"),
                size: byte_len,
                usage: wgpu::BufferUsages::QUERY_RESOLVE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bench_cull_stamps_readback"),
                size: byte_len,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        )
    };

    let mut rounds: Vec<(Round, Round, FrameStats, FrameStats)> = Vec::with_capacity(ROUNDS);
    for round in 0..ROUNDS {
        let stamps = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("bench_cull_stamps"),
            ty: wgpu::QueryType::Timestamp,
            count: (FRAMES * STAMPS_PER_FRAME) as u32,
        });
        let (resolve, readback) = make_buffers();
        // 交替顺序每轮翻转，抵消"后跑的一档赶上 GPU 升温"的系统偏差。
        if round % 2 == 0 {
            let off = run_round(device, queue, &stamps, &resolve, &readback, |base| {
                path.frame(
                    device, queue, &stamps, base, scene, matrices, cache, anim, view_proj, eye,
                    false,
                )
            });
            let on = run_round(device, queue, &stamps, &resolve, &readback, |base| {
                path.frame(
                    device, queue, &stamps, base, scene, matrices, cache, anim, view_proj, eye,
                    true,
                )
            });
            rounds.push((off.0, on.0, off.1, on.1));
        } else {
            let on = run_round(device, queue, &stamps, &resolve, &readback, |base| {
                path.frame(
                    device, queue, &stamps, base, scene, matrices, cache, anim, view_proj, eye,
                    true,
                )
            });
            let off = run_round(device, queue, &stamps, &resolve, &readback, |base| {
                path.frame(
                    device, queue, &stamps, base, scene, matrices, cache, anim, view_proj, eye,
                    false,
                )
            });
            rounds.push((off.0, on.0, off.1, on.1));
        }
        let (off, on, _, _) = rounds.last().copied().unwrap();
        println!(
            "  {label} 第 {} 轮: 关 场景{:.1}/CPU{:.1} | 开 场景{:.1}/CPU{:.1}（µs/帧）",
            round + 1,
            off.scene_us,
            off.cpu_us,
            on.scene_us,
            on.cpu_us,
        );
    }

    let pick = |f: fn(&Round) -> f64, idx: usize| {
        let mut v: Vec<f64> = rounds
            .iter()
            .map(|r| f(if idx == 0 { &r.0 } else { &r.1 }))
            .collect();
        median(&mut v)
    };
    let off = Round {
        scene_us: pick(|r| r.scene_us, 0),
        post_us: pick(|r| r.post_us, 0),
        cpu_us: pick(|r| r.cpu_us, 0),
    };
    let on = Round {
        scene_us: pick(|r| r.scene_us, 1),
        post_us: pick(|r| r.post_us, 1),
        cpu_us: pick(|r| r.cpu_us, 1),
    };
    let off_stats = rounds.last().map(|r| r.2).unwrap_or_default();
    let on_stats = rounds.last().map(|r| r.3).unwrap_or_default();

    // P2.5 的教训：基准必须断言"活真干了"，否则剔除没生效时会得到一张
    // "开剔除更快"的假表。
    assert!(
        off.scene_us > 0.0 && on.scene_us > 0.0,
        "{label}: 时间戳读数为 0：适配器是不是没真正写入查询？读数不可信"
    );
    assert_eq!(
        off_stats.culled_entities, 0,
        "{label}: 关剔除时不该有实体被跳过"
    );
    assert_eq!(
        off_stats.opaque_instances, ACTORS,
        "{label}: 关剔除时必须全部进主 pass"
    );
    assert!(
        on_stats.culled_entities > ACTORS / 2,
        "{label}: 机位没把大部分方块放到视锥外（只剔了 {}），基准测不出收益",
        on_stats.culled_entities
    );
    assert_eq!(
        on_stats.opaque_instances + on_stats.culled_instances,
        ACTORS,
        "{label}: 开剔除时'画了 + 剔了'必须等于全部绘制项（漏算 = 有实体凭空消失）"
    );
    // 剔除只该省活，不该倒贴：给 5% 余量吸收测量噪声。
    assert!(
        on.cpu_us <= off.cpu_us * 1.05,
        "{label}: 开剔除反而更慢：{:.1} vs {:.1} µs/帧（每实体包围盒 + 平面测试的成本失控）",
        on.cpu_us,
        off.cpu_us
    );

    println!(
        "  {label}[中位数]: 关 场景{:.1}+后处理{:.1}={:.1}（CPU {:.1}）| 开 场景{:.1}+后处理{:.1}={:.1}（CPU {:.1}）\
         —— 批次 {}→{}、实例 {}→{}、剔除 {} 实例（{:.1}%）",
        off.scene_us,
        off.post_us,
        off.scene_us + off.post_us,
        off.cpu_us,
        on.scene_us,
        on.post_us,
        on.scene_us + on.post_us,
        on.cpu_us,
        off_stats.opaque_batches,
        on_stats.opaque_batches,
        off_stats.opaque_instances,
        on_stats.opaque_instances,
        on_stats.culled_instances,
        on_stats.cull_ratio() * 100.0,
    );
    println!(
        "  {label}[差额]: 场景 -{:.1} µs/帧（{:+.0}%）、CPU -{:.1} µs/帧（{:+.0}%）",
        off.scene_us - on.scene_us,
        (on.scene_us - off.scene_us) / off.scene_us * 100.0,
        off.cpu_us - on.cpu_us,
        (on.cpu_us - off.cpu_us) / off.cpu_us * 100.0,
    );

    ((off, off_stats), (on, on_stats))
}

#[test]
#[ignore = "手动运行的 GPU 微基准，不进 CI 门禁"]
fn culling_cost() {
    let Some((device, queue, adapter)) = gpu() else {
        return;
    };
    let info = adapter.get_info();
    println!(
        "culling_cost: {} ({:?}, {:?}) {}x{} —— {ACTORS} 实体，开/关剔除交替",
        info.name, info.device_type, info.backend, WIDTH, HEIGHT
    );

    let anim = AnimationDriver::new();
    let (eye, view_proj) = actor_camera();
    let mut path = BenchPath::new(&device, &adapter);
    println!(
        "  MSAA x{}；30° 窄视场、方阵 {GRID}x{GRID}、间距 {SPACING}",
        path.pipeline.sample_count(),
    );

    let shared = actor_scene(false);
    let shared_matrices = shared.world_matrices();
    let shared_cache = actor_cache(&device, &queue, false);
    let ((off_shared, off_shared_stats), (on_shared, on_shared_stats)) = measure(
        &device,
        &queue,
        &mut path,
        &shared,
        &shared_matrices,
        &shared_cache,
        &anim,
        &eye,
        &view_proj,
        "共享网格",
    );

    let unique = actor_scene(true);
    let unique_matrices = unique.world_matrices();
    let unique_cache = actor_cache(&device, &queue, true);
    let ((off_unique, off_unique_stats), (on_unique, on_unique_stats)) = measure(
        &device,
        &queue,
        &mut path,
        &unique,
        &unique_matrices,
        &unique_cache,
        &anim,
        &eye,
        &view_proj,
        "各自网格",
    );

    println!(
        "culling_cost[总结]: 共享网格 CPU {:.1}→{:.1} µs/帧（{:+.0}%）、场景 {:.1}→{:.1} µs；\
         各自网格 CPU {:.1}→{:.1} µs/帧（{:+.0}%）、场景 {:.1}→{:.1} µs",
        off_shared.cpu_us,
        on_shared.cpu_us,
        (on_shared.cpu_us - off_shared.cpu_us) / off_shared.cpu_us * 100.0,
        off_shared.scene_us,
        on_shared.scene_us,
        off_unique.cpu_us,
        on_unique.cpu_us,
        (on_unique.cpu_us - off_unique.cpu_us) / off_unique.cpu_us * 100.0,
        off_unique.scene_us,
        on_unique.scene_us,
    );
    // 合批结构的护栏：剔除不许改变同网格的分组（只少实例、不少批次序号语义）。
    assert_eq!(
        on_shared_stats.opaque_batches, off_shared_stats.opaque_batches,
        "共享网格场景的批次数不该因剔除而变化（只该少实例）"
    );
    assert!(
        off_unique_stats.opaque_batches >= ACTORS,
        "各自网格场景一条都不该合并（合错了 = 画面错乱）"
    );
    // 剔除在未合批场景里必须真的砍掉 draw call —— 这是 CPU 侧收益的来源。
    // 阈值取"至少少一半"（实测剔除比例 ~57%）。
    assert!(
        on_unique_stats.opaque_batches * 2 <= off_unique_stats.opaque_batches,
        "剔除没有减少 draw call：{} → {}",
        off_unique_stats.opaque_batches,
        on_unique_stats.opaque_batches
    );
}
