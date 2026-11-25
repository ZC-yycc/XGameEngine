//! `xgc-editor` 入口：可传项目目录、`project.xgc` 或世界文件。

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let path: Option<PathBuf> = std::env::args().nth(1).map(PathBuf::from);
    match xgc_editor::run(path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("编辑器启动失败：{err}");
            ExitCode::from(2)
        }
    }
}
