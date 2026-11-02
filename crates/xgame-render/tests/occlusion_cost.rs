//! 微基准：时域硬件遮挡查询的收益与固定税。
//!
//! 默认 ignore；手动跑：
//! `cargo test -p xgame-render --release --test occlusion_cost -- --ignored --nocapture`
//!
//! 两个场景都含 2048 个同网格实例、同一批次，只改变遮挡率：
//! - 高遮挡：每 128 个实例一层，16 层沿视线重叠；
//! - 无遮挡：2048 个实例铺在面向相机的网格上，每组都有可见样本。
//!
//! 开 / 关遮挡逐轮换序，GPU 用时间戳量完整场景段，CPU 量编码 + submit。
//! 绝对值只可同机背靠背比较；基准同时检查异步可见实例数，防止“路径没跑却
//! 得出更快”的假数据。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use xgame_core::{Entity, Light, Scene};
use xgame_math::glam::{Mat4, Vec3};
use xgame_render::animation::AnimationDriver;
use xgame_render::gltf_loader::GltfMesh;
use xgame_render::mesh::BuiltinMesh;
use xgame_render::postfx::{HdrTargets, HDR_FORMAT};
use xgame_render::scene_pipeline::{
    supported_sample_count, FrameStats, SceneLighting, ScenePipeline,
};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const ACTORS: usize = 2048;
const GROUP: usize = 128;
const FRAMES: usize = 64;
const ROUNDS: usize = 8;
const STAMPS_PER_FRAME: usize = 2;
const WARMUP: Duration = Duration::from_millis(1500);

const _: () = assert!((FRAMES * STAMPS_PER_FRAME * 8).is_multiple_of(256));
const _: () = assert!(ACTORS.is_multiple_of(GROUP));

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
    let queue_resolve = adapter
        .get_downlevel_capabilities()
        .flags
        .contains(wgpu::DownlevelFlags::NONBLOCKING_QUERY_RESOLVE);
    if !adapter.features().contains(required) || !queue_resolve {
        eprintln!("跳过 occlusion_cost：适配器缺时间戳或队列时间线查询解析能力");
        return None;
    }
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("occlusion_cost device"),
            required_features: required,
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        },
        None,
    ))
    .ok()?;
    Some((device, queue, adapter))
}

fn scene(occluded: bool, mesh: BuiltinMesh) -> Scene {
    let mut scene = Scene::new(if occluded {
        "occlusion-cost-hidden"
    } else {
        "occlusion-cost-open"
    });
    scene.ambient = [0.08; 3];
    scene.lights = vec![Light::directional("sun", [0.4, -1.0, -0.3], [1.0; 3], 2.0)];
    scene.lights[0].casts_shadow = false;

    for i in 0..ACTORS {
        let mut cube = Entity::new(format!("cube-{i}"));
        cube.model = mesh.uri().to_owned();
        cube.transform.position = if occluded {
            // 同一查询组的 128 个实例完全重合；组与组沿视线向后排。
            Vec3::new(0.0, 0.0, -4.0 * (i / GROUP) as f32)
        } else {
            let x = (i % 46) as f32 - 22.5;
            let y = (i / 46) as f32 - 22.0;
            Vec3::new(x * 1.5, y * 1.5, 0.0)
        };
        scene.add_entity(cube);
    }
    scene
}

fn camera() -> (Vec3, Mat4) {
    let eye = Vec3::new(0.0, 0.0, 150.0);
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
}

impl BenchPath {
    fn new(device: &wgpu::Device, adapter: &wgpu::Adapter) -> Self {
        let sample_count = supported_sample_count(adapter, HDR_FORMAT);
        let mut pipeline =
            ScenePipeline::new_with_occlusion_support(device, HDR_FORMAT, sample_count, true);
        pipeline.set_frustum_culling(false);
        assert!(pipeline.occlusion_culling_supported());
        Self {
            pipeline,
            hdr: HdrTargets::new(device, "bench_occlusion", WIDTH, HEIGHT, sample_count),
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
        eye: &Vec3,
        view_proj: &Mat4,
        enabled: bool,
    ) -> FrameStats {
        self.pipeline.set_occlusion_culling(enabled);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench_occlusion_encoder"),
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
            xgame_core::SkyConfig::default(),
            1.0,
            xgame_core::ShadowConfig::default(),
            &[],
        );
        encoder.write_timestamp(stamps, base + 1);
        queue.submit(std::iter::once(encoder.finish()));
        self.pipeline.frame_stats()
    }
}

#[derive(Clone, Copy, Default)]
struct Timing {
    gpu_us: f64,
    cpu_us: f64,
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

#[allow(clippy::too_many_arguments)]
fn run_round(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    path: &mut BenchPath,
    scene: &Scene,
    matrices: &[Mat4],
    cache: &HashMap<String, GltfMesh>,
    anim: &AnimationDriver,
    eye: &Vec3,
    view_proj: &Mat4,
    enabled: bool,
) -> (Timing, FrameStats, Option<usize>) {
    let stamps = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("bench_occlusion_stamps"),
        ty: wgpu::QueryType::Timestamp,
        count: (FRAMES * STAMPS_PER_FRAME) as u32,
    });
    let byte_len = (FRAMES * STAMPS_PER_FRAME * 8) as u64;
    let resolve = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bench_occlusion_resolve"),
        size: byte_len,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bench_occlusion_readback"),
        size: byte_len,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut cpu = Vec::with_capacity(FRAMES);
    let mut stats = FrameStats::default();
    for i in 0..FRAMES {
        let start = Instant::now();
        stats = path.frame(
            device,
            queue,
            &stamps,
            (i * STAMPS_PER_FRAME) as u32,
            scene,
            matrices,
            cache,
            anim,
            eye,
            view_proj,
            enabled,
        );
        cpu.push(start.elapsed().as_secs_f64() * 1e6);
    }

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench_occlusion_timestamp_resolve"),
    });
    encoder.resolve_query_set(&stamps, 0..(FRAMES * STAMPS_PER_FRAME) as u32, &resolve, 0);
    encoder.copy_buffer_to_buffer(&resolve, 0, &readback, 0, byte_len);
    queue.submit(std::iter::once(encoder.finish()));
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv()
        .expect("时间戳读回通道断开")
        .expect("时间戳映射失败");
    let ticks: Vec<u64> = {
        let mapped = slice.get_mapped_range();
        mapped
            .as_chunks::<8>()
            .0
            .iter()
            .map(|bytes| u64::from_le_bytes(*bytes))
            .collect()
    };
    readback.unmap();
    let period = queue.get_timestamp_period() as f64;
    let mut gpu = (0..FRAMES)
        .map(|frame| {
            let base = frame * STAMPS_PER_FRAME;
            (ticks[base + 1] - ticks[base]) as f64 * period / 1000.0
        })
        .collect::<Vec<_>>();
    let visible = if enabled {
        path.pipeline.occlusion_visible_instances()
    } else {
        None
    };
    (
        Timing {
            gpu_us: median(&mut gpu),
            cpu_us: median(&mut cpu),
        },
        stats,
        visible,
    )
}

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
) -> (Timing, Timing, FrameStats, usize) {
    let warmup = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("bench_occlusion_warmup"),
        ty: wgpu::QueryType::Timestamp,
        count: 2,
    });
    let start = Instant::now();
    while start.elapsed() < WARMUP {
        for enabled in [false, true] {
            path.frame(
                device, queue, &warmup, 0, scene, matrices, cache, anim, eye, view_proj, enabled,
            );
        }
    }
    device.poll(wgpu::Maintain::Wait);

    let mut rounds = Vec::with_capacity(ROUNDS);
    for round in 0..ROUNDS {
        let (off, on) = if round % 2 == 0 {
            (
                run_round(
                    device, queue, path, scene, matrices, cache, anim, eye, view_proj, false,
                ),
                run_round(
                    device, queue, path, scene, matrices, cache, anim, eye, view_proj, true,
                ),
            )
        } else {
            let on = run_round(
                device, queue, path, scene, matrices, cache, anim, eye, view_proj, true,
            );
            let off = run_round(
                device, queue, path, scene, matrices, cache, anim, eye, view_proj, false,
            );
            (off, on)
        };
        println!(
            "  {label} 第 {} 轮: 关 GPU {:.1}/CPU {:.1} | 开 GPU {:.1}/CPU {:.1} µs，实例 {:?}",
            round + 1,
            off.0.gpu_us,
            off.0.cpu_us,
            on.0.gpu_us,
            on.0.cpu_us,
            on.2,
        );
        rounds.push((off.0, on.0));
    }

    let select = |enabled: bool, field: fn(Timing) -> f64| {
        let mut values = rounds
            .iter()
            .map(|(off, on)| field(if enabled { *on } else { *off }))
            .collect::<Vec<_>>();
        median(&mut values)
    };
    let off = Timing {
        gpu_us: select(false, |t| t.gpu_us),
        cpu_us: select(false, |t| t.cpu_us),
    };
    let on = Timing {
        gpu_us: select(true, |t| t.gpu_us),
        cpu_us: select(true, |t| t.cpu_us),
    };
    // 最后一轮为抵消热序可能以“关”收尾；补一段不计时的开启档，确保异步诊断
    // 对应的就是本场景最新查询，而不是上一轮遗留值。
    let diagnostics = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("bench_occlusion_diagnostics"),
        ty: wgpu::QueryType::Timestamp,
        count: 2,
    });
    let mut stats = FrameStats::default();
    for _ in 0..16 {
        stats = path.frame(
            device,
            queue,
            &diagnostics,
            0,
            scene,
            matrices,
            cache,
            anim,
            eye,
            view_proj,
            true,
        );
        if stats.occlusion_query_groups == 0
            || path.pipeline.occlusion_visible_instances().is_some()
        {
            break;
        }
    }
    if stats.occlusion_query_groups > 0 && path.pipeline.occlusion_visible_instances().is_none() {
        // 只在 ignored 基准诊断兜底等待；生产帧始终只做非阻塞 Poll。
        device.poll(wgpu::Maintain::Wait);
        stats = path.frame(
            device,
            queue,
            &diagnostics,
            0,
            scene,
            matrices,
            cache,
            anim,
            eye,
            view_proj,
            true,
        );
    }
    let visible = if stats.occlusion_query_groups == 0 {
        assert!(!stats.occlusion_active, "自动门槛跳过时不能冒充已剔除");
        ACTORS
    } else {
        path.pipeline
            .occlusion_visible_instances()
            .expect("遮挡查询的异步诊断应已完成")
    };
    println!(
        "  {label}[中位数]: 关 GPU {:.1}/CPU {:.1} | 开 GPU {:.1}/CPU {:.1} µs；实例 {ACTORS}→{visible}，查询 {}",
        off.gpu_us,
        off.cpu_us,
        on.gpu_us,
        on.cpu_us,
        stats.occlusion_query_groups,
    );
    (off, on, stats, visible)
}

#[test]
#[ignore = "手动运行的 GPU 微基准，不进 CI 门禁"]
fn occlusion_cost() {
    let Some((device, queue, adapter)) = gpu() else {
        return;
    };
    let info = adapter.get_info();
    println!(
        "occlusion_cost: {} ({:?}, {:?}) {}x{}，{ACTORS} 实例",
        info.name, info.device_type, info.backend, WIDTH, HEIGHT
    );
    let mut path = BenchPath::new(&device, &adapter);
    println!(
        "  MSAA x{}，每组 {GROUP} 实例",
        path.pipeline.sample_count()
    );
    let cache = [BuiltinMesh::Cube, BuiltinMesh::Sphere]
        .into_iter()
        .map(|mesh| {
            (
                mesh.uri().to_owned(),
                GltfMesh::from_mesh_data(&device, &queue, &mesh.build(), mesh.label()),
            )
        })
        .collect::<HashMap<_, _>>();
    let anim = AnimationDriver::new();
    let (eye, view_proj) = camera();

    for (mesh_label, mesh) in [
        ("低面立方体", BuiltinMesh::Cube),
        ("高面球体", BuiltinMesh::Sphere),
    ] {
        let hidden = scene(true, mesh);
        let hidden_matrices = hidden.world_matrices();
        let hidden_label = format!("{mesh_label}/高遮挡");
        let (hidden_off, hidden_on, hidden_stats, hidden_visible) = measure(
            &device,
            &queue,
            &mut path,
            &hidden,
            &hidden_matrices,
            &cache,
            &anim,
            &eye,
            &view_proj,
            &hidden_label,
        );
        if mesh == BuiltinMesh::Cube {
            assert_eq!(hidden_visible, ACTORS, "低面批次必须被自动门槛直通");
            assert_eq!(hidden_stats.occlusion_query_groups, 0);
            assert!(!hidden_stats.occlusion_active);
        } else {
            // 球体的保守 AABB 四角伸出球面轮廓，近处若干组会得到“可能可见”；
            // 只许多画，不许按理想球轮廓误删。基准仍要求至少剔掉四分之一。
            assert!(
                hidden_visible < ACTORS * 3 / 4,
                "保守 AABB 没有产生足够遮挡：{hidden_visible}/{ACTORS}"
            );
            assert!(hidden_stats.occlusion_query_groups > 0);
            assert!(hidden_stats.occlusion_active, "高遮挡高面场景必须启用压缩");
        }

        let open = scene(false, mesh);
        let open_matrices = open.world_matrices();
        let open_label = format!("{mesh_label}/无遮挡");
        let (open_off, open_on, open_stats, open_visible) = measure(
            &device,
            &queue,
            &mut path,
            &open,
            &open_matrices,
            &cache,
            &anim,
            &eye,
            &view_proj,
            &open_label,
        );
        assert_eq!(open_visible, ACTORS, "负对照里每组都应可见");
        assert!(
            !open_stats.occlusion_active,
            "无遮挡场景必须自动保持直接绘制"
        );
        if mesh == BuiltinMesh::Sphere {
            assert!(
                open_stats.occlusion_query_groups > 0,
                "高面负对照必须真的评估过查询结果"
            );
        }
        assert!(hidden_off.gpu_us > 0.0 && hidden_on.gpu_us > 0.0);
        assert!(open_off.gpu_us > 0.0 && open_on.gpu_us > 0.0);
        println!(
            "occlusion_cost[{mesh_label}总结]: 高遮挡 GPU {:.1}→{:.1} ({:+.1}%) / CPU {:.1}→{:.1}；无遮挡 GPU {:.1}→{:.1} ({:+.1}%) / CPU {:.1}→{:.1} µs",
            hidden_off.gpu_us,
            hidden_on.gpu_us,
            (hidden_on.gpu_us - hidden_off.gpu_us) / hidden_off.gpu_us * 100.0,
            hidden_off.cpu_us,
            hidden_on.cpu_us,
            open_off.gpu_us,
            open_on.gpu_us,
            (open_on.gpu_us - open_off.gpu_us) / open_off.gpu_us * 100.0,
            open_off.cpu_us,
            open_on.cpu_us,
        );
    }
}
