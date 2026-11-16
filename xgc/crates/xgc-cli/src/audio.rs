//! `xgc play <wav>`：解码 WAV 后交给默认输出设备。

use std::path::Path;
use std::process::ExitCode;

pub fn play_wav(path: Option<&str>) -> ExitCode {
    let Some(path) = path else {
        eprintln!("用法：xgc play <音频.wav>");
        return ExitCode::from(2);
    };
    let clip = match xgc_sim::load_wav(Path::new(path)) {
        Ok(clip) => clip,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };
    let player = match xgc_audio::play_clip(&clip) {
        Ok(player) => player,
        Err(err) => {
            eprintln!("播放失败：{err}");
            return ExitCode::from(2);
        }
    };
    println!(
        "play: {} {}Hz {}ch {:.2}s",
        clip.name,
        clip.sample_rate,
        clip.channels,
        player.duration().as_secs_f32()
    );
    player.wait();
    ExitCode::SUCCESS
}
