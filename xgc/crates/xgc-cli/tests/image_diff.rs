//! `xgc image-diff … report` 的集成回归：归因分桶 / 通道均值 / 最差块。
//!
//! 归因数字会直接进割接文档（"余差里 26% 是 ±1 舍入、其余是结构性差异"），
//! 所以这里用手工 PPM 把每一个桶钉死；同时确认不加 `report` 时输出不变。

use std::process::Command;

const WIDTH: usize = 32;
const HEIGHT: usize = 18;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_xgc")
}

fn temp_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("xgc-image-diff-{}-{label}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

/// 写 PPM P6：像素由回调给 RGB。
///
/// 像素值刻意避开 ASCII 空白（9/10/11/12/13/32）：解码器在 maxval 之后
/// 会跳过一个空白，若首像素首通道恰好是 `\n` / 空格就会被吃掉。
fn write_ppm(
    path: &std::path::Path,
    pixel: impl Fn(usize, usize) -> [u8; 3],
) -> std::path::PathBuf {
    let mut bytes = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            bytes.extend_from_slice(&pixel(x, y));
        }
    }
    std::fs::write(path, bytes).expect("写 PPM");
    path.to_path_buf()
}

#[test]
fn report_splits_rounding_from_structural_differences() {
    let dir = temp_dir("report");
    let base = write_ppm(&dir.join("base.ppm"), |_, _| [40, 50, 60]);
    let noise = write_ppm(&dir.join("noise.ppm"), |x, y| {
        if (x, y) == (0, 0) {
            [41, 50, 60] // ±1
        } else if (x, y) == (1, 0) {
            [42, 50, 60] // 2..=3
        } else if (x, y) == (2, 0) {
            [40, 54, 60] // 4..=7
        } else if (x, y) == (3, 0) {
            [60, 50, 60] // >= 8
        } else {
            [40, 50, 60]
        }
    });
    let output = Command::new(binary())
        .args([
            "image-diff",
            base.to_str().unwrap(),
            noise.to_str().unwrap(),
            "0",
            "report",
        ])
        .output()
        .expect("运行 xgc image-diff");
    assert!(output.status.success(), "归因必须成功：{:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let pixels = (WIDTH * HEIGHT) as f64;
    assert!(
        stdout.contains(&format!(
            "分桶 相同={} ±1=1 2-3=1 4-7=1 >=8=1",
            WIDTH * HEIGHT - 4
        )),
        "分桶不对：{stdout}"
    );
    assert!(
        stdout.contains(&format!("结构性={:.2}%", 100.0 / pixels)),
        "结构性占比不对：{stdout}"
    );
    assert!(
        stdout.contains("通道均值 R=0.04 G=0.01 B=0.00"),
        "通道均值不对：{stdout}"
    );
    // 32x18 切成 16x9 块后每块 2x2：阈值附近的像素都落在行 0 的前两块里。
    assert!(
        stdout.contains("最差块 row=0 col=1 差异=50.00%"),
        "最差块不对：{stdout}"
    );
    assert!(
        stdout.contains("最差块 row=0 col=0 差异=50.00%"),
        "第二差块不对：{stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn plain_diff_keeps_its_old_output() {
    let dir = temp_dir("plain");
    let left = write_ppm(&dir.join("left.ppm"), |_, _| [40, 50, 60]);
    let right = write_ppm(&dir.join("right.ppm"), |_, _| [40, 50, 60]);
    let output = Command::new(binary())
        .args([
            "image-diff",
            left.to_str().unwrap(),
            right.to_str().unwrap(),
        ])
        .output()
        .expect("运行 xgc image-diff");
    assert!(output.status.success(), "{:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(stdout.contains("差异 0/576 (0.00%)"), "总览不对：{stdout}");
    assert!(
        !stdout.contains("image-diff-report"),
        "没写 report 时不该多打归因：{stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_fourth_argument_is_a_usage_error() {
    let dir = temp_dir("usage");
    let left = write_ppm(&dir.join("left.ppm"), |_, _| [0, 0, 0]);
    let right = write_ppm(&dir.join("right.ppm"), |_, _| [0, 0, 0]);
    let output = Command::new(binary())
        .args([
            "image-diff",
            left.to_str().unwrap(),
            right.to_str().unwrap(),
            "0",
            "verbose",
        ])
        .output()
        .expect("运行 xgc image-diff");
    assert_eq!(output.status.code(), Some(2), "应为用法错误退出码 2");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stderr.contains("用法：xgc image-diff"),
        "用法串不对：{stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
