//! `xgc cook <项目根> <输出目录>`：复制运行时资产并写 cook.json。

use std::path::Path;
use std::process::ExitCode;

pub fn cook(root: Option<&str>, out: Option<&str>) -> ExitCode {
    let (Some(root), Some(out)) = (root, out) else {
        eprintln!("用法：xgc cook <项目根> <输出目录>");
        return ExitCode::from(2);
    };
    match xgc_asset::cook_project(Path::new(root), Path::new(out)) {
        Ok(manifest) => {
            let bytes: u64 = manifest.files.iter().map(|file| file.size).sum();
            println!(
                "cook: {} 个文件，{} 字节 -> {}",
                manifest.files.len(),
                bytes,
                out
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("烹饪失败：{err}");
            ExitCode::from(1)
        }
    }
}
