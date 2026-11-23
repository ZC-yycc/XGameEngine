//! `xgc trace`：逐帧状态哈希必须确定、可复现，并与进程内重放逐步一致。
//!
//! 这里不 spawn 旧引擎（它没有逐帧 trace），只钉住 XGC 侧合同：
//! 同一世界 + 同一帧数 ⇒ 同一串输出；换一个世界 ⇒ 第 0 帧就不同。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use xgc_sim::Simulation;
use xgc_store::{load_world, save_world};
use xgc_world::{Transform, Velocity, World};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_xgc")
}

/// 写一个只含确定运动的小世界：位置随 x 偏移，速度向下。
/// 不落脚本 / 不落物理体，避免依赖外部资产。
fn world_file(label: &str, offset: f32) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("xgc-trace-{}-{label}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("world.json");
    let mut world = World::new();
    for index in 0..3 {
        world.spawn((
            Transform {
                position: [offset + index as f32, 2.0, 0.0],
                ..Transform::default()
            },
            Velocity {
                linear: [0.0, -1.0, 0.0],
            },
        ));
    }
    save_world(&path, &world.world_data()).unwrap();
    path
}

fn trace(path: &Path, frames: &str) -> Output {
    Command::new(binary())
        .arg("trace")
        .arg(path)
        .arg(frames)
        .output()
        .unwrap()
}

/// 无资产根、显式线程数的 trace：第三个位置用 `-` 占位，第四个位置是线程数。
fn trace_threads(path: &Path, frames: &str, threads: &str) -> Output {
    Command::new(binary())
        .arg("trace")
        .arg(path)
        .arg(frames)
        .arg("-")
        .arg(threads)
        .output()
        .unwrap()
}

/// 带脚本的资产根 + 世界：`move.lua` 每帧把实体 x 设成 5，
/// 用来区分"跑了脚本"与"只跑系统"两条 trace 路径。
const MOVE_LUA: &str = "function update(ctx)\n  set_pos(ctx, 5.0, 0.0, 0.0)\nend\n";

fn scripted_world(label: &str) -> (PathBuf, PathBuf) {
    scripted_world_with(label, MOVE_LUA)
}

/// 同 [`scripted_world`]，但脚本内容由调用方给（坏脚本用例要能造语法错）。
fn scripted_world_with(label: &str, lua: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("xgc-trace-script-{}-{label}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let assets = dir.join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("move.lua"), lua).unwrap();
    let path = dir.join("world.json");
    let mut world = World::new();
    world.spawn((
        Transform::default(),
        xgc_world::Script {
            path: "move.lua".to_string(),
        },
    ));
    save_world(&path, &world.world_data()).unwrap();
    (path, assets)
}

/// 带资产根的 trace：第 3 个参数走脚本路径。
fn trace_with_assets(path: &Path, frames: &str, assets: &Path) -> Output {
    Command::new(binary())
        .arg("trace")
        .arg(path)
        .arg(frames)
        .arg(assets)
        .output()
        .unwrap()
}

fn stdout_lines(output: &Output) -> Vec<&str> {
    let text = std::str::from_utf8(&output.stdout).unwrap();
    text.lines().collect()
}

#[test]
fn trace_prints_one_line_per_frame_and_matches_the_simulation() {
    let path = world_file("simulation", 0.0);
    let output = trace(&path, "8");
    assert!(
        output.status.success(),
        "trace 必须成功：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines = stdout_lines(&output);
    assert_eq!(lines.len(), 10, "8 帧应有 9 行帧状态 + 1 行汇总：{lines:?}");

    // 进程内重放同一份磁盘世界，逐步对表。
    let data = load_world(&path).unwrap();
    let mut world = World::new();
    world.restore_world_data(&data).unwrap();
    let mut simulation = Simulation::new(world);
    let mut expected = Vec::new();
    for frame in 0..=8 {
        if frame > 0 {
            simulation.step(Vec::new()).unwrap();
        }
        let world = simulation.world();
        expected.push(format!(
            "frame={frame} entities={} revision={} hash={:016x}",
            world.len(),
            world.revision(),
            world.state_hash()
        ));
    }
    for (frame, expected) in expected.iter().enumerate() {
        assert_eq!(
            lines[frame], expected,
            "第 {frame} 行的帧状态必须与进程内重放一致"
        );
    }

    let world = simulation.world();
    assert_eq!(
        lines[9],
        format!(
            "trace: frames=8 entities={} revision={} hash={:016x}",
            world.len(),
            world.revision(),
            world.state_hash()
        ),
        "汇总行必须与末帧一致"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn trace_is_byte_identical_across_runs_and_worlds_diverge_at_frame_zero() {
    let path = world_file("repeat", 0.0);
    let first = trace(&path, "6");
    let second = trace(&path, "6");
    assert!(first.status.success() && second.status.success());
    assert!(
        first.stdout == second.stdout,
        "同一世界同一帧数两次 trace 必须逐字节相同"
    );

    // 换一个世界：第 0 帧就要不同（否则 trace 看不出输入差异）。
    let other = world_file("other", 5.0);
    let moved = trace(&other, "6");
    assert!(
        moved.status.success(),
        "不同世界同样要跑通：{}",
        String::from_utf8_lossy(&moved.stderr)
    );
    let first_lines = stdout_lines(&first);
    let moved_lines = stdout_lines(&moved);
    assert_eq!(first_lines.len(), moved_lines.len());
    assert_ne!(
        first_lines[0], moved_lines[0],
        "不同世界的第 0 帧状态必须不同"
    );

    let _ = std::fs::remove_dir_all(path.parent().unwrap());
    let _ = std::fs::remove_dir_all(other.parent().unwrap());
}

#[test]
fn trace_reports_usage_errors_as_exit_code_two() {
    let missing = Command::new(binary()).arg("trace").output().unwrap();
    assert_eq!(missing.status.code(), Some(2), "缺参数必须退出码 2");
    let text = String::from_utf8_lossy(&missing.stderr);
    assert!(
        text.contains("用法：xgc trace"),
        "缺参数必须打印用法：{text}"
    );

    let path = world_file("bad-frames", 0.0);
    let bad = trace(&path, "abc");
    assert_eq!(bad.status.code(), Some(2), "帧数不是整数必须退出码 2");
    assert!(String::from_utf8_lossy(&bad.stderr).contains("帧数不是整数"));
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn trace_summary_matches_run_world_output() {
    // `trace` 与 `run-world` 共用同一条"读世界 → 建 Simulation → 步进"路径；
    // 这条断言钉住那个不变量：以后谁改了其中一条而没改另一条，这里先红。
    let path = world_file("run-world", 0.0);
    let traced = trace(&path, "6");
    assert!(traced.status.success());
    let run = Command::new(binary())
        .arg("run-world")
        .arg(&path)
        .arg("6")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "run-world 必须跑通：{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let traced_lines = stdout_lines(&traced);
    let summary = traced_lines.last().expect("trace 必须有汇总行").to_string();
    let run_line = String::from_utf8(run.stdout).unwrap();
    let run_line = run_line.trim_end();
    assert_eq!(
        summary.trim_end(),
        format!(
            "trace: {}",
            run_line
                .strip_prefix("run-world: ")
                .expect("run-world 末行必须以 run-world: 开头")
        ),
        "trace 汇总行必须与 run-world 末行逐字一致（同一条步进路径）"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn trace_with_assets_root_runs_the_scripts() {
    let (path, assets) = scripted_world("runs");
    let plain = trace(&path, "3");
    let scripted = trace_with_assets(&path, "3", &assets);
    assert!(
        plain.status.success(),
        "无脚本 trace 必须成功：{}",
        String::from_utf8_lossy(&plain.stderr)
    );
    assert!(
        scripted.status.success(),
        "脚本 trace 必须成功：{}",
        String::from_utf8_lossy(&scripted.stderr)
    );
    let plain = stdout_lines(&plain);
    let scripted = stdout_lines(&scripted);
    // 第 0 帧还没跑脚本，两条路径必须相同；从第 1 帧起脚本把 x 设成 5。
    assert_eq!(plain[0], scripted[0], "第 0 帧是装载态，两条路径必须一致");
    assert_ne!(
        plain[1], scripted[1],
        "带资产根时脚本必须在第 1 帧前跑过（否则两条路径没有区别）"
    );
    assert_eq!(scripted.len(), 5, "3 帧应有 4 行帧状态 + 1 行汇总");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn scripted_trace_is_byte_identical_and_matches_run_scripts() {
    let (path, assets) = scripted_world("matches");
    let first = trace_with_assets(&path, "4", &assets);
    let second = trace_with_assets(&path, "4", &assets);
    assert!(first.status.success() && second.status.success());
    assert_eq!(
        first.stdout, second.stdout,
        "同一世界 + 同一脚本的两次逐帧 trace 必须逐字节相同"
    );

    // 汇总行必须与 `run-scripts` 同口径（同一条 ScriptedRunner 路径）。
    let run = Command::new(binary())
        .arg("run-scripts")
        .arg(&path)
        .arg(&assets)
        .arg("4")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "run-scripts 必须成功：{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let run_text = String::from_utf8(run.stdout).unwrap();
    let run_last = run_text.lines().last().unwrap();
    let run_summary = run_last.replacen("run-scripts: ", "trace: ", 1);
    let trace_text = String::from_utf8(first.stdout).unwrap();
    let trace_last = trace_text.lines().last().unwrap();
    assert_eq!(
        trace_last, run_summary,
        "脚本 trace 的汇总行必须与 run-scripts 末行逐字一致（同一条步进路径）"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn trace_reports_script_errors_as_exit_code_two() {
    // 坏脚本不能"静默跑完退出 0"：trace 的用途就是拿它和真实运行对表，
    // 脚本没生效却报成功会让对表结论整体失真。
    let (path, assets) =
        scripted_world_with("broken", "function update(ctx)\n  this is not lua\nend\n");
    let output = trace_with_assets(&path, "3", &assets);
    assert_eq!(
        output.status.code(),
        Some(2),
        "脚本语法错误必须退出码 2：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("第 1 帧运行失败"), "错误必须点名帧号：{text}");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn update_errors_are_reported_like_load_errors() {
    // `run_frame` 里的错误是分阶段记的（load / start / update / plan / apply），
    // 只测 load 期错误的话，update 分支的 `note_error` 重构改坏了也看不见。
    let (path, assets) = scripted_world_with(
        "update-error",
        "function update(ctx)\n  error(\"boom\")\nend\n",
    );
    let output = trace_with_assets(&path, "3", &assets);
    assert_eq!(
        output.status.code(),
        Some(2),
        "update 里抛错必须退出码 2：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("第 1 帧运行失败"), "错误必须点名帧号：{text}");
    assert!(text.contains("boom"), "错误必须带上脚本原文：{text}");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn run_scripts_can_write_the_final_world() {
    // 割接对表的入口：旧端用 `--dump-scene` 落终态，XGC 侧必须能在同一条
    // 「系统 → 脚本 → 粒子」路径跑完后把世界原样落盘，否则只能比截图。
    let (path, assets) = scripted_world("dump");
    let dir = path.parent().unwrap();
    let first_out = dir.join("final-a.json");
    let second_out = dir.join("final-b.json");

    let run = |out: &Path| {
        Command::new(binary())
            .arg("run-scripts")
            .arg(&path)
            .arg(&assets)
            .arg("3")
            .arg(out)
            .output()
            .unwrap()
    };

    let first = run(&first_out);
    assert!(
        first.status.success(),
        "run-scripts 落盘必须成功：{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let text = String::from_utf8(first.stdout).unwrap();
    assert!(text.contains("已写出世界"), "必须报告落盘路径：{text}");
    assert!(first_out.is_file(), "输出世界必须真的写出来");

    let data = load_world(&first_out).unwrap();
    let mut world = World::new();
    world.restore_world_data(&data).unwrap();
    let mut positions = Vec::new();
    world.for_each::<Transform>(|_, transform| positions.push(transform.position));
    assert_eq!(positions.len(), 1, "落盘世界必须保住原实体数");
    assert_eq!(positions[0][0], 5.0, "落盘的是脚本跑完后的终态，不是初始态");

    let second = run(&second_out);
    assert!(second.status.success());
    assert_eq!(
        std::fs::read(&first_out).unwrap(),
        std::fs::read(&second_out).unwrap(),
        "同一世界 + 同一帧数的两次落盘必须逐字节相同"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn parallel_threads_do_not_change_the_trace() {
    let path = world_file("parallel", 0.0);
    let serial = trace(&path, "10");
    let parallel = trace_threads(&path, "10", "4");
    assert!(serial.status.success());
    assert!(
        parallel.status.success(),
        "带线程数的 trace 必须成功：{}",
        String::from_utf8_lossy(&parallel.stderr)
    );
    assert_eq!(
        String::from_utf8(serial.stdout).unwrap(),
        String::from_utf8(parallel.stdout).unwrap(),
        "线程数只影响执行方式，逐帧哈希必须逐字节一致"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn parallel_threads_do_not_change_run_world() {
    let path = world_file("parallel-run", 0.0);
    let serial = Command::new(binary())
        .arg("run-world")
        .arg(&path)
        .arg("6")
        .output()
        .unwrap();
    let parallel = Command::new(binary())
        .arg("run-world")
        .arg(&path)
        .arg("6")
        .arg("4")
        .output()
        .unwrap();
    assert!(serial.status.success());
    assert!(
        parallel.status.success(),
        "run-world 带线程数必须成功：{}",
        String::from_utf8_lossy(&parallel.stderr)
    );
    assert_eq!(
        String::from_utf8(serial.stdout).unwrap(),
        String::from_utf8(parallel.stdout).unwrap(),
        "线程数不能改变最终哈希"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
