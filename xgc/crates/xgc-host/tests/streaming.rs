//! S5 同步懒读基线。Release 下单独运行，默认只创建三个小 GLB。
//!
//! `XGC_S5_PROFILE=prepare-2gib` 单独生成大档资产，无需初始化 GPU；
//! `XGC_S5_PROFILE=2gib` 在新进程测 3,600 固定步，避免把生成计入进程峰值。
//! 可用 `XGC_S5_SESSION` 将两个进程隔离在 `tmp/xgc-s5` 下同一个已创建的会话目录。
//! 两档均使用实际脚本步进、视距选择、GLB 导入、纹理上传和 Renderer 提交。

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use xgc_asset::AssetPaths;
use xgc_host::{asset_paths, ScriptedRunner};
use xgc_render::{GpuContext, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{GlobalTransform, WorldData};

#[path = "streaming/fixture.rs"]
mod fixture;

use fixture::{
    restore, scene, verify_assets, write_assets, CAMERA_Z, FULL, FULL_RGBA8_BYTES, SMOKE,
};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;

thread_local! {
    static ALLOCATIONS: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
}

struct CountingAllocator;

fn count(pointer: *mut u8, bytes: usize) -> *mut u8 {
    if !pointer.is_null() {
        let _ = ALLOCATIONS.try_with(|counter| {
            if let Some((calls, total)) = counter.get() {
                counter.set(Some((calls + 1, total + bytes as u64)));
            }
        });
    }
    pointer
}

// SAFETY: 转发 System 的原始指针和布局。计数仅使用本线程的 Cell，不读写指针目标。
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

fn hash_pixels(pixels: &[u8]) -> u64 {
    pixels.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(windows)]
fn peak_working_set() -> Option<u64> {
    // 包括进程及驱动在进程内的 RAM。设备显存没有从这个值推断。
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

#[cfg(target_os = "linux")]
fn peak_working_set() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let kib = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    Some(kib * 1024)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn peak_working_set() -> Option<u64> {
    None
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Phase {
    Outside,
    FirstUse,
    Resident,
    Revisit,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::Outside => "outside",
            Self::FirstUse => "first_use",
            Self::Resident => "resident",
            Self::Revisit => "revisit",
        }
    }
}

struct Sample {
    step: i32,
    zone: Option<usize>,
    phase: Phase,
    render_to_submit: Duration,
    step_to_submit: Duration,
    submit_to_readback: Duration,
    step_to_readback: Duration,
    allocation_calls: u64,
    allocation_bytes: u64,
    draws: usize,
    draw_calls: usize,
    logical_passes: usize,
    cached_models: usize,
    camera_x: f32,
    probe_rgba8: [u8; 4],
}

fn report_latency(label: &str, samples: impl Iterator<Item = Duration>) {
    let mut sorted: Vec<_> = samples.collect();
    if sorted.is_empty() {
        return;
    }
    sorted.sort_unstable();
    let rank = |percentile: usize| {
        sorted[(sorted.len() * percentile).div_ceil(100) - 1].as_secs_f64() * 1_000.0
    };
    println!(
        "S5 {label} samples={} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} max_ms={:.3}",
        sorted.len(),
        rank(50),
        rank(95),
        rank(99),
        rank(100),
    );
}

const CHECKPOINT_STEPS: i32 = 60;

fn open_samples(path: &Path) -> BufWriter<File> {
    let mut out = BufWriter::with_capacity(16 * 1024, File::create_new(path).unwrap());
    writeln!(out, "step\tzone\tphase\trender_to_submit_ns\tstep_to_submit_ns\tsubmit_to_readback_ns\tstep_to_readback_ns\tallocation_calls\tallocation_bytes\tdraws\tdraw_calls\tlogical_passes\tcached_models\tcamera_x\tprobe_rgba8").unwrap();
    out.flush().unwrap();
    out
}

fn write_sample(out: &mut BufWriter<File>, sample: &Sample) {
    write!(out, "{}\t", sample.step).unwrap();
    if let Some(zone) = sample.zone {
        write!(out, "{zone}").unwrap();
    }
    writeln!(
        out,
        "\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{camera_x}\t{probe_rgba8:08x}",
        sample.phase.label(),
        sample.render_to_submit.as_nanos(),
        sample.step_to_submit.as_nanos(),
        sample.submit_to_readback.as_nanos(),
        sample.step_to_readback.as_nanos(),
        sample.allocation_calls,
        sample.allocation_bytes,
        sample.draws,
        sample.draw_calls,
        sample.logical_passes,
        sample.cached_models,
        camera_x = sample.camera_x,
        probe_rgba8 = u32::from_be_bytes(sample.probe_rgba8),
    )
    .unwrap();
}

fn workspace_root() -> std::path::PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tmp/xgc-s5");
    fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    match std::env::var("XGC_S5_SESSION") {
        Ok(name) => {
            assert!(name.starts_with("session-") && name.len() <= 80);
            assert!(name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'));
            let directory = root.join(name);
            assert!(directory.is_dir(), "会话目录必须已创建");
            assert_eq!(
                directory.canonicalize().unwrap(),
                directory,
                "会话目录须在临时根内"
            );
            directory
        }
        Err(std::env::VarError::NotPresent) => root,
        Err(error) => panic!("读取 XGC_S5_SESSION 失败：{error}"),
    }
}

fn headless_hash(data: &WorldData, assets: AssetPaths, steps: i32) -> u64 {
    let (mut runner, meshes) = ScriptedRunner::with_assets(restore(data), assets);
    assert!(meshes.is_empty());
    for step in 0..steps {
        assert_eq!(runner.step().unwrap(), step as u64);
    }
    assert_eq!(runner.script_count(), 1);
    runner.world().state_hash()
}

#[test]
#[ignore = "S5 Release 基线需独占 GPU，2 GiB 档须显式选择"]
fn streaming() {
    if cfg!(debug_assertions) {
        panic!("S5 基线须使用 --release");
    }
    let root = workspace_root();
    let full_root = root.join("assets-2gib");
    let mode = std::env::var("XGC_S5_PROFILE").unwrap_or_else(|error| match error {
        std::env::VarError::NotPresent => "smoke".into(),
        error => panic!("读取 XGC_S5_PROFILE 失败：{error}"),
    });
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let run = root.join(format!("{mode}-{}-{nonce}", std::process::id()));
    if mode == "prepare-2gib" {
        assert!(
            !full_root.exists(),
            "大档资产已存在：{}",
            full_root.display()
        );
        println!("S5 准备大档资产 {}", run.display());
        write_assets(&run, FULL);
        let disk = verify_assets(&run, FULL);
        assert!(disk >= FULL_RGBA8_BYTES);
        fs::rename(&run, &full_root).unwrap();
        println!(
            "S5 assets={} fixture_disk_bytes={disk} fixture_rgba8_bytes={}",
            full_root.display(),
            FULL.rgba8_bytes()
        );
        return;
    }
    let (profile, assets_root) = match mode.as_str() {
        "smoke" => {
            write_assets(&run, SMOKE);
            (SMOKE, run.clone())
        }
        "2gib" => {
            fs::create_dir(&run).unwrap();
            (FULL, full_root)
        }
        _ => panic!("XGC_S5_PROFILE 只接受 smoke、prepare-2gib 或 2gib"),
    };
    println!("S5 run_dir={}", run.display());
    let samples_path = run.join("frames.tsv");
    let mut samples_file = open_samples(&samples_path);
    let disk_bytes = verify_assets(&assets_root, profile);
    let rgba8_bytes = profile.rgba8_bytes();
    if mode == "2gib" {
        assert_eq!(rgba8_bytes, FULL_RGBA8_BYTES);
        assert!(disk_bytes >= rgba8_bytes);
        assert_eq!(profile.steps, 3_600);
    }
    let (data, camera) = scene(profile);
    let context = GpuContext::headless().expect("S5 需要可用的 wgpu 适配器");
    let adapter = context.adapter_info();
    assert!(
        matches!(
            format!("{:?}", adapter.device_type).as_str(),
            "DiscreteGpu" | "IntegratedGpu"
        ),
        "S5 需要硬件 GPU：{adapter:?}"
    );
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "xgc-s5-viewport",
            "rgba8unorm-srgb",
            WIDTH,
            HEIGHT,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let start = Instant::now();
    let assets = asset_paths(Some(&assets_root), &[]);
    let (mut runner, meshes) = ScriptedRunner::with_assets(restore(&data), assets.clone());
    assert!(meshes.is_empty(), "没有根运动时启动不应预读模型");
    let mut renderer = Renderer::new(meshes);
    renderer.set_sample_count(1);
    assert_eq!(runner.fixed_dt(), 1.0_f32 / 60.0);
    assert_eq!(renderer.gpu_mesh_count(), 0);
    let setup = start.elapsed();
    let initial_hash = runner.world().state_hash();
    println!(
        "S5 profile={mode} steps={} simulated_seconds={:.3} zones={} viewport={WIDTH}x{HEIGHT} msaa=1 adapter={adapter:?} fixture_disk_bytes={disk_bytes} fixture_rgba8_bytes={rgba8_bytes} setup_ms={:.3}",
        profile.steps, f64::from(profile.steps) / 60.0, profile.zones, setup.as_secs_f64() * 1_000.0,
    );

    let mut seen = vec![false; profile.zones];
    let mut centers = HashSet::with_capacity(profile.zones);
    let mut samples = Vec::with_capacity(profile.steps as usize);
    let mut first_image = None;
    let mut previous = None;
    let mut cached = 0;
    let mut visited_first_again = false;
    let mut clear = None;
    let mut journal_time = Duration::ZERO;
    let traversal = Instant::now();
    for step in 0..profile.steps {
        ALLOCATIONS.set(Some((0, 0)));
        let begin = Instant::now();
        let frame = runner
            .step()
            .unwrap_or_else(|err| panic!("第 {step} 步执行失败：{err}"));
        renderer.set_particles(runner.particle_quads().to_vec());
        let render_begin = Instant::now();
        let stats = renderer
            .render(&context, texture, runner.world())
            .unwrap_or_else(|err| panic!("第 {step} 步渲染失败：{err}"));
        let submitted = Instant::now();
        let (allocation_calls, allocation_bytes) = ALLOCATIONS.take().unwrap();
        // 现有公开 API 的最小回读等待队列完成，同时承担 1 像素复制、映射与机器调度开销。
        let pixel: [u8; 4] = context
            .read_texture_rgba8(texture, 1, 1)
            .unwrap_or_else(|err| panic!("第 {step} 步回读失败：{err}"))
            .try_into()
            .unwrap();
        let completed = Instant::now();

        assert_eq!(frame, step as u64);
        let world = runner.world();
        let x = profile.camera_x(step);
        let pose = world
            .get::<GlobalTransform>(world.key(camera).unwrap())
            .unwrap();
        assert_eq!(pose.position, [x, 0.0, CAMERA_Z], "第 {step} 步相机路线");
        let zone = profile.zone_at(x);
        let draws = usize::from(zone.is_some());
        assert_eq!(stats.draws, draws, "第 {step} 步 LOD 选择");
        assert_eq!(stats.draw_calls, draws, "第 {step} 步主场景提交");
        assert_eq!(stats.point_shadow_draw_calls, 0);
        let phase = match zone {
            None => Phase::Outside,
            Some(zone) if !seen[zone] => {
                seen[zone] = true;
                cached += 1;
                Phase::FirstUse
            }
            Some(zone) if previous != Some(zone) => {
                visited_first_again |= zone == 0;
                Phase::Revisit
            }
            Some(_) => Phase::Resident,
        };
        assert_eq!(renderer.meshes().len(), cached, "第 {step} 步 CPU 缓存");
        assert_eq!(renderer.gpu_mesh_count(), cached, "第 {step} 步 GPU 缓存");
        previous = zone;
        if zone.is_none() {
            assert_eq!(*clear.get_or_insert(pixel), pixel);
        }
        if let Some(center) = profile.outbound_center(step) {
            assert_eq!(zone, Some(center));
            assert_ne!(Some(pixel), clear, "区 {center} 须写入真实颜色");
            assert!(centers.insert(pixel), "区 {center} 须采样自己的贴图");
            if center == 0 {
                first_image = Some(hash_pixels(
                    &context.read_texture_rgba8(texture, WIDTH, HEIGHT).unwrap(),
                ));
            }
        }
        let sample = Sample {
            step,
            zone,
            phase,
            render_to_submit: submitted - render_begin,
            step_to_submit: submitted - begin,
            submit_to_readback: completed - submitted,
            step_to_readback: completed - begin,
            allocation_calls,
            allocation_bytes,
            draws: stats.draws,
            draw_calls: stats.draw_calls,
            logical_passes: stats.passes,
            cached_models: cached,
            camera_x: pose.position[0],
            probe_rgba8: pixel,
        };
        // 只在完整验证后记帧。保留进程退出前已刷的检查点，计时与分配窗口已结束。
        let journal_begin = Instant::now();
        write_sample(&mut samples_file, &sample);
        let first_draw = phase == Phase::FirstUse && cached == 1;
        let steps_done = step + 1;
        if step == 0
            || first_draw
            || steps_done % CHECKPOINT_STEPS == 0
            || steps_done == profile.steps
        {
            samples_file.flush().unwrap();
        }
        journal_time += journal_begin.elapsed();
        if step == 0 || first_draw {
            println!(
                "S5 checkpoint step={step} phase={} cached_models={cached} draw_calls={}",
                phase.label(),
                stats.draw_calls
            );
        }
        samples.push(sample);
    }
    let traversal_wall = traversal.elapsed();
    assert_eq!(samples.len(), profile.steps as usize);
    let peak_ram = peak_working_set();
    assert_eq!(cached, profile.zones);
    assert_eq!(centers.len(), profile.zones);
    assert!(visited_first_again);
    assert_eq!(previous, Some(0));
    assert_eq!(profile.camera_x(profile.steps - 1), 0.0);
    assert_eq!(runner.script_count(), 1);
    let world_hash = runner.world().state_hash();
    assert_ne!(world_hash, initial_hash);
    let image_hash = hash_pixels(&context.read_texture_rgba8(texture, WIDTH, HEIGHT).unwrap());
    assert_eq!(
        Some(image_hash),
        first_image,
        "回访首区的整帧 RGBA 哈希应与首次一致"
    );
    drop((renderer, target, context, runner));
    assert_eq!(world_hash, headless_hash(&data, assets, profile.steps));

    for phase in [
        Phase::Outside,
        Phase::FirstUse,
        Phase::Resident,
        Phase::Revisit,
    ] {
        let subset = || samples.iter().filter(|sample| sample.phase == phase);
        let label = phase.label();
        report_latency(
            &format!("{label} render_to_submit"),
            subset().map(|s| s.render_to_submit),
        );
        report_latency(
            &format!("{label} step_to_submit"),
            subset().map(|s| s.step_to_submit),
        );
        report_latency(
            &format!("{label} submit_to_readback"),
            subset().map(|s| s.submit_to_readback),
        );
        report_latency(
            &format!("{label} step_to_readback"),
            subset().map(|s| s.step_to_readback),
        );
    }
    report_latency(
        "all render_to_submit",
        samples.iter().map(|s| s.render_to_submit),
    );
    report_latency(
        "all step_to_readback",
        samples.iter().map(|s| s.step_to_readback),
    );
    println!(
        "S5 first_use_zones={cached} visible_zone_colors={} final_frame={} final_hash={world_hash:016x} final_rgba8_hash={image_hash:016x} traversal_wall_ms={:.3} journal_ms={:.3} peak_process_ram_bytes={peak_ram:?} frames_tsv={}",
        centers.len(), profile.steps - 1, traversal_wall.as_secs_f64() * 1_000.0,
        journal_time.as_secs_f64() * 1_000.0, samples_path.display(),
    );
    println!("S5 RAM 取回访整帧回读和无头回放前的进程峰值。分配只计测试线程步进入口至提交的请求，包含重分配。队列按帧串行，墙钟含回读探针。帧计时结束后缓冲逐帧日志，首步、首次绘制、末步及每 {CHECKPOINT_STEPS} 步写入操作系统缓存，遍历墙钟包含日志检查开销。GPU 帧与逐 pass 时间戳、VRAM、窗口合成与 present 卡顿待测。资产读取同步，CPU / GPU 缓存保留全部已访问分区。");
}
