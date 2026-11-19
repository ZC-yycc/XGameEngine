//! `xgc compare-headless <旧 headless.json> <世界.json>`：统计对照。

use std::path::Path;
use std::process::ExitCode;

use xgc_migrate::compare_headless;
use xgc_render::{diff_breakdown, diff_images, ImageData};
use xgc_store::load_world;

pub fn compare(old: Option<&str>, world: Option<&str>) -> ExitCode {
    let (Some(old), Some(world)) = (old, world) else {
        eprintln!("用法：xgc compare-headless <旧 headless.json> <世界.json>");
        return ExitCode::from(2);
    };
    let text = match std::fs::read_to_string(Path::new(old)) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("读取旧 headless JSON 失败：{err}");
            return ExitCode::from(2);
        }
    };
    let data = match load_world(Path::new(world)) {
        Ok(data) => data,
        Err(err) => {
            eprintln!("读取世界失败：{err}");
            return ExitCode::from(2);
        }
    };
    match compare_headless(&text, &data) {
        Ok(report) => {
            println!(
                "compare-headless: entities {} / bodies {} / colliders {} / emitters {} / audio {}",
                report.new.entities,
                report.new.bodies,
                report.new.colliders,
                report.new.emitters,
                report.new.audio_active
            );
            for line in &report.differences {
                eprintln!("差异：{line}");
            }
            if report.differences.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(errors) => {
            for line in errors {
                eprintln!("{line}");
            }
            ExitCode::from(1)
        }
    }
}

/// `xgc image-diff <旧图> <新图> [阈值] [report]`：PNG / PPM 逐像素差异。
///
/// 第四个参数写 `report` 时额外打印「余差归因」：按最大通道差把像素分成
/// 相同 / ±1 / 2-3 / 4-7 / >=8 五桶，再给三通道均值与最差的 16×9 块。
/// 视觉割接门槛只报一个 32% 会把渲染舍入和阴影 / UI 的结构性差异混在一起，
/// 归因才能判断余差该从哪条渲染路径收。
///
/// 注意两套均值的分母不同：上面的 `mean_abs` 是 `diff_images` 的 **RGBA 四通道**
/// 平均绝对差（含 alpha），而归因里的 R / G / B 是 **RGB 三通道**各自对像素数求均值
/// ——口径不同（四通道 vs 三通道），不能当作同一指标直接比较。
pub fn image(
    old: Option<&str>,
    new: Option<&str>,
    threshold: Option<&str>,
    report: Option<&str>,
) -> ExitCode {
    let (Some(old), Some(new)) = (old, new) else {
        eprintln!("用法：xgc image-diff <旧图> <新图> [阈值] [report]");
        return ExitCode::from(2);
    };
    if !matches!(report, None | Some("report")) {
        eprintln!("用法：xgc image-diff <旧图> <新图> [阈值] [report]");
        return ExitCode::from(2);
    }
    let threshold = threshold
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(0);
    let left = match ImageData::load(Path::new(old)) {
        Ok(image) => image,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };
    let right = match ImageData::load(Path::new(new)) {
        Ok(image) => image,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };
    match diff_images(&left, &right, threshold) {
        Ok(diff) => {
            println!(
                "image-diff: {}x{} 差异 {}/{} ({:.2}%) mean_abs={:.2} max={}",
                diff.width,
                diff.height,
                diff.different,
                diff.pixels,
                diff.ratio * 100.0,
                diff.mean_abs,
                diff.max_delta
            );
            if report.is_some() {
                print_breakdown(&left, &right);
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

/// 打印余差归因。分桶用最大 RGB 通道差，跟 `[阈值]` 无关——两者的用途不同：
/// 阈值决定"算不算不同"，分桶回答"差多少"。
fn print_breakdown(left: &ImageData, right: &ImageData) {
    let report = match diff_breakdown(left, right) {
        Ok(report) => report,
        Err(err) => {
            // `diff_images` 已经挡过尺寸不同，这里实际不可达；真到了就只打印错、
            // 不改退出码（总览的结果仍然有效，归因只是附加信息）。
            debug_assert!(false, "尺寸校验应与 diff_images 一致：{err}");
            eprintln!("{err}");
            return;
        }
    };
    let buckets = report.buckets;
    println!(
        "image-diff-report: 分桶 相同={} ±1={} 2-3={} 4-7={} >=8={} 结构性={:.2}%",
        buckets.identical,
        buckets.one,
        buckets.two_to_three,
        buckets.four_to_seven,
        buckets.structural,
        buckets.structural_ratio(report.pixels) * 100.0
    );
    println!(
        "image-diff-report: 通道均值 R={:.2} G={:.2} B={:.2}",
        report.channel_mean_abs[0], report.channel_mean_abs[1], report.channel_mean_abs[2]
    );
    for block in &report.worst_blocks {
        println!(
            "image-diff-report: 最差块 row={} col={} 差异={:.2}%",
            block.row,
            block.column,
            block.ratio * 100.0
        );
    }
}
