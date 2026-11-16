//! XGC 命令行 M0：输出 schema、跑 10k 实体基准。

use std::process::ExitCode;
use std::time::Instant;

use xgc_reflect::Registry;
use xgc_world::{Name, Transform, Velocity, World};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("schema") => schema(),
        Some("bench") => bench(),
        Some(other) => {
            eprintln!("未知命令：{other}");
            eprintln!("用法：xgc <schema|bench>");
            ExitCode::from(2)
        }
        None => {
            eprintln!("用法：xgc <schema|bench>");
            ExitCode::from(2)
        }
    }
}

fn schema() -> ExitCode {
    let mut registry = Registry::new();
    if let Err(err) = registry.register::<Transform>() {
        eprintln!("注册 core.transform 失败：{err}");
        return ExitCode::from(1);
    }
    if let Err(err) = registry.register::<Velocity>() {
        eprintln!("注册 core.velocity 失败：{err}");
        return ExitCode::from(1);
    }
    if let Err(err) = registry.register::<Name>() {
        eprintln!("注册 core.name 失败：{err}");
        return ExitCode::from(1);
    }

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
