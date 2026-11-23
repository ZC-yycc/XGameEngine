//! `xgc gate <世界.json> <资产根> <参考目录> [总差异%] [结构性%]`：割接门禁。
//!
//! 把 CUTOVER 里靠人眼 + 一次性脚本判断的两件事做成可复现命令：
//!
//! - **逐帧 trace 对照**：参考目录里的 `trace.txt` 是 `xgc trace` 的原样输出
//!   （逐帧 `frame=N entities=E revision=R hash=H`）。门禁按同一条
//!   `ScriptedRunner` 路径重放并逐帧比 hash：`revision` 是内部计数器（同一批
//!   实体换顺序都会变），不参与比对；`hash` 才是状态内容指纹，实体数一起比是
//!   为了区分「数量变了」和「取值变了」。
//! - **截图阈值判定**：参考目录里的 `frame-<N>.png|ppm` 是旧端在验收机生成的
//!   参考截图。门禁按 `render-frame` 同一条渲染路径（同一份 `render_world_rgba`）
//!   渲染第 N 帧，再用总差异比例与结构性（单通道 ≥8）比例两条线判定。阈值缺省
//!   2% / 0.5%，可用第 4 / 5 个参数覆盖；机器换了参考图就必须重新生成——
//!   旧端参考跨 GPU 环境会有整片 ±1 的舍入差，不重生成会把门禁变成噪声。
//!
//! 退出码：0 全部通过；1 有检查未过（门禁判不通过）；2 用法 / IO / GPU 错误。
//! 目录里两类参考都没有时按 2 处理：空目录不能当成「通过」。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use xgc_render::{diff_breakdown, diff_images, ImageData};
use xgc_sim::{HierarchySystem, System, UiLayoutSystem};

use super::render::render_world_rgba;
use super::runtime_cmd::ScriptedRunner;
use super::world_cmd::load_world_state;

/// 缺省判定线：总差异像素比例与结构性（单通道 ≥8）比例。
const DEFAULT_MAX_TOTAL_PCT: f64 = 2.0;
const DEFAULT_MAX_STRUCTURAL_PCT: f64 = 0.5;

/// 单个参考截图的判定线（比例 0..1）。
#[derive(Clone, Copy, Debug)]
struct VisualLimits {
    total_ratio: f32,
    structural_ratio: f32,
}

/// `trace.txt` 里一帧的期望值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrameExpect {
    entities: usize,
    hash: u64,
}

pub fn gate(
    world_path: Option<&str>,
    assets_root: Option<&str>,
    references: Option<&str>,
    max_total: Option<&str>,
    max_structural: Option<&str>,
) -> ExitCode {
    let (Some(world_path), Some(assets_root), Some(references)) =
        (world_path, assets_root, references)
    else {
        eprintln!("用法：xgc gate <世界.json> <资产根> <参考目录> [总差异%] [结构性%]");
        return ExitCode::from(2);
    };
    let limits = match visual_limits(max_total, max_structural) {
        Ok(limits) => limits,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let references = Path::new(references);
    let expected = match read_trace_reference(&references.join("trace.txt")) {
        Ok(expected) => expected,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let visuals = match visual_references(references) {
        Ok(visuals) => visuals,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    if expected.is_none() && visuals.is_empty() {
        eprintln!(
            "参考目录 {} 里既没有 trace.txt 也没有 frame-*.png/ppm：空目录不能算通过",
            references.display()
        );
        return ExitCode::from(2);
    }

    let mut failures = 0usize;
    if let Some(expected) = expected {
        match check_trace(Path::new(world_path), Path::new(assets_root), &expected) {
            Ok(count) => failures += count,
            Err(message) => {
                eprintln!("{message}");
                return ExitCode::from(2);
            }
        }
    }
    for (frame, reference) in visuals {
        match check_visual(
            Path::new(world_path),
            assets_root,
            frame,
            &reference,
            limits,
        ) {
            Ok(true) => {}
            Ok(false) => failures += 1,
            Err(message) => {
                eprintln!("{message}");
                return ExitCode::from(2);
            }
        }
    }

    if failures == 0 {
        println!("gate: 全部通过");
        ExitCode::SUCCESS
    } else {
        eprintln!("gate: {failures} 项未通过");
        ExitCode::from(1)
    }
}

fn visual_limits(
    max_total: Option<&str>,
    max_structural: Option<&str>,
) -> Result<VisualLimits, String> {
    fn percent(value: Option<&str>, default: f64, label: &str) -> Result<f32, String> {
        let percent = match value {
            Some(text) => text
                .parse::<f64>()
                .map_err(|_| format!("{label}不是数字：{text}"))?,
            None => default,
        };
        if !(0.0..=100.0).contains(&percent) {
            return Err(format!("{label}必须在 0..=100 之间：{percent}"));
        }
        Ok((percent / 100.0) as f32)
    }
    Ok(VisualLimits {
        total_ratio: percent(max_total, DEFAULT_MAX_TOTAL_PCT, "总差异阈值")?,
        structural_ratio: percent(max_structural, DEFAULT_MAX_STRUCTURAL_PCT, "结构性阈值")?,
    })
}

/// 读 `trace.txt`；文件不存在返回 `Ok(None)`（参考目录只放截图是合法的）。
fn read_trace_reference(path: &Path) -> Result<Option<Vec<(u64, FrameExpect)>>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("读取 {} 失败：{err}", path.display())),
    };
    let mut frames = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || !line.starts_with("frame=") {
            continue;
        }
        let mut frame = None;
        let mut entities = None;
        let mut hash = None;
        for token in line.split_whitespace() {
            let Some((key, value)) = token.split_once('=') else {
                continue;
            };
            match key {
                "frame" => frame = value.parse::<u64>().ok(),
                "entities" => entities = value.parse::<usize>().ok(),
                "hash" => hash = u64::from_str_radix(value, 16).ok(),
                _ => {}
            }
        }
        let (Some(frame), Some(entities), Some(hash)) = (frame, entities, hash) else {
            return Err(format!(
                "{} 第 {} 行缺 frame / entities / hash：{line}",
                path.display(),
                index + 1
            ));
        };
        frames.push((frame, FrameExpect { entities, hash }));
    }
    if frames.is_empty() {
        return Err(format!("{} 里没有可用的 frame= 行", path.display()));
    }
    for (offset, (frame, _)) in frames.iter().enumerate() {
        if *frame != offset as u64 {
            return Err(format!(
                "{} 的帧号必须从 0 开始连续递增，第 {} 条是 frame={frame}",
                path.display(),
                offset + 1
            ));
        }
    }
    Ok(Some(frames))
}

/// 收集 `frame-<N>.png|ppm`；按帧号升序（渲染顺序稳定，输出可复现）。
fn visual_references(dir: &Path) -> Result<Vec<(u64, PathBuf)>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("读取参考目录 {} 失败：{err}", dir.display())),
    };
    let mut references = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| format!("读取参考目录失败：{err}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(rest) = name.strip_prefix("frame-") else {
            continue;
        };
        let Some((stem, extension)) = rest.rsplit_once('.') else {
            continue;
        };
        if !matches!(extension.to_ascii_lowercase().as_str(), "png" | "ppm") {
            continue;
        }
        let frame = stem.parse::<u64>().map_err(|_| {
            format!(
                "参考截图文件名必须是 frame-<帧号>.png|ppm，看不懂：{}",
                path.display()
            )
        })?;
        references.push((frame, path));
    }
    references.sort_by_key(|(frame, _)| *frame);
    Ok(references)
}

fn check_trace(
    world_path: &Path,
    assets_root: &Path,
    expected: &[(u64, FrameExpect)],
) -> Result<usize, String> {
    let world = load_world_state(&world_path.to_string_lossy())
        .map_err(|_| format!("加载世界 {} 失败", world_path.display()))?;
    let mut runner = ScriptedRunner::new(world, assets_root);
    let mut failures = 0usize;
    let last = expected.last().map(|(frame, _)| *frame).unwrap_or(0);
    for frame in 0..=last {
        if frame > 0 {
            runner
                .step()
                .map_err(|err| format!("第 {frame} 帧运行失败：{err}"))?;
        }
        let want = expected[frame as usize].1;
        let world = runner.world();
        let hash = world.state_hash();
        if hash != want.hash || world.len() != want.entities {
            eprintln!(
                "trace 第 {frame} 帧不一致：期望 entities={} hash={:016x}，实际 entities={} hash={:016x}",
                want.entities,
                want.hash,
                world.len(),
                hash
            );
            failures += 1;
        }
    }
    if failures == 0 {
        println!("gate: trace 逐帧对照通过（{} 帧）", last + 1);
    }
    Ok(failures)
}

fn check_visual(
    world_path: &Path,
    assets_root: &str,
    frame: u64,
    reference: &Path,
    limits: VisualLimits,
) -> Result<bool, String> {
    let expected = ImageData::load(reference)?;
    let (width, height) = (expected.width, expected.height);
    let world = load_world_state(&world_path.to_string_lossy())
        .map_err(|_| format!("加载世界 {} 失败", world_path.display()))?;
    let mut runner = ScriptedRunner::new(world, Path::new(assets_root));
    for step in 1..=frame {
        runner
            .step()
            .map_err(|err| format!("第 {step} 帧运行失败：{err}"))?;
    }
    let (mut world, particles) = runner.into_parts();
    // 与 `render-frame` 同序：先推进到第 N 帧，再补派生全局变换与 UI 布局。
    HierarchySystem::default().run(&mut world, 0.0);
    UiLayoutSystem {
        viewport: [width as f32, height as f32],
    }
    .run(&mut world, 0.0);
    let (pixels, _) = render_world_rgba(Some(assets_root), &world, particles, width, height)?;
    let actual = ImageData {
        width,
        height,
        rgba: pixels
            .chunks_exact(4)
            .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
            .collect(),
    };
    let diff = diff_images(&expected, &actual, 0)?;
    let report = diff_breakdown(&expected, &actual)?;
    let structural = report.buckets.structural_ratio(report.pixels);
    let ok = diff.ratio <= limits.total_ratio && structural <= limits.structural_ratio;
    println!(
        "gate: frame-{frame} 差异 {:.2}%（限 {:.2}%）结构性 {:.2}%（限 {:.2}%）mean_abs {:.2} max {} -> {}",
        diff.ratio * 100.0,
        limits.total_ratio * 100.0,
        structural * 100.0,
        limits.structural_ratio * 100.0,
        diff.mean_abs,
        diff.max_delta,
        if ok { "通过" } else { "未通过" }
    );
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_default_to_the_recorded_visual_baseline() {
        let limits = visual_limits(None, None).unwrap();
        assert!((limits.total_ratio - 0.02).abs() < 1e-6);
        assert!((limits.structural_ratio - 0.005).abs() < 1e-6);
    }

    #[test]
    fn limits_reject_out_of_range_and_garbage() {
        assert!(visual_limits(Some("101"), None).is_err());
        assert!(visual_limits(None, Some("-1")).is_err());
        assert!(visual_limits(Some("abc"), None).is_err());
        let limits = visual_limits(Some("1.5"), Some("0.25")).unwrap();
        assert!((limits.total_ratio - 0.015).abs() < 1e-6);
        assert!((limits.structural_ratio - 0.0025).abs() < 1e-6);
    }

    #[test]
    fn trace_reference_requires_a_contiguous_sequence_from_zero() {
        let dir = std::env::temp_dir().join(format!("xgc-gate-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trace.txt");
        std::fs::write(
            &path,
            "frame=0 entities=1 revision=1 hash=00000000000000ab\n\
             trace: frames=1 entities=1 revision=1 hash=00000000000000ab\n",
        )
        .unwrap();
        let parsed = read_trace_reference(&path).unwrap().unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].1.entities, 1);
        assert_eq!(parsed[0].1.hash, 0xab);

        std::fs::write(
            &path,
            "frame=1 entities=1 revision=1 hash=00000000000000ab\n",
        )
        .unwrap();
        assert!(read_trace_reference(&path).is_err());

        std::fs::write(&path, "frame=0 entities=1 revision=1\n").unwrap();
        assert!(read_trace_reference(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
