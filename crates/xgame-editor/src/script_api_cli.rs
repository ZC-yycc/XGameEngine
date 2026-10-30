//! `--export-script-api`：把脚本 API 清单导出成 JSON。
//!
//! # 为什么在编辑器里做
//!
//! 清单数据在 [`xgame_core::script_api`]（纯数据，不依赖 mlua）；JSON 序列化需要
//! `serde_json`，而 `xgame-core` 刻意不引它。编辑器已经依赖 `serde_json`
//! （`cargo --message-format=json` 用），所以 CLI 落在这里。MCP 的
//! `script_api_read` 走同一份清单 —— 两边不会漂移。
//!
//! # 输出契约
//!
//! - 无 `--out`：JSON 打 stdout，末尾一个换行，适合管道 / 重定向。
//! - `--out <path>`：写文件，自动建目录；stdout 只回一行确认。
//! - 输出是**确定性**的：没有时间戳 / 随机 ID；同一份代码连续导出逐字节相同。
//! - 失败退出码 1，成功 0；不复用 `--check` 的 0/1/2。

use std::path::{Path, PathBuf};

const USAGE: &str = "\
用法:
  xgame-editor --export-script-api [--out <path>]

  --export-script-api   导出脚本 API 的机器可读清单（JSON）
  --out <path>          写入文件；缺省 = stdout
  -h, --help            显示本帮助

输出是确定性的：没有时间戳，同一份代码连续导出逐字节相同。
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

/// 序列化当前脚本 API 清单，末尾补换行，方便 `cat` / diff / 管道。
pub fn render_json() -> Result<String, String> {
    let doc = xgame_core::script_api::script_api();
    let mut json = serde_json::to_string_pretty(&doc)
        .map_err(|error| format!("序列化脚本 API 清单失败: {error}"))?;
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

/// CLI 入口；`main` 只负责分发和 `process::exit`。
pub fn cli(args: &[String]) -> i32 {
    let command = match parse(args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("export-script-api FAILED: {message}\n\n{USAGE}");
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
            eprintln!("export-script-api FAILED: {message}");
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
                println!("script api: {} ({} bytes)", path.display(), json.len());
                0
            }
            Err(message) => {
                eprintln!("export-script-api FAILED: {message}");
                1
            }
        },
        // parse 已经处理了 Help；这里只为 match 穷尽。
        Command::Help => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    fn temp_json_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "xgame_script_api_{tag}_{}_{}.json",
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
            parse(&args(&["--out", "script_api.json"])),
            Ok(Command::File(PathBuf::from("script_api.json")))
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

    /// JSON 必须可解析、确定、包含全部函数 / 钩子 / 类别。
    #[test]
    fn json_is_valid_and_deterministic() {
        let first = render_json().expect("脚本 API 清单应可序列化");
        let second = render_json().expect("脚本 API 清单应可序列化");
        assert_eq!(first, second, "两次导出必须逐字节相同");
        assert!(first.ends_with('\n'));

        let value: serde_json::Value = serde_json::from_str(&first).expect("必须是合法 JSON");
        assert_eq!(value["format_version"], 1);
        assert_eq!(
            value["engine_version"],
            env!("CARGO_PKG_VERSION"),
            "引擎版本来自 core crate"
        );
        let categories = value["categories"]
            .as_array()
            .expect("categories 必须是数组");
        assert_eq!(categories.len(), 18, "类别数");
        assert_eq!(
            value["hooks"].as_array().expect("hooks 必须是数组").len(),
            6,
            "生命周期钩子数"
        );
        assert!(
            value["conventions"]
                .as_array()
                .expect("conventions 必须是数组")
                .len()
                >= 8
        );
        let function_count: usize = categories
            .iter()
            .map(|category| {
                category["functions"]
                    .as_array()
                    .expect("functions 必须是数组")
                    .len()
            })
            .sum();
        assert_eq!(function_count, 176, "函数总数");
        assert!(
            categories
                .iter()
                .any(|category| category["id"] == "transform"),
            "缺 transform 类别"
        );
    }

    /// `--out` 落盘内容必须与 stdout 完全一致。
    #[test]
    fn out_writes_the_same_text_as_stdout() {
        let path = temp_json_path("out");
        let _ = std::fs::remove_file(&path);

        let code = cli(&args(&[
            "--out",
            path.to_str().expect("临时路径必须是 UTF-8"),
        ]));
        assert_eq!(code, 0, "写文件应成功");
        let written = std::fs::read_to_string(&path).expect("文件应可读");
        assert_eq!(written, render_json().unwrap());
        assert!(written.contains("\"format_version\": 1"));

        let _ = std::fs::remove_file(&path);
    }

    /// `Docs/脚本API.md` 的 API 表与机器清单必须逐名一致。
    ///
    /// 只比函数名：文档的说明文字是给人读的，清单的 summary 可以更精炼；
    /// 但“文档里有一个函数、清单里没有”会让智能体照着文档写出调用不了的名字，
    /// 所以名字集合必须完全相同。
    #[test]
    fn docs_api_table_matches_the_manifest() {
        let docs_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Docs/脚本API.md");
        let markdown = std::fs::read_to_string(&docs_path)
            .unwrap_or_else(|error| panic!("读 {} 失败: {error}", docs_path.display()));
        let documented = documented_function_names(&markdown);
        let doc = xgame_core::script_api::script_api();
        let manifest: BTreeSet<String> = doc.function_names().map(str::to_string).collect();

        assert_eq!(
            documented, manifest,
            "Docs/脚本API.md 与机器清单的函数名不一致"
        );
        assert_eq!(documented.len(), 176, "文档 API 表应有 176 个函数");
    }

    /// 从 Markdown 的 API 表第一列提取函数名。
    fn documented_function_names(markdown: &str) -> BTreeSet<String> {
        let mut in_api_section = false;
        let mut names = BTreeSet::new();
        for line in markdown.lines() {
            let line = line.trim();
            if line.starts_with("## ") {
                in_api_section = line.contains("API");
                continue;
            }
            if !in_api_section || !line.starts_with('|') {
                continue;
            }
            let cells = split_unescaped_pipes(line);
            let Some(first) = cells.get(1) else {
                continue;
            };
            for span in code_spans(first) {
                let Some((name, _)) = span.split_once('(') else {
                    continue;
                };
                let name = name.trim();
                if is_lua_identifier(name) {
                    names.insert(name.to_string());
                }
            }
        }
        names
    }

    /// 按未转义的 `|` 切表格行；`\|` 是单元格内的字面竖线。
    fn split_unescaped_pipes(line: &str) -> Vec<&str> {
        let mut cells = Vec::new();
        let bytes = line.as_bytes();
        let mut start = 0;
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'\\' && index + 1 < bytes.len() && bytes[index + 1] == b'|' {
                index += 2;
                continue;
            }
            if bytes[index] == b'|' {
                cells.push(&line[start..index]);
                start = index + 1;
            }
            index += 1;
        }
        cells.push(&line[start..]);
        cells
    }

    /// 取出单元格里所有反引号包住的片段。
    fn code_spans(cell: &str) -> Vec<&str> {
        let mut spans = Vec::new();
        let mut rest = cell;
        while let Some(start) = rest.find('`') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('`') else {
                break;
            };
            spans.push(&after[..end]);
            rest = &after[end + 1..];
        }
        spans
    }

    fn is_lua_identifier(text: &str) -> bool {
        let mut chars = text.chars();
        match chars.next() {
            Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
            _ => return false,
        }
        chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    }
}
