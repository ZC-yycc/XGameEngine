//! `--export-schema`：把场景格式说明导出成 JSON。
//!
//! # 为什么在编辑器而不是 core
//!
//! `xgame-core` 的落盘格式是 RON，它刻意不依赖 `serde_json`；编辑器本来就带
//! `serde_json`（插件构建的 `cargo --message-format=json` 在用）。说明书的
//! **内容**在 [`xgame_core::schema::scene_schema`]，这里只负责参数解析与出口。
//! MCP 的 `schema_read` 走同一个函数 —— 两条出口不会漂移。
//!
//! # 输出契约
//!
//! - 无 `--out`：JSON 打到 stdout（末尾一个换行），适合管道 / 重定向。
//! - `--out <path>`：写文件（自动建父目录），stdout 只回一行确认。
//! - 输出是**确定性**的：没有时间戳 / 随机 ID；同一份代码两次导出逐字节相同。
//! - 参数错误退出码 1；成功 0。与 `--check` 的 0/1/2 语义互不干扰。

use std::path::{Path, PathBuf};

const USAGE: &str = "\
用法:
  xgame-editor --export-schema [--out <path>]

  --export-schema   输出场景格式的机器可读说明（JSON）
  --out <path>      写入文件（缺省 = stdout）
  -h, --help        显示本帮助

输出是确定性的：没有时间戳，同一份代码两次导出逐字节相同。
";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Stdout,
    File(PathBuf),
    Help,
}

fn parse(args: &[String]) -> Result<Command, String> {
    let mut out: Option<PathBuf> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--out" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--out 缺少路径参数".to_string())?;
                if value.trim().is_empty() {
                    return Err("--out 的路径不能为空".into());
                }
                if out.replace(PathBuf::from(value)).is_some() {
                    return Err("--out 只能出现一次".into());
                }
                index += 2;
            }
            other => return Err(format!("未知参数: {other}")),
        }
    }
    Ok(match out {
        Some(path) => Command::File(path),
        None => Command::Stdout,
    })
}

/// 序列化当前 schema（带末尾换行，便于 `cat` / diff / 管道）。
pub fn render_json() -> Result<String, String> {
    let doc = xgame_core::schema::scene_schema();
    let mut json = serde_json::to_string_pretty(&doc)
        .map_err(|error| format!("序列化 schema 失败: {error}"))?;
    json.push('\n');
    Ok(json)
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("创建目录 {} 失败: {error}", parent.display()))?;
        }
    }
    std::fs::write(path, text).map_err(|error| format!("写入 {} 失败: {error}", path.display()))
}

/// 命令行入口（`main` 只负责分发与 `process::exit`）。
pub fn cli(args: &[String]) -> i32 {
    let command = match parse(args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("export-schema FAILED: {message}\n\n{USAGE}");
            return 1;
        }
    };
    if command == Command::Help {
        print!("{USAGE}");
        return 0;
    }

    let json = match render_json() {
        Ok(json) => json,
        Err(message) => {
            eprintln!("export-schema FAILED: {message}");
            return 1;
        }
    };

    match command {
        Command::Stdout => {
            print!("{json}");
            0
        }
        Command::File(path) => match write_file(&path, &json) {
            Ok(()) => {
                println!("schema: {} ({} bytes)", path.display(), json.len());
                0
            }
            Err(message) => {
                eprintln!("export-schema FAILED: {message}");
                1
            }
        },
        // parse 已经处理过 Help，这里只是让 match 穷举。
        Command::Help => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    fn temp_json_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "xgame_schema_{tag}_{}_{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn parse_defaults_to_stdout() {
        assert_eq!(parse(&[]), Ok(Command::Stdout));
    }

    #[test]
    fn parse_accepts_out_and_help() {
        assert_eq!(
            parse(&args(&["--out", "schema.json"])),
            Ok(Command::File(PathBuf::from("schema.json")))
        );
        assert_eq!(parse(&args(&["--help"])), Ok(Command::Help));
        assert_eq!(parse(&args(&["-h"])), Ok(Command::Help));
    }

    #[test]
    fn parse_rejects_bad_arguments() {
        assert!(parse(&args(&["--out"])).unwrap_err().contains("缺少路径"));
        assert!(parse(&args(&["--out", "   "]))
            .unwrap_err()
            .contains("不能为空"));
        assert!(parse(&args(&["--out", "a", "--out", "b"]))
            .unwrap_err()
            .contains("只能出现一次"));
        assert!(parse(&args(&["--wat"])).unwrap_err().contains("未知参数"));
    }

    /// JSON 必须是可解析的、确定性的，并且带两个根文件与全部类型。
    #[test]
    fn json_is_valid_and_deterministic() {
        let first = render_json().expect("schema 应可序列化");
        let second = render_json().expect("schema 应可序列化");
        assert_eq!(first, second, "两次导出必须逐字节相同");
        assert!(first.ends_with('\n'));

        let value: serde_json::Value = serde_json::from_str(&first).expect("必须是合法 JSON");
        assert_eq!(value["format_version"], 1);
        assert_eq!(value["roots"][0]["root_type"], "Scene");
        assert_eq!(value["roots"][1]["root_type"], "Prefab");
        let types = value["types"].as_array().expect("types 数组");
        assert!(types.len() >= 30, "类型目录太小: {}", types.len());
        assert!(types.iter().any(|ty| ty["name"] == "Entity"), "缺 Entity");
        assert!(
            types
                .iter()
                .any(|ty| ty["name"] == "Entity" && ty["default_ron"].is_null()),
            "Entity 没有固定默认值，default_ron 必须是 null"
        );
    }

    /// `--out` 真的写盘，内容与 stdout 出口完全一致。
    #[test]
    fn out_writes_the_same_text_as_stdout() {
        let path = temp_json_path("out");
        let _ = std::fs::remove_file(&path);

        let code = cli(&args(&[
            "--out",
            path.to_str().expect("临时路径必须是 UTF-8"),
        ]));
        assert_eq!(code, 0, "写文件应成功");
        let written = std::fs::read_to_string(&path).expect("文件应存在");
        assert_eq!(written, render_json().unwrap());
        assert!(written.contains("\"format_version\": 1"));

        let _ = std::fs::remove_file(&path);
    }
}
