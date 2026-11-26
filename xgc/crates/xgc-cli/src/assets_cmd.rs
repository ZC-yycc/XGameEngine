//! `xgc assets <项目根>`：扫描 GUID 资产目录。

use std::path::Path;
use std::process::ExitCode;

use crate::scan_assets;

pub fn assets(root: Option<&str>) -> ExitCode {
    let Some(root) = root else {
        eprintln!("用法：xgc assets <项目根>");
        return ExitCode::from(2);
    };
    match scan_assets(Path::new(root)) {
        Ok(catalog) => {
            println!(
                "assets: {} 个 GUID，{} 条问题",
                catalog.len(),
                catalog.issues().len()
            );
            for issue in catalog.issues() {
                eprintln!("assets: {issue}");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("扫描资产失败：{err}");
            ExitCode::from(1)
        }
    }
}
