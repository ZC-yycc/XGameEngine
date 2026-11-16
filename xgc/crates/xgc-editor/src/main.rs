//! `xgc-editor` 入口：可选场景路径，缺省为空场景。

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
