//! `xgc-player <cooked 目录> [帧数]`。

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(dir) = args.next() else {
        eprintln!("用法：xgc-player <cooked 目录> [帧数]");
        return ExitCode::from(2);
    };
    let frames: u64 = match args.next().map(|value| value.parse::<u64>()) {
        Some(Ok(frames)) => frames,
        Some(Err(_)) => {
            eprintln!("帧数不是整数");
            return ExitCode::from(2);
        }
        None => 300,
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
