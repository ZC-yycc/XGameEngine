//! XGC 命令行 M1：schema / check / bench / export / import。

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use xgc_agent::Agent;
use xgc_migrate::{migrate_dir, migrate_str};
use xgc_reflect::Registry;
use xgc_render::{GpuContext, Pass, RenderGraph, ResourceDesc, ResourceUsage};
use xgc_sim::{play, Scenario};
use xgc_store::{load_world, save_world};
use xgc_world::{Name, Transform, Velocity, World};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    match command.as_deref() {
        Some("schema") => schema(),
        Some("check") => check(args.next().as_deref()),
        Some("bench") => bench(),
        Some("export") => export(args.next().as_deref()),
        Some("import") => import(args.next().as_deref()),
        Some("run") => run(args.next().as_deref()),
        Some("serve") => serve(),
        Some("migrate") => migrate(args.next().as_deref(), args.next().as_deref()),
        Some("render-graph") => render_graph(),
        Some("gpu-probe") => gpu_probe(),
        Some(other) => {
            eprintln!("未知命令：{other}");
            eprintln!(
                "用法：xgc <schema|check [路径]|bench|serve|export <路径>|import <路径>|run [帧数]|migrate <旧.ron|旧目录> <新.json|新目录>|render-graph|gpu-probe>"
            );
            ExitCode::from(2)
        }
        None => {
            eprintln!(
                "用法：xgc <schema|check [路径]|bench|serve|export <路径>|import <路径>|run [帧数]|migrate <旧.ron|旧目录> <新.json|新目录>|render-graph|gpu-probe>"
            );
            ExitCode::from(2)
        }
    }
}

fn builtin_registry() -> Result<Registry, String> {
    Ok(xgc_world::builtin_registry())
}

fn schema() -> ExitCode {
    let registry = match builtin_registry() {
        Ok(registry) => registry,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(1);
        }
    };

    match serde_json::to_string_pretty(&registry.schema()) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("schema 序列化失败：{err}");
            ExitCode::from(1)
        }
    }
}

fn check(path: Option<&str>) -> ExitCode {
    let registry = match builtin_registry() {
        Ok(registry) => registry,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(1);
        }
    };
    let mut diagnostics = registry.validate();
    let mut world_summary = None;

    if let Some(path) = path {
        let data = match load_world(Path::new(path)) {
            Ok(data) => data,
            Err(err) => {
                eprintln!("读取世界失败：{err}");
                return ExitCode::from(1);
            }
        };
        world_summary = Some((data.entities.len(), data.revision));
        diagnostics.extend(World::validate_data(&data, &registry));
    }

    if diagnostics.is_empty() {
        match world_summary {
            Some((entities, revision)) => println!(
                "check: {} 个组件，schema 语义 OK；世界 {entities} 个实体，revision {revision}",
                registry.len()
            ),
            None => println!("check: {} 个组件，schema 语义 OK", registry.len()),
        }
        ExitCode::SUCCESS
    } else {
        for diagnostic in &diagnostics {
            eprintln!("{diagnostic}");
        }
        ExitCode::from(1)
    }
}

fn demo_world() -> World {
    let mut world = World::new();
    for i in 0..3 {
        world.spawn((
            Transform {
                position: [i as f32, 0.0, 0.0],
                ..Transform::default()
            },
            Name {
                value: format!("demo_{i}"),
            },
        ));
    }
    world
}

fn export(path: Option<&str>) -> ExitCode {
    let Some(path) = path else {
        eprintln!("用法：xgc export <路径>");
        return ExitCode::from(2);
    };
    let world = demo_world();
    let data = world.world_data();
    match save_world(Path::new(path), &data) {
        Ok(()) => {
            println!(
                "export: {} 个实体，revision {} -> {path}",
                data.entities.len(),
                data.revision
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("导出失败：{err}");
            ExitCode::from(1)
        }
    }
}

fn import(path: Option<&str>) -> ExitCode {
    let Some(path) = path else {
        eprintln!("用法：xgc import <路径>");
        return ExitCode::from(2);
    };
    let data = match load_world(Path::new(path)) {
        Ok(data) => data,
        Err(err) => {
            eprintln!("读取失败：{err}");
            return ExitCode::from(1);
        }
    };
    let mut world = World::new();
    if let Err(message) = world.restore_world_data(&data) {
        eprintln!("恢复失败：{message}");
        return ExitCode::from(1);
    }
    println!(
        "import: {} 个实体，revision {}，hash {:016x}",
        world.len(),
        world.revision(),
        world.state_hash()
    );
    ExitCode::SUCCESS
}

fn migrate(input: Option<&str>, output: Option<&str>) -> ExitCode {
    let (Some(input), Some(output)) = (input, output) else {
        eprintln!("用法：xgc migrate <旧.ron|旧目录> <新.json|新目录>");
        return ExitCode::from(2);
    };
    if Path::new(input).is_dir() {
        return match migrate_dir(Path::new(input), Path::new(output)) {
            Ok(batch) => {
                for failure in &batch.failures {
                    eprintln!("migrate 失败：{failure}");
                }
                println!(
                    "migrate: {} 个场景，{} 个实体，{} 个带旧组件，{} 个组件已映射，{} 个父节点缺失，{} 个失败 -> {}",
                    batch.scenes,
                    batch.entities,
                    batch.with_old_components,
                    batch.mapped_components,
                    batch.missing_parents,
                    batch.failures.len(),
                    output
                );
                if batch.failures.is_empty() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(1)
                }
            }
            Err(err) => {
                eprintln!("批量迁移失败：{err}");
                ExitCode::from(1)
            }
        };
    }
    let text = match std::fs::read_to_string(input) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("读取旧场景失败：{err}");
            return ExitCode::from(1);
        }
    };
    let (data, report) = match migrate_str(&text) {
        Ok(result) => result,
        Err(err) => {
            eprintln!("迁移失败：{err}");
            return ExitCode::from(1);
        }
    };
    if let Err(err) = save_world(Path::new(output), &data) {
        eprintln!("写入迁移结果失败：{err}");
        return ExitCode::from(1);
    }
    println!(
        "migrate: {} 个实体，{} 个带旧组件，{} 个组件已映射，{} 个父节点缺失 -> {}",
        report.entities,
        report.with_old_components,
        report.mapped_components,
        report.missing_parents,
        output
    );
    ExitCode::SUCCESS
}

fn render_graph() -> ExitCode {
    let mut graph = RenderGraph::new();
    let resources = [
        ResourceDesc::texture2d(
            "shadow_map",
            "depth32float",
            2048,
            2048,
            vec![ResourceUsage::DepthAttachment, ResourceUsage::Sampled],
        ),
        ResourceDesc::texture2d(
            "color",
            "rgba16float",
            1280,
            720,
            vec![ResourceUsage::ColorAttachment, ResourceUsage::Sampled],
        ),
        ResourceDesc::texture2d(
            "backbuffer",
            "bgra8unorm",
            1280,
            720,
            vec![ResourceUsage::ColorAttachment],
        ),
    ];
    for resource in resources {
        if let Err(err) = graph.add_resource(resource) {
            eprintln!("声明渲染资源失败：{err}");
            return ExitCode::from(1);
        }
    }
    let passes = [
        Pass::new("shadow", &[], &["shadow_map"]),
        Pass::new("opaque", &["shadow_map"], &["color"]),
        Pass::new("post", &["color"], &["backbuffer"]),
    ];
    for pass in passes {
        if let Err(err) = graph.add(pass) {
            eprintln!("构建渲染图失败：{err}");
            return ExitCode::from(1);
        }
    }
    match graph.order_names() {
        Ok(order) => {
            println!(
                "render-graph: {} 个 pass -> {}",
                order.len(),
                order.join(" -> ")
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("渲染图排序失败：{err}");
            ExitCode::from(1)
        }
    }
}

fn gpu_probe() -> ExitCode {
    match GpuContext::headless() {
        Ok(context) => {
            let info = context.adapter_info();
            println!("gpu: {} / {:?}", info.name, info.backend);
            let desc = ResourceDesc::texture2d(
                "probe",
                "rgba8unorm",
                4,
                4,
                vec![ResourceUsage::ColorAttachment, ResourceUsage::Sampled],
            );
            match context.create_resource(&desc) {
                Ok(resource) => {
                    let Some(texture) = resource.as_texture() else {
                        eprintln!("GPU 资源不是纹理");
                        return ExitCode::from(1);
                    };
                    if let Err(err) = context.clear_texture_rgba(texture, [1.0, 0.0, 0.0, 1.0]) {
                        eprintln!("清屏失败：{err}");
                        return ExitCode::from(1);
                    }
                    match context.read_texture_rgba8(texture, 4, 4) {
                        Ok(pixels) => {
                            println!("gpu: clear pass + readback {:?}", &pixels[..4]);
                            ExitCode::SUCCESS
                        }
                        Err(err) => {
                            eprintln!("回读失败：{err}");
                            ExitCode::from(1)
                        }
                    }
                }
                Err(err) => {
                    eprintln!("创建 GPU 资源失败：{err}");
                    ExitCode::from(1)
                }
            }
        }
        Err(err) => {
            eprintln!("GPU 不可用：{err}");
            ExitCode::from(1)
        }
    }
}

fn serve() -> ExitCode {
    let mut agent = Agent::new();
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("读取 stdin 失败：{err}");
                return ExitCode::from(1);
            }
        };
        if let Some(response) = agent.handle_line(&line) {
            if writeln!(stdout, "{response}").is_err() || stdout.flush().is_err() {
                eprintln!("写入 stdout 失败");
                return ExitCode::from(1);
            }
        }
    }
    ExitCode::SUCCESS
}

fn run(frames: Option<&str>) -> ExitCode {
    let frames: u64 = match frames {
        Some(text) => match text.parse() {
            Ok(frames) => frames,
            Err(_) => {
                eprintln!("帧数不是整数：{text}");
                return ExitCode::from(2);
            }
        },
        None => 60,
    };
    let world = demo_world();
    let scenario = Scenario {
        initial: world.world_data(),
        frames,
        fixed_dt: 1.0 / 60.0,
        commands: Vec::new(),
    };
    match play(&scenario) {
        Ok((_world, trace)) => {
            if let Some(last) = trace.frames.last() {
                println!(
                    "run: frames={frames} entities={} revision={} hash={:016x}",
                    last.entities, last.revision, last.state_hash
                );
            } else {
                println!("run: frames=0");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("运行失败：{err}");
            ExitCode::from(1)
        }
    }
}

fn bench() -> ExitCode {
    const N: usize = 10_000;
    let mut world = World::new();

    let start = Instant::now();
    let mut keys = Vec::with_capacity(N);
    for i in 0..N {
        keys.push(world.spawn((
            Transform {
                position: [i as f32, 0.0, 0.0],
                ..Transform::default()
            },
            Velocity::default(),
        )));
    }
    let spawn = start.elapsed();

    let query_start = Instant::now();
    let mut sum = 0.0f64;
    world.for_each::<Transform>(|_, transform| sum += transform.position[0] as f64);
    let query = query_start.elapsed();
    let expected = (0..N).map(|i| i as i32).sum::<i32>() as f64;
    if sum != expected {
        eprintln!("查询结果不对：sum={sum} expected={expected}");
        return ExitCode::from(1);
    }

    let despawn_start = Instant::now();
    for key in keys {
        if !world.despawn(key) {
            eprintln!("despawn 失败：{key:?}");
            return ExitCode::from(1);
        }
    }
    let despawn = despawn_start.elapsed();
    if !world.is_empty() {
        eprintln!("despawn 之后世界不为空：{}", world.len());
        return ExitCode::from(1);
    }

    println!(
        "xgc bench: n={N} spawn={spawn:?} query={query:?} despawn={despawn:?} revision={}",
        world.revision()
    );
    ExitCode::SUCCESS
}
