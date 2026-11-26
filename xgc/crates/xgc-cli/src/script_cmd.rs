//! 用与运行时相同的 Lua 编译器检查单个文件。

use std::fs;
use std::process::ExitCode;

pub fn check(path: Option<&str>) -> ExitCode {
    let Some(path) = path else {
        eprintln!("用法：xgc script-check <脚本.lua>");
        return ExitCode::from(2);
    };
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("读取 {path} 失败：{err}");
            return ExitCode::from(1);
        }
    };
    match xgc_script::check_source(path, &source) {
        Ok(()) => {
            println!("script-check: {path} OK");
            ExitCode::SUCCESS
        }
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            ExitCode::from(2)
        }
    }
}
