//! 微基准：SSAO 四趟 pass（AO 生成 + 模糊横 + 模糊纵 + 乘算）的每帧 GPU 成本。
//!
//! 默认 `#[ignore]`（耗时波动大、需要显卡，不该进 CI 门禁）。手动跑：
//!
//! ```sh
//! cargo test -p xgame-render --release --test ssao_cost -- --ignored --nocapture
//! ```
//!
//! # 比什么
//!
//! | 路径 | 组成 |
//! |---|---|
//! | 关 | 场景 pass → HDR → PostFx（SSAO 整段跳过） |
//! | 开 | 场景 pass → HDR → 【SSAO 四趟】→ PostFx |
//!
//! 差额 = AO 生成（半分辨率、16 样本 kernel）+ 模糊横纵（半分辨率 10 抽头）
//! \+ 乘算（全分辨率读两张纹理）。同一台设备、同一场景、同一分辨率交替跑，
//! **GPU 时间戳**取中位数（口径与 `postfx_cost` 相同：墙上时钟混着 CPU 编码
//! 与队列调度，测的不是 GPU 成本）。
//!
//! ⚠ 绝对值随机器状态浮动，**跨版本比较必须同机背靠背**。

use std::collections::HashMap;
use std::time::Instant;

use xgame_core::{Entity, Light, RenderConfig, Scene, SsaoConfig};
use xgame_math::glam::{Mat4, Vec3};
use xgame_render::animation::AnimationDriver;
use xgame_render::gltf_loader::GltfMesh;
use xgame_render::mesh::BuiltinMesh;
use xgame_render::postfx::{HdrTargets, PostFx, HDR_FORMAT};
use xgame_render::scene_pipeline::{supported_sample_count, SceneLighting, ScenePipeline};
use xgame_render::ssao::SsaoPass;

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
/// 每帧 3 个时间戳：帧头 / SSAO 段前（= 场景 pass 末）/ 帧末。
const STAMPS_PER_FRAME: usize = 3;
const FRAMES: usize = 64;
const ROUNDS: usize = 8;
const WARMUP_TIME: std::time::Duration = std::time::Duration::from_millis(1500);

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
        eprintln!(
            "跳过 ssao_cost：适配器不支持时间戳查询（{required:?}）—— 墙钟口径噪声太大，不强测"
        );
        return None;
    }
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("ssao_cost device"),
            required_features: required,
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        },
        None,
    ))
    .ok()?;
    Some((device, queue, adapter))
}

/// 基准场景：地面 + 8×8 立方体阵（与 `postfx_cost` 同构 —— SSAO 是全屏
/// pass，空场景测出来的成本没有代表性；大量内角给 kernel 真实的遮蔽负载）。
fn bench_scene() -> Scene {
    let mut scene = Scene::new("ssao-cost");
    scene.ambient = [0.03, 0.03, 0.05];
    scene.lights = vec![Light::directional(
        "sun",
        [0.5, -1.0, -0.4],
        [1.0, 0.98, 0.92],
        2.2,
    )];

    let mut ground = Entity::new("Ground");
    ground.model = "builtin://plane".to_owned();
    ground.transform.scale = Vec3::splat(15.0);
    scene.add_entity(ground);

    for z in 0..8 {
        for x in 0..8 {
            let mut cube = Entity::new(format!("Cube_{x}_{z}"));
            cube.model = "builtin://cube".to_owned();
            cube.transform.position = Vec3::new(x as f32 - 3.5, 0.5, z as f32 - 3.5);
            scene.add_entity(cube);
        }
    }
    scene
}

fn model_cache(device: &wgpu::Device, queue: &wgpu::Queue) -> HashMap<String, GltfMesh> {
    let mut cache = HashMap::new();
    for builtin in [BuiltinMesh::Cube, BuiltinMesh::Plane] {
        let mesh = GltfMesh::from_mesh_data(device, queue, &builtin.build(), builtin.label());
        cache.insert(builtin.uri(), mesh);
    }
    cache
}

fn bench_camera() -> (Vec3, Mat4) {
    let eye = Vec3::new(11.0, 8.5, 11.0);
    let view = Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y);
    let proj = Mat4::perspective_rh(
        50.0_f32.to_radians(),
        WIDTH as f32 / HEIGHT as f32,
        0.1,
        200.0,
    );
    (eye, proj * view)
}

/// 一条完整路径：场景 pass + SSAO（可关）+ PostFx。
struct Path {
    pipeline: ScenePipeline,
    hdr: HdrTargets,
    _ldr: wgpu::Texture,
    ldr_view: wgpu::TextureView,
    postfx: PostFx,
    ssao: SsaoPass,
}

impl Path {
    fn new(device: &wgpu::Device, adapter: &wgpu::Adapter) -> Self {
        let sample_count = supported_sample_count(adapter, HDR_FORMAT);
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let ldr = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bench_ssao_ldr"),
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
        let hdr = HdrTargets::new(device, "bench", WIDTH, HEIGHT, sample_count);
        let postfx = PostFx::new(device, format);
        let ssao = SsaoPass::new(device, sample_count);
        Self {
            pipeline,
            hdr,
            _ldr: ldr,
            ldr_view,
            postfx,
            ssao,
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
        settings: RenderConfig,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench_ssao_encoder"),
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
            settings.sky,
            settings.lod_bias,
            settings.shadow,
            &[],
        );
        encoder.write_timestamp(stamps, base + 1);
        // SSAO 段（关着时整段跳过 —— 差额就是被测的那四趟 pass）。
        let (source_view, source_gen) = if settings.ssao.enabled {
            self.ssao.apply(
                device,
                queue,
                &mut encoder,
                &self.hdr.color_view,
                0,
                &self.hdr.depth_view,
                self.hdr.size,
                view_proj,
                eye,
                settings.ssao,
            )
        } else {
            (&self.hdr.color_view, 0u64)
        };
        self.postfx.render(
            device,
            queue,
            &mut encoder,
            source_view,
            source_gen,
            self.hdr.size,
            &self.ldr_view,
            settings,
        );
        encoder.write_timestamp(stamps, base + 2);
        queue.submit(std::iter::once(encoder.finish()));
    }
}

/// 一轮采样结果（µs/帧，取中位数）。
#[derive(Default, Clone, Copy)]
struct Round {
    /// 场景 pass 段。
    scene_us: f64,
    /// SSAO + PostFx 段（差额分析用）。
    tail_us: f64,
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
    mut frame: impl FnMut(u32),
) -> Round {
    for i in 0..FRAMES {
        frame(i as u32 * STAMPS_PER_FRAME as u32);
    }

    let byte_len = (FRAMES * STAMPS_PER_FRAME * 8) as u64;
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench_ssao_resolve"),
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
    let mut tail = Vec::with_capacity(FRAMES);
    for f in 0..FRAMES {
        let base = f * STAMPS_PER_FRAME;
        scene.push((ticks[base + 1] - ticks[base]) as f64 * period / 1000.0);
        tail.push((ticks[base + 2] - ticks[base + 1]) as f64 * period / 1000.0);
    }
    Round {
        scene_us: median(&mut scene),
        tail_us: median(&mut tail),
    }
}

/// SSAO 的账：关 / 开两档交替跑，差额 = 四趟 pass 的 GPU 成本。
#[test]
#[ignore = "手动运行的 GPU 微基准，不进 CI 门禁"]
fn ssao_cost() {
    let Some((device, queue, adapter)) = gpu() else {
        return;
    };
    let info = adapter.get_info();
    println!(
        "ssao_cost: {} ({:?}, {:?}) {}x{}",
        info.name, info.device_type, info.backend, WIDTH, HEIGHT
    );

    let scene = bench_scene();
    let cache = model_cache(&device, &queue);
    let anim = AnimationDriver::new();
    let matrices = scene.world_matrices();
    let (eye, view_proj) = bench_camera();

    // 两份独立实例：AO 纹理 / scratch / 绑定组在预热期建好、之后恒定复用
    // （同一实例上来回切设置测的是重建开销，不是稳态成本 —— 与
    // `postfx_cost` 的 Bloom 档同一纪律）。
    let mut off = Path::new(&device, &adapter);
    let mut on = Path::new(&device, &adapter);
    let on_settings = RenderConfig {
        ssao: SsaoConfig {
            enabled: true,
            ..Default::default()
        },
        ..RenderConfig::default()
    };
    println!("  MSAA x{}", off.pipeline.sample_count());

    let warmup = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("bench_ssao_warmup"),
        ty: wgpu::QueryType::Timestamp,
        count: (FRAMES * STAMPS_PER_FRAME) as u32,
    });
    let warmup_start = Instant::now();
    while warmup_start.elapsed() < WARMUP_TIME {
        off.frame(
            &device,
            &queue,
            &warmup,
            0,
            &scene,
            &matrices,
            &cache,
            &anim,
            &view_proj,
            &eye,
            RenderConfig::default(),
        );
        on.frame(
            &device,
            &queue,
            &warmup,
            0,
            &scene,
            &matrices,
            &cache,
            &anim,
            &view_proj,
            &eye,
            on_settings,
        );
    }
    device.poll(wgpu::Maintain::Wait);

    let byte_len = (FRAMES * STAMPS_PER_FRAME * 8) as u64;
    let mut pairs = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let stamps = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("bench_ssao_stamps"),
            ty: wgpu::QueryType::Timestamp,
            count: (FRAMES * STAMPS_PER_FRAME) as u32,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bench_ssao_stamps_resolve"),
            size: byte_len,
            usage: wgpu::BufferUsages::QUERY_RESOLVE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bench_ssao_stamps_readback"),
            size: byte_len,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // 交替顺序每轮翻转：先测谁的 GPU 频率爬坡便宜，翻转就互相抵消。
        let (a, b) = if pairs.len() % 2 == 0 {
            (
                run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
                    off.frame(
                        &device,
                        &queue,
                        &stamps,
                        base,
                        &scene,
                        &matrices,
                        &cache,
                        &anim,
                        &view_proj,
                        &eye,
                        RenderConfig::default(),
                    )
                }),
                run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
                    on.frame(
                        &device,
                        &queue,
                        &stamps,
                        base,
                        &scene,
                        &matrices,
                        &cache,
                        &anim,
                        &view_proj,
                        &eye,
                        on_settings,
                    )
                }),
            )
        } else {
            (
                run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
                    on.frame(
                        &device,
                        &queue,
                        &stamps,
                        base,
                        &scene,
                        &matrices,
                        &cache,
                        &anim,
                        &view_proj,
                        &eye,
                        on_settings,
                    )
                }),
                run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
                    off.frame(
                        &device,
                        &queue,
                        &stamps,
                        base,
                        &scene,
                        &matrices,
                        &cache,
                        &anim,
                        &view_proj,
                        &eye,
                        RenderConfig::default(),
                    )
                }),
            )
        };
        // 交替翻转后 a/b 的身份按轮次定：偶数轮 a=off、奇数轮 a=on。
        if pairs.len() % 2 == 0 {
            pairs.push((a, b));
        } else {
            pairs.push((b, a));
        }
    }

    // 轮间再取中位数（口径同 `postfx_cost`）。
    let mut off_scene = Vec::new();
    let mut off_tail = Vec::new();
    let mut on_scene = Vec::new();
    let mut on_tail = Vec::new();
    for (o, n) in &pairs {
        off_scene.push(o.scene_us);
        off_tail.push(o.tail_us);
        on_scene.push(n.scene_us);
        on_tail.push(n.tail_us);
    }
    let off_scene = median(&mut off_scene);
    let off_tail = median(&mut off_tail);
    let on_scene = median(&mut on_scene);
    let on_tail = median(&mut on_tail);

    println!(
        "  关：场景 {off_scene:.1} µs + 尾段 {off_tail:.1} µs = {:.1} µs/帧",
        off_scene + off_tail
    );
    println!(
        "  开：场景 {on_scene:.1} µs + 尾段 {on_tail:.1} µs = {:.1} µs/帧",
        on_scene + on_tail
    );
    println!(
        "  SSAO 差额：尾段 {off_tail:.1} → {on_tail:.1} µs（+{:.1} µs，+{:.1}%）",
        on_tail - off_tail,
        (on_tail - off_tail) / off_tail.max(0.001) * 100.0
    );

    // P2.5 纪律：读数为 0 必红（"活真干了"），以及"开着必须比关着贵"。
    assert!(off_tail > 0.0, "关档尾段读数为 0 —— 时间戳没打上");
    assert!(on_tail > off_tail, "开着不比关着贵 = SSAO pass 没跑");
}
