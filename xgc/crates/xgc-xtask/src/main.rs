//! XGC 开发门禁。
//!
//! M0 只做三件事：文件行数预算、crate 依赖方向、schema 确定性。它们都是
//! **可以在本地一条命令跑完**的硬判据；后续的分配 / 确定性 / 基准门禁继续加在这里。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use xgc_reflect::Registry;
use xgc_world::{Name, Transform, Velocity};

const SOFT_FILE_LINES: usize = 600;
const HARD_FILE_LINES: usize = 1000;

fn main() -> ExitCode {
    let root = workspace_root();
    let mut args = std::env::args().skip(1);
    let command = args.next();

    let result = match command.as_deref() {
        Some("check") => check(&root),
        Some("size") => size(&root),
        Some("deps") => deps(&root),
        Some("schema") => schema_check(),
        Some(other) => {
            eprintln!("未知命令：{other}");
            eprintln!("用法：xtask <check|size|deps|schema>");
            return ExitCode::from(2);
        }
        None => {
            eprintln!("用法：xtask <check|size|deps|schema>");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => {
            println!("xtask {}: OK", command.as_deref().unwrap_or("check"));
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!(
                "xtask {}: FAILED: {err}",
                command.as_deref().unwrap_or("check")
            );
            ExitCode::from(1)
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace 根目录不可解析")
}

fn check(root: &Path) -> Result<(), String> {
    size(root)?;
    deps(root)?;
    schema_check()?;
    Ok(())
}

fn size(root: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    collect_rust_files(&root.join("crates"), &mut files)?;
    let mut rows: Vec<(usize, PathBuf)> = files
        .into_iter()
        .map(|path| {
            let lines = fs::read_to_string(&path)
                .map(|text| text.lines().count())
                .map_err(|err| format!("读不了 {}: {err}", path.display()))?;
            let relative = path
                .strip_prefix(root)
                .map(Path::to_path_buf)
                .unwrap_or(path.clone());
            Ok((lines, relative))
        })
        .collect::<Result<_, String>>()?;
    rows.sort_by_key(|(lines, _)| std::cmp::Reverse(*lines));

    let mut hard = 0usize;
    for (lines, path) in &rows {
        if *lines > HARD_FILE_LINES {
            hard += 1;
            println!("HARD  {lines:5}  {}", path.display());
        } else if *lines > SOFT_FILE_LINES {
            println!("WARN  {lines:5}  {}", path.display());
        }
    }

    let max = rows.first().map(|(lines, _)| *lines).unwrap_or(0);
    println!(
        "size: {} 个 Rust 文件，最大 {max} 行（软 {SOFT_FILE_LINES} / 硬 {HARD_FILE_LINES}）",
        rows.len()
    );

    if hard > 0 {
        Err(format!("{hard} 个文件超过硬阈值 {HARD_FILE_LINES} 行"))
    } else {
        Ok(())
    }
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|err| format!("读不了 {}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("读不了目录项 {}: {err}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "target" || name == ".git" {
                continue;
            }
            collect_rust_files(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    Ok(())
}

fn deps(root: &Path) -> Result<(), String> {
    let allowed: &[(&str, &[&str])] = &[
        ("xgc-base", &[]),
        ("xgc-reflect", &["xgc-base"]),
        ("xgc-world", &["xgc-base", "xgc-reflect"]),
        ("xgc-store", &["xgc-base", "xgc-world"]),
        ("xgc-sim", &["xgc-base", "xgc-world"]),
        (
            "xgc-agent",
            &["xgc-base", "xgc-reflect", "xgc-world", "xgc-store"],
        ),
        (
            "xgc-cli",
            &[
                "xgc-base",
                "xgc-reflect",
                "xgc-world",
                "xgc-store",
                "xgc-sim",
                "xgc-agent",
            ],
        ),
        ("xgc-xtask", &["xgc-base", "xgc-reflect", "xgc-world"]),
    ];

    for (crate_name, allowed_deps) in allowed {
        let manifest = root.join("crates").join(crate_name).join("Cargo.toml");
        let text = fs::read_to_string(&manifest)
            .map_err(|err| format!("读不了 {}: {err}", manifest.display()))?;
        for dep in manifest_deps(&text) {
            if dep.starts_with("xgame-") {
                return Err(format!(
                    "{crate_name} 依赖了旧仓库的 {dep}；XGC 不许反向依赖旧引擎"
                ));
            }
            if !allowed_deps.contains(&dep.as_str()) {
                return Err(format!(
                    "{crate_name} 依赖 {dep}，超出允许集合 {allowed_deps:?}"
                ));
            }
        }
    }

    println!("deps: {} 个 crate 的依赖方向正确", allowed.len());
    Ok(())
}

fn manifest_deps(text: &str) -> Vec<String> {
    let mut section = "";
    let mut deps = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            section = trimmed;
            continue;
        }
        if !matches!(
            section,
            "[dependencies]" | "[dev-dependencies]" | "[build-dependencies]"
        ) {
            continue;
        }
        let Some((name, _)) = trimmed.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.starts_with("xgc-") || name.starts_with("xgame-") {
            deps.push(name.to_string());
        }
    }
    deps
}

fn schema_check() -> Result<(), String> {
    let mut registry = Registry::new();
    for result in [
        registry.register::<Transform>(),
        registry.register::<Velocity>(),
        registry.register::<Name>(),
    ] {
        result.map_err(|err| err.to_string())?;
    }

    let first = serde_json::to_string_pretty(&registry.schema())
        .map_err(|err| format!("schema 序列化失败：{err}"))?;
    let second = serde_json::to_string_pretty(&registry.schema())
        .map_err(|err| format!("schema 序列化失败：{err}"))?;
    if first != second {
        return Err("schema 两次导出不一致".to_string());
    }
    for required in ["core.transform", "core.velocity", "core.name"] {
        if !first.contains(required) {
            return Err(format!("schema 缺少 {required}"));
        }
    }
    let diagnostics = registry.validate();
    if !diagnostics.is_empty() {
        return Err(format!("schema 语义问题：{} 条", diagnostics.len()));
    }

    println!(
        "schema: {} 个类型，{} 字节，两次导出逐字节相同",
        registry.len(),
        first.len()
    );
    Ok(())
}
