//! `xgc gate`：逐帧 trace 对照的通过 / 不通过 / 配置错误三条退出码。
//!
//! 截图判定需要 wgpu 设备，不进这里：本文件只钉「状态轨迹门禁」这条，
//! 保证 CI（没有显卡）也能跑；截图阈值判定由验收机本地跑
//! `xgc gate … acceptance` 时覆盖。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use xgc_store::save_world;
use xgc_world::{Script, Transform, World};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_xgc")
}

/// 带脚本的小世界 + 资产根：脚本每帧写 x=5，让逐帧哈希真的在变化，
/// 这样「改一行参考」一定会被抓到。
fn scripted_world(label: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("xgc-gate-{}-{label}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let assets = dir.join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(
        assets.join("move.lua"),
        "function update(ctx)\n  set_pos(ctx, 5.0, 0.0, 0.0)\nend\n",
    )
    .unwrap();
    let path = dir.join("world.json");
    let mut world = World::new();
    world.spawn((
        Transform::default(),
        Script {
            paths: vec!["move.lua".into()],
        },
    ));
    save_world(&path, &world.world_data()).unwrap();
    (path, assets)
}

fn run(args: &[&str]) -> Output {
    Command::new(binary()).args(args).output().unwrap()
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// 用 `xgc trace` 自己生成参考（与门禁重放同一条运行路径），写到 `references/trace.txt`。
fn reference_dir(label: &str, world: &Path, assets: &Path) -> PathBuf {
    let references = std::env::temp_dir()
        .join(format!("xgc-gate-ref-{}-{label}", std::process::id()))
        .join("references");
    let _ = std::fs::remove_dir_all(&references);
    std::fs::create_dir_all(&references).unwrap();
    let trace = run(&[
        "trace",
        &world.to_string_lossy(),
        "4",
        &assets.to_string_lossy(),
    ]);
    assert!(trace.status.success(), "{}", stderr_of(&trace));
    std::fs::write(references.join("trace.txt"), trace.stdout).unwrap();
    references
}

#[test]
fn gate_passes_when_the_trace_reference_matches() {
    let (world, assets) = scripted_world("match");
    let references = reference_dir("match", &world, &assets);
    let output = run(&[
        "gate",
        &world.to_string_lossy(),
        &assets.to_string_lossy(),
        &references.to_string_lossy(),
    ]);
    assert!(output.status.success(), "{}", stderr_of(&output));
    let stdout = stdout_of(&output);
    assert!(
        stdout.contains("trace 逐帧对照通过（5 帧）"),
        "stdout={stdout}"
    );
    assert!(stdout.contains("gate: 全部通过"), "stdout={stdout}");
    let _ = std::fs::remove_dir_all(&references);
}

#[test]
fn gate_fails_when_a_frame_hash_diverges() {
    let (world, assets) = scripted_world("diverge");
    let references = reference_dir("diverge", &world, &assets);
    let path = references.join("trace.txt");
    let text = std::fs::read_to_string(&path).unwrap();
    // 把第 3 帧的 hash 最后一个十六进制位改掉；其余帧保持原样。
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            if line.starts_with("frame=3 ") {
                format!("{}0", &line[..line.len() - 1])
            } else {
                line.to_string()
            }
        })
        .collect();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    let output = run(&[
        "gate",
        &world.to_string_lossy(),
        &assets.to_string_lossy(),
        &references.to_string_lossy(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr_of(&output);
    assert!(stderr.contains("trace 第 3 帧不一致"), "stderr={stderr}");
    assert!(stderr.contains("1 项未通过"), "stderr={stderr}");
    let _ = std::fs::remove_dir_all(&references);
}

#[test]
fn gate_rejects_an_empty_reference_directory() {
    let (world, assets) = scripted_world("empty");
    let empty = std::env::temp_dir().join(format!("xgc-gate-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&empty);
    std::fs::create_dir_all(&empty).unwrap();
    let output = run(&[
        "gate",
        &world.to_string_lossy(),
        &assets.to_string_lossy(),
        &empty.to_string_lossy(),
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr_of(&output).contains("空目录不能算通过"));
    let _ = std::fs::remove_dir_all(&empty);
}

#[test]
fn gate_rejects_a_reference_that_does_not_start_at_frame_zero() {
    let (world, assets) = scripted_world("offset");
    let references = reference_dir("offset", &world, &assets);
    let path = references.join("trace.txt");
    let text = std::fs::read_to_string(&path).unwrap();
    let rest: Vec<&str> = text.lines().skip(1).collect();
    std::fs::write(&path, rest.join("\n") + "\n").unwrap();
    let output = run(&[
        "gate",
        &world.to_string_lossy(),
        &assets.to_string_lossy(),
        &references.to_string_lossy(),
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr_of(&output).contains("必须从 0 开始连续递增"));
    let _ = std::fs::remove_dir_all(&references);
}
