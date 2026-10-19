//! 微基准：HDR + 色调映射链的每帧 GPU 成本（对照旧 LDR 直渲）。
//!
//! 默认 `#[ignore]`（耗时波动大、需要显卡，不该进 CI 门禁）。手动跑：
//!
//! ```sh
//! cargo test -p xgame-render --release --test postfx_cost -- --ignored --nocapture
//! ```
//!
//! # 比什么、为什么这么比
//!
//! 后处理链上线改变了**每一个像素**都要走的路（附件格式、resolve、多一趟
//! 全屏 pass），这类改动的账只能用"同机背靠背"的方式算 —— 仓库纪律："性能
//! 改动先量后改，量数据写进记录"。两条路径在同一台设备、同一场景、同一
//! 分辨率下交替跑：
//!
//! | 路径 | 组成 |
//! |---|---|
//! | 旧 LDR 直渲 | 场景 pass → MSAA(`Rgba8UnormSrgb`) → resolve → LDR 目标（无后处理） |
//! | HDR + ACES | 场景 pass → MSAA(`Rgba16Float`) → resolve → 后处理（曝光 + ACES）→ LDR 目标 |
//! | HDR + ACES + Bloom | 同上，后处理里多跑一条 Bloom 链（intensity 0.6，3 级 mip） |
//!
//! 差额 = fp16 附件的写带宽 + fp16 resolve + 全屏后处理 pass + Bloom 链
//! （亮部提取 + 2 次降采样 + 6 次可分离模糊，全部在半分辨率以下）。旧路径的
//! 代码不在仓库里（管线已被替换），这里**现搭一条等价的老管线**（同
//! `ScenePipeline`、同采样数，只把格式换成 sRGB、去掉后处理）—— 唯一变动的
//! 就是被测的那几项，其余全部同源。
//!
//! 写成 `tests/` 下的集成测试而不是 `src` 里的单元测试，理由与
//! `update_throughput` 相同：基准要能独立于被测源码的改动而存在，这样对
//! 同一份基准跑"改动前 / 改动后"两份代码才是公平的。
//!
//! # 读数口径：GPU 时间戳，不是墙上时钟
//!
//! 首版用"编码 + 提交 + 最终等待"的墙上时钟，实测被本机噪声打成筛子
//! （同一路径轮间 207~515 µs，最小值法甚至让两条路径的胜负翻转）——
//! 墙钟口径里混着 CPU 编码、提交队列调度和系统负载，测的不是 GPU 成本。
//! 现改为 **GPU 时间戳查询**：每帧头 / 场景 pass 末 / 帧末各打一个点，
//! 得到"场景 pass"与"后处理 pass"两段纯 GPU 执行时长；取**中位数**（少数
//! 帧被系统抢了时间不会污染读数）。CPU 侧的编码 + 提交耗时单独记一档，
//! 那是"后处理给主线程加的记录开销"，与 GPU 带宽账分开看。
//!
//! 适配器不支持时间戳查询时跳过（与截图测试的"拿不到就跳过"同款纪律）。
//!
//! 本文件还有第二条入口 **`sky_ibl_cost`**：同一套读数口径（GPU 时间戳、
//! 交替换轮、轮间中位数）用到天空盒 + 环境光照（IBL）上，三档对比
//! （关 / 稳态开 / 每帧重建），细节见该测试的模块文档。两条基准分开成独立
//! 入口，是为了各自的结论（后处理的账 / 天空的账）能被独立引用与重跑。
//!
//! ⚠ 绝对值随机器状态浮动（与 `update_throughput` 同款警告），**跨版本
//! 比较必须同机背靠背**。

use std::collections::HashMap;
use std::time::Instant;

use xgame_core::{BloomConfig, Entity, Light, RenderConfig, Scene, SkyConfig};
use xgame_math::glam::{Mat4, Vec3};
use xgame_render::animation::AnimationDriver;
use xgame_render::gltf_loader::GltfMesh;
use xgame_render::mesh::BuiltinMesh;
use xgame_render::postfx::{HdrTargets, PostFx, HDR_FORMAT};
use xgame_render::scene_pipeline::{supported_sample_count, SceneLighting, ScenePipeline};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
/// 每帧 3 个时间戳：帧头 / 场景 pass 末（= 后处理开始）/ 帧末。
const STAMPS_PER_FRAME: usize = 3;
/// 每轮帧数。
const FRAMES: usize = 64;
/// 交替轮数（把全部帧的读数并起来取中位数）。
const ROUNDS: usize = 8;
/// 预热时长（墙钟）。按时间而不是帧数：GPU 频率爬升是秒级的，
/// 首版按 8 帧预热，前几轮读数比稳态高 ~25%，会把中位数带歪。
const WARMUP_TIME: std::time::Duration = std::time::Duration::from_millis(1500);

/// 一帧 3 个 u64 的跨度刚好 24 字节 —— 整轮一次性 resolve 的字节数是
/// `3 × FRAMES × 8`，这里断言它 256 对齐（`resolve_query_set` 的目标偏移
/// 在 WebGPU 里要求 256 对齐；整轮读回就绕开了逐帧对齐填充）。
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
    // 时间戳查询是读数口径的前提（见模块头）：两条都得有 ——
    // `TIMESTAMP_QUERY` 管查询集的创建，`..._INSIDE_ENCODERS` 管在
    // command encoder 上打点。
    let required =
        wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    if !adapter.features().contains(required) {
        eprintln!(
            "跳过 postfx_cost：适配器不支持时间戳查询（{required:?}）—— 墙钟口径噪声太大，不强测"
        );
        return None;
    }
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("postfx_cost device"),
            required_features: required,
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        },
        None,
    ))
    .ok()?;
    Some((device, queue, adapter))
}

/// 基准场景：一块 30×30 的地面 + 8×8 立方体阵 + 一盏带阴影的方向光。
///
/// 让画面**铺满屏幕**是有意的：后处理是全屏 pass，空场景测出来的差额没有
/// 代表性；64 个立方体 + 地面让"场景 pass 成本"与"后处理成本"处在同一
/// 量级，差额比例才有意义。阴影开着（真实默认），两条路径同样付这笔钱。
fn bench_scene() -> Scene {
    let mut scene = Scene::new("postfx-cost");
    scene.ambient = [0.03, 0.03, 0.05];
    scene.lights = vec![Light::directional(
        "sun",
        [0.5, -1.0, -0.4],
        [1.0, 0.98, 0.92],
        2.2,
    )];

    let mut ground = Entity::new("Ground");
    ground.model = "builtin://plane".to_owned();
    ground.transform.scale = Vec3::new(30.0, 1.0, 30.0);
    scene.add_entity(ground);

    for i in 0..64 {
        let mut e = Entity::new(format!("c{i}"));
        e.model = "builtin://cube".to_owned();
        e.transform.position = Vec3::new(
            (i % 8) as f32 * 1.6 - 5.6,
            0.5,
            (i / 8) as f32 * 1.6 - 5.6,
        );
        scene.add_entity(e);
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

/// 基准相机：俯视 3/4 角，把地面与立方体阵框满画面。两条基准共用，
/// 保证不同入口之间的读数差异只来自被测路径本身。
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

fn depth_target(device: &wgpu::Device, sample_count: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench_depth"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// 旧 LDR 直渲路径：场景 pass 直接写 sRGB 目标（MSAA 开到能开的档位）。
///
/// 纹理本体字段带下划线前缀：它们只负责"活着"（附件不能被提前释放），
/// 代码里只读对应的 view —— 与 `ScreenshotRenderer::_ldr` 同款约定。
struct LdrPath {
    pipeline: ScenePipeline,
    _msaa: wgpu::Texture,
    msaa_view: wgpu::TextureView,
    _color: wgpu::Texture,
    color_view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
}

impl LdrPath {
    fn new(device: &wgpu::Device, adapter: &wgpu::Adapter) -> Self {
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let sample_count = supported_sample_count(adapter, format);
        let extent = wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        };
        let msaa = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bench_ldr_msaa"),
            size: extent,
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let msaa_view = msaa.create_view(&wgpu::TextureViewDescriptor::default());
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bench_ldr_color"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let (depth, depth_view) = depth_target(device, sample_count);
        let pipeline = ScenePipeline::new(device, format, sample_count);
        Self {
            pipeline,
            _msaa: msaa,
            msaa_view,
            _color: color,
            color_view,
            _depth: depth,
            depth_view,
        }
    }

    /// 编码一帧并提交；帧头 / 场景末 / 帧末打时间戳。
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
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench_ldr_encoder"),
        });
        encoder.write_timestamp(stamps, base);
        self.pipeline.render(
            device,
            queue,
            &mut encoder,
            &self.color_view,
            Some(&self.msaa_view),
            &self.depth_view,
            &scene.entities,
            matrices,
            cache,
            anim,
            view_proj,
            eye,
            SceneLighting::new(scene.ambient, &scene.lights),
            SkyConfig::default(),
        );
        encoder.write_timestamp(stamps, base + 1);
        // 旧路径没有第二段；连着打两个点，让"后处理段"恒为 0（读数表结构统一）。
        encoder.write_timestamp(stamps, base + 2);
        queue.submit(std::iter::once(encoder.finish()));
    }
}

/// 新路径：HDR 附件 + 后处理 pass（与三家宿主的接法一致）。
struct HdrPath {
    pipeline: ScenePipeline,
    hdr: HdrTargets,
    _ldr: wgpu::Texture,
    ldr_view: wgpu::TextureView,
    postfx: PostFx,
}

impl HdrPath {
    fn new(device: &wgpu::Device, adapter: &wgpu::Adapter) -> Self {
        let sample_count = supported_sample_count(adapter, HDR_FORMAT);
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let ldr = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bench_hdr_ldr"),
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
        Self {
            pipeline,
            hdr,
            _ldr: ldr,
            ldr_view,
            postfx,
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
            label: Some("bench_hdr_encoder"),
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
            eye,
            SceneLighting::new(scene.ambient, &scene.lights),
            settings.sky,
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
            settings,
        );
        encoder.write_timestamp(stamps, base + 2);
        queue.submit(std::iter::once(encoder.finish()));
    }
}

/// 一轮采样结果（µs/帧，取中位数）。
#[derive(Default, Clone, Copy)]
struct Round {
    /// 场景 pass 段（含 MSAA resolve）。
    scene_us: f64,
    /// 后处理 pass 段；旧路径恒为 0（连着打点）。
    post_us: f64,
    /// CPU 侧"编码 + 提交"（不含任何等待）。
    cpu_us: f64,
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// 跑一轮：`FRAMES` 帧，返回两段 GPU 时长与 CPU 编码的中位数。
fn run_round(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    stamps: &wgpu::QuerySet,
    resolve: &wgpu::Buffer,
    readback: &wgpu::Buffer,
    mut frame: impl FnMut(u32),
) -> Round {
    let mut cpu = Vec::with_capacity(FRAMES);
    for i in 0..FRAMES {
        let start = Instant::now();
        frame(i as u32 * STAMPS_PER_FRAME as u32);
        cpu.push(start.elapsed().as_secs_f64() * 1e6);
    }

    // 整轮一次性 resolve → 拷回 → 等映射。
    let byte_len = (FRAMES * STAMPS_PER_FRAME * 8) as u64;
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench_resolve"),
    });
    encoder.resolve_query_set(
        stamps,
        0..(FRAMES * STAMPS_PER_FRAME) as u32,
        resolve,
        0,
    );
    encoder.copy_buffer_to_buffer(resolve, 0, readback, 0, byte_len);
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv().expect("读回通道断开").expect("映射时间戳缓冲失败");
    let ticks: Vec<u64> = {
        let view = slice.get_mapped_range();
        view.as_chunks::<8>().0.iter().map(|c| u64::from_le_bytes(*c)).collect()
    };
    readback.unmap();

    // 时间戳单位是"tick"，换算系数由队列给出（Vulkan 上是 ns/tick）。
    let period = queue.get_timestamp_period() as f64;
    let mut scene = Vec::with_capacity(FRAMES);
    let mut post = Vec::with_capacity(FRAMES);
    for f in 0..FRAMES {
        let base = f * STAMPS_PER_FRAME;
        scene.push((ticks[base + 1] - ticks[base]) as f64 * period / 1000.0);
        post.push((ticks[base + 2] - ticks[base + 1]) as f64 * period / 1000.0);
    }
    Round {
        scene_us: median(&mut scene),
        post_us: median(&mut post),
        cpu_us: median(&mut cpu),
    }
}

/// 一轮的完整读数（三条路径交替测，见 `postfx_cost` 主流程）。
struct RoundTriple {
    ldr: Round,
    hdr: Round,
    bloom: Round,
}

/// `sky_ibl_cost` 的一轮读数（复用 `Round`，但字段名按天空三档来）。
struct SkyTriple {
    off: Round,
    steady: Round,
    churn: Round,
}

#[test]
#[ignore = "手动运行的 GPU 微基准，不进 CI 门禁"]
fn postfx_cost() {
    let Some((device, queue, adapter)) = gpu() else {
        return;
    };
    let info = adapter.get_info();
    println!(
        "postfx_cost: {} ({:?}, {:?}) {}x{}",
        info.name, info.device_type, info.backend, WIDTH, HEIGHT
    );

    let scene = bench_scene();
    let cache = model_cache(&device, &queue);
    let anim = AnimationDriver::new();
    let matrices = scene.world_matrices();

    // 俯视 3/4 角，把地面与立方体阵框满画面。
    let (eye, view_proj) = bench_camera();

    let mut ldr = LdrPath::new(&device, &adapter);
    let mut hdr = HdrPath::new(&device, &adapter);
    // 第三条路径单独一份实例：Bloom 链按设置惰性建/丢，同一实例上来回切设置
    // 会每轮重建一次链（池化分配 + 绑定组）—— 那测的就是重建开销了。两份
    // 实例各自保持恒定的设置，链在预热期建好、之后只是每帧记录 9 趟 pass。
    let mut bloom = HdrPath::new(&device, &adapter);
    let bloom_settings = RenderConfig {
        bloom: BloomConfig {
            intensity: 0.6,
            ..Default::default()
        },
        ..RenderConfig::default()
    };
    println!(
        "  MSAA: 旧路径 x{}（sRGB 目标）/ 新路径 x{}（HDR 附件）",
        ldr.pipeline.sample_count(),
        hdr.pipeline.sample_count()
    );

    // 预热：时间戳写进一次性查询集（不 resolve），让管线缓存与 GPU 频率
    // 爬到稳态 —— 三条路径交替预热，避免"谁先测谁吃亏"。
    let warmup = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("bench_warmup_stamps"),
        ty: wgpu::QueryType::Timestamp,
        count: (FRAMES * STAMPS_PER_FRAME) as u32,
    });
    let warmup_start = Instant::now();
    while warmup_start.elapsed() < WARMUP_TIME {
        ldr.frame(&device, &queue, &warmup, 0, &scene, &matrices, &cache, &anim, &view_proj, &eye);
        hdr.frame(
            &device, &queue, &warmup, 0, &scene, &matrices, &cache, &anim, &view_proj, &eye,
            RenderConfig::default(),
        );
        bloom.frame(
            &device, &queue, &warmup, 0, &scene, &matrices, &cache, &anim, &view_proj, &eye,
            bloom_settings,
        );
    }
    device.poll(wgpu::Maintain::Wait);

    // 时间戳读回缓冲：resolve 目标要 QUERY_RESOLVE | COPY_DST，读回要 MAP_READ。
    // 每轮新建（本轮的值只属于本轮，避免"没打点的槽位读到上一轮"的隐式依赖）。
    let byte_len = (FRAMES * STAMPS_PER_FRAME * 8) as u64;
    let make_buffers = || {
        (
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bench_stamps_resolve"),
                size: byte_len,
                usage: wgpu::BufferUsages::QUERY_RESOLVE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bench_stamps_readback"),
                size: byte_len,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        )
    };

    let mut triples = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let stamps = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("bench_stamps"),
            ty: wgpu::QueryType::Timestamp,
            count: (FRAMES * STAMPS_PER_FRAME) as u32,
        });
        let (resolve, readback) = make_buffers();

        let l = run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
            ldr.frame(
                &device, &queue, &stamps, base, &scene, &matrices, &cache, &anim, &view_proj, &eye,
            );
        });
        let h = run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
            hdr.frame(
                &device, &queue, &stamps, base, &scene, &matrices, &cache, &anim, &view_proj, &eye,
                RenderConfig::default(),
            );
        });
        let b = run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
            bloom.frame(
                &device, &queue, &stamps, base, &scene, &matrices, &cache, &anim, &view_proj, &eye,
                bloom_settings,
            );
        });
        println!(
            "  第 {} 轮: 旧 场景{:.1}/后处理{:.1}/CPU{:.1} | ACES 场景{:.1}/后处理{:.1}/CPU{:.1} | Bloom 场景{:.1}/后处理{:.1}/CPU{:.1}（µs/帧）",
            triples.len() + 1,
            l.scene_us,
            l.post_us,
            l.cpu_us,
            h.scene_us,
            h.post_us,
            h.cpu_us,
            b.scene_us,
            b.post_us,
            b.cpu_us,
        );
        triples.push(RoundTriple {
            ldr: l,
            hdr: h,
            bloom: b,
        });
    }

    // 汇总：各段再取一次轮间中位数（轮内已是中位数，双保险抗单轮抖动）。
    let pick = |f: fn(&Round) -> f64, path: fn(&RoundTriple) -> &Round| {
        let mut v: Vec<f64> = triples.iter().map(|p| f(path(p))).collect();
        median(&mut v)
    };
    let ldr_scene = pick(|r| r.scene_us, |p| &p.ldr);
    let ldr_post = pick(|r| r.post_us, |p| &p.ldr);
    let ldr_cpu = pick(|r| r.cpu_us, |p| &p.ldr);
    let hdr_scene = pick(|r| r.scene_us, |p| &p.hdr);
    let hdr_post = pick(|r| r.post_us, |p| &p.hdr);
    let hdr_cpu = pick(|r| r.cpu_us, |p| &p.hdr);
    let bloom_scene = pick(|r| r.scene_us, |p| &p.bloom);
    let bloom_post = pick(|r| r.post_us, |p| &p.bloom);
    let bloom_cpu = pick(|r| r.cpu_us, |p| &p.bloom);

    // P2.5 的教训：基准必须断言"活真干了" —— 如果时间戳全 0（某后端没实现
    // 写入），这里的数字就成了"空调度"的假快，必须自己把它打红。
    assert!(
        ldr_scene > 0.0 && hdr_scene > 0.0,
        "时间戳读数为 0：适配器是不是没真正写入查询？读数不可信"
    );
    assert!(
        hdr_post > 0.0,
        "后处理段读数为 0：全屏 pass 没被执行？"
    );
    // Bloom 链真被记录了才会比纯 ACES 的后处理段贵（9 趟额外的全屏 pass）。
    // 阈值放宽到 1 µs：只要链缺席，差额就是 0，这条必红。
    assert!(
        bloom_post > hdr_post + 1.0,
        "Bloom 路径的后处理段与纯 ACES 无明显差别（{bloom_post:.1} vs {hdr_post:.1} µs）：\
         链是不是没跑？"
    );
    assert!(
        (bloom_scene - hdr_scene).abs() < 5.0,
        "Bloom 路径的场景段应当与纯 ACES 几乎一致（都是同一段场景渲染）：\
         {bloom_scene:.1} vs {hdr_scene:.1} µs"
    );

    println!(
        "postfx_cost[中位数]: 旧 LDR 直渲 场景 {ldr_scene:.1} + 后处理 {ldr_post:.1} = {:.1} µs/帧（CPU {ldr_cpu:.1}）",
        ldr_scene + ldr_post
    );
    println!(
        "postfx_cost[中位数]: HDR + ACES 场景 {hdr_scene:.1} + 后处理 {hdr_post:.1} = {:.1} µs/帧（CPU {hdr_cpu:.1}）",
        hdr_scene + hdr_post
    );
    println!(
        "postfx_cost[中位数]: HDR + ACES + Bloom 场景 {bloom_scene:.1} + 后处理 {bloom_post:.1} = {:.1} µs/帧（CPU {bloom_cpu:.1}）",
        bloom_scene + bloom_post
    );
    let delta = (hdr_scene + hdr_post) - (ldr_scene + ldr_post);
    println!(
        "postfx_cost[差额]: ACES 相对旧路径 {delta:+.1} µs/帧（{:+.1}%），其中后处理 pass {hdr_post:.1} µs、\
         场景段差 {:+.1} µs（fp16 附件带宽 + fp16 resolve）",
        delta / (ldr_scene + ldr_post) * 100.0,
        hdr_scene - ldr_scene
    );
    let bloom_delta = (bloom_scene + bloom_post) - (hdr_scene + hdr_post);
    println!(
        "postfx_cost[差额]: Bloom 相对 ACES {bloom_delta:+.1} µs/帧（{:+.1}%）—— \
         亮部提取 + 降采样 + 6 趟可分离模糊全在半分辨率以下",
        bloom_delta / (hdr_scene + hdr_post) * 100.0
    );
}

// ---------------------------------------------------------------------------
// 天空 + IBL 的每帧成本
// ---------------------------------------------------------------------------

/// 只跑一帧的单帧读数（口径与 `run_round` 相同，噪声未做中位数平滑）。
///
/// 用途只有一个："打开天空开关的那一帧"（BRDF LUT 生成 + 首次预过滤 +
/// 球谐投影）——那是一次性成本，不能混进稳态轮次里取中位数。
/// `cpu_us` 不测（写 0）；调用方自己知道这一帧的语义。
fn run_single_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    frame: impl FnOnce(&wgpu::QuerySet, u32),
) -> Round {
    let stamps = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("bench_single_stamps"),
        ty: wgpu::QueryType::Timestamp,
        count: STAMPS_PER_FRAME as u32,
    });
    frame(&stamps, 0);

    // 单帧只有 24 字节：不需要整轮口径那种 256 对齐（那是 resolve 目标偏移的
    // 要求，这里是偏移 0 + 普通 copy）。
    let resolve = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bench_single_resolve"),
        size: 256,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bench_single_readback"),
        size: 256,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench_single_resolve_encoder"),
    });
    encoder.resolve_query_set(&stamps, 0..STAMPS_PER_FRAME as u32, &resolve, 0);
    encoder.copy_buffer_to_buffer(&resolve, 0, &readback, 0, (STAMPS_PER_FRAME * 8) as u64);
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv().expect("读回通道断开").expect("映射时间戳缓冲失败");
    let ticks: Vec<u64> = {
        let view = slice.get_mapped_range();
        view.as_chunks::<8>().0.iter().map(|c| u64::from_le_bytes(*c)).collect()
    };
    readback.unmap();

    let period = queue.get_timestamp_period() as f64;
    Round {
        scene_us: (ticks[1] - ticks[0]) as f64 * period / 1000.0,
        post_us: (ticks[2] - ticks[1]) as f64 * period / 1000.0,
        cpu_us: 0.0,
    }
}

/// 重建档的每帧设置：`sun_size` 在两个相邻值之间来回切 ⇒ uniform key 每帧都变。
///
/// 为什么不用"每帧转向光源"来模拟昼夜循环：重建路径只认"key 变了"，重建
/// 内容与哪个字段变**无关**（同样 30 趟 pass）；挑一个不牵连场景其他环节
/// （灯光打包、阴影）的字段，读数里就只有天空自己的账。代价与昼夜循环同量级。
fn churn_settings(frame: u32) -> RenderConfig {
    let mut sky = SkyConfig {
        enabled: true,
        ..Default::default()
    };
    sky.sun_size = 4.0 + (frame % 2) as f32 * 0.001;
    RenderConfig {
        sky,
        ..RenderConfig::default()
    }
}

/// 天空 + IBL 微基准：三档（关 / 稳态开 / 每帧重建）。
///
/// # 三档比什么
///
/// | 档 | 组成 | 读数回答的问题 |
/// |---|---|---|
/// | 关 | HDR + ACES，天空默认关（uniform 照写、开关位 0、不建内容） | 基线 |
/// | 稳态 | 天空开、参数恒定（key 不变） | 每帧**增量**：天空盒全屏 pass + 场景片元的 IBL 采样（环境立方体 + BRDF LUT） |
/// | 重建 | 天空开、每帧微调 `sun_size`（key 每帧都变） | **最坏情况**：球谐重投影（CPU 1024 点）+ 30 趟预过滤 pass |
///
/// 稳态档在预热后单独测**首帧**（BRDF LUT 生成 + 首次预过滤 + 球谐）——
/// 那是"打开天空开关的那一帧"的一次性账；后续每帧只剩 80 字节 uniform 写入，
/// 这是"固定分配、按 key 重建"设计的直接读数。
///
/// # 断言（P2.5 纪律：基准必须证明"活真干了"）
///
/// - 稳态场景段 > 关 + 1 µs（天空盒与 IBL 采样真在跑）；
/// - 重建场景段 > 稳态 + 10 µs（30 趟预过滤真在重跑）；
/// - 重建 CPU > 稳态 + 5 µs（球谐投影与 30 趟 pass 的编码真在跑）。
///
/// key 因实现回归不再变化时，后两条必红（这正是它们要守的东西）。
#[test]
#[ignore = "手动运行的 GPU 微基准，不进 CI 门禁"]
fn sky_ibl_cost() {
    /// 只热身管线与 GPU 频率（让"首帧天空"的读数少受冷启动污染）。
    const PIPELINE_WARMUP: std::time::Duration = std::time::Duration::from_millis(500);

    let Some((device, queue, adapter)) = gpu() else {
        return;
    };
    let info = adapter.get_info();
    println!(
        "sky_ibl_cost: {} ({:?}, {:?}) {}x{}",
        info.name, info.device_type, info.backend, WIDTH, HEIGHT
    );

    let scene = bench_scene();
    let cache = model_cache(&device, &queue);
    let anim = AnimationDriver::new();
    let matrices = scene.world_matrices();
    let (eye, view_proj) = bench_camera();

    let off = RenderConfig::default();
    let sky_on = RenderConfig {
        sky: SkyConfig {
            enabled: true,
            ..Default::default()
        },
        ..RenderConfig::default()
    };

    // [0] 关 / [1] 稳态 / [2] 重建 —— 与 Bloom 档同款纪律：重建档的设置每帧
    // 都变，单独一份实例（那份实例的 SkyResources 一直在重建，别搅进稳态档）。
    let mut paths = [
        HdrPath::new(&device, &adapter),
        HdrPath::new(&device, &adapter),
        HdrPath::new(&device, &adapter),
    ];
    println!(
        "  MSAA: x{}（HDR 附件）；天空：稳态档预过滤 128²/5 mip、BRDF LUT 128²",
        paths[0].pipeline.sample_count()
    );

    // -- 阶段 A：管线与频率热身。稳态档此刻还用着"关"，为的是让"首帧天空"
    // 的语义 = "已经渲过若干帧、然后打开天空开关的那一帧"（管线已热）。
    let warmup = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("sky_bench_warmup_stamps"),
        ty: wgpu::QueryType::Timestamp,
        count: (FRAMES * STAMPS_PER_FRAME) as u32,
    });
    let warmup_start = Instant::now();
    while warmup_start.elapsed() < PIPELINE_WARMUP {
        paths[0].frame(&device, &queue, &warmup, 0, &scene, &matrices, &cache, &anim, &view_proj, &eye, off);
        paths[1].frame(&device, &queue, &warmup, 0, &scene, &matrices, &cache, &anim, &view_proj, &eye, off);
        paths[2].frame(&device, &queue, &warmup, 0, &scene, &matrices, &cache, &anim, &view_proj, &eye, churn_settings(0));
    }
    device.poll(wgpu::Maintain::Wait);

    // -- 阶段 B："打开天空开关的那一帧"（BRDF LUT + 首次预过滤 + 球谐）。
    let first = run_single_frame(&device, &queue, |stamps, base| {
        paths[1].frame(
            &device, &queue, stamps, base, &scene, &matrices, &cache, &anim, &view_proj, &eye,
            sky_on,
        );
    });
    println!(
        "sky_ibl_cost[首帧]: 场景 {:.1} + 后处理 {:.1} µs —— 打开天空开关的那一帧\
         （BRDF LUT 生成 + 首次预过滤 30 趟 + 球谐投影；单帧读数，含冷态开销）",
        first.scene_us, first.post_us
    );

    // -- 阶段 C：稳态热身（三档各就各位）。
    let warmup_start = Instant::now();
    while warmup_start.elapsed() < WARMUP_TIME {
        paths[0].frame(&device, &queue, &warmup, 0, &scene, &matrices, &cache, &anim, &view_proj, &eye, off);
        paths[1].frame(&device, &queue, &warmup, 0, &scene, &matrices, &cache, &anim, &view_proj, &eye, sky_on);
        paths[2].frame(&device, &queue, &warmup, 0, &scene, &matrices, &cache, &anim, &view_proj, &eye, churn_settings(0));
    }
    device.poll(wgpu::Maintain::Wait);

    let byte_len = (FRAMES * STAMPS_PER_FRAME * 8) as u64;
    let make_buffers = || {
        (
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("sky_bench_stamps_resolve"),
                size: byte_len,
                usage: wgpu::BufferUsages::QUERY_RESOLVE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("sky_bench_stamps_readback"),
                size: byte_len,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        )
    };

    let mut triples = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let stamps = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("sky_bench_stamps"),
            ty: wgpu::QueryType::Timestamp,
            count: (FRAMES * STAMPS_PER_FRAME) as u32,
        });
        let (resolve, readback) = make_buffers();
        let o = run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
            paths[0].frame(&device, &queue, &stamps, base, &scene, &matrices, &cache, &anim, &view_proj, &eye, off);
        });
        let s = run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
            paths[1].frame(&device, &queue, &stamps, base, &scene, &matrices, &cache, &anim, &view_proj, &eye, sky_on);
        });
        let c = run_round(&device, &queue, &stamps, &resolve, &readback, |base| {
            let frame = base / STAMPS_PER_FRAME as u32;
            paths[2].frame(&device, &queue, &stamps, base, &scene, &matrices, &cache, &anim, &view_proj, &eye, churn_settings(frame));
        });
        println!(
            "  第 {} 轮: 关 场景{:.1}/CPU{:.1} | 稳态 场景{:.1}/CPU{:.1} | 重建 场景{:.1}/CPU{:.1}（µs/帧）",
            triples.len() + 1,
            o.scene_us,
            o.cpu_us,
            s.scene_us,
            s.cpu_us,
            c.scene_us,
            c.cpu_us,
        );
        triples.push(SkyTriple {
            off: o,
            steady: s,
            churn: c,
        });
    }

    let pick = |f: fn(&Round) -> f64, path: fn(&SkyTriple) -> &Round| {
        let mut v: Vec<f64> = triples.iter().map(|p| f(path(p))).collect();
        median(&mut v)
    };
    let off_scene = pick(|r| r.scene_us, |p| &p.off);
    let off_post = pick(|r| r.post_us, |p| &p.off);
    let off_cpu = pick(|r| r.cpu_us, |p| &p.off);
    let steady_scene = pick(|r| r.scene_us, |p| &p.steady);
    let steady_post = pick(|r| r.post_us, |p| &p.steady);
    let steady_cpu = pick(|r| r.cpu_us, |p| &p.steady);
    let churn_scene = pick(|r| r.scene_us, |p| &p.churn);
    let churn_post = pick(|r| r.post_us, |p| &p.churn);
    let churn_cpu = pick(|r| r.cpu_us, |p| &p.churn);

    assert!(
        off_scene > 0.0 && steady_scene > 0.0,
        "时间戳读数为 0：适配器是不是没真正写入查询？读数不可信"
    );
    assert!(
        steady_scene > off_scene + 1.0,
        "稳态档的场景段与关天空无差别（{steady_scene:.1} vs {off_scene:.1} µs）：\
         天空盒 / IBL 采样是不是没跑？"
    );
    assert!(
        churn_scene > steady_scene + 10.0,
        "重建档的场景段与稳态无差别（{churn_scene:.1} vs {steady_scene:.1} µs）：\
         预过滤是不是没在 key 变化时重跑？"
    );
    assert!(
        churn_cpu > steady_cpu + 5.0,
        "重建档的 CPU 编码与稳态无差别（{churn_cpu:.1} vs {steady_cpu:.1} µs）：\
         球谐投影 / 30 趟 pass 的编码是不是没跑？"
    );

    println!(
        "sky_ibl_cost[中位数]: 关 场景 {off_scene:.1} + 后处理 {off_post:.1} = {:.1} µs/帧（CPU {off_cpu:.1}）",
        off_scene + off_post
    );
    println!(
        "sky_ibl_cost[中位数]: 稳态开 场景 {steady_scene:.1} + 后处理 {steady_post:.1} = {:.1} µs/帧（CPU {steady_cpu:.1}）",
        steady_scene + steady_post
    );
    println!(
        "sky_ibl_cost[中位数]: 每帧重建 场景 {churn_scene:.1} + 后处理 {churn_post:.1} = {:.1} µs/帧（CPU {churn_cpu:.1}）",
        churn_scene + churn_post
    );
    let steady_delta = steady_scene - off_scene;
    println!(
        "sky_ibl_cost[差额]: 稳态开相对关 {steady_delta:+.1} µs/帧（场景段，{:+.1}%）= 天空盒 pass + IBL 采样\
         （后处理段 {:.1} vs {:.1} µs，应几乎相等）",
        steady_delta / off_scene * 100.0,
        steady_post,
        off_post
    );
    let churn_delta = churn_scene - steady_scene;
    println!(
        "sky_ibl_cost[差额]: 每帧重建相对稳态 {churn_delta:+.1} µs/帧（{:+.1}%）—— 球谐投影（CPU 1024 点）+ 30 趟预过滤 pass；\
         首帧额外 {:.1} µs（BRDF LUT + 冷态）",
        churn_delta / steady_scene * 100.0,
        first.scene_us - churn_scene
    );
}
