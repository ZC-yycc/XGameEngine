//! `xgc assets <项目根>`：扫描 GUID 资产目录。

use std::path::Path;
use std::process::ExitCode;

use xgc_world::{ComponentData, Value};

use crate::scan_assets;

/// 保留实际层级索引，让检查与打包看到同一份模型引用。
pub(crate) fn lod_models(component: &ComponentData) -> impl Iterator<Item = (usize, &str)> {
    component
        .fields
        .iter()
        .find_map(|(name, value)| match (name.as_str(), value) {
            ("levels", Value::List(levels)) => Some(levels),
            _ => None,
        })
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, level)| {
            let Value::Struct(fields) = level else {
                return None;
            };
            let model = fields.get("model")?.as_string()?;
            (!model.is_empty()).then_some((index, model))
        })
}

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
