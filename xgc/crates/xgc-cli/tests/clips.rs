//! `xgc clips` 对模型文件：打印 glTF 自带剪辑（目录模式见 lib 的 `.ron` 库用例）。

use std::path::Path;
use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_xgc")
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .args(args)
        .output()
        .expect("启动 xgc")
}

#[test]
fn clips_lists_gltf_animations() {
    let path = Path::new("../../../projects/XGame/Resources/Fox.gltf");
    if !path.exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let path = path.to_str().unwrap();
    let first = run(&["clips", path]);
    assert!(first.status.success(), "{:?}", first);
    let stdout = String::from_utf8_lossy(&first.stdout);
    for clip in ["Survey", "Walk", "Run"] {
        assert!(stdout.contains(clip), "缺剪辑 {clip}: {stdout}");
    }
    assert!(stdout.contains("channels=21"), "{stdout}");
    let second = run(&["clips", path]);
    assert_eq!(first.stdout, second.stdout, "输出必须逐字节确定");
}

#[test]
fn clips_reports_models_without_animations() {
    let path = Path::new("../../../projects/XGame/Resources/Box.glb");
    if !path.exists() {
        eprintln!("skip: 没有 Box.glb");
        return;
    }
    let output = run(&["clips", path.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("没有 glTF 动画"), "{stdout}");
}
