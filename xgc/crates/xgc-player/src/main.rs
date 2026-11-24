//! `xgc-player <cooked 目录 | 项目目录> [帧数]`。
//!
//! 项目目录以 `project.xgc` 为标记；此时入口场景由清单给出，资产根仍是目录本身。

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(dir) = args.next() else {
        eprintln!("用法：xgc-player <cooked 目录 | 项目目录> [帧数]");
        return ExitCode::from(2);
    };
    let frames = match args.next() {
        None => {
            if let Err(err) = xgc_player::window::run(Path::new(&dir)) {
                eprintln!("播放器窗口失败：{err}");
                return ExitCode::from(1);
            }
            return ExitCode::SUCCESS;
        }
        Some(value) => match value.parse::<u64>() {
            Ok(frames) => frames,
            Err(_) => {
                eprintln!("帧数不是整数");
                return ExitCode::from(2);
            }
        },
    };
    match xgc_player::run_cooked(Path::new(&dir), frames) {
        Ok(stats) => {
            println!(
                "xgc-player: frames={} entities={} scripts={} hash={:016x}",
                stats.frames, stats.entities, stats.scripts, stats.state_hash
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}
