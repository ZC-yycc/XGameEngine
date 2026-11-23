//! `xgc parity`：把结构性差异拆成轮廓 1px 带与内部两部分，并按门槛判定。
//!
//! 这里只钉 CLI 的判定逻辑与退出码；验收数字由 `xgc parity acceptance/frame-N.png
//! <render-frame 输出>` 在验收机上跑（旧端参考图必须在那台机器重新生成）。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_xgc")
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("xgc-parity-{}-{label}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 写一张 PPM：`pixel(x, y) -> [r, g, b]`。
fn write_ppm(path: &Path, width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) {
    let mut bytes = format!("P6\n{width} {height}\n255\n").into_bytes();
    for y in 0..height {
        for x in 0..width {
            bytes.extend_from_slice(&pixel(x, y));
        }
    }
    std::fs::write(path, bytes).unwrap();
}

fn run(args: &[&str]) -> Output {
    Command::new(binary()).args(args).output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn identical_images_pass() {
    let dir = temp_dir("identical");
    let left = dir.join("left.ppm");
    let right = dir.join("right.ppm");
    write_ppm(&left, 32, 32, |x, y| [x as u8 * 4, y as u8 * 4, 128]);
    write_ppm(&right, 32, 32, |x, y| [x as u8 * 4, y as u8 * 4, 128]);
    let output = run(&["parity", left.to_str().unwrap(), right.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    assert!(text.contains("结构性 0（0.000%"), "{text}");
    assert!(text.contains("内部 0"), "{text}");
    assert!(text.contains("-> 通过"), "{text}");
}

#[test]
fn interior_blot_exceeding_the_limit_fails() {
    let dir = temp_dir("interior");
    let left = dir.join("left.ppm");
    let right = dir.join("right.ppm");
    // 64×64 纯色底；右图每隔 8px 放一个 +30 的孤立像素（邻域差 30 < 40 ⇒ 不算轮廓）。
    let blot = |x: u32, y: u32| x % 8 == 1 && y % 8 == 1;
    write_ppm(&left, 64, 64, |_, _| [100, 100, 100]);
    write_ppm(&right, 64, 64, |x, y| {
        if blot(x, y) {
            [130, 100, 100]
        } else {
            [100, 100, 100]
        }
    });
    let output = run(&["parity", left.to_str().unwrap(), right.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let text = stdout(&output);
    assert!(text.contains("-> 未通过"), "{text}");
    assert!(text.contains("内部 64"), "{text}");
}

#[test]
fn edge_only_differences_pass_when_the_structural_limit_is_loose() {
    let dir = temp_dir("edge");
    let left = dir.join("left.ppm");
    let right = dir.join("right.ppm");
    // 黑底白竖条整体右移 1 列：差异像素全部贴着轮廓。
    write_ppm(&left, 64, 64, |x, _| {
        if x == 32 {
            [255, 255, 255]
        } else {
            [0, 0, 0]
        }
    });
    write_ppm(&right, 64, 64, |x, _| {
        if x == 33 {
            [255, 255, 255]
        } else {
            [0, 0, 0]
        }
    });
    let output = run(&[
        "parity",
        left.to_str().unwrap(),
        right.to_str().unwrap(),
        "5",
        "0.01",
    ]);
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    assert!(text.contains("内部 0"), "{text}");
    assert!(text.contains("-> 通过"), "{text}");
}

#[test]
fn usage_errors_exit_with_two() {
    let output = run(&["parity"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");

    let dir = temp_dir("usage");
    let left = dir.join("left.ppm");
    let right = dir.join("right.ppm");
    write_ppm(&left, 8, 8, |_, _| [0, 0, 0]);
    write_ppm(&right, 8, 8, |_, _| [0, 0, 0]);
    let output = run(&[
        "parity",
        left.to_str().unwrap(),
        right.to_str().unwrap(),
        "-1",
    ]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");

    // 尺寸不同：读图成功但没法比 ⇒ 用法错误而不是"未通过"。
    let small = dir.join("small.ppm");
    write_ppm(&small, 4, 8, |_, _| [0, 0, 0]);
    let output = run(&["parity", left.to_str().unwrap(), small.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}
