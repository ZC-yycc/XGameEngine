//! `xgc clips`：列 `.ron` 剪辑目录或 glTF 模型自带的动画剪辑。
//!
//! 模型分支是切片 192 加的：旧引擎把动画挂在模型节点层级上，XGC 迁移后的世界
//! 只有 `animation.animator`，剪辑得从模型文件读。两条路都输出同样的
//! `clips: <名字> duration=…` 行，便于跨端对照。

use std::path::Path;
use std::process::ExitCode;

use xgc_sim::AnimationLibrary;

const USAGE: &str = "用法：xgc clips <剪辑目录|模型.glb|模型.gltf>";

pub(crate) fn clips(target: Option<&str>) -> ExitCode {
    let Some(target) = target else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let path = Path::new(target);
    // 模型文件：打印 glTF 自带的动画剪辑（旧引擎动画挂在模型节点层级上）。
    if path.is_file()
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "glb" | "gltf"))
    {
        return clips_model(path);
    }
    match AnimationLibrary::load_dir(path) {
        Ok((library, issues)) => {
            for name in library.names() {
                if let Some(clip) = library.get(name) {
                    println!(
                        "clips: {name} duration={:.3} keys={}",
                        clip.duration,
                        clip.translation.len()
                    );
                }
            }
            for issue in &issues {
                eprintln!("clips: {issue}");
            }
            println!("clips: {} 个剪辑，{} 条问题", library.len(), issues.len());
            if issues.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(err) => {
            eprintln!("加载剪辑失败：{err}");
            ExitCode::from(2)
        }
    }
}

fn clips_model(path: &Path) -> ExitCode {
    match xgc_render::load_model(path) {
        Ok(mesh) => {
            let Some(animations) = &mesh.animations else {
                println!("clips: 模型没有 glTF 动画");
                return ExitCode::SUCCESS;
            };
            let mut names: Vec<&str> = animations
                .clips
                .iter()
                .map(|clip| clip.name.as_str())
                .collect();
            names.sort_unstable();
            for name in names {
                if let Some(clip) = animations.clip(name) {
                    println!(
                        "clips: {name} duration={:.3} channels={}",
                        clip.duration,
                        clip.channels.len()
                    );
                }
            }
            println!(
                "clips: {} 个剪辑，{} 个节点",
                animations.clips.len(),
                animations.nodes.len()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("加载模型失败：{err}");
            ExitCode::from(2)
        }
    }
}
