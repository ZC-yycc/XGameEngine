//! `xgc compare-headless <旧 headless.json> <世界.json>`：统计对照。

use std::path::Path;
use std::process::ExitCode;

use xgc_migrate::compare_headless;
use xgc_render::{diff_images, ImageData};
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

/// `xgc image-diff <旧图> <新图> [阈值]`：PNG / PPM 逐像素差异。
pub fn image(old: Option<&str>, new: Option<&str>, threshold: Option<&str>) -> ExitCode {
    let (Some(old), Some(new)) = (old, new) else {
        eprintln!("用法：xgc image-diff <旧图> <新图> [阈值]");
        return ExitCode::from(2);
    };
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
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}
