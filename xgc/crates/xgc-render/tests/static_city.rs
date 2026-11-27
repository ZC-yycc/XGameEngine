//! S1 Static City 中档基线。Release 下串行单独运行，预算见框架设计 §13.2。
//! `cargo test -p xgc-render --release --test static_city -- --ignored --nocapture --test-threads=1`

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use xgc_render::{
    build_draw_list, camera_view_projection, transform_point, DrawList, GpuContext, MeshCache,
    QualityPreset, RenderQuality, RenderSettings, RenderStats, Renderer, SHADOW_RESOLUTION,
};
use xgc_world::{
    Camera, GlobalTransform, LightKind, Material, MeshRenderer, SceneEnvironment, SceneLight,
    Transform, World,
};

const COLUMNS: usize = 500;
const ROWS: usize = 400;
const INSTANCES: usize = 200_000;
const POINT_LIGHTS: usize = 8;
// 现有实现中 lights.rs 的 uniform 和 shadow_csm.rs 的固定层数；渲染统计没有这两个计数。
const PACKED_LIGHT_LIMIT: usize = 8;
const CSM_CASCADES: usize = 4;
const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const MODEL: &str = "builtin://cube";
const DEFAULT_WARMUP: usize = 3;
const DEFAULT_SAMPLES: usize = 20;
const ALLOCATION_SAMPLES: usize = 3;
const MATERIAL_COLORS: [[f32; 4]; 8] = [
    [0.75, 0.25, 0.125, 1.0],
    [0.75, 0.5, 0.125, 1.0],
    [0.75, 0.75, 0.25, 1.0],
    [0.25, 0.625, 0.125, 1.0],
    [0.125, 0.625, 0.625, 1.0],
    [0.25, 0.375, 0.75, 1.0],
    [0.625, 0.25, 0.75, 1.0],
    [0.625, 0.625, 0.625, 1.0],
];

thread_local! {
    static ALLOCATIONS: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
}

struct CountingAllocator;

fn count(pointer: *mut u8, bytes: usize) -> *mut u8 {
    if !pointer.is_null() {
        let _ = ALLOCATIONS.try_with(|active| {
            if let Some((calls, total)) = active.get() {
                active.set(Some((calls + 1, total + bytes as u64)));
            }
        });
    }
    pointer
}

// SAFETY: 原封转发指针与布局给 System。计数使用线程本地 Cell，不分配或解引用指针。
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

struct City {
    root: PathBuf,
    materials: [Material; MATERIAL_COLORS.len()],
}

impl City {
    fn write_assets() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tmp/xgc-s1")
            .join(format!("assets-{}", std::process::id()));
        fs::create_dir_all(root.join("Materials")).unwrap();
        let materials = std::array::from_fn(|index| {
            let asset = format!("Materials/building-{index}.mat");
            let [r, g, b, a] = MATERIAL_COLORS[index];
            let roughness = 0.25 + index as f32 / 16.0;
            let metallic = index as f32 / 16.0;
            fs::write(
                root.join(&asset),
                format!(
                    "(base_color: ({r:?}, {g:?}, {b:?}, {a:?}), metallic: {metallic:?}, roughness: {roughness:?})"
                ),
            )
            .unwrap();
            Material {
                asset,
                roughness,
                ..Material::default()
            }
        });
        Self { root, materials }
    }

    fn world(&self) -> World {
        let mut world = World::new();
        let half_pitch = -0.14_f32;
        let camera = Transform {
            position: [0.0, 10.0, 68.0],
            rotation: [half_pitch.sin(), 0.0, 0.0, half_pitch.cos()],
            ..Transform::default()
        };
        world.spawn((
            camera,
            GlobalTransform::from_local(&camera),
            Camera {
                fov_y_deg: 65.0,
                near: 0.25,
                far: 220.0,
                active: true,
            },
        ));
        for z in 0..ROWS {
            for x in 0..COLUMNS {
                let material = (x + 3 * z) % self.materials.len();
                let height = 0.45 + ((x * 13 + z * 17) % 19) as f32 * 0.125;
                let local = Transform {
                    position: [
                        (x as f32 - 250.0) * 0.32,
                        height * 0.5,
                        z as f32 * 0.32 - 65.0,
                    ],
                    scale: [0.23, height, 0.23],
                    ..Transform::default()
                };
                world.spawn((
                    local,
                    GlobalTransform::from_local(&local),
                    MeshRenderer {
                        model: MODEL.into(),
                    },
                    self.materials[material].clone(),
                ));
            }
        }
        let mut lights = vec![SceneLight {
            casts_shadow: true,
            ..SceneLight::directional("S1 Sun", [0.6, -1.0, 0.45], [1.0, 0.94, 0.86], 0.9)
        }];
        lights.extend((0..POINT_LIGHTS).map(|index| SceneLight {
            name: format!("S1 Point {}", index + 1),
            kind: LightKind::Point,
            position: [
                (index % 4) as f32 * 18.0 - 27.0,
                3.0,
                44.0 - (index / 4) as f32 * 42.0,
            ],
            color: [0.65, 0.78, 1.0],
            intensity: 1.25,
            range: 40.0,
            casts_shadow: false,
            ..SceneLight::default()
        }));
        world.set_environment(SceneEnvironment {
            lights,
            ambient: [0.08, 0.09, 0.12],
        });
        world
    }

    fn assert_extraction(&self, draw_list: &DrawList) -> [usize; MATERIAL_COLORS.len()] {
        assert_eq!(draw_list.len(), INSTANCES);
        let mut material_counts = [0; MATERIAL_COLORS.len()];
        for item in &draw_list.items {
            assert_eq!(item.model, MODEL);
            assert!(item.has_material);
            assert!(item.pose.is_none() && item.animation.is_none());
            let index = self
                .materials
                .iter()
                .position(|mat| mat.asset == item.material_asset)
                .expect("DrawList 引用必须指向 S1 的八份材质之一");
            assert_eq!(item.roughness, self.materials[index].roughness);
            material_counts[index] += 1;
        }
        assert_eq!(
            material_counts,
            [INSTANCES / MATERIAL_COLORS.len(); MATERIAL_COLORS.len()]
        );
        material_counts
    }

    fn assert_assets(&self) -> Duration {
        // 渲染后用空 MeshCache 单独读一遍；文件由同一进程生成，操作系统页缓存已热。
        let mut cache = MeshCache::new(&self.root);
        let start = Instant::now();
        let mesh = cache.get(MODEL).unwrap().expect("立方体必须进入 MeshCache");
        let mut load = start.elapsed();
        assert_eq!(mesh.indices.len(), 36);
        for (index, material) in self.materials.iter().enumerate() {
            let start = Instant::now();
            let loaded = cache
                .material(&material.asset)
                .unwrap()
                .expect("S1 材质资产必须可加载");
            load += start.elapsed();
            assert_eq!(loaded.base_color, MATERIAL_COLORS[index]);
            assert_eq!(loaded.roughness, material.roughness);
            assert_eq!(loaded.metallic, index as f32 / 16.0);
        }
        load
    }
}

fn assert_world(world: &World) {
    assert_eq!(world.len(), INSTANCES + 1);
    let lights = &world.environment().lights;
    assert_eq!(lights.len(), 1 + POINT_LIGHTS);
    assert_eq!(lights[0].kind, LightKind::Directional);
    assert!(lights[0].is_active() && lights[0].casts_shadow);
    assert!(lights[1..].iter().all(|light| {
        light.kind == LightKind::Point
            && light.is_active()
            && light.range > 0.0
            && !light.casts_shadow
    }));
    let view_proj = camera_view_projection(world, WIDTH, HEIGHT).expect("必须使用场景相机");
    let [x, y, z, w] = transform_point(view_proj, [0.0, 1.0, 30.0, 1.0]);
    assert!(w > 0.0 && x.abs() < w && y.abs() < w && 0.0 < z && z < w);
}

fn assert_frame(stats: RenderStats) {
    assert_eq!(stats.draws, INSTANCES);
    // 静态实例按模型合批，八份材质通过实例流传入。方向阴影与全屏 pass 另计。
    assert_eq!(stats.draw_calls, 1);
    assert_eq!(stats.passes, 2);
    assert_eq!(stats.point_shadow_draw_calls, 0);
    assert_eq!(stats.ui_draws, 0);
}

fn target(context: &GpuContext) -> wgpu::Texture {
    context.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("xgc-s1-display"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn sample_count(name: &str, default: usize) -> usize {
    match std::env::var(name) {
        Ok(raw) => raw
            .parse::<usize>()
            .ok()
            .filter(|count| *count > 0)
            .unwrap_or_else(|| panic!("{name} 必须为正整数")),
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => panic!("读取 {name} 失败：{error}"),
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn distribution(samples: &[Duration]) -> Value {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let rank = |percentile: usize| ms(sorted[(sorted.len() * percentile).div_ceil(100) - 1]);
    json!({ "p50": rank(50), "p95": rank(95), "p99": rank(99),
        "min": ms(sorted[0]), "max": rank(100),
        "samples": samples.iter().copied().map(ms).collect::<Vec<_>>() })
}

fn image_hash(pixels: &[u8]) -> String {
    let hash = pixels.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}

#[cfg(windows)]
fn peak_working_set() -> Option<u64> {
    // 进程峰值包含渲染及驱动的宿主内存，不代表设备显存。查询放在采样以外。
    let query = format!("(Get-Process -Id {}).PeakWorkingSet64", std::process::id());
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &query])
        .output()
        .ok()?;
    if !output.status.success() {
        eprintln!(
            "读取峰值工作集失败：{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
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

#[test]
#[ignore = "S1 Release 中档基线需要独占 CPU / GPU"]
fn static_city() {
    if cfg!(debug_assertions) {
        panic!("S1 基线须使用 --release");
    }
    let warmup = sample_count("XGC_S1_WARMUP", DEFAULT_WARMUP);
    let samples = sample_count("XGC_S1_SAMPLES", DEFAULT_SAMPLES);
    let city = City::write_assets();
    let start = Instant::now();
    let mut world = city.world();
    let fixture_build = start.elapsed();
    assert_world(&world);
    let before_hash = world.state_hash();
    assert_ne!(before_hash, 0);

    let start = Instant::now();
    let context = GpuContext::headless().expect("S1 需要可用的 wgpu 设备");
    let device_init = start.elapsed();
    let adapter = context.adapter_info();
    assert_ne!(
        adapter.device_type,
        wgpu::DeviceType::Cpu,
        "S1 须使用真实 GPU"
    );
    let target = target(&context);
    assert_eq!((target.width(), target.height()), (WIDTH, HEIGHT));
    let quality = RenderQuality {
        preset: Some(QualityPreset::Medium),
        ..RenderQuality::default()
    };
    let device_msaa = context.scene_sample_count();
    let resolved = quality.resolve(device_msaa);
    assert_eq!(resolved.shadow_resolution, SHADOW_RESOLUTION);
    let mut renderer = Renderer::new(MeshCache::new(&city.root));
    renderer.apply_project_settings(&context, RenderSettings::default(), quality);
    println!("S1 adapter={adapter:?} device_features={:?} warmup={warmup} samples={samples} fixture_build_ms={:.3}",
        context.device().features(), ms(fixture_build));

    let start = Instant::now();
    let cold = renderer.render(&context, &target, &world).unwrap();
    let cold_render = start.elapsed();
    context.device().poll(wgpu::Maintain::Wait);
    let cold_complete = start.elapsed();
    assert_frame(cold);
    assert_eq!(renderer.meshes().len(), 1);
    assert_eq!(renderer.gpu_mesh_count(), 1);
    println!(
        "S1 cold_cpu_render_ms={:.3} cold_queue_complete_ms={:.3}",
        ms(cold_render),
        ms(cold_complete)
    );
    for _ in 1..warmup {
        assert_frame(renderer.render(&context, &target, &world).unwrap());
        context.device().poll(wgpu::Maintain::Wait);
    }

    let mut render_cpu = Vec::with_capacity(samples);
    let mut submit_wait = Vec::with_capacity(samples);
    let mut queue_complete = Vec::with_capacity(samples);
    for _ in 0..samples {
        // 上一帧已完成：分开记入口到返回、返回到完成和整段墙钟。
        let start = Instant::now();
        let stats = renderer.render(&context, &target, &world).unwrap();
        let submitted = Instant::now();
        context.device().poll(wgpu::Maintain::Wait);
        let completed = Instant::now();
        render_cpu.push(submitted - start);
        submit_wait.push(completed - submitted);
        queue_complete.push(completed - start);
        assert_frame(stats);
    }

    let mut extraction_cpu = Vec::with_capacity(samples);
    for _ in 0..samples {
        // 与帧入口共用抽取实现。独立计数不含相机入口、材质解析或 DrawList 析构。
        let start = Instant::now();
        let draw_list = black_box(build_draw_list(&world));
        extraction_cpu.push(start.elapsed());
        assert_eq!(draw_list.len(), INSTANCES);
    }
    let material_counts = city.assert_extraction(&build_draw_list(&world));
    let asset_cache_fill = city.assert_assets();

    let mut allocations = Vec::with_capacity(ALLOCATION_SAMPLES);
    for _ in 0..ALLOCATION_SAMPLES {
        ALLOCATIONS.set(Some((0, 0)));
        let stats = renderer.render(&context, &target, &world).unwrap();
        let (calls, bytes) = ALLOCATIONS.take().unwrap();
        allocations.push(json!({ "calls": calls, "bytes": bytes }));
        assert_frame(stats);
        context.device().poll(wgpu::Maintain::Wait);
    }
    let rss = peak_working_set();

    // 分开读回，确保实际输出稳定、实际消费了方向阴影与点光。截图不进入帧计量。
    let pixels = || context.read_texture_rgba8(&target, WIDTH, HEIGHT).unwrap();
    let lit = pixels();
    assert_eq!(lit.len(), (WIDTH * HEIGHT * 4) as usize);
    assert_frame(renderer.render(&context, &target, &world).unwrap());
    assert!(lit == pixels(), "静态场景连续两帧输出必须相同");
    let original_env = world.environment().clone();
    let mut without_ninth = original_env.clone();
    without_ninth.lights.truncate(PACKED_LIGHT_LIMIT);
    world.set_environment(without_ninth);
    assert_frame(renderer.render(&context, &target, &world).unwrap());
    assert!(lit == pixels(), "第九盏灯应在当前光源缓冲上限之外");
    let mut no_shadow = original_env.clone();
    no_shadow.lights[0].casts_shadow = false;
    world.set_environment(no_shadow);
    assert_frame(renderer.render(&context, &target, &world).unwrap());
    let shadow_changed_pixels = lit
        .chunks_exact(4)
        .zip(pixels().chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert!(shadow_changed_pixels > 0, "关闭方向光阴影必须改变输出");
    let mut without_points = original_env.clone();
    without_points.lights.truncate(1);
    world.set_environment(without_points);
    assert_frame(renderer.render(&context, &target, &world).unwrap());
    let point_changed_pixels = lit
        .chunks_exact(4)
        .zip(pixels().chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert!(point_changed_pixels > 0, "移除点光必须改变输出");
    world.set_environment(original_env);
    assert_eq!(world.state_hash(), before_hash);
    assert_world(&world);

    let result = json!({
        "workload": { "instances": INSTANCES, "material_counts": material_counts,
            "resolution": [WIDTH, HEIGHT], "msaa": resolved.msaa,
            "authored_directional": 1, "authored_points": POINT_LIGHTS,
            "packed_directional_from_source": 1, "packed_points_from_source": PACKED_LIGHT_LIMIT - 1,
            "csm_cascades_from_source": CSM_CASCADES, "csm_resolution": resolved.shadow_resolution },
        "adapter": { "name": adapter.name, "backend": format!("{:?}", adapter.backend),
            "device_type": format!("{:?}", adapter.device_type), "driver": adapter.driver,
            "driver_info": adapter.driver_info },
        "warmup": warmup, "samples": samples,
        "ms": { "cpu_draw_list_extract": distribution(&extraction_cpu),
            "cpu_renderer_entry_through_submit": distribution(&render_cpu),
            "host_post_submit_queue_wait": distribution(&submit_wait),
            "host_frame_start_to_queue_complete": distribution(&queue_complete),
            "fixture_world_build": ms(fixture_build), "device_init": ms(device_init),
            "mesh_and_material_cache_fill_with_warm_filesystem": ms(asset_cache_fill),
            "cold_renderer_entry_through_submit": ms(cold_render),
            "cold_renderer_entry_through_queue_complete": ms(cold_complete) },
        "draw_calls": { "main_geometry": cold.draw_calls, "point_shadow": cold.point_shadow_draw_calls,
            "ui": cold.ui_draws, "directional_shadow_from_source": INSTANCES * CSM_CASCADES,
            "tone_map_from_source": 1, "logical_passes": cold.passes },
        "memory": { "test_thread_allocation_samples": allocations,
            "process_peak_working_set_bytes_before_readback": rss, "peak_vram_bytes": null },
        "evidence": { "world_state_hash": format!("{before_hash:016x}"), "frame_rgba8_hash": image_hash(&lit),
            "shadow_changed_pixels": shadow_changed_pixels, "points_changed_pixels": point_changed_pixels,
            "identical_without_ninth_light": true },
        "missing": ["exclusive CPU submit", "GPU frame and per-pass timestamps", "peak VRAM",
            "disk scene deserialize and full project load", "ninth light slot", "direct directional draw/pass counters"]
    });
    println!("S1_RESULT {}", result);
}
