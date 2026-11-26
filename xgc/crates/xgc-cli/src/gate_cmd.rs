//! `xgc gate <世界.json> <资产根> <参考目录> [总差异%] [结构性%] [--strict-total]`：
//! 割接门禁。
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
//!   渲染第 N 帧，再用总差异比例与结构性（单通道 ≥8）比例两条线判定。
//!
//! **跨机器的总差异不可信**：旧端参考换 GPU / 驱动会有整片 ±1 的舍入差。参考
//! 目录里的 `env.json` 记录实际生成参考的 GPU 与驱动指纹；完整指纹一致
//! （或显式给了总差异阈值 / `--strict-total`）时，才把总差异当
//! 硬阈值，否则它只打印为诊断，结构性阈值仍然判定。这样仓库里的参考图在别的
//! 机器上不会变成 16% 的假回归，也不会把真正的结构性变化放过去。
//!
//! 退出码：0 全部通过；1 有检查未过（门禁判不通过）；2 用法 / IO / GPU 错误。
//! 目录里两类参考都没有时按 2 处理：空目录不能当成「通过」。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use xgc_render::{diff_breakdown, diff_images, GpuContext, ImageData};
use xgc_sim::{HierarchySystem, System, UiLayoutSystem};

use super::render::render_world_rgba;
use super::world_cmd::load_world_state;
use crate::runtime_cmd::scripted_runner;

/// 缺省判定线：总差异像素比例与结构性（单通道 ≥8）比例。
const DEFAULT_MAX_TOTAL_PCT: f64 = 2.0;
const DEFAULT_MAX_STRUCTURAL_PCT: f64 = 0.5;
const ENV_FILE: &str = "env.json";

/// 单个参考截图的判定线（比例 0..1）。`total_ratio = None` 表示只打印诊断。
#[derive(Clone, Copy, Debug)]
struct VisualLimits {
    total_ratio: Option<f32>,
    structural_ratio: f32,
}

/// 缺少驱动信息的参考仍可对照结构差异，不能确认总差异的舍入口径。
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct GateEnv {
    gpu: String,
    backend: String,
    #[serde(default)]
    driver: String,
    #[serde(default)]
    driver_info: String,
}

impl GateEnv {
    fn from_context(context: &GpuContext) -> Self {
        let info = context.adapter_info();
        Self {
            gpu: info.name.clone(),
            backend: format!("{:?}", info.backend),
            driver: info.driver.clone(),
            driver_info: info.driver_info.clone(),
        }
    }

    fn matches(&self, other: &Self) -> bool {
        self == other && !self.driver.trim().is_empty() && !self.driver_info.trim().is_empty()
    }
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
    strict_total: bool,
) -> ExitCode {
    let (Some(world_path), Some(assets_root), Some(references)) =
        (world_path, assets_root, references)
    else {
        eprintln!(
            "用法：xgc gate <世界.json> <资产根> <参考目录> [总差异%] [结构性%] [--strict-total]"
        );
        return ExitCode::from(2);
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

    // 只有结构性差异跨机器稳定；总差异要靠环境指纹确认参考图是同一种
    // 舍入口径，否则整片 ±1 会把门禁变成假回归。
    let context = if visuals.is_empty() {
        None
    } else {
        match GpuContext::headless() {
            Ok(context) => Some(context),
            Err(err) => {
                eprintln!("GPU 不可用：{err}");
                return ExitCode::from(2);
            }
        }
    };
    let current_env = context.as_ref().map(GateEnv::from_context);
    let reference_env = match read_gate_env(&references.join(ENV_FILE)) {
        Ok(env) => env,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let env_matches = matches!(
        (&current_env, &reference_env),
        (Some(current), Some(reference)) if current.matches(reference)
    );
    let enforce_total = strict_total || max_total.is_some() || env_matches;
    let limits = match visual_limits(max_total, max_structural, enforce_total) {
        Ok(limits) => limits,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    if let Some(current) = &current_env {
        println!(
            "gate: gpu={} backend={} driver={} driver_info={}",
            current.gpu, current.backend, current.driver, current.driver_info
        );
        match (&reference_env, env_matches) {
            (Some(_), true) => println!("gate: 参考环境指纹一致"),
            (Some(reference), false) => println!("gate: 参考环境指纹不同或不完整：{reference:?}"),
            (None, _) => println!("gate: 参考目录没有 {ENV_FILE}"),
        }
        if enforce_total {
            println!("gate: 总差异按阈值判定");
        } else {
            println!("gate: 总差异仅诊断（要强制请加 --strict-total）");
        }
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
    if let Some(context) = &context {
        for (frame, reference) in visuals {
            match check_visual(
                context,
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
    enforce_total: bool,
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
    let total_ratio = if enforce_total {
        Some(percent(max_total, DEFAULT_MAX_TOTAL_PCT, "总差异阈值")?)
    } else {
        if max_total.is_some() {
            return Err("总差异阈值已给定，必须同时按硬阈值执行".to_string());
        }
        None
    };
    Ok(VisualLimits {
        total_ratio,
        structural_ratio: percent(max_structural, DEFAULT_MAX_STRUCTURAL_PCT, "结构性阈值")?,
    })
}

fn read_gate_env(path: &Path) -> Result<Option<GateEnv>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("读取 {} 失败：{err}", path.display())),
    };
    let env: GateEnv = serde_json::from_str(&text)
        .map_err(|err| format!("{} 不是合法的环境指纹：{err}", path.display()))?;
    Ok(Some(env))
}

/// `xgc gpu-env <参考目录>`：记录 XGC 当前选择的适配器。
pub fn write_env(reference_dir: Option<&str>) -> ExitCode {
    let dir = Path::new(reference_dir.unwrap_or("acceptance"));
    let env = match GpuContext::headless() {
        Ok(context) => GateEnv::from_context(&context),
        Err(err) => {
            eprintln!("GPU 不可用：{err}");
            return ExitCode::from(1);
        }
    };
    let path = dir.join(ENV_FILE);
    let text = match serde_json::to_string_pretty(&env) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("环境指纹序列化失败：{err}");
            return ExitCode::from(1);
        }
    };
    if let Err(err) = std::fs::create_dir_all(dir) {
        eprintln!("创建 {} 失败：{err}", dir.display());
        return ExitCode::from(1);
    }
    if let Err(err) = std::fs::write(&path, text + "\n") {
        eprintln!("写入 {} 失败：{err}", path.display());
        return ExitCode::from(1);
    }
    println!(
        "gpu-env: {} / {} / {} / {} -> {}",
        env.gpu,
        env.backend,
        env.driver,
        env.driver_info,
        path.display()
    );
    ExitCode::SUCCESS
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
    let mut runner = scripted_runner(world, assets_root)?;
    let mut failures = 0usize;
    let last = expected.last().map(|(frame, _)| *frame).unwrap_or(0);
    for frame in 0..=last {
        if frame > 0 {
            runner.step()?;
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
    context: &GpuContext,
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
    let mut runner = scripted_runner(world, Path::new(assets_root))?;
    for _ in 0..frame {
        runner.step()?;
    }
    let (mut world, particles) = runner.into_parts();
    // 与 `render-frame` 同序：先推进到第 N 帧，再补派生全局变换与 UI 布局。
    HierarchySystem::default().run(&mut world, 0.0);
    UiLayoutSystem {
        viewport: [width as f32, height as f32],
    }
    .run(&mut world, 0.0);
    let (pixels, _) =
        render_world_rgba(context, Some(assets_root), &world, particles, width, height)?;
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
    // 总差异在参考环境不一致时只作诊断；结构性差异不受舍入口径影响，始终硬判。
    let total_ok = match limits.total_ratio {
        Some(limit) => diff.ratio <= limit,
        None => true,
    };
    let ok = total_ok && structural <= limits.structural_ratio;
    let total_note = match limits.total_ratio {
        Some(limit) => format!("（限 {:.2}%）", limit * 100.0),
        None => "（仅诊断，未判）".to_string(),
    };
    println!(
        "gate: frame-{frame} 差异 {:.2}%{} 结构性 {:.2}%（限 {:.2}%）mean_abs {:.2} max {} -> {}",
        diff.ratio * 100.0,
        total_note,
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
        let limits = visual_limits(None, None, true).unwrap();
        assert!((limits.total_ratio.unwrap() - 0.02).abs() < 1e-6);
        assert!((limits.structural_ratio - 0.005).abs() < 1e-6);
    }

    #[test]
    fn total_is_diagnostic_only_when_environment_is_unknown() {
        let limits = visual_limits(None, None, false).unwrap();
        assert!(limits.total_ratio.is_none());
        assert!((limits.structural_ratio - 0.005).abs() < 1e-6);
        // 显式给了总差异阈值却又不判，是调用方错误。
        assert!(visual_limits(Some("2"), None, false).is_err());
    }

    #[test]
    fn limits_reject_out_of_range_and_garbage() {
        assert!(visual_limits(Some("101"), None, true).is_err());
        assert!(visual_limits(None, Some("-1"), true).is_err());
        assert!(visual_limits(Some("abc"), None, true).is_err());
        let limits = visual_limits(Some("1.5"), Some("0.25"), true).unwrap();
        assert!((limits.total_ratio.unwrap() - 0.015).abs() < 1e-6);
        assert!((limits.structural_ratio - 0.0025).abs() < 1e-6);
    }

    #[test]
    fn an_unrecorded_or_different_driver_version_cannot_enable_the_total_limit() {
        let fingerprint = |driver_info: Option<&str>| {
            let mut json = serde_json::json!({
                "gpu": "GPU A",
                "backend": "Vulkan",
                "driver": "NVIDIA",
            });
            if let Some(version) = driver_info {
                json["driver_info"] = version.into();
            }
            serde_json::from_value::<GateEnv>(json).unwrap()
        };
        let current = fingerprint(Some("572.16"));
        assert!(current.matches(&fingerprint(Some("572.16"))));
        assert!(!current.matches(&fingerprint(Some("566.36"))));
        assert!(!current.matches(&fingerprint(None)));
    }

    #[test]
    fn env_match_requires_the_same_adapter_backend_and_driver() {
        let recorded = GateEnv {
            gpu: "GPU A".to_string(),
            backend: "Vulkan".to_string(),
            driver: "NVIDIA".to_string(),
            driver_info: "572.16".to_string(),
        };
        let mut current: GateEnv =
            serde_json::from_value(serde_json::to_value(&recorded).unwrap()).unwrap();
        assert!(recorded.matches(&current));
        current.backend = "Dx12".to_string();
        assert!(!recorded.matches(&current));
        current.backend.clone_from(&recorded.backend);
        current.gpu = "GPU B".to_string();
        assert!(!recorded.matches(&current));
        current.gpu.clone_from(&recorded.gpu);
        current.driver.clear();
        assert!(!recorded.matches(&current));
        assert!(!current.matches(&current));
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
