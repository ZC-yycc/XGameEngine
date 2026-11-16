//! XGC 命令行 M1：schema / check / bench / export / import。

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use xgc_reflect::Registry;
use xgc_store::{load_world, save_world};
use xgc_world::{Name, Transform, Velocity, World};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    match command.as_deref() {
        Some("schema") => schema(),
        Some("check") => check(),
        Some("bench") => bench(),
        Some("export") => export(args.next().as_deref()),
        Some("import") => import(args.next().as_deref()),
        Some(other) => {
            eprintln!("未知命令：{other}");
            eprintln!("用法：xgc <schema|check|bench|export <路径>|import <路径>>");
            ExitCode::from(2)
        }
        None => {
            eprintln!("用法：xgc <schema|check|bench|export <路径>|import <路径>>");
            ExitCode::from(2)
        }
    }
}

fn builtin_registry() -> Result<Registry, String> {
    let mut registry = Registry::new();
    registry
        .register::<Transform>()
        .map_err(|err| format!("注册 core.transform 失败：{err}"))?;
    registry
        .register::<Velocity>()
        .map_err(|err| format!("注册 core.velocity 失败：{err}"))?;
    registry
        .register::<Name>()
        .map_err(|err| format!("注册 core.name 失败：{err}"))?;
    Ok(registry)
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

fn check() -> ExitCode {
    let registry = match builtin_registry() {
        Ok(registry) => registry,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(1);
        }
    };
    let diagnostics = registry.validate();
    if diagnostics.is_empty() {
        println!("check: {} 个组件，schema 语义 OK", registry.len());
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
