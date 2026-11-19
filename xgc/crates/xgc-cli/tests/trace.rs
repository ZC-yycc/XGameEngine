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
