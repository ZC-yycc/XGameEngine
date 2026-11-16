//! `xgc input <input.ron>`：校验项目输入映射并列出动作 / 轴。

use std::path::Path;
use std::process::ExitCode;

use xgc_sim::InputMap;

pub fn inspect(path: Option<&str>) -> ExitCode {
    let Some(path) = path else {
        eprintln!("用法：xgc input <input.ron>");
        return ExitCode::from(2);
    };
    let text = match std::fs::read_to_string(Path::new(path)) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("读取输入映射失败：{err}");
            return ExitCode::from(2);
        }
    };
    match InputMap::parse_ron(&text) {
        Ok(map) => {
            println!(
                "input: {} actions, {} axes",
                map.actions.len(),
                map.axes.len()
            );
            println!("actions: {}", map.action_names().join(", "));
            println!("axes: {}", map.axis_names().join(", "));
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(2)
        }
    }
}
