//! `xgc migrate`：单场景 / flat 目录 / 旧项目目录三种入口。
//!
//! 项目目录先走 `check_old_project`，坏项目不开始迁移；输出目录由
//! `xgc-migrate` 用 staging + rename 保证事务性。

use std::path::Path;
use std::process::ExitCode;

use xgc_migrate::{migrate_dir, migrate_old_project, migrate_str, ProjectMigrationError};
use xgc_store::save_world;

pub(crate) fn migrate(input: Option<&str>, output: Option<&str>, flag: Option<&str>) -> ExitCode {
    let dry_run = match flag {
        None => false,
        Some("--dry-run") => true,
        Some(other) => {
            eprintln!("未知参数：{other}（只支持 --dry-run）");
            return ExitCode::from(2);
        }
    };
    let (Some(input), Some(output)) = (input, output) else {
        eprintln!("用法：xgc migrate <旧.ron|旧目录> <新.json|新目录> [--dry-run]");
        return ExitCode::from(2);
    };
    if Path::new(input).is_dir() {
        if Path::new(input).join("project.ron").is_file() {
            return migrate_project(Path::new(input), Path::new(output), dry_run);
        }
        if dry_run {
            eprintln!("dry-run 只支持旧项目目录（需要 project.ron）或单个场景 .ron");
            return ExitCode::from(2);
        }
        return match migrate_dir(Path::new(input), Path::new(output)) {
            Ok(batch) => {
                for failure in &batch.failures {
                    eprintln!("migrate 失败：{failure}");
                }
                println!(
                    "migrate: {} 个场景，{} 个实体，{} 个带旧组件，{} 个组件已映射，{} 个父节点缺失，{} 个失败 -> {}",
                    batch.scenes,
                    batch.entities,
                    batch.with_old_components,
                    batch.mapped_components,
                    batch.missing_parents,
                    batch.failures.len(),
                    output
                );
                if batch.failures.is_empty() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(1)
                }
            }
            Err(err) => {
                eprintln!("批量迁移失败：{err}");
                ExitCode::from(1)
            }
        };
    }
    let text = match std::fs::read_to_string(input) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("读取旧场景失败：{err}");
            return ExitCode::from(1);
        }
    };
    let (data, report) = match migrate_str(&text) {
        Ok(result) => result,
        Err(err) => {
            eprintln!("迁移失败：{err}");
            return ExitCode::from(1);
        }
    };
    if dry_run {
        println!(
            "migrate: {} 个实体，{} 个带旧组件，{} 个组件已映射，{} 个父节点缺失 -> {}（dry-run，未写盘）",
            report.entities,
            report.with_old_components,
            report.mapped_components,
            report.missing_parents,
            output
        );
        return ExitCode::SUCCESS;
    }
    if let Err(err) = save_world(Path::new(output), &data) {
        eprintln!("写入迁移结果失败：{err}");
        return ExitCode::from(1);
    }
    println!(
        "migrate: {} 个实体，{} 个带旧组件，{} 个组件已映射，{} 个父节点缺失 -> {}",
        report.entities,
        report.with_old_components,
        report.mapped_components,
        report.missing_parents,
        output
    );
    ExitCode::SUCCESS
}

fn migrate_project(input: &Path, output: &Path, dry_run: bool) -> ExitCode {
    match migrate_old_project(input, output, dry_run) {
        Ok(report) => {
            println!("{}", report.render());
            ExitCode::SUCCESS
        }
        Err(ProjectMigrationError::Problems(problems)) => {
            eprintln!("项目体检未通过（{} 个问题），未开始迁移：", problems.len());
            for problem in problems {
                eprintln!("  {problem}");
            }
            ExitCode::from(2)
        }
        Err(err @ ProjectMigrationError::Check(_)) => {
            eprintln!("项目迁移失败：{err}");
            ExitCode::from(1)
        }
        Err(err @ ProjectMigrationError::OutputExists(_)) => {
            eprintln!("项目迁移失败：{err}");
            ExitCode::from(2)
        }
        Err(err) => {
            eprintln!("项目迁移失败：{err}");
            ExitCode::from(1)
        }
    }
}
