//! `xgc-player <cooked 目录 | 项目目录> [帧数] [--scene <场景名>]`。
//!
//! 项目目录以 `project.xgc` 为标记；此时入口场景由清单给出，资产根仍是目录本身。
//! `--scene` 只对项目目录有效，写清单登记名或唯一文件名主干（`character`）。

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(dir) = args.next() else {
        eprintln!("用法：xgc-player <cooked 目录 | 项目目录> [帧数] [--scene <场景名>]");
        return ExitCode::from(2);
    };
    let mut frames: Option<u64> = None;
    let mut scene: Option<String> = None;
    while let Some(arg) = args.next() {
        if arg == "--scene" {
            let Some(name) = args.next() else {
                eprintln!("--scene 需要一个场景名");
                return ExitCode::from(2);
            };
            if name.is_empty() {
                eprintln!("--scene 不能为空");
                return ExitCode::from(2);
            }
            scene = Some(name);
        } else if frames.is_none() {
            match arg.parse::<u64>() {
                Ok(value) => frames = Some(value),
                Err(_) => {
                    eprintln!("未知参数：{arg}");
                    return ExitCode::from(2);
                }
            }
        } else {
            eprintln!("多余参数：{arg}");
            return ExitCode::from(2);
        }
    }
    match frames {
        None => {
            if let Err(err) = xgc_player::window::run_scene(Path::new(&dir), scene.as_deref()) {
                eprintln!("播放器窗口失败：{err}");
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        Some(frames) => {
            match xgc_player::run_cooked_scene(Path::new(&dir), frames, scene.as_deref()) {
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
    }
}
