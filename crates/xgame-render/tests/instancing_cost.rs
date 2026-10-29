//! 微基准：GPU 实例化（draw batching）的每帧成本。
//!
//! 默认 `#[ignore]`（耗时波动大、需要显卡，不该进 CI 门禁）。手动跑：
//!
//! ```sh
//! cargo test -p xgame-render --release --test instancing_cost -- --ignored --nocapture
//! ```
//!
//! # 比什么
//!
//! 差距分析里"2000 实体靠逐 drawcall 撑住"这一条：每帧 2000 次
//! `set_bind_group` / `set_vertex_buffer` / `draw_indexed` 是 CPU 侧的固定税，
//! 与 GPU 有没有活干无关。实例化把"同网格 + 同材质"的绘制项合成一次
//! `draw_indexed(0..n, 0, first..first+count)`，代价是逐实例数据要走存储缓冲。
//!
//! 两个场景**几何完全相同**，只有"批次能不能合并"这一项不同：
//!
//! | 场景 | 组成 | 批次 |
//! |---|---|---|
//! | 共享网格 | 2000 个实体全用 `builtin://cube` | 2 批（地面 1 + 方块 1，实例化真正生效） |
//! | 各自网格 | 2000 个实体各自一份 `GltfMesh`（同几何、不同材质槽） | 2001 批（退化为逐 draw） |
//!
//! 后者不是"假想的最坏情况"——它同时是回归护栏：合批逻辑一旦把不同网格
//! 错并成一批（顶点缓冲不同却当成同批），画面上会直接错乱，这里也是唯一
//! 能通过 CPU 读数看出"合批没干活"的入口。
//!
//! # 读数口径
//!
//! 与 `postfx_cost` / `sky_ibl_cost` 同一套：**GPU 时间戳**取场景段（关后处理
//! 噪声），CPU 段单独记"编码 + 提交"（这是实例化真正要砍的开销）。两档交替
//! 跑 `ROUNDS` 轮取中位数。
//!
//! ⚠ 绝对值随机器状态浮动（与 `update_throughput` 同款警告），**跨版本
//! 比较必须同机背靠背**。

use std::collections::HashMap;
use std::time::Instant;

use xgame_core::{BloomConfig, Entity, Light, MaterialOverride, RenderConfig, Scene};
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
/// 每轮帧数。
const FRAMES: usize = 64;
/// 交替轮数。
const ROUNDS: usize = 8;
/// 预热时长（墙钟，与 `postfx_cost` 同款纪律）。
const WARMUP_TIME: std::time::Duration = std::time::Duration::from_millis(1500);

/// 演员数量：差距分析里点名的 "2000 实体"。
const ACTORS: usize = 2000;
/// 网格边长（`ACTORS` 个方块铺成方阵；2000 ≤ 45² = 2025）。
const GRID: usize = 45;
/// 方块间距（方块本身是单位立方体）。
const SPACING: f32 = 1.35;

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
        eprintln!("跳过 instancing_cost：适配器不支持时间戳查询");
        return None;
    }
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("instancing_cost device"),
            required_features: required,
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        },
        None,
    ))
    .ok()?;
    Some((device, queue, adapter))
}

/// 演员阵场景：一块地面 + `ACTORS` 个方块。
///
/// `unique_meshes = false` 时全部实体共用 `builtin://cube`（可实例化）；
/// `true` 时每个实体各持一份网格（素材侧就是"每个物体一个材质"，合批无门）。
/// 两种情况的**几何与材质参数完全一致**，读数差只来自批次数量。
///
/// 每 3 个实体给一次实体级材质覆盖：逐实例材质是实例化的核心命题之一
/// （覆盖写进实例数据而不是材质绑定组），带上它才是真实的负载。
fn actor_scene(unique_meshes: bool) -> Scene {
    let mut scene = Scene::new(if unique_meshes {
        "instancing-unique"
    } else {
        "instancing-shared"
    });
    scene.ambient = [0.03, 0.03, 0.05];
    scene.lights = vec![Light::directional(
        "sun",
        [0.5, -1.0, -0.4],
        [1.0, 0.98, 0.92],
        2.2,
    )];

    let mut ground = Entity::new("Ground");
    ground.model = "builtin://plane".to_owned();
    ground.transform.scale = Vec3::new(GRID as f32 * SPACING, 1.0, GRID as f32 * SPACING);
    scene.add_entity(ground);

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
        if i % 3 == 0 {
            e.material = Some(MaterialOverride {
                base_color: None,
                metallic: Some(0.85),
                roughness: Some(0.3),
            });
        }
        scene.add_entity(e);
    }
    scene
}

/// 共享场景的模型缓存：内置图元（与真实宿主同一条加载入口）。
fn shared_cache(device: &wgpu::Device, queue: &wgpu::Queue) -> HashMap<String, GltfMesh> {
    let mut cache = HashMap::new();
    for builtin in [BuiltinMesh::Cube, BuiltinMesh::Plane] {
        let mesh = GltfMesh::from_mesh_data(device, queue, &builtin.build(), builtin.label());
        cache.insert(builtin.uri(), mesh);
    }
    cache
}

/// 各自网格场景的模型缓存：`ACTORS` 份同几何网格（各持自己的材质槽）。
fn unique_cache(device: &wgpu::Device, queue: &wgpu::Queue) -> HashMap<String, GltfMesh> {
    let data = BuiltinMesh::Cube.build();
    let mut cache = HashMap::with_capacity(ACTORS + 1);
    let plane = GltfMesh::from_mesh_data(device, queue, &BuiltinMesh::Plane.build(), "plane");
    cache.insert(BuiltinMesh::Plane.uri(), plane);
    for i in 0..ACTORS {
        let mesh = GltfMesh::from_mesh_data(device, queue, &data, "bench cube");
        cache.insert(format!("bench://mesh{i}"), mesh);
    }
    cache
}

/// 固定机位：把演员阵与地面尽量框满（两条路径共用同一个 `view_proj`）。
fn actor_camera() -> (Vec3, Mat4) {
    let eye = Vec3::new(46.0, 36.0, 46.0);
    let view = Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y);
    let proj = Mat4::perspective_rh(
        50.0_f32.to_radians(),
        WIDTH as f32 / HEIGHT as f32,
        0.1,
        400.0,
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
            label: Some("bench_inst_ldr"),
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
        // 这条基准测的是**合批**：关掉剔除，让 2000 个演员全部进实例表，
        // 两档的绘制项数才严格相等（剔除另有 `culling_cost` 基准）。
        pipeline.set_frustum_culling(false);
        let hdr = HdrTargets::new(device, "bench_inst", WIDTH, HEIGHT, sample_count);
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
            label: Some("bench_inst_encoder"),
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
            &[], // 无粒子：这条 bench 量的是场景 / 后处理成本
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

/// 跑一轮：`FRAMES` 帧，返回两段 GPU 时长与 CPU 编码的中位数；顺带记录
/// 最后一帧的绘制统计（每帧相同，取一帧即可）。
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
        label: Some("bench_inst_resolve"),
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

#[test]
#[ignore = "手动运行的 GPU 微基准，不进 CI 门禁"]
fn instancing_cost() {
    let Some((device, queue, adapter)) = gpu() else {
        return;
    };
    let info = adapter.get_info();
    println!(
        "instancing_cost: {} ({:?}, {:?}) {}x{} —— {ACTORS} 实体 × 2 场景",
        info.name, info.device_type, info.backend, WIDTH, HEIGHT
    );

    let shared_scene = actor_scene(false);
    let unique_scene = actor_scene(true);
    let shared_matrices = shared_scene.world_matrices();
    let unique_matrices = unique_scene.world_matrices();
    let shared_cache = shared_cache(&device, &queue);
    let unique_cache = unique_cache(&device, &queue);
    let anim = AnimationDriver::new();
    let (eye, view_proj) = actor_camera();

    let mut shared = BenchPath::new(&device, &adapter);
    let mut unique = BenchPath::new(&device, &adapter);
    println!(
        "  MSAA x{}；两场景各 {ACTORS} 实体（批次结构见末尾统计）",
        shared.pipeline.sample_count(),
    );

    // 预热（时间戳写进一次性查询集，不 resolve）。
    let warmup = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("bench_inst_warmup"),
        ty: wgpu::QueryType::Timestamp,
        count: (FRAMES * STAMPS_PER_FRAME) as u32,
    });
    let warmup_start = Instant::now();
    while warmup_start.elapsed() < WARMUP_TIME {
        shared.frame(
            &device,
            &queue,
            &warmup,
            0,
            &shared_scene,
            &shared_matrices,
            &shared_cache,
            &anim,
            &view_proj,
            &eye,
        );
        unique.frame(
            &device,
            &queue,
            &warmup,
            0,
            &unique_scene,
            &unique_matrices,
            &unique_cache,
            &anim,
            &view_proj,
            &eye,
        );
    }
    device.poll(wgpu::Maintain::Wait);

    let byte_len = (FRAMES * STAMPS_PER_FRAME * 8) as u64;
    let make_buffers = || {
        (
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bench_inst_stamps_resolve"),
                size: byte_len,
                usage: wgpu::BufferUsages::QUERY_RESOLVE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bench_inst_stamps_readback"),
                size: byte_len,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        )
    };

    let mut rounds: Vec<(Round, Round, FrameStats, FrameStats)> = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let stamps = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("bench_inst_stamps"),
            ty: wgpu::QueryType::Timestamp,
            count: (FRAMES * STAMPS_PER_FRAME) as u32,
        });
        let (resolve, readback) = make_buffers();
        let (s, s_stats) = run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
            shared.frame(
                &device,
                &queue,
                &stamps,
                base,
                &shared_scene,
                &shared_matrices,
                &shared_cache,
                &anim,
                &view_proj,
                &eye,
            )
        });
        let (u, u_stats) = run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
            unique.frame(
                &device,
                &queue,
                &stamps,
                base,
                &unique_scene,
                &unique_matrices,
                &unique_cache,
                &anim,
                &view_proj,
                &eye,
            )
        });
        println!(
            "  第 {} 轮: 共享 场景{:.1}/CPU{:.1} | 各自 场景{:.1}/CPU{:.1}（µs/帧）",
            rounds.len() + 1,
            s.scene_us,
            s.cpu_us,
            u.scene_us,
            u.cpu_us,
        );
        rounds.push((s, u, s_stats, u_stats));
    }

    let pick = |f: fn(&Round) -> f64, idx: usize| {
        let mut v: Vec<f64> = rounds
            .iter()
            .map(|r| f(if idx == 0 { &r.0 } else { &r.1 }))
            .collect();
        median(&mut v)
    };
    let s_scene = pick(|r| r.scene_us, 0);
    let s_post = pick(|r| r.post_us, 0);
    let s_cpu = pick(|r| r.cpu_us, 0);
    let u_scene = pick(|r| r.scene_us, 1);
    let u_post = pick(|r| r.post_us, 1);
    let u_cpu = pick(|r| r.cpu_us, 1);
    let s_stats = rounds.last().map(|r| r.2).unwrap_or_default();
    let u_stats = rounds.last().map(|r| r.3).unwrap_or_default();

    println!(
        "instancing_cost[中位数]: 共享网格 场景 {s_scene:.1} + 后处理 {s_post:.1} = {:.1} µs/帧（CPU {s_cpu:.1}）\
         —— 不透明 {}/{} 批/实例",
        s_scene + s_post,
        s_stats.opaque_batches,
        s_stats.opaque_instances,
    );
    println!(
        "instancing_cost[中位数]: 各自网格 场景 {u_scene:.1} + 后处理 {u_post:.1} = {:.1} µs/帧（CPU {u_cpu:.1}）\
         —— 不透明 {}/{} 批/实例",
        u_scene + u_post,
        u_stats.opaque_batches,
        u_stats.opaque_instances,
    );
    println!(
        "instancing_cost[差额]: 合批省下 CPU {:.1} µs/帧（{:+.0}%）、场景段 {:.1} µs/帧；\
         半透明绘制项 {} / {}（两场景应相同）",
        u_cpu - s_cpu,
        (s_cpu - u_cpu) / u_cpu * 100.0,
        u_scene - s_scene,
        s_stats.blended_draws,
        u_stats.blended_draws,
    );

    // P2.5 的教训：基准必须断言"活真干了"（宿主静默失败的场景在这里是
    // 假快 —— 数字好看但调度根本没发生）。
    assert!(
        s_scene > 0.0 && u_scene > 0.0,
        "时间戳读数为 0：适配器是不是没真正写入查询？读数不可信"
    );
    // 实例化生效的硬读数：共享网格场景必须压成"远小于绘制项数"的批次数。
    // 阈值刻意松（1%），不钉具体数字 —— 将来批次结构变多（按渲染状态分组
    // 之类）也不该把它弄红；退化成逐 draw 则必然红。
    assert!(
        s_stats.opaque_batches * 100 <= s_stats.opaque_instances,
        "共享网格场景没有合批：{} 批 / {} 实例",
        s_stats.opaque_batches,
        s_stats.opaque_instances
    );
    // 反向护栏：各自一份网格的场景一条都不许合并（并错 = 画面错乱）。
    assert!(
        u_stats.opaque_batches >= ACTORS,
        "各自网格场景的批次不应减少（合错了）：{} 批 < {ACTORS}",
        u_stats.opaque_batches
    );
    assert_eq!(
        s_stats.opaque_instances, u_stats.opaque_instances,
        "两种形态的绘制项数必须相同（几何完全相同）"
    );
    assert_eq!(
        s_stats.blended_draws, u_stats.blended_draws,
        "半透明绘制项数两场景应当相同"
    );
    // CPU 档位：合批必须真的省下"编码 + 提交"开销。给足余量（同机实测
    // 6 倍以上），不做精确比值 —— 绝对值随机器状态浮动 1.5× 是常态。
    assert!(
        s_cpu * 2.0 < u_cpu,
        "合批未能显著省下 CPU：共享 {s_cpu:.1} vs 各自 {u_cpu:.1} µs/帧"
    );
}
