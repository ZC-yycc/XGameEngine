//! 项目检查里「从 RON 文本 / 文件系统拿引用」的那一半：预制体、脚本、GUID、
//! 目录扫描。留在 `project_check.rs` 会让入口文件超过软限 600 行，所以按
//! 「解析项目结构」与「解析引用」拆开；两边共用 `ProjectCheckReport`。

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use xgc_asset::AssetCatalog;
use xgc_base::path::normalize_project_relative;

use super::ProjectCheckReport;
use crate::old_scene::OldScene;
use crate::OLD_SCENE_FORMAT_VERSION;

#[allow(clippy::too_many_arguments)]
pub(super) fn check_prefab_ref(
    report: &mut ProjectCheckReport,
    root: &Path,
    rel: &str,
    source: &str,
    source_id: u64,
    catalog: &AssetCatalog,
    cache: &mut BTreeMap<PathBuf, Result<HashSet<u64>, String>>,
) {
    let path = match resolve_prefab_path(root, source, catalog) {
        Ok(path) => path,
        Err(message) => {
            report
                .problems
                .push(format!("[prefab.source] {rel}: {message}"));
            return;
        }
    };
    match prefab_ids(&path, catalog, cache, report) {
        Ok(ids) => {
            if !ids.contains(&source_id) {
                report.problems.push(format!(
                    "[prefab.source_id] {rel}: 预制体 {} 里没有 source_id {source_id}",
                    path.display()
                ));
            }
        }
        Err(message) => report
            .problems
            .push(format!("[prefab.parse] {rel}: {message}")),
    }
}

pub(crate) fn resolve_prefab_path(
    root: &Path,
    source: &str,
    catalog: &AssetCatalog,
) -> Result<PathBuf, String> {
    if let Some(guid) = source.strip_prefix("guid://") {
        return catalog
            .resolve(guid)
            .map(Path::to_path_buf)
            .ok_or_else(|| format!("预制体 GUID 未登记：{source}"));
    }
    let rel = normalize_project_relative(source).map_err(|defect| defect.to_string())?;
    let path = root.join(rel);
    if !path.is_file() {
        return Err(format!("预制体文件不存在：{}", path.display()));
    }
    Ok(path)
}

/// 预制体文件只解析一次；缓存成功 ID 集合或失败原因。
pub(super) fn prefab_ids(
    path: &Path,
    catalog: &AssetCatalog,
    cache: &mut BTreeMap<PathBuf, Result<HashSet<u64>, String>>,
    report: &mut ProjectCheckReport,
) -> Result<HashSet<u64>, String> {
    if let Some(existing) = cache.get(path) {
        return existing.clone();
    }
    let result = read_prefab_ids(path, catalog, report);
    cache.insert(path.to_path_buf(), result.clone());
    result
}

fn read_prefab_ids(
    path: &Path,
    catalog: &AssetCatalog,
    report: &mut ProjectCheckReport,
) -> Result<HashSet<u64>, String> {
    let text = fs::read_to_string(path).map_err(|err| err.to_string())?;
    let prefab: OldScene = ron::from_str(&text).map_err(|err| err.to_string())?;
    if prefab.format_version != OLD_SCENE_FORMAT_VERSION {
        return Err(format!(
            "只认 format_version {OLD_SCENE_FORMAT_VERSION}，实际 {}",
            prefab.format_version
        ));
    }
    for guid in guid_refs(&text) {
        if !catalog.contains(&guid) {
            report.problems.push(format!(
                "[asset.dangling] {}: guid://{guid} 没有对应的 .meta",
                path.display()
            ));
        }
    }
    let mut ids = HashSet::new();
    let mut duplicates = BTreeSet::new();
    for entity in &prefab.entities {
        if !ids.insert(entity.id.0) {
            duplicates.insert(entity.id.0);
        }
    }
    for id in duplicates {
        report.problems.push(format!(
            "[prefab.duplicate_id] {}: 实体 ID {id} 重复",
            path.display()
        ));
    }
    Ok(ids)
}

pub(super) fn normalize_into(
    report: &mut ProjectCheckReport,
    raw: &str,
    code: &str,
) -> Option<String> {
    match normalize_project_relative(raw) {
        Ok(normalized) => Some(normalized),
        Err(defect) => {
            report
                .problems
                .push(format!("[{code}] 路径不合法（{}）：{raw}", defect.ascii()));
            None
        }
    }
}

pub(super) fn resolve_read(root: &Path, rel: &str, search_paths: &[String]) -> Option<PathBuf> {
    let direct = root.join(rel);
    if direct.is_file() {
        return Some(direct);
    }
    search_paths
        .iter()
        .map(|search| root.join(search).join(rel))
        .find(|candidate| candidate.is_file())
}

/// 扫 `Scenes/` 下的场景文件（`.ron`，不含 `.prefab.ron`），返回项目相对路径。
pub(super) fn scene_files(
    root: &Path,
    scenes_dir: Option<&str>,
    report: &mut ProjectCheckReport,
) -> Vec<String> {
    let Some(dir) = scenes_dir else {
        return Vec::new();
    };
    let absolute = root.join(dir);
    let entries = match fs::read_dir(&absolute) {
        Ok(entries) => entries,
        Err(err) => {
            report.problems.push(format!(
                "[content.scenes_dir] 读不了 {}：{err}",
                absolute.display()
            ));
            return Vec::new();
        }
    };
    let mut scenes = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(".ron") || name.ends_with(".prefab.ron") {
            continue;
        }
        scenes.push(format!("{dir}/{name}"));
    }
    scenes.sort();
    scenes
}

/// 项目检查和迁移共用遍历，避免嵌套资产只被其中一边发现。
pub(crate) fn prefab_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    xgc_asset::find_files(dir, |path| {
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".prefab.ron"))
    })
}

/// 从 RON 文本里抠出 `guid://<hex>` 引用；只做字面扫描，不猜意图。
pub(super) fn guid_refs(text: &str) -> BTreeSet<String> {
    let mut refs = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut cursor = 0;
    while let Some(offset) = text[cursor..].find("guid://") {
        let start = cursor + offset + "guid://".len();
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_hexdigit() {
            end += 1;
        }
        if end > start {
            refs.insert(text[start..end].to_ascii_lowercase());
        }
        cursor = end.max(start);
    }
    refs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_scan_stops_at_non_hex() {
        let refs =
            guid_refs(r#"model: "guid://43b41a5b4d7e4a4fb2d4b0a0c9b0a111", other: "guid://xyz""#);
        assert_eq!(
            refs.into_iter().collect::<Vec<_>>(),
            vec!["43b41a5b4d7e4a4fb2d4b0a0c9b0a111".to_string()]
        );
    }
}
