use std::path::{Path, PathBuf};

use crate::{migrate_str, MigrationBatch, MigrationError};

pub fn migrate_dir(input: &Path, output: &Path) -> Result<MigrationBatch, MigrationError> {
    let entries = std::fs::read_dir(input)
        .map_err(|err| MigrationError::Io(format!("读取目录 {}：{err}", input.display())))?;
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "ron"))
        .collect();
    files.sort();

    std::fs::create_dir_all(output)
        .map_err(|err| MigrationError::Io(format!("创建目录 {}：{err}", output.display())))?;

    let mut batch = MigrationBatch::default();
    for path in files {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                batch
                    .failures
                    .push(format!("{}：读取失败 {err}", path.display()));
                continue;
            }
        };
        match migrate_str(&text) {
            Ok((data, report)) => {
                let stem = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "scene".to_string());
                let target = output.join(format!("{stem}.json"));
                if let Err(err) = xgc_store::save_world(&target, &data) {
                    batch
                        .failures
                        .push(format!("{}：写入失败 {err}", path.display()));
                    continue;
                }
                batch.scenes += 1;
                batch.entities += report.entities;
                batch.with_old_components += report.with_old_components;
                batch.mapped_components += report.mapped_components;
                batch.missing_parents += report.missing_parents;
                batch
                    .unmatched
                    .extend(report.unmatched(&path.to_string_lossy()));
            }
            Err(err) => batch.failures.push(format!("{}：{err}", path.display())),
        }
    }
    Ok(batch)
}
