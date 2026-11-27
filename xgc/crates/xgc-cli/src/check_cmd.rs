//! `xgc check` 只读：按输入选择 schema、单世界、旧迁移源或原生项目。

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::process::ExitCode;

use xgc_migrate::{check_old_project, ProjectCheckReport};
use xgc_reflect::Registry;
use xgc_store::load_world;
use xgc_world::World;

pub(crate) mod project;

pub fn check(path: Option<&str>) -> ExitCode {
    let registry = xgc_world::builtin_registry();
    let diagnostics = registry.validate();
    if !diagnostics.is_empty() {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::from(1);
    }

    let Some(path) = path else {
        println!("check: {} 个组件，schema 语义 OK", registry.len());
        return ExitCode::SUCCESS;
    };
    let target = Path::new(path);
    if target.file_name().is_some_and(|name| name == "project.xgc") {
        return print_project(project::check(target, &registry));
    }
    if target.is_dir() {
        let manifest = target.join("project.xgc");
        // 只有确知原生清单不存在才把目录当作迁移源，坏清单必须报错。
        return match fs::symlink_metadata(&manifest) {
            Err(err) if err.kind() == ErrorKind::NotFound => {
                print_project(check_old_project(target).map_err(|err| err.to_string()))
            }
            _ => print_project(project::check(&manifest, &registry)),
        };
    }
    check_world(target, &registry)
}

fn print_project(result: Result<ProjectCheckReport, String>) -> ExitCode {
    match result {
        Ok(report) => {
            println!("{}", report.render());
            if report.has_problems() {
                ExitCode::from(2)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(err) => {
            eprintln!("项目检查失败：{err}");
            ExitCode::from(1)
        }
    }
}

fn check_world(path: &Path, registry: &Registry) -> ExitCode {
    let data = match load_world(path) {
        Ok(data) => data,
        Err(err) => {
            eprintln!("读取世界失败：{err}");
            return ExitCode::from(1);
        }
    };
    let diagnostics = World::validate_data(&data, registry);
    if diagnostics.is_empty() {
        println!(
            "check: {} 个组件，schema 语义 OK；世界 {} 个实体，revision {}",
            registry.len(),
            data.entities.len(),
            data.revision
        );
        ExitCode::SUCCESS
    } else {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        ExitCode::from(2)
    }
}
